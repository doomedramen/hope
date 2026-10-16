#!/usr/bin/env bash

# Copyright (c) 2021-2026 community-scripts ORG
# Author: doomedramen
# License: MIT | https://github.com/community-scripts/ProxmoxVE/raw/main/LICENSE
# Source: https://github.com/doomedramen/hope

source /dev/stdin <<<"$FUNCTIONS_FILE_PATH"
color
verb_ip6
catch_errors
setting_up_container
network_check
update_os

msg_info "Installing Dependencies"
$STD apt-get install -y libcap2-bin caddy
msg_ok "Installed Dependencies"

# A published release carries the server binary and the console export; without one (the
# project is pre-release) the server is built from source, which needs Rust and Node.js.
ARCH_NAME="$(arch_resolve x86_64 aarch64)"
mkdir -p /opt/hope/bin /opt/hope/releases /etc/hope /var/lib/hope
if fetch_and_deploy_gh_release "hope" "doomedramen/hope" "prebuild" "latest" "/opt/hope/release" "hope-server-${ARCH_NAME}-linux.tar.gz"; then
  msg_info "Installing ${APPLICATION} from the release package"
  install -m 755 /opt/hope/release/hope-server /opt/hope/bin/hope-server
  rm -rf /opt/hope/web
  cp -R /opt/hope/release/web /opt/hope/web
  msg_ok "Installed ${APPLICATION} from the release package"
else
  msg_warn "No prebuilt release for ${ARCH_NAME} yet; building from source (10 to 20 minutes)"
  $STD apt-get install -y git build-essential pkg-config cmake
  setup_rust
  NODE_VERSION="22" setup_nodejs
  msg_info "Building ${APPLICATION} from source"
  rm -rf /opt/hope/src
  $STD git clone --depth 1 https://github.com/doomedramen/hope.git /opt/hope/src
  cd /opt/hope/src/web || exit 1
  $STD npm ci --no-audit --no-fund
  $STD npm run build
  cd /opt/hope/src || exit 1
  $STD "$HOME/.cargo/bin/cargo" build --release --locked -p hope-server
  install -m 755 target/release/hope-server /opt/hope/bin/hope-server
  rm -rf /opt/hope/web
  cp -R web/out /opt/hope/web
  cd /
  msg_ok "Built ${APPLICATION} from source"
fi

msg_info "Configuring ${APPLICATION}"
getent passwd hope >/dev/null || useradd --system --home-dir /var/lib/hope --shell /usr/sbin/nologin --user-group hope
chown -R hope:hope /var/lib/hope
chmod 700 /var/lib/hope
setcap cap_net_raw=ep /opt/hope/bin/hope-server
LOCAL_IP="$(hostname -I | awk '{print $1}')"
cat <<EOF >/etc/hope/hope.env
# H.O.P.E server settings (https://github.com/doomedramen/hope/blob/main/docs/operations.md)
HOPE_SERVER_URL=https://${LOCAL_IP}
HOPE_BIND=127.0.0.1:8080
HOPE_DATA_DIR=/var/lib/hope
HOPE_WEB_DIR=/opt/hope/web
HOPE_RELEASE_DIR=/opt/hope/releases
HOPE_DISCOVERY_MODE=native
HOPE_AGENT_CA=/etc/hope/ca.crt
HOPE_TRUSTED_PROXIES=
RUST_LOG=hope_server=info
EOF
chmod 640 /etc/hope/hope.env
msg_ok "Configured ${APPLICATION}"

msg_info "Creating Service"
cat <<EOF >/etc/systemd/system/hope.service
[Unit]
Description=H.O.P.E network observatory
Documentation=https://github.com/doomedramen/hope
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=hope
Group=hope
EnvironmentFile=/etc/hope/hope.env
ExecStart=/opt/hope/bin/hope-server
WorkingDirectory=/var/lib/hope
Restart=on-failure
RestartSec=5
AmbientCapabilities=CAP_NET_RAW
CapabilityBoundingSet=CAP_NET_RAW
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=/var/lib/hope
UMask=0077

[Install]
WantedBy=multi-user.target
EOF
systemctl enable -q --now hope
msg_ok "Created Service"

msg_info "Configuring Caddy (HTTPS with an internal CA for ${LOCAL_IP})"
cat <<EOF >/etc/caddy/Caddyfile
${LOCAL_IP} {
    tls internal
    encode gzip
    reverse_proxy 127.0.0.1:8080 {
        flush_interval -1
    }
}
EOF
systemctl enable -q --now caddy
systemctl reload caddy
for _ in $(seq 1 30); do
  [[ -f /var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt ]] && break
  sleep 1
done
if [[ -f /var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt ]]; then
  install -m 644 /var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt /etc/hope/ca.crt
  msg_ok "Configured Caddy; agents must trust /etc/hope/ca.crt (copy it to the same path on each agent host)"
else
  msg_warn "Caddy has not created its internal CA yet; copy root.crt to /etc/hope/ca.crt later"
fi

for _ in $(seq 1 30); do
  curl -fsS --max-time 2 http://127.0.0.1:8080/healthz >/dev/null 2>&1 && break
  sleep 1
done
TOKEN="$(journalctl -u hope -b --no-pager 2>/dev/null | sed -n 's/.*setup token: \([A-Za-z0-9_-]*\).*/\1/p' | tail -n 1)"
{
  echo "H.O.P.E console: https://${LOCAL_IP}"
  echo "First-run setup token: ${TOKEN:-see journalctl -u hope}"
  echo "Agents need /etc/hope/ca.crt at the same path on their hosts"
} >>~/hope.creds

motd_ssh
customize
cleanup_lxc
