#!/bin/sh
# Release gate fixture entry point. See tests/gates/README.md.
#   tests/gates/run.sh all        build, start, run every gate, print the matrix, tear down
#   tests/gates/run.sh build|up|proxy|matrix|ssh|report|down|clean-caches
#   tests/gates/run.sh updater    the agent updater under real systemd (a leaner, Linux-only fixture)
# HOPEGATE_WORK=DIR keeps keys, staged artifacts and results between runs (must be outside the repository).
set -eu
here=$(cd "$(dirname "$0")" && pwd -P)
command -v docker >/dev/null 2>&1 || { echo "run.sh: docker is required" >&2; exit 1; }
case "${1:-}" in
    updater) shift; exec python3 -I "$here/updater.py" "$@" ;;
esac
exec python3 -I "$here/gates.py" "$@"
