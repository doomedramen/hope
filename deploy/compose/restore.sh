#!/usr/bin/env bash
set -Eeuo pipefail

umask 077

usage() {
  cat >&2 <<'EOF'
Usage: restore.sh BACKUP-DIRECTORY --force --with-secrets

Restores a backup produced by backup.sh into the Compose deployment. This
replaces the database and server PKI. --force and --with-secrets are required;
the database credentials cannot be decrypted with a different key.
EOF
}

if [[ "$#" -lt 2 || "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 2
fi

if [[ ! -d "$1" ]]; then
  printf 'Backup directory not found: %s\n' "$1" >&2
  exit 1
fi
backup_dir="$(cd "$1" && pwd)"
shift
force=0
with_secrets=0
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --force) force=1 ;;
    --with-secrets) with_secrets=1 ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'Unknown option: %s\n' "$1" >&2; usage; exit 2 ;;
  esac
  shift
done

if [[ "$force" != 1 ]]; then
  printf '%s\n' "Refusing destructive restore without --force." >&2
  exit 2
fi

if [[ "$with_secrets" != 1 ]]; then
  printf '%s\n' "Refusing restore without --with-secrets: the credential master key is part of server-pki." >&2
  exit 2
fi

for required in manifest.txt SHA256SUMS postgres.dump postgres-globals.sql pki agent-releases deployment.env; do
  if [[ ! -e "$backup_dir/$required" ]]; then
    printf 'Backup is missing %s\n' "$required" >&2
    exit 1
  fi
done

if [[ ! -f "$backup_dir/credential-master-key" ]]; then
  printf '%s\n' "Backup is missing credential-master-key." >&2
  exit 1
fi

(
  cd "$backup_dir"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c SHA256SUMS
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c SHA256SUMS
  else
    printf '%s\n' "sha256sum or shasum is required" >&2
    exit 1
  fi
)

if ! command -v docker >/dev/null 2>&1; then
  printf '%s\n' "docker is required" >&2
  exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
compose=(docker compose --env-file "$backup_dir/deployment.env" -f "$script_dir/docker-compose.yml")
release_dir="${HOPE_AGENT_RELEASE_DIR_HOST:-$script_dir/agent-releases}"
helper_id=""

cleanup() {
  if [[ -n "$helper_id" ]]; then
    docker rm -f "$helper_id" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

printf '%s\n' "Stopping application services. The database and PKI will be replaced."
"${compose[@]}" stop server worker >/dev/null
"${compose[@]}" up -d postgres >/dev/null

ready=0
for _ in {1..60}; do
  if "${compose[@]}" exec -T postgres sh -c 'pg_isready -U "$POSTGRES_USER" -d postgres' >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 1
done
if [[ "$ready" != 1 ]]; then
  printf '%s\n' "PostgreSQL did not become ready" >&2
  exit 1
fi

"${compose[@]}" exec -T postgres sh -c 'dropdb --if-exists --username="$POSTGRES_USER" "$POSTGRES_DB" && createdb --username="$POSTGRES_USER" "$POSTGRES_DB"'
"${compose[@]}" exec -T postgres sh -c 'pg_restore --exit-on-error --no-owner --no-acl --username="$POSTGRES_USER" --dbname="$POSTGRES_DB"' <"$backup_dir/postgres.dump"

if [[ -e "$release_dir" ]]; then
  rm -rf "$release_dir"
fi
mkdir -p "$(dirname "$release_dir")"
cp -a "$backup_dir/agent-releases" "$release_dir"

helper_id="$("${compose[@]}" run -d --no-deps --entrypoint sh server -c 'sleep 300')"
docker exec "$helper_id" sh -c 'find /app/data/pki -mindepth 1 -maxdepth 1 -exec rm -rf {} +'
docker cp "$backup_dir/pki" "$helper_id:/app/data/"
docker cp "$backup_dir/credential-master-key" \
  "$helper_id:/app/data/pki/credential-master-key"
docker exec "$helper_id" chmod 0400 /app/data/pki/credential-master-key
docker exec "$helper_id" chmod -R go-rwx /app/data/pki
docker rm -f "$helper_id" >/dev/null
helper_id=""

"${compose[@]}" up -d server worker >/dev/null
"${compose[@]}" ps
printf 'Restore complete from %s\n' "$backup_dir"
