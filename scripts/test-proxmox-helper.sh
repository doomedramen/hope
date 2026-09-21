#!/usr/bin/env bash

set -Eeuo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root=""

cleanup() {
  local test_exit_code=$?

  trap - EXIT
  [[ -z "$test_root" ]] || rm -rf -- "$test_root"
  exit "$test_exit_code"
}

fail() {
  printf 'test failure: %s\n' "$*" >&2
  exit 1
}

require_file() {
  [[ -f "$1" ]] || fail "missing $1"
}

line_number() {
  grep -n -- "$1" "$2" | head -n 1 | cut -d: -f1
}

require_file "$repo_root/ct/hope.sh"
require_file "$repo_root/install/hope-install.sh"
require_file "$repo_root/install/hope-update.sh"

bash -n "$repo_root/ct/hope.sh"
bash -n "$repo_root/install/hope-install.sh"
bash -n "$repo_root/install/hope-update.sh"

grep -Fqx '_CS_DEFAULT_URL="https://raw.githubusercontent.com/doomedramen/hope/main"' \
  "$repo_root/ct/hope.sh" ||
  fail "Helper Script does not resolve Hope's repository"
grep -Fqx "REPO_SLUG=\"\${REPO_SLUG:-doomedramen/hope}\"" "$repo_root/ct/hope.sh" ||
  fail "Helper Script does not identify Hope's repository"
grep -Fqx 'var_install="hope-install"' "$repo_root/ct/hope.sh" ||
  fail "Helper Script does not select Hope installer"
grep -Fqx "var_arm64=\"\${var_arm64:-no}\"" "$repo_root/ct/hope.sh" ||
  fail "Helper Script must not offer unsupported arm64 images"

docker compose version >/dev/null
compose_test_dir="$(mktemp -d)"
cat >"$compose_test_dir/.env" <<'EOF'
POSTGRES_USER=hope
POSTGRES_PASSWORD=0123456789abcdef0123456789abcdef0123456789abcdef
POSTGRES_DB=hope
HOPE_IMAGE=ghcr.io/doomedramen/hope:main
HOPE_HTTP_PORT=80
HOPE_HTTPS_PORT=443
HOPE_COOKIE_SECURE=false
EOF
docker compose --project-name hope-helper-test --env-file "$compose_test_dir/.env" \
  -f "$repo_root/deploy/compose/docker-compose.yml" config -q
rm -rf -- "$compose_test_dir"

test_root="$(mktemp -d)"
trap cleanup EXIT
mkdir -p "$test_root/bin" "$test_root/hope/deploy/compose"
mkdir -p "$test_root/hope/bin"

cat >"$test_root/bin/curl" <<'EOF'
#!/bin/sh
output_file=""
url=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      output_file=$2
      shift 2
      ;;
    http://*|https://*)
      url=$1
      shift
      ;;
    *)
      shift
      ;;
  esac
done

[ -n "$output_file" ] || exit 1
case "$url" in
  */deploy/compose/docker-compose.yml)
    printf 'services: {}\n' >"$output_file"
    ;;
  */deploy/compose/backup.sh|*/deploy/compose/restore.sh|*/install/hope-update.sh)
    printf '#!/bin/sh\nexit 0\n' >"$output_file"
    ;;
  *)
    exit 1
    ;;
esac
EOF

cat >"$test_root/bin/docker" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$HOPE_TEST_DOCKER_LOG"
exit 0
EOF

cat >"$test_root/bin/id" <<'EOF'
#!/bin/sh
if [ "$1" = "-u" ]; then
  printf '0\n'
  exit 0
fi
exec /usr/bin/id "$@"
EOF

chmod 0755 "$test_root/bin/curl" "$test_root/bin/docker" "$test_root/bin/id"
printf 'POSTGRES_PASSWORD=test\n' >"$test_root/hope/.env"
printf 'services: {old: {}}\n' >"$test_root/hope/deploy/compose/docker-compose.yml"

PATH="$test_root/bin:$PATH" \
HOPE_ROOT="$test_root/hope" \
HOPE_DEPLOY_DIR="$test_root/hope/deploy/compose" \
HOPE_ENV_FILE="$test_root/hope/.env" \
HOPE_BIN_DIR="$test_root/hope/bin" \
HOPE_UPDATE_SCRIPT="$test_root/hope/hope-update" \
HOPE_SCRIPTS_URL="https://example.invalid/hope" \
HOPE_TEST_DOCKER_LOG="$test_root/docker.log" \
  bash "$repo_root/install/hope-update.sh"

