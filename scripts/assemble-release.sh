#!/bin/sh
# Assemble, sign, verify and atomically publish a release directory.
#
#   HOPE_SIGNING_KEY=/path/outside/repo/release-signing.pem \
#   scripts/assemble-release.sh --staging DIR --version X.Y.Z [--out releases] \
#                               [--allow-partial] [--rotate-key] [--allow-downgrade]
#
# Input:  DIR holds hope-agent-linux-x86_64 and hope-agent-linux-x86_64.info for the supported target
#         release platforms (written by scripts/build-agent.sh).
# Output: OUT/<version>-<random>/ (a complete release directory) and the symlink OUT/current pointing at
#         it. The release holds the binaries, a per-artifact .sig, manifest.json, manifest.json.sig and
#         release-public.pem. The manifest (format 1) lists version, commit, key id, and per artifact the
#         sha256, size, minimum OS and target; its signature is what ties versions together. Point the
#         server's HOPE_RELEASE_DIR (and a Docker bind mount) at OUT, never at a versioned directory.
#
# Safety rules enforced here:
#   * Linux x86_64 is required; --allow-partial (local development only) marks the manifest partial
#   * every artifact must carry the same version and git commit, and its ELF/Mach-O header must match
#     the platform it is labeled with
#   * the signing key must not be inside this repository (path, case-insensitive, or same file)
#   * a different public key than the one already published in OUT is refused unless --rotate-key; a
#     rotation always re-signs every artifact in the same run and cannot be combined with --allow-partial
#   * an older version than the published one is refused unless --allow-downgrade
#   * the release is built in its own directory, verified there against the public key, and published by
#     atomically renaming a new `current` symlink over the old one; the two newest older releases and the
#     previous one are kept, the one `current` points to is never removed
# The signing key is read, never copied; only the public key is written.
set -eu

die() { echo "assemble-release: $*" >&2; exit 1; }
note() { echo "assemble-release: $*"; }

repo=$(cd "$(dirname "$0")/.." && pwd -P)
caller=$PWD
staging=$repo/dist/agents
out=$repo/releases
version=
partial=0
rotate=0
downgrade=0
while [ $# -gt 0 ]; do
    case "$1" in
        --staging) [ $# -ge 2 ] || die '--staging needs a directory'; staging=$2; shift ;;
        --out) [ $# -ge 2 ] || die '--out needs a directory'; out=$2; shift ;;
        --version) [ $# -ge 2 ] || die '--version needs a value'; version=$2; shift ;;
        --allow-partial) partial=1 ;;
        --rotate-key) rotate=1 ;;
        --allow-downgrade) downgrade=1 ;;
        *) die "unknown argument $1" ;;
    esac
    shift
