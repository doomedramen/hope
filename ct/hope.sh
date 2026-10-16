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
var_cpu="${var_cpu:-2}"
var_ram="${var_ram:-4096}"
var_disk="${var_disk:-12}"
var_os="${var_os:-debian}"
var_version="${var_version:-13}"
var_arm64="${var_arm64:-yes}"
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

  if [[ ! -d /opt/hope ]]; then
    msg_error "No ${APP} Installation Found!"
    exit
  fi

  # Containers built before the branch-tracking layout kept the binary and console under /opt/hope,
  # which is now the checkout; move their unit, settings and releases over and force a rebuild.
  if grep -q '^ExecStart=/opt/hope/bin/hope-server' /etc/systemd/system/hope.service 2>/dev/null; then
    msg_info "Migrating H.O.P.E to the current layout"
    systemctl stop hope
    mkdir -p /var/lib/hope/releases
    [[ -d /opt/hope/releases ]] && cp -a /opt/hope/releases/. /var/lib/hope/releases/
    [[ -f /opt/hope/bin/hope-server ]] && install -m 755 /opt/hope/bin/hope-server /usr/local/bin/hope-server
    sed -i -e 's|^HOPE_WEB_DIR=.*|HOPE_WEB_DIR=/opt/hope/web/out|' -e 's|^HOPE_RELEASE_DIR=.*|HOPE_RELEASE_DIR=/var/lib/hope/releases|' /etc/hope/hope.env
    grep -q '^HOPE_SETUP_TOKEN=' /etc/hope/hope.env || echo "HOPE_SETUP_TOKEN=$(random_password 32)" >>/etc/hope/hope.env
    chmod 600 /etc/hope/hope.env
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
    systemctl daemon-reload
    rm -f /root/.hope
    msg_ok "Migrated H.O.P.E to the current layout"
  fi

  if check_for_gh_branch "hope" "doomedramen/hope" "main"; then
    msg_info "Stopping Service"
    systemctl stop hope
    msg_ok "Stopped Service"

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
