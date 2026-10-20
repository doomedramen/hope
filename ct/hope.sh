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

  # Recover an interrupted activation before asking the release engine about versions.
  ensure_dependencies python3 openssl
  if [[ -f /var/lib/hope-activation/recover.py ]]; then
    python3 /var/lib/hope-activation/recover.py recover || return 1
  fi
  if check_for_gh_release "hope" "doomedramen/hope"; then
    local staged release activation release_dir
    staged=$(mktemp -d /var/tmp/hope-update.XXXXXX)
    release=$CHECK_UPDATE_RELEASE
    # Keep the engine's installed-version marker unchanged until activation is proven.
    if ! FORCE_UPDATE=1 CLEAN_INSTALL=1 fetch_and_deploy_gh_release "hope-staged" "doomedramen/hope" "prebuild" "$release" "$staged/server" "hope-server-x86_64-linux.tar.gz"; then
      rm -rf "$staged"
      return 1
    fi
    activation=$staged/server/tools/activate-server.py
    if [[ ! -f "$activation" ]]; then
      msg_error "This release lacks recoverable server activation; existing installation kept"
      rm -rf "$staged"
      return 1
    fi
    cp -p /etc/hope/hope.env "$staged/hope.env"
    release_dir=$(python3 "$activation" value --env "$staged/hope.env" --key HOPE_RELEASE_DIR)
    release_dir=${release_dir:-/var/lib/hope/releases}
    # The old checkout, toolchains, configuration and signed agents remain intact.
    if [[ -f /etc/systemd/system/hope.service ]] && grep -Fxq 'ExecStart=/usr/local/bin/hope-server' /etc/systemd/system/hope.service; then
      cp -p /etc/systemd/system/hope.service "$staged/hope.service"
    else
      cat >"$staged/hope.service" <<EOF
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
    fi
    msg_info "Activating Service"
    if ! python3 "$activation" install --package "$staged/server" --binary /usr/local/bin/hope-server \
        --candidate-env "$staged/hope.env" --candidate-unit "$staged/hope.service"; then
      rm -rf "$staged"
      msg_error "Activation failed; inspect the recovery result above"
      return 1
    fi
    cp "$HOME/.hope-staged" "$HOME/.hope"
    msg_ok "Activated Service"

    # Use the selected release for agents too. Publication follows successful server activation.
    if FORCE_UPDATE=1 CLEAN_INSTALL=1 fetch_and_deploy_gh_release "hope-agents" "doomedramen/hope" "prebuild" "$release" "$staged/agents" "hope-agents.tar.gz"; then
      msg_info "Signing Agent Release"
      if [[ ! -e /etc/hope/release-signing.pem && ! -e "$release_dir/current" ]]; then
        (umask 077 && openssl genrsa -out /etc/hope/release-signing.pem 3072 2>/dev/null) || true
      fi
      if [[ -f /etc/hope/release-signing.pem ]] && $STD env HOPE_SIGNING_KEY=/etc/hope/release-signing.pem sh "$staged/server/tools/assemble-release.sh" --staging "$staged/agents" --version "$(cat "$staged/server/VERSION")" --out "$release_dir"; then
        msg_ok "Signed Agent Release"
      else
        msg_warn "Agent release not signed; the previous one stays published (docs/operations.md, Signed agent releases)"
      fi
    else
      msg_warn "No agent release downloaded; the previous one stays published"
    fi
    rm -rf "$staged"
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
