#!/usr/bin/env bash
set -Eeuo pipefail

umask 077

usage() {
  cat >&2 <<'EOF'
Usage: backup.sh [backup-directory]

Creates a complete Hope backup containing the PostgreSQL database, server PKI,
agent release repository, and credential master key.

The credential master key is intentionally included only when
HOPE_BACKUP_INCLUDE_SECRETS=1 is set. Store the resulting directory as a
secret and do not put it in source control.
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/../.." && pwd)"
backup_root="${1:-${HOPE_BACKUP_DIR:-$repo_root/backups}}"
backup_root="$(mkdir -p -- "$backup_root" && cd -- "$backup_root" && pwd)"

case "${HOPE_BACKUP_INCLUDE_SECRETS:-}" in
  1|true|TRUE|yes|YES) ;;
  *)
    printf '%s\n' "Refusing an incomplete backup: set HOPE_BACKUP_INCLUDE_SECRETS=1 to include the credential master key." >&2
    exit 2
    ;;
esac

if ! command -v docker >/dev/null 2>&1; then
  printf '%s\n' "docker is required" >&2
  exit 1
fi

compose=(docker compose --project-directory "$repo_root" -f "$script_dir/docker-compose.yml")
created_at="$(date -u +%Y%m%dT%H%M%SZ)"
destination="$backup_root/hope-$created_at"
key_path="${HOPE_CREDENTIAL_MASTER_KEY_FILE_HOST:-$script_dir/secrets/credential-master-key}"

if [[ ! -f "$key_path" ]]; then
  printf 'Credential master key not found: %s\n' "$key_path" >&2
  exit 1
fi

if [[ ! -d "$script_dir/agent-releases" ]]; then
  printf 'Agent release repository not found: %s\n' "$script_dir/agent-releases" >&2
  exit 1
fi

mkdir -p -- "$destination"
cleanup() {
  if [[ "${backup_succeeded:-0}" != 1 ]]; then
    rm -rf -- "$destination"
  fi
}
trap cleanup EXIT

printf 'Writing backup to %s\n' "$destination"

postgres_container="$("${compose[@]}" ps -aq postgres)"
if [[ -z "$postgres_container" ]]; then
  printf '%s\n' "PostgreSQL container does not exist; start the Compose stack before backing up." >&2
  exit 1
fi

server_container="$("${compose[@]}" ps -aq server)"
if [[ -z "$server_container" ]]; then
  printf '%s\n' "Server container does not exist; start the Compose stack before backing up." >&2
  exit 1
fi

"${compose[@]}" exec -T postgres sh -c 'pg_dump --format=custom --no-owner --no-acl --username="$POSTGRES_USER" --dbname="$POSTGRES_DB"' >"$destination/postgres.dump"
"${compose[@]}" exec -T postgres sh -c 'pg_dumpall --globals-only --no-role-passwords --username="$POSTGRES_USER"' >"$destination/postgres-globals.sql"

if [[ ! -s "$destination/postgres.dump" ]]; then
  printf '%s\n' "PostgreSQL dump is empty" >&2
  exit 1
fi

"${compose[@]}" exec -T postgres pg_restore --list <"$destination/postgres.dump" >/dev/null

docker cp "$server_container:/app/data/pki" "$destination/"
cp -a -- "$script_dir/agent-releases" "$destination/agent-releases"
install -m 600 -- "$key_path" "$destination/credential-master-key"

git_revision="$(git -C "$repo_root" rev-parse --short HEAD 2>/dev/null || printf '%s' unknown)"
cat >"$destination/manifest.txt" <<EOF
format=hope-backup-v1
created_at=$created_at
git_revision=$git_revision
postgres_dump=postgres.dump
postgres_globals=postgres-globals.sql
server_pki=pki
agent_releases=agent-releases
credential_master_key=credential-master-key
EOF

(
  cd -- "$destination"
  if command -v sha256sum >/dev/null 2>&1; then
    find . -type f ! -name manifest.txt ! -name SHA256SUMS -print0 |
      while IFS= read -r -d '' file; do sha256sum "$file"; done >SHA256SUMS
  elif command -v shasum >/dev/null 2>&1; then
    find . -type f ! -name manifest.txt ! -name SHA256SUMS -print0 |
      while IFS= read -r -d '' file; do shasum -a 256 "$file"; done >SHA256SUMS
  else
    printf '%s\n' "sha256sum or shasum is required" >&2
    exit 1
  fi
)

chmod -R go-rwx -- "$destination"
backup_succeeded=1
printf 'Backup complete: %s\n' "$destination"
