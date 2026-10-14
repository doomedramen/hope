#!/bin/sh
# Local-development wrapper: build the agent for THIS machine and publish a PARTIAL signed release
# into ./releases (one platform only). Real releases are built per target on CI and published with
# scripts/assemble-release.sh from all four artifacts (see docs/operations.md).
#
#   HOPE_SIGNING_KEY=/path/outside/repo/dev-signing.pem scripts/package-agent.sh
#
# Publishes a new releases/<version>-<random> directory and repoints releases/current at it (older ones
# are pruned by assemble-release.sh). To switch to a different development key, delete the release
# directory first: assemble-release.sh refuses a key change, and --rotate-key is for complete releases.
set -eu
cd "$(dirname "$0")/.."
: "${HOPE_SIGNING_KEY:?Set HOPE_SIGNING_KEY to an RSA private key file stored outside the repository}"
case "$(uname -s)" in
    Linux) system=unknown-linux-musl ;;
    Darwin) system=apple-darwin ;;
    *) echo "package-agent: unsupported OS $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
    x86_64|amd64) architecture=x86_64 ;;
    arm64|aarch64) architecture=aarch64 ;;
    *) echo "package-agent: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac
repo=$(pwd -P)
staging=${HOPE_STAGING_DIR:-dist/agents-dev}
case "$staging" in /*) ;; *) staging=$repo/$staging ;; esac
# This directory is deleted below, so insist it is a scratch location: inside ./dist or the temp directory.
tmpbase=$(cd "${TMPDIR:-/tmp}" 2>/dev/null && pwd -P || echo /tmp)
case "$staging" in
    "$repo"/dist/?*|"$tmpbase"/?*|/tmp/?*) ;;
    *) echo "package-agent: HOPE_STAGING_DIR must be inside $repo/dist or $tmpbase (got $staging)" >&2; exit 1 ;;
esac
case "$staging" in */..|*/../*|"$HOME"|"$repo"|/) echo "package-agent: refusing HOPE_STAGING_DIR=$staging" >&2; exit 1 ;; esac
rm -rf "$staging"
scripts/build-agent.sh "$architecture-$system" --out "$staging"
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
scripts/assemble-release.sh --staging "$staging" --version "$version" --out "${HOPE_RELEASE_DIR:-releases}" --allow-partial