done
abspath() { case "$1" in /*) printf '%s\n' "${1%/}" ;; *) printf '%s\n' "$caller/${1%/}" ;; esac; }
staging=$(abspath "$staging")
out=$(abspath "$out")
[ -n "$version" ] || die 'usage: assemble-release.sh --staging DIR --version X.Y.Z [--out DIR] [--allow-partial] [--rotate-key]'
printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$' \
    || die "version '$version' must look like 1.2.3 or 1.2.3-rc.1 (no leading v; the tag is v$version)"
[ ${#version} -le 64 ] || die 'version is longer than 64 characters'
[ "$rotate" = 0 ] || [ "$partial" = 0 ] || die '--rotate-key requires a complete release; it cannot be combined with --allow-partial'

: "${HOPE_SIGNING_KEY:?Set HOPE_SIGNING_KEY to the release RSA private key file (outside the repository)}"
command -v openssl >/dev/null 2>&1 || die 'openssl is required'


# ---- the signing key must live outside the repository -----------------------------------------------
key=$HOPE_SIGNING_KEY
hops=0
while [ -L "$key" ]; do
    hops=$((hops + 1)); [ "$hops" -le 10 ] || die 'too many symbolic links in HOPE_SIGNING_KEY'
    target=$(readlink "$key")
    case "$target" in /*) key=$target ;; *) key=$(dirname "$key")/$target ;; esac
done
{ [ -f "$key" ] && [ -r "$key" ]; } || die "HOPE_SIGNING_KEY is not a readable file"
key_dir=$(cd "$(dirname "$key")" && pwd -P)
key_real=$key_dir/$(basename "$key")
lower() { printf '%s' "$1" | tr '[:upper:]' '[:lower:]'; }
case "$(lower "$key_real")/" in
    "$(lower "$repo")"/*) die "the signing key must be stored outside the repository ($repo); refusing $key_real" ;;
esac
# A hard link (or a case-insensitive alias) of the key inside the tree has a different path but the same file.
inside=$(find "$repo" \( -name target -o -name node_modules \) -prune -o -type f -samefile "$key_real" -print -quit 2>/dev/null || true)
[ -z "$inside" ] || die "the signing key is also reachable inside the repository as $inside"
key=$key_real
if [ -n "$(find "$key" -maxdepth 0 -perm -077 2>/dev/null)" ]; then
    note "warning: $key is readable by group or others; restrict it with chmod 600"
fi
openssl pkey -in "$key" -noout >/dev/null 2>&1 || die 'HOPE_SIGNING_KEY is not a usable private key'
openssl rsa -in "$key" -noout -check >/dev/null 2>&1 || die 'HOPE_SIGNING_KEY must be an RSA private key'
bits=$(openssl rsa -in "$key" -noout -text 2>/dev/null | sed -n 's/^Private-Key: (\([0-9]*\) bit.*/\1/p;s/^RSA Private-Key: (\([0-9]*\) bit.*/\1/p' | head -n 1)
{ [ -n "$bits" ] && [ "$bits" -ge 2048 ]; } || die 'the RSA signing key must be at least 2048 bits'

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d ' ' -f 1
    else openssl dgst -sha256 -r "$1" | cut -d ' ' -f 1; fi
}
size_of() { wc -c < "$1" | tr -d ' '; }
key_id_of_public() { openssl pkey -pubin -in "$1" -outform DER | openssl dgst -sha256 -r | cut -d ' ' -f 1; }
version_lt() {
    awk -v a="$1" -v b="$2" 'function norm(s) { sub(/^v/, "", s); sub(/[^0-9.].*$/, "", s); return s }
        BEGIN { n = split(norm(a), x, "."); m = split(norm(b), y, "."); k = (n > m) ? n : m
            for (i = 1; i <= k; i++) { if (x[i] + 0 < y[i] + 0) exit 0; if (x[i] + 0 > y[i] + 0) exit 1 }
            exit 1 }'
}
info_get() { sed -n "s/^$2=//p" "$1" | head -n 1; }

[ -d "$staging" ] || die "staging directory $staging does not exist"
for artifact in "$staging"/hope-agent-*; do
    [ -e "$artifact" ] || continue
    case "${artifact##*/}" in
        hope-agent-linux-x86_64|hope-agent-linux-x86_64.info) ;;
        *) die "unsupported agent target ${artifact##*/}; only Linux x86_64 is supported" ;;
    esac
done

# ELF/Mach-O header must match the platform label: refuses a mislabeled or wrong-architecture binary.
magic_ok() { # magic_ok FILE PLATFORM
    head=$(od -An -v -tx1 -N20 "$1" 2>/dev/null | tr -d ' \n')
    case "$2" in
        linux-x86_64) [ "$(printf '%s' "$head" | cut -c1-12)" = 7f454c460201 ] && [ "$(printf '%s' "$head" | cut -c37-40)" = 3e00 ] ;;
        *) return 1 ;;
    esac
}

# ---- collect and check the staged artifacts ---------------------------------------------------------
platforms='linux-x86_64'
present=
missing=
for platform in $platforms; do
    name=hope-agent-$platform
    if [ -s "$staging/$name" ] && [ -f "$staging/$name.info" ]; then
        present="$present $platform"
    else
        missing="$missing $platform"
    fi
done
if [ -n "$missing" ]; then
    [ "$partial" = 1 ] || die "missing required platform(s):$missing. Build Linux x86_64, or pass --allow-partial for local development only."
    [ -n "$present" ] || die 'no artifacts staged'
    note "warning: PARTIAL release (missing:$missing); the manifest will say partial"
