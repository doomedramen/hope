#!/usr/bin/env bash

# Copyright (c) 2026 Martin Page
# License: MIT
# Source: https://github.com/doomedramen/hope

set -Eeuo pipefail
umask 077

HOPE_ROOT="${HOPE_ROOT:-/opt/hope}"
HOPE_DEPLOY_DIR="${HOPE_DEPLOY_DIR:-$HOPE_ROOT/deploy/compose}"
HOPE_COMPOSE_FILE="$HOPE_DEPLOY_DIR/docker-compose.yml"
HOPE_ENV_FILE="${HOPE_ENV_FILE:-$HOPE_ROOT/.env}"
HOPE_RELEASE_DIR="$HOPE_DEPLOY_DIR/agent-releases"
HOPE_BIN_DIR="${HOPE_BIN_DIR:-/usr/local/sbin}"
HOPE_UPDATE_SCRIPT="${HOPE_UPDATE_SCRIPT:-$HOPE_BIN_DIR/hope-update}"
HOPE_PROJECT_NAME="${HOPE_PROJECT_NAME:-hope}"
hope_scripts_url="${HOPE_SCRIPTS_URL:-${COMMUNITY_SCRIPTS_URL:-https://raw.githubusercontent.com/doomedramen/hope/main}}"
hope_scripts_url="${hope_scripts_url%/}"

stage_compose=""
stage_backup=""
stage_restore=""
stage_update=""
services_stopped=0

log() {
  printf '%s\n' "$*"
}

die() {
  log "error: $*" >&2
  exit 1
}

cleanup() {
  local hope_exit_code=$?

  trap - EXIT
  for stage in "$stage_compose" "$stage_backup" "$stage_restore" "$stage_update"; do
    [[ -z "$stage" ]] || rm -f -- "$stage"
  done
  exit "$hope_exit_code"
}

update_failed() {
  local hope_exit_code=$?

  if [[ "$services_stopped" == 1 ]]; then
    log "Update stopped after it began stopping application services." >&2
    log "Do not start an older image if migrations may have run. See /opt/hope/deploy/compose/restore.sh and https://github.com/doomedramen/hope/blob/main/docs/operations/upgrade-recovery.md." >&2
  fi
  exit "$hope_exit_code"
}

trap cleanup EXIT
trap update_failed ERR

require_command() {
  local command_name

  for command_name in "$@"; do
    command -v "$command_name" >/dev/null 2>&1 ||
      die "required command not found: $command_name"
  done
}

compose() {
  docker compose --project-name "$HOPE_PROJECT_NAME" --env-file "$HOPE_ENV_FILE" \
    -f "$HOPE_COMPOSE_FILE" "$@"
}

fetch_stage() {
  local relative_path=$1
  local destination=$2
  local stage

  stage="$(mktemp "$(dirname "$destination")/.$(basename "$destination").XXXXXX")"
  if ! curl --fail --silent --show-error --location \
    --proto '=https' --proto-redir '=https' \
    --connect-timeout 10 --max-time 120 --retry 3 \
    "${hope_scripts_url}/${relative_path}" -o "$stage"; then
    rm -f -- "$stage"
    return 1
  fi
  printf '%s' "$stage"
}

install_stage() {
  local stage=$1
  local destination=$2
  local mode=$3

  chmod "$mode" "$stage"
  mv -f -- "$stage" "$destination"
}

wait_for_postgres() {
  for _ in {1..60}; do
    # shellcheck disable=SC2016 # Variables expand in PostgreSQL's container shell.
    if compose exec -T postgres sh -c 'pg_isready -U "$POSTGRES_USER" -d "$POSTGRES_DB"' >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done

  die "PostgreSQL did not become ready"
}

wait_for_server() {
  for _ in {1..60}; do
    if compose exec -T server \
      curl --fail --silent --show-error --max-time 5 \
      http://127.0.0.1:8080/health/ready >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done

  die "Hope server did not become ready"
}

[[ "$(id -u)" == 0 ]] || die "run as root"
[[ -f "$HOPE_ENV_FILE" ]] || die "deployment configuration not found: $HOPE_ENV_FILE"
[[ -f "$HOPE_COMPOSE_FILE" ]] || die "deployment compose file not found: $HOPE_COMPOSE_FILE"
require_command curl docker mktemp
docker compose version >/dev/null 2>&1 || die "Docker Compose v2 is required"

install -d -m 0755 "$HOPE_DEPLOY_DIR" "$HOPE_RELEASE_DIR" "$HOPE_BIN_DIR"

log "Fetching deployment files"
stage_compose="$(fetch_stage "deploy/compose/docker-compose.yml" "$HOPE_COMPOSE_FILE")"
stage_backup="$(fetch_stage "deploy/compose/backup.sh" "$HOPE_DEPLOY_DIR/backup.sh")"
stage_restore="$(fetch_stage "deploy/compose/restore.sh" "$HOPE_DEPLOY_DIR/restore.sh")"
stage_update="$(fetch_stage "install/hope-update.sh" "$HOPE_UPDATE_SCRIPT")"

docker compose --project-name "$HOPE_PROJECT_NAME" --env-file "$HOPE_ENV_FILE" \
  -f "$stage_compose" config -q

cp -p -- "$HOPE_COMPOSE_FILE" "$HOPE_COMPOSE_FILE.previous"
install_stage "$stage_compose" "$HOPE_COMPOSE_FILE" 0644
stage_compose=""
install_stage "$stage_backup" "$HOPE_DEPLOY_DIR/backup.sh" 0750
stage_backup=""
install_stage "$stage_restore" "$HOPE_DEPLOY_DIR/restore.sh" 0750
stage_restore=""
install_stage "$stage_update" "$HOPE_UPDATE_SCRIPT" 0750
stage_update=""

log "Pulling Hope image"
compose pull server worker

log "Stopping Hope server and worker"
services_stopped=1
compose stop worker server

log "Starting PostgreSQL"
compose up -d postgres
wait_for_postgres

log "Running database migrations"
compose run --rm --no-deps server migrate

log "Starting Hope server"
compose up -d server
wait_for_server

log "Starting Hope worker"
compose up -d worker
compose ps
log "Hope update complete"
