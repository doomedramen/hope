#!/bin/sh
# Build one release agent for one target triple and record what the artifact actually requires.
#
#   scripts/build-agent.sh <target-triple> [--out DIR] [--profile gate]
#
# Supported target (the release matrix):
#   x86_64-unknown-linux-musl   fully static; no libc or loader needed
#
# Output in DIR (default dist/agents):
#   hope-agent-<platform>-<arch>        the binary, named as the server and installer expect
#   hope-agent-<platform>-<arch>.info   key=value: version, target, min_os, commit (read by assemble-release.sh)
#
# A target must be built on a host that can link and ideally run it (see .github/workflows). This script
# never signs anything; signing happens only in scripts/assemble-release.sh.
# --profile gate builds the quick, unoptimised test agents the updater gate needs; releases use the default.
set -eu

die() { echo "build-agent: $*" >&2; exit 1; }

target=
out=dist/agents
profile=release
while [ $# -gt 0 ]; do
    case "$1" in
        --out) [ $# -ge 2 ] || die '--out needs a directory'; out=$2; shift ;;
        --profile)
            [ $# -ge 2 ] || die '--profile needs a name'
            case "$2" in release|gate) profile=$2 ;; *) die "unknown profile $2 (release or gate)" ;; esac
            shift ;;
        -*) die "unknown option $1" ;;
        *) [ -z "$target" ] || die 'only one target triple is accepted'; target=$1 ;;
    esac
    shift
done
[ -n "$target" ] || die 'usage: build-agent.sh <target-triple> [--out DIR]'

case "$target" in
    x86_64-unknown-linux-musl) platform=linux; arch=x86_64 ;;
    *) die "unsupported target $target" ;;
esac

cd "$(dirname "$0")/.."
host_os=$(uname -s)
host_arch=$(uname -m)
case "$host_arch" in arm64) host_arch=aarch64 ;; amd64) host_arch=x86_64 ;; esac
case "$platform:$host_os" in linux:Linux) ;; *) die "$target must be built on a Linux host (this is $host_os)" ;; esac
if [ "$platform" = linux ] && [ "$arch" != "$host_arch" ]; then
    die "$target needs a $arch Linux host (this is $host_arch); cross-linking is deliberately not supported so the artifact can be smoke-tested"
fi

if command -v rustup >/dev/null 2>&1; then
    rustup target list --installed | grep -qx "$target" || rustup target add "$target"
fi

underscore=$(printf '%s' "$target" | tr '-' '_')
if command -v musl-gcc >/dev/null 2>&1; then
    eval "[ -n \"\${CC_$underscore:-}\" ] || export CC_$underscore=musl-gcc"
fi

cargo build --profile "$profile" --locked -p hope-agent --target "$target"
built="${CARGO_TARGET_DIR:-target}/$target/$profile/hope-agent"
[ -s "$built" ] || die "cargo did not produce $built"

# HOPE_BUILD_VERSION (set by CI for published builds) is also compiled into the binary.
version=${HOPE_BUILD_VERSION:-$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)}
[ -n "$version" ] || die 'cannot read the workspace version from Cargo.toml'
commit=${HOPE_COMMIT:-$(git rev-parse HEAD 2>/dev/null || echo unknown)}
commit=$(printf '%s' "$commit" | cut -c1-12)
case "$commit" in *[!0-9a-f]*|'') commit=unknown ;; esac

# ---- determine what the artifact actually requires -------------------------------------------------
case "$platform" in
    macos)
        command -v otool >/dev/null 2>&1 || die 'otool is required to read the Mach-O minimum OS'
        minos=$(otool -l "$built" | awk '/LC_BUILD_VERSION/ {b=1} b && $1 == "minos" {print $2; exit}')
        if [ -z "$minos" ]; then
            minos=$(otool -l "$built" | awk '/LC_VERSION_MIN_MACOSX/ {b=1} b && $1 == "version" {print $2; exit}')
        fi
        [ -n "$minos" ] || die 'no minimum macOS version found in the binary'
        [ "$minos" = "$MACOS_MINIMUM" ] || die "binary minos is $minos but the pinned deployment target is $MACOS_MINIMUM"
        min_os="macOS $minos or later"
        ;;
    linux)
        command -v readelf >/dev/null 2>&1 || die 'readelf (binutils) is required to inspect the ELF binary'
        needed=$(readelf -d "$built" 2>/dev/null | grep -c '(NEEDED)' || true)
        [ "$needed" = 0 ] || die "musl build still has $needed shared-library dependencies"
        readelf -l "$built" | grep -q 'INTERP' && die 'musl build requests a dynamic loader'
        min_os="Linux kernel 3.2 or later, static musl (no glibc)"
        ;;
esac

mkdir -p "$out"
name="hope-agent-$platform-$arch"
cp "$built" "$out/$name.tmp"
chmod 755 "$out/$name.tmp"
mv "$out/$name.tmp" "$out/$name"
{
    printf 'version=%s\n' "$version"
    printf 'target=%s\n' "$target"
    printf 'min_os=%s\n' "$min_os"
    printf 'commit=%s\n' "$commit"
} > "$out/$name.info"

# Smoke test whenever this host can execute the result.
runner=
if [ "$arch" = "$host_arch" ]; then runner=run
elif [ "$arch" = x86_64 ] && [ "$host_os" = Darwin ] && [ "$platform" = macos ] && arch -x86_64 true 2>/dev/null; then runner=rosetta
fi
case "$runner" in
    run|rosetta)
        if [ "$runner" = run ]; then reported=$("$out/$name" --version); else reported=$(arch -x86_64 "$out/$name" --version); fi
        [ "$reported" = "hope-agent $version" ] || die "smoke test failed: '$reported'"
        echo "build-agent: smoke test ok ($reported)" ;;
    *) echo "build-agent: $name not executed (different architecture and no emulation here)" ;;
esac
echo "build-agent: built $out/$name for $target; requires: $min_os"
