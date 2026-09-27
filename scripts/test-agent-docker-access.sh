#!/usr/bin/env bash
# Exercise the actual account setup functions without modifying host accounts.
set -Eeuo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root=$(mktemp -d)
trap 'rm -rf -- "$test_root"' EXIT
python3 - "$test_root/docker.sock" <<'PY'
import socket, sys
sock = socket.socket(socket.AF_UNIX)
sock.bind(sys.argv[1])
sock.close()
PY

awk '
  /^ensure_service_account\(\) \{|^configure_docker_access\(\) \{/ { copy=1 }
  copy { print }
  copy && /^\}/ { copy=0 }
' "$repo_root/deploy/agent/install-agent.sh" | sed "s|/var/run/docker.sock|$test_root/docker.sock|g" > "$test_root/functions.sh"
# All account and filesystem mutations are mocked. Only the isolated socket is real.
source "$test_root/functions.sh"
SERVICE_USER=hope-agent
STATE_DIR=/var/lib/hope
log() { printf '%s\n' "$*" >> "$test_root/messages"; }
die() { log "$*"; return 1; }
ensure_nologin_shell() { printf '/usr/sbin/nologin\n'; }
check_path_not_symlink() { :; }
install() { :; }; chown() { :; }; chmod() { :; }; find() { :; }
command() { if [[ "$*" == '-v docker' ]]; then [[ "$cli" == yes ]]; else builtin command "$@"; fi; }
id() {
  case "$1" in
    -u) printf '999\n';; -g) printf '995\n';; -gn) printf 'hope-agent\n';;
    -nG) printf '%s\n' "$test_groups";;
    *) return 1;;
  esac
}
getent() {
  case "$1" in
    passwd) [[ "$existing" == yes ]] && printf 'hope-agent:x:999:995::/var/lib/hope:/usr/sbin/nologin\n';;
    group) printf '%s:x:%s:\n' "$test_socket_group" "$test_socket_gid";;
    *) return 1;;
  esac
}
stat() { printf '%s:%s:%s:%s\n' "$test_socket_uid" "$test_socket_gid" "$test_socket_group" "$test_socket_mode"; }
useradd() { existing=yes; }
usermod() {
  printf '%s\n' "$*" >> "$test_root/usermod"
  [[ "$*" == '-aG docker hope-agent' ]] || return 1
  [[ "$grant_fails" == no ]] || return 1
  test_groups+=' docker'
}
run_as_service() {
  [[ "$*" == "timeout 5s docker --host unix://$test_root/docker.sock version --format {{.Server.Version}}" ]] || return 1
  [[ "$test_groups" == *docker* && "$engine" == up ]]
}
reset_case() {
  existing=yes cli=yes engine=up test_socket_group=docker test_socket_gid=998
  test_socket_uid=0 test_socket_mode=660
  test_groups='hope-agent journal' grant_fails=no DOCKER_ACCESS=auto
  : > "$test_root/usermod"; : > "$test_root/messages"
}
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

for account in no yes; do
  reset_case; existing=$account
  ensure_service_account
  grep -Fqx -- '-aG docker hope-agent' "$test_root/usermod" || fail "Docker access missing for existing=$account"
  [[ "$test_groups" == 'hope-agent journal docker' ]] || fail 'existing groups were lost'
  grep -Fq 'root-equivalent' "$test_root/messages" || fail 'permission implication was not explained'
  grep -Fq 'Docker access verified' "$test_root/messages" || fail 'service-user access was not verified'
  ensure_service_account
  [[ $(wc -l < "$test_root/usermod") -eq 1 ]] || fail 'reinstall changed membership again'
done
for scenario in skip no-cli root-group custom-group non-root-owner not-group-writable missing-socket; do
  reset_case
  case "$scenario" in
    skip) DOCKER_ACCESS=skip;; no-cli) cli=no;;
    root-group) test_socket_group=root test_socket_gid=0;;
    custom-group) test_socket_group=administrators;;
    non-root-owner) test_socket_uid=1000;;
    not-group-writable) test_socket_mode=600;;
    missing-socket) mv "$test_root/docker.sock" "$test_root/saved.sock";;
  esac
  ensure_service_account
  [[ ! -s "$test_root/usermod" ]] || fail "unexpected group grant for $scenario"
done
mv "$test_root/saved.sock" "$test_root/docker.sock"
reset_case
mv "$test_root/docker.sock" "$test_root/real.sock"
ln -s "$test_root/real.sock" "$test_root/docker.sock"
ensure_service_account
[[ ! -s "$test_root/usermod" ]] || fail 'symlinked socket changed permissions'
rm "$test_root/docker.sock"
mv "$test_root/real.sock" "$test_root/docker.sock"
reset_case; engine=down
ensure_service_account
grep -Fq 'still unavailable' "$test_root/messages" || fail 'failed engine probe was hidden'
reset_case; grant_fails=yes
if ensure_service_account; then fail 'failed group grant was ignored'; fi
bash "$repo_root/deploy/agent/install-agent.sh" --help | grep -Fq -- '--docker-access' || fail 'option missing from help'
if bash "$repo_root/deploy/agent/install-agent.sh" --docker-access invalid > "$test_root/invalid" 2>&1; then fail 'invalid policy accepted'; fi
grep -Fq 'auto or skip' "$test_root/invalid" || fail 'invalid policy was not validated'
printf 'Agent Docker access checks passed.\n'
