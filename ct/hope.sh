#!/usr/bin/env bash
# H.O.P.E on Proxmox VE, built by the community-scripts engine (community-scripts/core).
#
#   bash -c "$(curl -fsSL https://raw.githubusercontent.com/doomedramen/hope/main/ct/hope.sh)"
#
# The engine creates and configures the container (defaults or advanced settings, storage,
# template, network, resources, the in-container `update` command) and runs install/hope-install.sh
# from the scripts root below inside it. COMMUNITY_SCRIPTS_URL names that root; it defaults to this
# repository so the pair works without being merged into community-scripts/ProxmoxVE (set it to a
# branch of your fork to test changes). The engine's own var_* settings apply, for example
# var_cpu, var_ram, var_disk, var_brg, var_net, var_unprivileged.
COMMUNITY_SCRIPTS_URL="${COMMUNITY_SCRIPTS_URL:-https://raw.githubusercontent.com/doomedramen/hope/main}"
export COMMUNITY_SCRIPTS_URL
# Banners live in the engine's own repository and are generated only for merged apps, so the engine
# would 404 (visibly) looking for ours. Seed its header cache with the same slant-font banner the
# generator would produce; the engine reads the cache before fetching anything.
_hope_header_cache="${COMMUNITY_SCRIPTS_HEADER_CACHE:-/usr/local/community-scripts}/headers/ct/hope"
if [[ ! -s "$_hope_header_cache" ]] && mkdir -p "$(dirname "$_hope_header_cache")" 2>/dev/null; then
  cat >"$_hope_header_cache" <<'HOPE_BANNER' || rm -f "$_hope_header_cache"
    __  ______  ____  ______
   / / / / __ \/ __ \/ ____/
  / /_/ / / / / /_/ / __/
 / __  / /_/ / ____/ /___
/_/ /_/\____/_/   /_____/
HOPE_BANNER
fi
unset _hope_header_cache
_cs_boot="${COMMUNITY_SCRIPTS_CORE_DIR:-$(dirname "${BASH_SOURCE[0]}")/../../core}/core/build.func"
source "$_cs_boot" 2>/dev/null || source <(curl -fsSL "${COMMUNITY_SCRIPTS_CORE_URL:-https://raw.githubusercontent.com/community-scripts/core/main}/core/build.func")
# Copyright (c) 2021-2026 community-scripts ORG
# Author: doomedramen
# License: MIT | https://github.com/community-scripts/ProxmoxVE/raw/main/LICENSE
# Source: https://github.com/doomedramen/hope

APP="HOPE"
var_tags="${var_tags:-monitoring;network}"
var_cpu="${var_cpu:-2}"
var_ram="${var_ram:-2048}"
var_disk="${var_disk:-8}"
var_os="${var_os:-debian}"
var_version="${var_version:-13}"
var_arm64="${var_arm64:-yes}"
var_unprivileged="${var_unprivileged:-1}"

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

  if check_for_gh_release "hope" "doomedramen/hope"; then
    msg_info "Stopping Service"
    systemctl stop hope
    msg_ok "Stopped Service"

    fetch_and_deploy_gh_release "hope" "doomedramen/hope" "prebuild" "latest" "/opt/hope/release" "hope-server-$(arch_resolve x86_64 aarch64)-linux.tar.gz"
    msg_info "Updating ${APP}"
    install -m 755 /opt/hope/release/hope-server /opt/hope/bin/hope-server
    setcap cap_net_raw=ep /opt/hope/bin/hope-server
    rm -rf /opt/hope/web
    cp -R /opt/hope/release/web /opt/hope/web
    msg_ok "Updated ${APP}"

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
echo -e "${INFO}${YW}Access it using the following URL (the setup token is in /root/hope.creds):${CL}"
echo -e "${GATEWAY}${BGN}https://${IP}${CL}"
