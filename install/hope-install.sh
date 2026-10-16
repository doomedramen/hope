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
$STD apt install -y \
  build-essential \
  pkg-config \
  cmake \
  caddy
msg_ok "Installed Dependencies"

setup_rust
NODE_VERSION="22" setup_nodejs
fetch_and_deploy_gh_branch "hope" "doomedramen/hope" "main"

msg_info "Building H.O.P.E (Patience)"
cd /opt/hope/web
$STD npm ci --no-audit --no-fund
$STD npm run build
cd /opt/hope
CARGO_BUILD_JOBS="$(get_parallel_jobs)" $STD cargo build --release --locked -p hope-server
install -m 755 target/release/hope-server /usr/local/bin/hope-server
msg_ok "Built H.O.P.E"

msg_info "Configuring Caddy"
mkdir -p /etc/hope
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
# Caddy mints its internal CA when it first serves the Caddyfile; agents trust it through HOPE_AGENT_CA.
for _ in $(seq 1 30); do
  [[ -f /var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt ]] && break
  sleep 1
done
install -m 644 /var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt /etc/hope/ca.crt
msg_ok "Configured Caddy"

msg_info "Configuring H.O.P.E"
mkdir -p /var/lib/hope/releases
cat <<EOF >/etc/hope/hope.env
HOPE_SERVER_URL=https://${LOCAL_IP}
HOPE_BIND=127.0.0.1:8080
HOPE_DATA_DIR=/var/lib/hope
HOPE_WEB_DIR=/opt/hope/web/out
HOPE_RELEASE_DIR=/var/lib/hope/releases
HOPE_DISCOVERY_MODE=native
HOPE_AGENT_CA=/etc/hope/ca.crt
HOPE_SETUP_TOKEN=$(random_password 32)
RUST_LOG=hope_server=info
EOF
chmod 600 /etc/hope/hope.env
msg_ok "Configured H.O.P.E"

msg_info "Creating Service"
cat <<EOF >/etc/systemd/system/hope.service
[Unit]
Description=H.O.P.E network observatory
Documentation=https://github.com/doomedramen/hope
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=/etc/hope/hope.env
WorkingDirectory=/var/lib/hope
ExecStart=/usr/local/bin/hope-server
Restart=on-failure
RestartSec=5
ProtectSystem=strict
ReadWritePaths=/var/lib/hope
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF
systemctl enable -q --now hope
msg_ok "Created Service"

motd_ssh
customize
cleanup_lxc