fi
commit=
for platform in $present; do
    info=$staging/hope-agent-$platform.info
    [ "$(info_get "$info" version)" = "$version" ] \
        || die "hope-agent-$platform was built as version '$(info_get "$info" version)', not $version (mixed versions refused)"
    min_os=$(info_get "$info" min_os)
    [ -n "$min_os" ] || die "hope-agent-$platform.info has no min_os"
    case "$min_os" in *[!A-Za-z0-9\ .,+\(\)-]*) die "hope-agent-$platform.info min_os has unsupported characters" ;; esac
    target=$(info_get "$info" target)
    case "$target" in ''|*[!A-Za-z0-9_-]*) die "hope-agent-$platform.info has no valid target" ;; esac
    this_commit=$(info_get "$info" commit)
    case "$this_commit" in
        unknown) [ "$partial" = 1 ] || die "hope-agent-$platform was built without a git commit (unknown); only --allow-partial accepts that" ;;
        *[!0-9a-f]*|'') die "hope-agent-$platform.info has an invalid commit" ;;
    esac
    if [ -z "$commit" ]; then commit=$this_commit
    elif [ "$commit" != "$this_commit" ]; then die "artifacts were built from different commits ($commit and $this_commit); rebuild all from one commit"
    fi
    magic_ok "$staging/hope-agent-$platform" "$platform" || die "hope-agent-$platform is not a $platform executable (header check failed)"
done

# ---- public key, key id, rotation and downgrade policy ----------------------------------------------
mkdir -p "$out"
[ ! -e "$out/current" ] || [ -L "$out/current" ] || die "$out/current exists but is not a symbolic link"
rand=$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')
name=$version-$rand
new=$out/$name
tmp_link=$out/.current-$rand
rows_file=$out/.rows-$rand
cleanup() {
    [ -z "$new" ] || rm -rf "$new"
    rm -f "$tmp_link" "$rows_file"
}
trap cleanup EXIT
trap 'cleanup; exit 1' INT TERM HUP
umask 022
mkdir "$new"
openssl pkey -in "$key" -pubout -out "$new/release-public.pem"
key_id=$(key_id_of_public "$new/release-public.pem")

# The release being replaced: the `current` target, or a flat pre-symlink directory used directly.
old_release=
old_name=
if [ -L "$out/current" ] && [ -d "$out/current" ]; then
    old_release=$out/current
    old_name=$(readlink "$out/current")
elif [ -f "$out/release-public.pem" ]; then
    old_release=$out
fi
if [ -n "$old_release" ] && [ -f "$old_release/release-public.pem" ]; then
    old_id=$(key_id_of_public "$old_release/release-public.pem" 2>/dev/null || echo unreadable)
    if [ "$old_id" != "$key_id" ]; then
        [ "$rotate" = 1 ] || die "$out already publishes key $old_id but HOPE_SIGNING_KEY is $key_id. Publishing both would leave artifacts signed by different keys. Pass --rotate-key to replace every artifact with this key (installers already fetched embed the old key and will reject the new release; fetch /agent/install again)"
        note "ROTATING signing key $old_id -> $key_id; every artifact is re-signed in this run"
    fi
fi
if [ -n "$old_release" ] && [ -f "$old_release/manifest.json" ] && [ "$downgrade" = 0 ]; then
    old_version=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$old_release/manifest.json" | head -n 1)
    if [ -n "$old_version" ] && version_lt "$version" "$old_version"; then
        die "published version is $old_version; refusing to publish older $version (pass --allow-downgrade)"
    fi
fi

