#!/usr/bin/env bash

# Copyright (c) 2026 Martin Page
# License: MIT
# Source: https://github.com/doomedramen/hope

_CS_DEFAULT_URL="https://raw.githubusercontent.com/doomedramen/hope/main"
_cs_boot="${COMMUNITY_SCRIPTS_CORE_DIR:-$(dirname "${BASH_SOURCE[0]}")/../../core}/core/build.func"
# shellcheck disable=SC1090
source "$_cs_boot" 2>/dev/null ||
  source <(curl -fsSL "${COMMUNITY_SCRIPTS_CORE_URL:-https://raw.githubusercontent.com/community-scripts/core/main}/core/build.func")
REPO_SLUG="${REPO_SLUG:-doomedramen/hope}"

APP="Hope"
var_tags="${var_tags:-monitoring;network}"
var_cpu="${var_cpu:-2}"
var_ram="${var_ram:-4096}"
var_disk="${var_disk:-16}"
var_os="${var_os:-debian}"
var_version="${var_version:-13}"
var_arm64="${var_arm64:-no}"
var_unprivileged="${var_unprivileged:-1}"
# shellcheck disable=SC2034 # Read by the Helper Script framework.
var_install="hope-install"

header_info "$APP"
variables
color
catch_errors

function update_script() {
  header_info
  check_container_storage
  check_container_resources

  if [[ ! -f /opt/hope/.env || ! -f /opt/hope/deploy/compose/docker-compose.yml ]]; then
    msg_error "No ${APP} installation found at /opt/hope"
    exit 1
  fi

  local updater
  if ! updater="$(_cs_fetch_text "install/hope-update.sh")" || [[ -z "$updater" ]]; then
    msg_error "Could not fetch install/hope-update.sh"
    exit 1
  fi

  msg_info "Updating ${APP}"
  bash -c "$updater"
  msg_ok "Updated ${APP}"
  exit 0
}

start
build_container
description

msg_ok "Completed successfully!"
echo -e "${CREATING}${GN}${APP} setup has been successfully initialized!${CL}"
echo -e "${INFO}${YW}Access it using the following URL:${CL}"
echo -e "${GATEWAY}${BGN}http://${IP}${CL}"
echo -e "${INFO}${YW}Agent gateway and enrollment use ports 8443 and 8444. Configure their public URLs before enrolling remote agents.${CL}"
