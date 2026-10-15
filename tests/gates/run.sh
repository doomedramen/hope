#!/bin/sh
# Release gate fixture entry point. See tests/gates/README.md.
#   tests/gates/run.sh all        build, start, run every gate, print the matrix, tear down
#   tests/gates/run.sh build|up|proxy|matrix|ssh|report|down|clean-caches
# HOPEGATE_WORK=DIR keeps keys, staged artifacts and results between runs (must be outside the repository).
set -eu
here=$(cd "$(dirname "$0")" && pwd -P)
command -v docker >/dev/null 2>&1 || { echo "run.sh: docker is required" >&2; exit 1; }
exec python3 -I "$here/gates.py" "$@"
