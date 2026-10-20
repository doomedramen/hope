#!/bin/sh
# Assemble server artifacts downloaded into separate server-<arch> directories.
# The caller puts hope-agents.tar.gz in --out before invoking this script.
set -eu
staging=
out=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --staging) staging=$2; shift 2 ;;
        --out) out=$2; shift 2 ;;
        *) echo "assemble-server-release: unknown argument $1" >&2; exit 1 ;;
    esac
done
if [ -z "$staging" ] || [ -z "$out" ]; then
    echo 'Usage: assemble-server-release.sh --staging DIR --out DIR' >&2
    exit 1
fi
architecture=x86_64
for directory in "$staging"/server-*; do
    [ -e "$directory" ] || continue
    [ "${directory##*/}" = "server-$architecture" ] || {
        echo "Unsupported server artifact directory ${directory##*/}; only Linux x86_64 is supported" >&2
        exit 1
    }
done
directory=$staging/server-$architecture
name=hope-server-$architecture-linux.tar.gz
if [ ! -s "$directory/$name" ] || [ ! -s "$directory/SHA256SUMS-server" ]; then
    echo "Missing server artifact for $architecture" >&2
    exit 1
fi
# Verify the architecture's own manifest before copying; never overlay the manifest.
expected=$(awk -v name="$name" '$2 == name { print $1 }' "$directory/SHA256SUMS-server")
actual=$(sha256sum "$directory/$name" | awk '{ print $1 }')
[ "$expected" = "$actual" ] || { echo "Invalid server checksum for $architecture" >&2; exit 1; }
[ -s "$out/hope-agents.tar.gz" ] || { echo 'Missing agent bundle' >&2; exit 1; }
for artifact in "$out"/hope-server-*-linux.tar.gz; do
    [ -e "$artifact" ] || continue
    [ "${artifact##*/}" = "$name" ] || {
        echo "Unsupported server artifact ${artifact##*/}; only Linux x86_64 is supported" >&2
        exit 1
    }
done
cp "$directory/$name" "$out/$name"
temporary=$(mktemp "$out/.server-checksums.XXXXXX")
trap 'rm -f "$temporary"' EXIT HUP INT TERM
(cd "$out" && sha256sum hope-server-x86_64-linux.tar.gz hope-agents.tar.gz) > "$temporary"
# Explicit filenames above are the complete release contract, independent of stale manifests.
mv "$temporary" "$out/SHA256SUMS-server"
(cd "$out" && sha256sum -c SHA256SUMS-server)
