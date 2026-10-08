#!/bin/sh
# Replace the contract only after a successful generator run.
set -eu
cd "$(dirname "$0")/.."
temporary=$(mktemp web/lib/protocol.ts.XXXXXX)
trap 'rm -f "$temporary"' EXIT HUP INT TERM
cargo run --quiet --locked -p hope-server -- export-types > "$temporary"
test -s "$temporary"
mv "$temporary" web/lib/protocol.ts
