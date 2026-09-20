#!/usr/bin/env bash
set -Eeuo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ -z "${M10_DATABASE_URL:-}" ]]; then
  printf '%s\n' \
    'M10_DATABASE_URL is required. Use a throwaway PostgreSQL database.' >&2
  exit 2
fi

cd -- "$repo_root"
exec cargo test --release -p server --test m10_scale -- \
  --ignored --nocapture --test-threads=1