grep -Fqx 'services: {}' "$test_root/hope/deploy/compose/docker-compose.yml" ||
  fail "updater did not replace managed Compose file"
[[ -x "$test_root/hope/deploy/compose/backup.sh" ]] ||
  fail "updater did not install backup helper"
[[ -x "$test_root/hope/deploy/compose/restore.sh" ]] ||
  fail "updater did not install restore helper"
[[ -x "$test_root/hope/hope-update" ]] ||
  fail "updater did not refresh itself"

pull_line="$(line_number ' pull server worker$' "$test_root/docker.log")"
stop_line="$(line_number ' stop worker server$' "$test_root/docker.log")"
postgres_line="$(line_number ' up -d postgres$' "$test_root/docker.log")"
migrate_line="$(line_number ' run --rm --no-deps server migrate$' "$test_root/docker.log")"
server_line="$(line_number ' up -d server$' "$test_root/docker.log")"
worker_line="$(line_number ' up -d worker$' "$test_root/docker.log")"

[[ -n "$pull_line" && -n "$stop_line" && -n "$postgres_line" && -n "$migrate_line" && -n "$server_line" && -n "$worker_line" ]] ||
  fail "updater did not run all expected Compose operations"
(( pull_line < stop_line && stop_line < postgres_line && postgres_line < migrate_line )) ||
  fail "updater did not stop writers before migration"
(( migrate_line < server_line && server_line < worker_line )) ||
  fail "updater did not wait to start worker until server migration path completed"

install_root="$test_root/install-hope"
framework_functions="$(cat <<'EOF'
color() { :; }
verb_ip6() { :; }
catch_errors() { :; }
setting_up_container() { :; }
network_check() { :; }
update_os() { :; }
setup_docker() { :; }
msg_info() { :; }
msg_ok() { :; }
msg_error() { printf '%s\n' "$*" >&2; }
run_os_setup() { setup_deb_based; }
motd_ssh() { :; }
customize() { :; }
cleanup_lxc() { :; }
EOF
)"

PATH="$test_root/bin:$PATH" \
FUNCTIONS_FILE_PATH="$framework_functions" \
HOPE_ROOT="$install_root" \
HOPE_BIN_DIR="$install_root/bin" \
HOPE_TEST_DOCKER_LOG="$test_root/install-docker.log" \
STD="" \
  bash "$repo_root/install/hope-install.sh"

install_password="$(sed -n 's/^POSTGRES_PASSWORD=//p' "$install_root/.env")"
[[ "$install_password" =~ ^[0-9a-f]{48}$ ]] ||
  fail "installer did not generate a safe PostgreSQL password"
grep -Fqx 'COMPOSE_PROJECT_NAME=hope' "$install_root/.env" ||
  fail "installer did not pin its Compose project name"
grep -Fqx 'HOPE_HTTP_PORT=80' "$install_root/.env" ||
  fail "installer did not publish HTTP on port 80"
grep -Fqx 'HOPE_HTTPS_PORT=443' "$install_root/.env" ||
  fail "installer did not publish agent HTTPS on port 443"
[[ -f "$install_root/deploy/compose/docker-compose.yml" ]] ||
  fail "installer did not deploy Compose file"
[[ -x "$install_root/deploy/compose/backup.sh" ]] ||
  fail "installer did not deploy backup helper"
[[ -x "$install_root/bin/hope-update" ]] ||
  fail "installer did not deploy local updater"

PATH="$test_root/bin:$PATH" \
FUNCTIONS_FILE_PATH="$framework_functions" \
HOPE_ROOT="$install_root" \
HOPE_BIN_DIR="$install_root/bin" \
HOPE_TEST_DOCKER_LOG="$test_root/install-docker.log" \
STD="" \
  bash "$repo_root/install/hope-install.sh"

[[ "$(sed -n 's/^POSTGRES_PASSWORD=//p' "$install_root/.env")" == "$install_password" ]] ||
  fail "installer replaced an existing deployment password"

printf 'Proxmox helper script checks passed.\n'
