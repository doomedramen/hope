#!/usr/bin/env bash

# Copyright (c) 2026 Martin Page
# License: MIT
# Source: https://github.com/doomedramen/hope

# shellcheck disable=SC1091 # Helper Script framework passes bootstrap functions by stdin.
source /dev/stdin <<<"$FUNCTIONS_FILE_PATH"
color
verb_ip6
catch_errors
setting_up_container
network_check
update_os

readonly HOPE_ROOT="${HOPE_ROOT:-/opt/hope}"
readonly HOPE_DEPLOY_DIR="${HOPE_DEPLOY_DIR:-$HOPE_ROOT/deploy/compose}"
readonly HOPE_COMPOSE_FILE="${HOPE_COMPOSE_FILE:-$HOPE_DEPLOY_DIR/docker-compose.yml}"
readonly HOPE_ENV_FILE="${HOPE_ENV_FILE:-$HOPE_ROOT/.env}"
readonly HOPE_RELEASE_DIR="${HOPE_RELEASE_DIR:-$HOPE_DEPLOY_DIR/agent-releases}"
readonly HOPE_BIN_DIR="${HOPE_BIN_DIR:-/usr/local/sbin}"
readonly HOPE_UPDATE_SCRIPT="${HOPE_UPDATE_SCRIPT:-$HOPE_BIN_DIR/hope-update}"

hope_scripts_url="${COMMUNITY_SCRIPTS_URL:-https://raw.githubusercontent.com/doomedramen/hope/main}"
hope_scripts_url="${hope_scripts_url%/}"

download_hope_file() {
  local relative_path=$1
  local destination=$2
  local mode=$3
  local stage

  stage="$(mktemp "${destination}.XXXXXX")"
  if ! curl --fail --silent --show-error --location \
    --proto '=https' --proto-redir '=https' \
    --connect-timeout 10 --max-time 120 --retry 3 \
    "${hope_scripts_url}/${relative_path}" -o "$stage"; then
    rm -f -- "$stage"
    return 1
  fi

  chmod "$mode" "$stage"
  mv -f -- "$stage" "$destination"
}

generate_postgres_password() {
  od -An -N24 -tx1 /dev/urandom | tr -d ' \n'
}

write_deployment_env() {
  local password

  if [[ -e "$HOPE_ENV_FILE" ]]; then
    msg_info "Keeping existing Hope configuration"
    return
  fi

  password="$(generate_postgres_password)"
  [[ "${#password}" == 48 ]] || {
    msg_error "Could not generate PostgreSQL password"
    exit 1
  }

  umask 077
  cat <<EOF >"$HOPE_ENV_FILE"
# Managed by Hope's Proxmox VE Helper Script.
# Change settings here. Keep this file private.
COMPOSE_PROJECT_NAME=hope
HOPE_IMAGE=ghcr.io/doomedramen/hope:main

POSTGRES_USER=hope
POSTGRES_PASSWORD=${password}
POSTGRES_DB=hope

HOPE_COOKIE_SECURE=false
# Configure a certificate and public URLs before enrolling remote agents.
HOPE_AGENT_ENROLL_URL=https://localhost:8444
HOPE_AGENT_GATEWAY_URL=wss://localhost:8443
HOPE_AGENT_RELEASE_DIR_HOST=./agent-releases
EOF
  chmod 0600 "$HOPE_ENV_FILE"
}

wait_for_server() {
  for _ in {1..60}; do
    if docker compose --project-name hope --env-file "$HOPE_ENV_FILE" \
      -f "$HOPE_COMPOSE_FILE" exec -T server \
      curl --fail --silent --show-error --max-time 5 \
      http://127.0.0.1:8080/health/ready >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done

  msg_error "Hope did not become ready. Inspect: docker compose --project-name hope --env-file $HOPE_ENV_FILE -f $HOPE_COMPOSE_FILE logs --tail=200"
  return 1
}

setup_deb_based() {
  msg_info "Installing Docker Engine and Compose"
  setup_docker
  msg_ok "Installed Docker Engine and Compose"

  msg_info "Preparing Hope deployment"
  install -d -m 0755 "$HOPE_DEPLOY_DIR" "$HOPE_RELEASE_DIR" "$HOPE_BIN_DIR"
  write_deployment_env
  download_hope_file "deploy/compose/docker-compose.yml" "$HOPE_COMPOSE_FILE" 0644
  download_hope_file "deploy/compose/backup.sh" "$HOPE_DEPLOY_DIR/backup.sh" 0750
  download_hope_file "deploy/compose/restore.sh" "$HOPE_DEPLOY_DIR/restore.sh" 0750
  download_hope_file "install/hope-update.sh" "$HOPE_UPDATE_SCRIPT" 0750
  docker compose --project-name hope --env-file "$HOPE_ENV_FILE" \
    -f "$HOPE_COMPOSE_FILE" config -q
  msg_ok "Prepared Hope deployment"

  msg_info "Pulling Hope image"
  $STD docker compose --project-name hope --env-file "$HOPE_ENV_FILE" \
    -f "$HOPE_COMPOSE_FILE" pull
  msg_ok "Pulled Hope image"

  msg_info "Starting Hope"
  $STD docker compose --project-name hope --env-file "$HOPE_ENV_FILE" \
    -f "$HOPE_COMPOSE_FILE" up -d
  wait_for_server
  msg_ok "Started Hope"
}

run_os_setup

motd_ssh
customize
cleanup_lxc
