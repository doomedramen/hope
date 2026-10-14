#!/bin/sh
# Build release binaries and run the repeatable agent resource benchmark.
# Usage: scripts/bench-agent.sh [bench-agent.py options]   (default: 60 s warmup, 1800 s measurement, 300 s outage)
set -eu
root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
command -v python3 >/dev/null 2>&1 || { echo "python3 is required" >&2; exit 1; }
command -v openssl >/dev/null 2>&1 || { echo "openssl is required" >&2; exit 1; }
cargo build --release --locked -p hope-server -p hope-agent
exec python3 -I scripts/bench-agent.py "$@"
