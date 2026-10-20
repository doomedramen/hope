#!/bin/sh
# Build a Linux agent target inside Docker (for example on a Mac), natively when the container
# architecture matches the host and under emulation otherwise.
#
#   scripts/build-agent-docker.sh x86_64-unknown-linux-musl [--out DIR]
#
# Same output as scripts/build-agent.sh. The source tree is mounted read-only; a named volume per
# The container runs as root only to install musl-tools and write the build cache; the outputs are chowned
# to the invoking user. The commit is read on the host because the read-only mount cannot be queried by git
# as another owner. A named volume per architecture caches the cargo target directory (remove with: docker volume rm hope-agent-target-<arch>).
set -eu
target=${1:?usage: build-agent-docker.sh <triple> [--out DIR]}
out=dist/agents
if [ "${2:-}" = --out ]; then out=${3:?--out needs a directory}; fi
case "$target" in
    x86_64-unknown-linux-musl) arch=x86_64; platform=linux/amd64 ;;
    *) echo "build-agent-docker: only x86_64-unknown-linux-musl is supported" >&2; exit 1 ;;
esac
repo=$(cd "$(dirname "$0")/.." && pwd -P)
mkdir -p "$out"
out=$(cd "$out" && pwd -P)
commit=$(git -C "$repo" rev-parse HEAD 2>/dev/null || echo unknown)
docker run --rm --platform "$platform" -e HOPE_COMMIT="$commit" -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
    -v "$repo":/src:ro -v "$out":/out \
    -v "hope-agent-target-$arch":/cache -e CARGO_TARGET_DIR=/cache/target \
    -v "hope-agent-cargo-$arch":/usr/local/cargo/registry \
    -w /src rust:1.96-bookworm \
    sh -c "apt-get update -qq && apt-get install -y -qq --no-install-recommends musl-tools binutils >/dev/null && scripts/build-agent.sh $target --out /out && chown \"\$HOST_UID:\$HOST_GID\" /out/hope-agent-linux-$arch /out/hope-agent-linux-$arch.info"
