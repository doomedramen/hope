#!/bin/sh
# Package the server for install-hope.sh: hope-server-<arch>-linux.tar.gz holding the release
# binary, the static console export, the licence, VERSION and tools/assemble-release.sh, plus a
# SHA256SUMS-server line.
#
#   scripts/package-server.sh --out dist/server          # builds web/out and target/release first
#   scripts/package-server.sh --out dist/server --no-build
set -eu
cd "$(dirname "$0")/.."
out=dist/server
build=1
while [ $# -gt 0 ]; do
    case "$1" in
    --out) out=$2; shift 2 ;;
    --no-build) build=0; shift ;;
    *) echo "package-server: unknown argument $1" >&2; exit 1 ;;
    esac
done
[ "$(uname -s)" = Linux ] || { echo "package-server: the server package is Linux only" >&2; exit 1; }
case "$(uname -m)" in
    x86_64|amd64) architecture=x86_64 ;;
    *) echo "package-server: H.O.P.E server packages require Linux x86_64 (found $(uname -m))" >&2; exit 1 ;;
esac
if [ "$build" = 1 ]; then
    (cd web && npm ci --no-audit --no-fund && npm run build)
    cargo build --release --locked -p hope-server
fi
[ -x target/release/hope-server ] || { echo "package-server: target/release/hope-server is missing" >&2; exit 1; }
[ -f web/out/index.html ] || { echo "package-server: web/out/index.html is missing" >&2; exit 1; }
# CI names each build (HOPE_BUILD_VERSION); the agents in the same release carry that version, and
# the installer signs them with --version "$(cat VERSION)", so the two must agree.
version=${HOPE_BUILD_VERSION:-$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)}
staging=$(mktemp -d "${TMPDIR:-/tmp}/hope-server-package.XXXXXX")
trap 'rm -rf "$staging"' EXIT
mkdir -p "$staging/hope-server-$version" "$out"
install -m 755 target/release/hope-server "$staging/hope-server-$version/hope-server"
cp -R web/out "$staging/hope-server-$version/web"
cp LICENSE "$staging/hope-server-$version/LICENSE"
# install-hope.sh signs the release's hope-agents.tar.gz with this, using the server's own key.
mkdir -p "$staging/hope-server-$version/tools"
install -m 755 scripts/assemble-release.sh "$staging/hope-server-$version/tools/assemble-release.sh"
install -m 755 scripts/activate-server.py "$staging/hope-server-$version/tools/activate-server.py"
printf '%s\n' "$version" > "$staging/hope-server-$version/VERSION"
name=hope-server-$architecture-linux.tar.gz
# Reproducible ordering and ownership; the installer extracts into its own staging directory.
tar -C "$staging/hope-server-$version" --owner=0 --group=0 --numeric-owner --sort=name -czf "$out/$name" .
(cd "$out" && sha256sum "$name" >> SHA256SUMS-server && sort -u -k2 SHA256SUMS-server -o SHA256SUMS-server)
echo "package-server: wrote $out/$name ($(wc -c < "$out/$name") bytes)"