# ---- write artifacts, per-artifact signatures and the manifest -------------------------------------
created=$(date -u +%Y-%m-%dT%H:%M:%SZ)
rows=
count=0
total=0
for platform in $present; do total=$((total + 1)); done
for platform in $present; do
    count=$((count + 1))
    file=hope-agent-$platform
    cp "$staging/$file" "$new/$file"
    chmod 644 "$new/$file"
    openssl dgst -sha256 -sign "$key" -out "$new/$file.sig" "$new/$file"
    os=${platform%%-*}
    arch=${platform#*-}
    comma=,
    [ "$count" -lt "$total" ] || comma=
    rows="$rows    {\"platform\":\"$os\",\"arch\":\"$arch\",\"file\":\"$file\",\"sha256\":\"$(sha256_of "$new/$file")\",\"size\":$(size_of "$new/$file"),\"min_os\":\"$(info_get "$staging/$file.info" min_os)\",\"target\":\"$(info_get "$staging/$file.info" target)\"}$comma
"
done
if [ "$partial" = 1 ] || [ -n "$missing" ]; then partial_json=true; else partial_json=false; fi
{
    printf '{\n'
    printf '  "format": 1,\n'
    printf '  "version": "%s",\n' "$version"
    printf '  "commit": "%s",\n' "$commit"
    printf '  "created": "%s",\n' "$created"
    printf '  "key_id": "%s",\n' "$key_id"
    printf '  "partial": %s,\n' "$partial_json"
    printf '  "artifacts": [\n'
    printf '%s' "$rows"
    printf '  ]\n'
    printf '}\n'
} > "$new/manifest.json"
openssl dgst -sha256 -sign "$key" -out "$new/manifest.json.sig" "$new/manifest.json"
chmod 644 "$new"/*

# ---- verify the new directory exactly as a client would ---------------------------------------------
pub=$new/release-public.pem
[ "$(key_id_of_public "$pub")" = "$key_id" ] || die 'internal error: key id changed'
openssl dgst -sha256 -verify "$pub" -signature "$new/manifest.json.sig" "$new/manifest.json" >/dev/null || die 'manifest signature does not verify'
[ "$(sed -n 's/^ *"key_id": *"\([0-9a-f]*\)".*/\1/p' "$new/manifest.json")" = "$key_id" ] || die 'manifest key_id does not match the public key'
[ "$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$new/manifest.json")" = "$version" ] || die 'manifest version mismatch'
listed=0
grep -F '"platform":' "$new/manifest.json" > "$rows_file"
while IFS= read -r row; do
    file=$(printf '%s' "$row" | sed -n 's/.*"file":"\([^"]*\)".*/\1/p')
    sha=$(printf '%s' "$row" | sed -n 's/.*"sha256":"\([0-9a-f]\{64\}\)".*/\1/p')
    size=$(printf '%s' "$row" | sed -n 's/.*"size":\([0-9]*\)[,}].*/\1/p')
    { [ -n "$file" ] && [ -f "$new/$file" ]; } || die "manifest lists a missing file '$file'"
    [ "$(sha256_of "$new/$file")" = "$sha" ] || die "$file checksum differs from the manifest"
    [ "$(size_of "$new/$file")" = "$size" ] || die "$file size differs from the manifest"
    openssl dgst -sha256 -verify "$pub" -signature "$new/$file.sig" "$new/$file" >/dev/null || die "$file signature does not verify"
    listed=$((listed + 1))
done < "$rows_file"
expected=0
for platform in $present; do expected=$((expected + 1)); done
[ "$listed" = "$expected" ] || die "manifest lists $listed artifacts, expected $expected"
if [ "$partial" = 0 ]; then
    [ "$listed" = 1 ] || die 'a complete release must list exactly the Linux x86_64 artifact'
fi
for file in "$new"/*; do
    case "${file##*/}" in
        hope-agent-*.sig|manifest.json|manifest.json.sig|release-public.pem) ;;
        hope-agent-*) grep -qF "\"file\":\"${file##*/}\"" "$new/manifest.json" || die "${file##*/} is not listed in the manifest" ;;
        *) die "unexpected file ${file##*/}" ;;
    esac
done

# ---- publish: atomically repoint `current` ------------------------------------------------------------
ln -s "$name" "$tmp_link"
# -T (GNU) / -h (BSD, macOS): treat the destination symlink itself as the target instead of the directory it names.
if mv --version 2>/dev/null | grep -q GNU; then mv -fT "$tmp_link" "$out/current"; else mv -fh "$tmp_link" "$out/current"; fi
[ "$(readlink "$out/current")" = "$name" ] || die "could not repoint $out/current"
new=
if [ -n "$old_name" ] && [ "$old_name" != "$name" ]; then ln -sfn "$old_name" "$out/previous"; fi

# Keep the current release and the two newest older ones (always including the one just replaced);
# never touch anything else.
kept=0
# shellcheck disable=SC2012,SC2045 # names are generated by this script: version-12hex
for entry in $(ls -1t "$out"); do
    case "$entry" in
        [0-9]*-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) ;;
        *) continue ;;
    esac
    { [ -d "$out/$entry" ] && [ ! -L "$out/$entry" ]; } || continue
    [ "$entry" != "$name" ] || continue
    kept=$((kept + 1))
    [ "$kept" -le 2 ] || [ "$entry" = "$old_name" ] || rm -rf "${out:?}/$entry"
done
note "published $out/current -> $name: version $version, commit $commit, key $key_id, ${listed} artifact(s)$( [ "$partial_json" = true ] && echo ' (PARTIAL)')"
