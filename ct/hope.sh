#!/usr/bin/env bash
_CS_DEFAULT_URL="https://raw.githubusercontent.com/doomedramen/hope/main"
_cs_boot="${COMMUNITY_SCRIPTS_CORE_DIR:-$(dirname "${BASH_SOURCE[0]}")/../../core}/core/build.func"
source "$_cs_boot" 2>/dev/null || source <(curl -fsSL "${COMMUNITY_SCRIPTS_CORE_URL:-https://raw.githubusercontent.com/community-scripts/core/main}/core/build.func")
# Copyright (c) 2021-2026 community-scripts ORG
# Author: doomedramen
# License: MIT | https://github.com/community-scripts/ProxmoxVE/raw/main/LICENSE
# Source: https://github.com/doomedramen/hope

APP="HOPE"
var_tags="${var_tags:-monitoring;network}"
var_cpu="${var_cpu:-1}"
var_ram="${var_ram:-512}"
var_disk="${var_disk:-4}"
var_os="${var_os:-debian}"
var_version="${var_version:-13}"
var_unprivileged="${var_unprivileged:-1}"

# The engine generates banners only for scripts merged into community-scripts; seed its cache so
# header_info draws this one instead of reporting a 404.
if [[ ! -s /usr/local/community-scripts/headers/ct/hope ]] && mkdir -p /usr/local/community-scripts/headers/ct 2>/dev/null; then
  cat <<'EOF' >/usr/local/community-scripts/headers/ct/hope
    __  ______  ____  ______
   / / / / __ \/ __ \/ ____/
  / /_/ / / / / /_/ / __/
 / __  / /_/ / ____/ /___
/_/ /_/\____/_/   /_____/
EOF
fi

header_info "$APP"
variables
color
catch_errors

function update_script() {
  header_info
  check_container_storage
  check_container_resources

  if [[ ! -f /etc/hope/hope.env ]]; then
    msg_error "No ${APP} Installation Found!"
    exit
  fi

  # ARP discovery needs raw sockets. Keep this as a small drop-in so upgrades do not
  # replace locally maintained service settings. If the container's capability set
  # does not permit it, revert the drop-in and leave the existing service running.
  local capability_dropin=/etc/systemd/system/hope.service.d/10-arp-capability.conf
  if ! grep -Fxq 'AmbientCapabilities=CAP_NET_RAW' "$capability_dropin" 2>/dev/null; then
    install -d -m 755 "${capability_dropin%/*}"
    cat >"$capability_dropin" <<'EOF'
[Service]
AmbientCapabilities=CAP_NET_RAW
EOF
    systemctl daemon-reload
    if systemctl is-active --quiet hope && ! systemctl restart hope; then
      rm -f "$capability_dropin"
      systemctl daemon-reload
      systemctl restart hope || true
      msg_warn "CAP_NET_RAW is unavailable in this Proxmox container; restored the previous service configuration"
    fi
  fi

  if check_for_gh_release "hope" "doomedramen/hope"; then
    msg_info "Stopping Service"
    systemctl stop hope
    msg_ok "Stopped Service"

    # Earlier installs compiled H.O.P.E in the container (a git checkout in /opt/hope, or binaries
    # in /opt/hope/bin). Keep their agent releases, then drop the checkout and the toolchains; the
    # toolchain records go too, or the engine would reinstall them before the next update.
    if [[ -d /opt/hope/.git || -d /opt/hope/bin ]]; then
      msg_info "Removing the H.O.P.E source build"
      mkdir -p /var/lib/hope/releases
      [[ -d /opt/hope/releases ]] && cp -a /opt/hope/releases/. /var/lib/hope/releases/
      rm -rf /opt/hope "$HOME/.cargo" "$HOME/.rustup"
      rm -f /var/cache/app-versions/rust_version.txt /var/cache/app-versions/nodejs_version.txt
      $STD apt purge -y nodejs build-essential cmake pkg-config
      $STD apt autoremove -y
      msg_ok "Removed the H.O.P.E source build"
    fi
    sed -i -e 's|^HOPE_WEB_DIR=.*|HOPE_WEB_DIR=/opt/hope/web|' -e 's|^HOPE_RELEASE_DIR=.*|HOPE_RELEASE_DIR=/var/lib/hope/releases|' /etc/hope/hope.env
    grep -q '^HOPE_SETUP_TOKEN=' /etc/hope/hope.env || echo "HOPE_SETUP_TOKEN=$(random_password 32)" >>/etc/hope/hope.env
    chmod 600 /etc/hope/hope.env
    if ! grep -q '^ExecStart=/usr/local/bin/hope-server$' /etc/systemd/system/hope.service; then
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
AmbientCapabilities=CAP_NET_RAW
ProtectSystem=strict
ReadWritePaths=/var/lib/hope
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF
      systemctl daemon-reload
    fi

    CLEAN_INSTALL=1 fetch_and_deploy_gh_release "hope" "doomedramen/hope" "prebuild" "latest" "/opt/hope" "hope-server-x86_64-linux.tar.gz"
    install -m 755 /opt/hope/hope-server /usr/local/bin/hope-server

    # Re-sign the release's agents with this server's key (created here for containers installed
    # before agents were signed, unless a release signed elsewhere is already published). A failure
    # keeps the previous agent release and never leaves the service stopped.
    ensure_dependencies openssl
    if CLEAN_INSTALL=1 fetch_and_deploy_gh_release "hope-agents" "doomedramen/hope" "prebuild" "latest" "/opt/hope-agents" "hope-agents.tar.gz"; then
      msg_info "Signing Agent Release"
      if [[ ! -e /etc/hope/release-signing.pem && ! -e /var/lib/hope/releases/current ]]; then
        (umask 077 && openssl genrsa -out /etc/hope/release-signing.pem 3072 2>/dev/null) || true
      fi
      if [[ -f /etc/hope/release-signing.pem ]] && $STD env HOPE_SIGNING_KEY=/etc/hope/release-signing.pem sh /opt/hope/tools/assemble-release.sh --staging /opt/hope-agents --version "$(cat /opt/hope/VERSION)" --out /var/lib/hope/releases; then
        msg_ok "Signed Agent Release"
      else
        msg_warn "Agent release not signed; the previous one stays published (docs/operations.md, Signed agent releases)"
      fi
    else
      msg_warn "No agent release downloaded; the previous one stays published"
    fi

    msg_info "Starting Service"
    systemctl start hope
    msg_ok "Started Service"
    msg_ok "Updated successfully!"
  fi
  exit
}

start
build_container
description

msg_ok "Completed successfully!\n"
echo -e "${CREATING}${GN}${APP} setup has been successfully initialized!${CL}"
echo -e "${INFO}${YW}Access it using the following URL (the setup token is HOPE_SETUP_TOKEN in /etc/hope/hope.env):${CL}"
echo -e "${GATEWAY}${BGN}https://${IP}${CL}"
