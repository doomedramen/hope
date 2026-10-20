#!/bin/sh
# H.O.P.E server installer: one command, interactive, idempotent.
#
#   bash -c "$(curl -fsSL https://raw.githubusercontent.com/doomedramen/hope/main/scripts/install-hope.sh)"
#
# On a Proxmox VE host it hands over to ct/hope.sh, which the community-scripts engine
# (community-scripts/core build.func) runs: the engine creates the Debian LXC container with its
# usual default/advanced settings and executes install/hope-install.sh inside it. On any other
# systemd Linux host this script installs H.O.P.E directly: a dedicated `hope` account, /opt/hope,
# /var/lib/hope, /etc/hope/hope.env, a hardened systemd unit with CAP_NET_RAW for discovery, and
# optionally Caddy as the HTTPS reverse proxy (public certificate for a real domain, or Caddy's
# internal CA for LAN-only names, whose root certificate is exported for agents at /etc/hope/ca.crt).
#
# The server binary comes from the latest GitHub release when one is published for this
# architecture (checksum verified) and otherwise from a source build (Rust and Node are
# installed under /opt/hope/build; nothing else on the system is touched). A release install also
# downloads the release's hope-agents.tar.gz (same checksum file) and signs it into
# /opt/hope/releases with this server's own key, /etc/hope/release-signing.pem (created once, kept).
# Re-running upgrades the binary and console, re-signs the agents with the existing key and keeps
# /etc/hope/hope.env, the key and the database.
#
# Non-interactive use (every prompt has an environment answer; unset values prompt):
#   HOPE_INSTALL_TARGET=host|lxc        where to install (lxc = community-scripts engine, Proxmox only; default there)
#   HOPE_INSTALL_URL=https://hope.lan   canonical HTTPS URL (host name or IP; default https://<host IP>)
#   HOPE_INSTALL_PROXY=caddy|none       install Caddy in front (default caddy)
#   HOPE_INSTALL_SOURCE=auto|release|source
#   HOPE_INSTALL_VERSION=vX.Y.Z         pin a release tag (default: latest)
#   HOPE_INSTALL_REF=main               git ref for source builds
#   HOPE_INSTALL_NONINTERACTIVE=1       never prompt; missing answers use the defaults above
#   HOPE_INSTALL_CT_SCRIPT_URL=...      ct script the Proxmox hand-over runs (default: ct/hope.sh of HOPE_INSTALL_REPO)
#   HOPE_INSTALL_REPO=owner/repo        (default doomedramen/hope)
#   HOPE_INSTALL_RELEASE_URL=...        TEST ONLY: base URL holding hope-server-<arch>-linux.tar.gz and hope-agents.tar.gz
#   HOPE_INSTALL_ROOT=/prefix           TEST ONLY: prefix for every absolute system path
set -eu
umask 022

repo=${HOPE_INSTALL_REPO:-doomedramen/hope}
script_url=${HOPE_INSTALL_SCRIPT_URL:-https://raw.githubusercontent.com/$repo/main/scripts/install-hope.sh}
root=${HOPE_INSTALL_ROOT:-}
root=${root%/}
case "$root" in ''|/*) ;; *) echo "HOPE_INSTALL_ROOT must be absolute" >&2; exit 1 ;; esac
noninteractive=${HOPE_INSTALL_NONINTERACTIVE:-0}
node_version=${HOPE_INSTALL_NODE_VERSION:-22.22.0}
staging=
agents_dir=
agents_signed=

# ---------- output ----------
if [ -t 1 ] && [ "${NO_COLOR:-}" = "" ]; then
    c_bold=$(printf '\033[1m'); c_blue=$(printf '\033[34m'); c_green=$(printf '\033[32m')
    c_yellow=$(printf '\033[33m'); c_red=$(printf '\033[31m'); c_off=$(printf '\033[0m')
else
    c_bold=; c_blue=; c_green=; c_yellow=; c_red=; c_off=
fi
# Progress goes to stderr so command substitutions (template lookup) only capture results.
info() { printf '%s  ›%s %s\n' "$c_blue" "$c_off" "$*" >&2; }
ok() { printf '%s  ✓%s %s\n' "$c_green" "$c_off" "$*" >&2; }
warn() { printf '%s  !%s %s\n' "$c_yellow" "$c_off" "$*" >&2; }
die() { printf '%s  ✗ %s%s\n' "$c_red" "$*" "$c_off" >&2; exit 1; }
banner() {
    printf '%s\n' "$c_bold"
    cat <<'EOF'
   _  _   ___  ___ ___
  | || | / _ \| _ \ __|   Home network discovery
  | __ || (_) |  _/ _|    and host monitoring
  |_||_| \___/|_| |___|   https://github.com/doomedramen/hope
EOF
    printf '%s\n' "$c_off"
}
cleanup() {
    [ -n "$staging" ] && rm -rf "$staging"
    return 0
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# ---------- prompts ----------
has_tty() { [ "$noninteractive" != 1 ] && [ -r /dev/tty ] && [ -w /dev/tty ]; }
use_whiptail() { has_tty && command -v whiptail >/dev/null 2>&1 && [ "${HOPE_INSTALL_PLAIN:-0}" != 1 ]; }
# ask VAR "Question" "default": environment answer wins, then whiptail, then a plain prompt.
ask() {
    _var=$1; _question=$2; _default=$3
    eval "_preset=\${$_var:-}"
    if [ -n "$_preset" ]; then eval "$_var=\$_preset"; return; fi
    if use_whiptail; then
        _answer=$(whiptail --backtitle "H.O.P.E installer" --inputbox "$_question" 10 70 "$_default" 3>&1 1>&2 2>&3 </dev/tty) || die 'Cancelled.'
    elif has_tty; then
        printf '%s%s%s [%s]: ' "$c_bold" "$_question" "$c_off" "$_default" >/dev/tty
        read -r _answer </dev/tty || _answer=
    else
        _answer=
    fi
    [ -n "$_answer" ] || _answer=$_default
    eval "$_var=\$_answer"
}
# choose VAR "Question" "default" "key1|Label one" "key2|Label two" ...
choose() {
    _var=$1; _question=$2; _default=$3; shift 3
    eval "_preset=\${$_var:-}"
    if [ -n "$_preset" ]; then eval "$_var=\$_preset"; return; fi
    if use_whiptail; then
        set -- "$@"
        _menu=
        for _option in "$@"; do _menu="$_menu ${_option%%|*} \"${_option#*|}\""; done
        _answer=$(eval "whiptail --backtitle 'H.O.P.E installer' --default-item '$_default' --menu \"\$_question\" 16 70 6 $_menu 3>&1 1>&2 2>&3 </dev/tty") || die 'Cancelled.'
    elif has_tty; then
        printf '%s%s%s\n' "$c_bold" "$_question" "$c_off" >/dev/tty
        for _option in "$@"; do printf '    %-10s %s\n' "${_option%%|*}" "${_option#*|}" >/dev/tty; done
        printf '  choice [%s]: ' "$_default" >/dev/tty
        read -r _answer </dev/tty || _answer=
    else
        _answer=
    fi
    [ -n "$_answer" ] || _answer=$_default
    for _option in "$@"; do [ "${_option%%|*}" = "$_answer" ] && { eval "$_var=\$_answer"; return; }; done
    die "Invalid choice: $_answer"
}
confirm() {
    _question=$1; _default=$2
    if use_whiptail; then
        if [ "$_default" = yes ]; then whiptail --backtitle "H.O.P.E installer" --yesno "$_question" 10 70 </dev/tty
        else whiptail --backtitle "H.O.P.E installer" --defaultno --yesno "$_question" 10 70 </dev/tty; fi
        return
    fi
    if has_tty; then
        printf '%s%s%s [%s]: ' "$c_bold" "$_question" "$c_off" "$([ "$_default" = yes ] && echo Y/n || echo y/N)" >/dev/tty
        read -r _answer </dev/tty || _answer=
        case "${_answer:-$_default}" in y|Y|yes|YES) return 0 ;; *) return 1 ;; esac
    fi
    [ "$_default" = yes ]
}

# ---------- system facts ----------
arch() {
    case "$(uname -m)" in
    x86_64|amd64) echo x86_64 ;;
    *) die "H.O.P.E server packages require Linux x86_64 (found $(uname -m))." ;;
    esac
}
os_id() {
    _release=$root/etc/os-release
    [ -r "$_release" ] || { echo unknown; return; }
    _id=$(sed -n 's/^ID=//p' "$_release" | tr -d '"')
    _like=$(sed -n 's/^ID_LIKE=//p' "$_release" | tr -d '"')
    case "$_id $_like" in
    *debian*|*ubuntu*) echo debian ;;
    *fedora*|*rhel*|*centos*) echo fedora ;;
    *arch*) echo arch ;;
    *) echo "$_id" ;;
    esac
}
is_proxmox() { command -v pct >/dev/null 2>&1 && command -v pvesh >/dev/null 2>&1 && [ -z "$root" ]; }
systemd_running() { [ -n "$root" ] || [ -d /run/systemd/system ]; }
primary_ip() {
    _ip=$(ip -4 route get 1.1.1.1 2>/dev/null | sed -n 's/.*src \([0-9.]*\).*/\1/p' | head -n 1)
    [ -n "$_ip" ] || _ip=$(hostname -I 2>/dev/null | awk '{print $1}')
    echo "${_ip:-127.0.0.1}"
}
# Debian, Fedora and Arch package names; the installer touches nothing beyond these.
# pkg_install_group base|build|caddy expands the distribution's package list (word splitting intended).
pkg_install_group() {
    _packages=$(deps_for "$1")
    # shellcheck disable=SC2086
    pkg_install $_packages
}
pkg_install() {
    case "$(os_id)" in
    debian)
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq >/dev/null
        apt-get install -y -qq --no-install-recommends "$@" >/dev/null ;;
    fedora) dnf install -y -q "$@" >/dev/null ;;
    arch) pacman -Sy --noconfirm --needed "$@" >/dev/null ;;
    *) die "Unsupported distribution; install H.O.P.E with Docker instead (see README)." ;;
    esac
}
deps_for() {
    case "$(os_id)" in
    debian) case "$1" in base) echo "curl ca-certificates tar xz-utils openssl python3" ;; build) echo "git build-essential pkg-config cmake" ;; caddy) echo caddy ;; esac ;;
    fedora) case "$1" in base) echo "curl ca-certificates tar xz openssl python3" ;; build) echo "git gcc gcc-c++ make pkgconf-pkg-config cmake" ;; caddy) echo caddy ;; esac ;;
    arch) case "$1" in base) echo "curl ca-certificates tar xz openssl python" ;; build) echo "git base-devel cmake" ;; caddy) echo caddy ;; esac ;;
    esac
}

# ---------- obtaining the server ----------
release_url() {
    # Prints the tarball URL, on a second line the checksum file URL and on a third the agents
    # archive URL (either may be empty); nothing when no release fits.
    if [ -n "${HOPE_INSTALL_RELEASE_URL:-}" ]; then
        printf '%s/hope-server-%s-linux.tar.gz\n%s/SHA256SUMS-server\n%s/hope-agents.tar.gz\n' "$HOPE_INSTALL_RELEASE_URL" "$(arch)" "$HOPE_INSTALL_RELEASE_URL" "$HOPE_INSTALL_RELEASE_URL"
        return
    fi
    if [ -n "${HOPE_INSTALL_VERSION:-}" ]; then _api="https://api.github.com/repos/$repo/releases/tags/$HOPE_INSTALL_VERSION"
    else _api="https://api.github.com/repos/$repo/releases/latest"; fi
    _json=$(curl -fsSL --max-time 20 -H 'Accept: application/vnd.github+json' "$_api" 2>/dev/null) || return 0
    _tar=$(printf '%s' "$_json" | tr ',' '\n' | sed -n 's/.*"browser_download_url": *"\([^"]*hope-server-'"$(arch)"'-linux\.tar\.gz\)".*/\1/p' | head -n 1)
    [ -n "$_tar" ] || return 0
    _sums=$(printf '%s' "$_json" | tr ',' '\n' | sed -n 's/.*"browser_download_url": *"\([^"]*SHA256SUMS-server\)".*/\1/p' | head -n 1)
    _agents=$(printf '%s' "$_json" | tr ',' '\n' | sed -n 's/.*"browser_download_url": *"\([^"]*\/hope-agents\.tar\.gz\)".*/\1/p' | head -n 1)
    printf '%s\n%s\n%s\n' "$_tar" "$_sums" "$_agents"
}
fetch_release() {
    _urls=$(release_url) || true
    [ -n "$_urls" ] || return 1
    _tar=$(printf '%s\n' "$_urls" | sed -n 1p)
    _sums=$(printf '%s\n' "$_urls" | sed -n 2p)
    _agents=$(printf '%s\n' "$_urls" | sed -n 3p)
    info "Downloading $(basename "$_tar")"
    curl -fsSL --max-time 600 -o "$staging/server.tar.gz" "$_tar" || die "Download failed: $_tar"
    if [ -n "$_sums" ] && curl -fsSL --max-time 60 -o "$staging/SHA256SUMS" "$_sums" 2>/dev/null; then
        _expected=$(grep " $(basename "$_tar")\$" "$staging/SHA256SUMS" | awk '{print $1}' | head -n 1)
        [ -n "$_expected" ] || die 'The release checksum file does not list this artifact.'
        _actual=$(sha256sum "$staging/server.tar.gz" | awk '{print $1}')
        [ "$_expected" = "$_actual" ] || die 'Checksum mismatch; the download is corrupt or tampered with.'
        ok 'Checksum verified'
    else
        warn 'No checksum file in this release; relying on HTTPS alone.'
    fi
    mkdir -p "$staging/server"
    tar -xzf "$staging/server.tar.gz" -C "$staging/server" || die 'The release archive could not be extracted.'
    { [ -x "$staging/server/hope-server" ] && [ -f "$staging/server/web/index.html" ]; } || die 'The release archive is incomplete (hope-server and web/index.html expected).'
    ok "Release $(basename "$_tar") ready"
    fetch_agents "$_agents"
}
# fetch_agents URL: the release's unsigned agent binaries, verified like the server package and
# extracted to $staging/agents; sets agents_dir. A release without them only warns (agents can still
# be published by hand); a checksum mismatch aborts like the server's, before anything is installed.
fetch_agents() {
    _missing='agents cannot be added until a signed agent release is published (docs/operations.md, Signed agent releases).'
    [ -n "$1" ] || { warn "This release has no hope-agents.tar.gz; $_missing"; return 0; }
    info 'Downloading hope-agents.tar.gz'
    curl -fsSL --max-time 600 -o "$staging/agents.tar.gz" "$1" 2>/dev/null || { warn "Could not download $1; $_missing"; return 0; }
    if [ -f "$staging/SHA256SUMS" ]; then
        _expected=$(grep " hope-agents\.tar\.gz\$" "$staging/SHA256SUMS" | awk '{print $1}' | head -n 1)
        [ -n "$_expected" ] || { warn "The release checksum file does not list hope-agents.tar.gz; not signing unverified agents, so $_missing"; return 0; }
        _actual=$(sha256sum "$staging/agents.tar.gz" | awk '{print $1}')
        [ "$_expected" = "$_actual" ] || die 'Checksum mismatch for hope-agents.tar.gz; the download is corrupt or tampered with.'
        ok 'Agent checksum verified'
    fi
    mkdir -p "$staging/agents"
    tar -xzf "$staging/agents.tar.gz" --no-same-owner -C "$staging/agents" 2>/dev/null || { warn "hope-agents.tar.gz could not be extracted; $_missing"; return 0; }
    # The binaries sit at the top level or in one directory below it.
    _info=$(find "$staging/agents" -maxdepth 2 -type f -name 'hope-agent-*.info' | head -n 1)
    [ -n "$_info" ] || { warn "hope-agents.tar.gz holds no hope-agent-*.info; $_missing"; return 0; }
    agents_dir=$(dirname "$_info")
}
build_from_source() {
    _ref=${HOPE_INSTALL_REF:-main}
    _build=$root/opt/hope/build
    info "Building from source ($repo@$_ref); this takes 10 to 20 minutes and about 3 GB"
    pkg_install_group build
    mkdir -p "$_build"
    RUSTUP_HOME="$_build/rustup"; CARGO_HOME="$_build/cargo"; export RUSTUP_HOME CARGO_HOME
    if [ ! -x "$CARGO_HOME/bin/cargo" ]; then
        curl -fsSL --max-time 120 https://sh.rustup.rs -o "$staging/rustup.sh" || die 'Cannot download rustup.'
        sh "$staging/rustup.sh" -y --no-modify-path --profile minimal >/dev/null || die 'rustup failed.'
    fi
    case "$(arch)" in x86_64) _node_arch=x64 ;; esac
    _node="$_build/node-v$node_version-linux-$_node_arch"
    if [ ! -x "$_node/bin/node" ]; then
        curl -fsSL --max-time 300 -o "$staging/node.tar.xz" "https://nodejs.org/dist/v$node_version/node-v$node_version-linux-$_node_arch.tar.xz" || die 'Cannot download Node.js.'
        tar -xJf "$staging/node.tar.xz" -C "$_build"
    fi
    PATH="$_node/bin:$CARGO_HOME/bin:$PATH"; export PATH
    rm -rf "$_build/src"
    git clone -q --depth 1 --branch "$_ref" "https://github.com/$repo.git" "$_build/src" || die 'git clone failed.'
    (cd "$_build/src/web" && npm ci --no-audit --no-fund >/dev/null && npm run build >/dev/null) || die 'Console build failed.'
    (cd "$_build/src" && cargo build --release --locked -p hope-server) || die 'Server build failed.'
    mkdir -p "$staging/server"
    cp "$_build/src/target/release/hope-server" "$staging/server/hope-server"
    cp -R "$_build/src/web/out" "$staging/server/web"
    mkdir -p "$staging/server/tools"
    cp "$_build/src/scripts/activate-server.py" "$staging/server/tools/activate-server.py"
    sed -n 's/^version = "\(.*\)"$/\1/p' "$_build/src/Cargo.toml" | head -n 1 > "$staging/server/VERSION"
    ok 'Built from source'
}
obtain_server() {
    _source=${HOPE_INSTALL_SOURCE:-auto}
    case "$_source" in
    release) fetch_release || die 'No published server release matches this architecture (set HOPE_INSTALL_SOURCE=source to build).' ;;
    source) build_from_source ;;
    auto)
        if fetch_release; then :; else
            warn 'No prebuilt server release is published for this architecture yet.'
            if [ "$noninteractive" = 1 ] || confirm 'Build H.O.P.E from source now?' yes; then build_from_source
            else die 'Nothing to install.'; fi
        fi ;;
    *) die "HOPE_INSTALL_SOURCE must be auto, release or source" ;;
    esac
}

# ---------- host installation ----------
# Signs the downloaded agents into /opt/hope/releases with this server's key, creating the key on
# the first install only: installed agents trust its public key, so it is never replaced. Never
# fatal: assemble-release.sh repoints `current` only after a verified publish, so on any failure
# the previous release stays live.
sign_agents() {
    [ -n "$agents_dir" ] || return 0
    _tool=$staging/server/tools/assemble-release.sh
    _version=$(cat "$staging/server/VERSION" 2>/dev/null || true)
    if [ ! -f "$_tool" ] || [ -z "$_version" ]; then
        warn 'The server package has no tools/assemble-release.sh or VERSION; agents not signed.'
        return 0
    fi
    _key=$root/etc/hope/release-signing.pem
    _release_path=$(python3 "$staging/server/tools/activate-server.py" value --env "$root/etc/hope/hope.env" --key HOPE_RELEASE_DIR)
    _releases=$root${_release_path:-/opt/hope/releases}
    if [ ! -e "$_key" ] && [ ! -L "$_key" ]; then
        if [ -e "$_releases/current" ] || [ -L "$_releases/current" ]; then
            # Published by hand with a key kept elsewhere; a new key here would be refused anyway.
            warn "/opt/hope/releases holds a release signed with a key outside /etc/hope; agents not signed. Publish with that key (scripts/assemble-release.sh), or see Rotation in docs/operations.md."
            return 0
        fi
        info 'Creating the agent release signing key /etc/hope/release-signing.pem'
        rm -f "$_key.new"
        if ! (umask 077 && openssl genrsa -out "$_key.new" 3072 >/dev/null 2>&1); then
            rm -f "$_key.new"
            warn 'openssl could not create the signing key; agents not signed.'
            return 0
        fi
        mv "$_key.new" "$_key"
    fi
    info "Signing agent release $_version"
    if HOPE_SIGNING_KEY=$_key sh "$_tool" --staging "$agents_dir" --version "$_version" --out "$_releases" >"$staging/assemble.log" 2>&1; then
        chmod 755 "$_releases"
        agents_signed=$_version
        ok "Agent release $_version signed into /opt/hope/releases"
    else
        warn 'Signing the agent release failed; the previous agent release (if any) stays published:'
        sed 's/^/      /' "$staging/assemble.log" >&2
    fi
}
write_unit() {
    if [ -f "$root/etc/systemd/system/hope.service" ] && grep -Fxq 'ExecStart=/opt/hope/bin/hope-server' "$root/etc/systemd/system/hope.service"; then
        cp -p "$root/etc/systemd/system/hope.service" "$staging/hope.service"
        return
    fi
    cat > "$staging/hope.service" <<EOF
[Unit]
Description=H.O.P.E network observatory
Documentation=https://github.com/$repo
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=hope
Group=hope
EnvironmentFile=/etc/hope/hope.env
ExecStart=/opt/hope/bin/hope-server
WorkingDirectory=/var/lib/hope
Restart=on-failure
RestartSec=5
TimeoutStopSec=20
# Discovery needs raw sockets for ICMP and ARP; nothing else.
AmbientCapabilities=CAP_NET_RAW
CapabilityBoundingSet=CAP_NET_RAW
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ProtectKernelTunables=true
ProtectControlGroups=true
RestrictSUIDSGID=true
ReadWritePaths=/var/lib/hope
UMask=0077

[Install]
WantedBy=multi-user.target
EOF
}
write_env() {
    # Created once; re-runs keep the operator's values.
    _env=$staging/hope.env
    if [ -f "$root/etc/hope/hope.env" ]; then
        cp -p "$root/etc/hope/hope.env" "$_env"
        grep -q '^HOPE_SERVER_URL=' "$_env" || printf 'HOPE_SERVER_URL=%s\n' "$url" >> "$_env"
        return
    fi
    cat > "$_env" <<EOF
# H.O.P.E server settings (see docs/operations.md). Restart with: systemctl restart hope
HOPE_SERVER_URL=$url
HOPE_BIND=127.0.0.1:8080
HOPE_DATA_DIR=/var/lib/hope
HOPE_WEB_DIR=/opt/hope/web
HOPE_RELEASE_DIR=/opt/hope/releases
HOPE_DISCOVERY_MODE=native
# Path on AGENT hosts of a private CA bundle, embedded in install commands; empty for public CAs.
HOPE_AGENT_CA=
HOPE_TRUSTED_PROXIES=
RUST_LOG=hope_server=info
EOF
    chmod 640 "$_env"
}
set_env() {
    _env=$staging/hope.env
    if grep -q "^$1=" "$_env"; then sed "s|^$1=.*|$1=$2|" "$_env" > "$_env.new"; chmod 640 "$_env.new"; mv "$_env.new" "$_env"; else printf '%s=%s\n' "$1" "$2" >> "$_env"; fi
}
install_caddy() {
    _host=${url#https://}; _host=${_host#http://}; _host=${_host%%/*}
    info "Installing Caddy for $_host"
    pkg_install_group caddy
    mkdir -p "$root/etc/caddy"
    [ -f "$root/etc/caddy/Caddyfile" ] && ! grep -q 'H.O.P.E' "$root/etc/caddy/Caddyfile" && cp "$root/etc/caddy/Caddyfile" "$root/etc/caddy/Caddyfile.before-hope"
    _tls=
    case "$_host" in
    localhost|*.local|*.lan|*.home|*.internal|*.home.arpa|[0-9]*.[0-9]*.[0-9]*.[0-9]*) _tls='    tls internal' ;;
    *) [ "${HOPE_INSTALL_TLS:-auto}" = internal ] && _tls='    tls internal' ;;
    esac
    [ "${HOPE_INSTALL_TLS:-auto}" = public ] && _tls=
    cat > "$root/etc/caddy/Caddyfile" <<EOF
# H.O.P.E reverse proxy (written by install-hope.sh). Keep Host and Origin unchanged and never
# buffer /api/v1/events; Caddy does both by default.
$_host {
$_tls
    encode gzip
    reverse_proxy 127.0.0.1:8080 {
        flush_interval -1
    }
}
EOF
    systemctl enable --now caddy >/dev/null 2>&1 || true
    systemctl reload caddy >/dev/null 2>&1 || systemctl restart caddy >/dev/null 2>&1 || warn 'Caddy did not reload; check: journalctl -u caddy'
    if [ -n "$_tls" ]; then
        _ca=$root/var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt
        _n=0
        while [ ! -f "$_ca" ] && [ $_n -lt 30 ]; do sleep 1; _n=$((_n + 1)); done
        if [ -f "$_ca" ]; then
            install -m 644 "$_ca" "$root/etc/hope/ca.crt"
            set_env HOPE_AGENT_CA /etc/hope/ca.crt
            ca_exported=1
            ok 'Caddy internal CA exported to /etc/hope/ca.crt'
        else
            warn 'Caddy has not created its internal CA yet; copy root.crt to /etc/hope/ca.crt later and set HOPE_AGENT_CA.'
        fi
    fi
}
host_install() {
    if [ "$(id -u)" != 0 ]; then
        if command -v sudo >/dev/null 2>&1 && [ -z "$root" ]; then
            info 'Re-running with sudo'
            exec sudo -E sh -c "$(cat "$0" 2>/dev/null || curl -fsSL "$script_url")"
        fi
        die 'Run this installer as root.'
    fi
    [ "$(uname -s)" = Linux ] || die 'The server installer supports Linux only; use Docker on macOS.'
    systemd_running || die 'systemd is not running here; use Docker (compose.yaml) or run hope-server under your own supervisor.'
    [ "$(os_id)" != unknown ] || die 'Cannot read /etc/os-release.'
    ca_exported=0
    ask HOPE_INSTALL_URL 'Canonical HTTPS URL of this server (host name or IP your browser and agents will use)' "https://$(primary_ip)"
    url=${HOPE_INSTALL_URL%/}
    case "$url" in https://*) ;; http://localhost*|http://127.0.0.1*) ;; *) die "The URL must start with https:// (got $url)." ;; esac
    choose HOPE_INSTALL_PROXY 'Reverse proxy' caddy 'caddy|Install Caddy here (automatic HTTPS)' 'none|I run my own reverse proxy'
    info "Installing packages"
    pkg_install_group base
    staging=$(mktemp -d "${TMPDIR:-/tmp}/hope-install.XXXXXX")
    obtain_server
    info 'Installing files'
    getent passwd hope >/dev/null 2>&1 || useradd --system --home-dir /var/lib/hope --shell /usr/sbin/nologin --user-group hope
    mkdir -p "$root/opt/hope/bin" "$root/opt/hope/releases" "$root/var/lib/hope" "$root/etc/hope" "$root/etc/systemd/system"
    _activation=$staging/server/tools/activate-server.py
    [ -f "$_activation" ] || die 'This package lacks tools/activate-server.py; use a current release for recoverable activation.'
    python3 "$_activation" recover --root "$root" || die 'An earlier activation needs recovery before upgrading.'
    write_env
    write_unit
    _data_path=$(python3 "$_activation" value --env "$staging/hope.env" --key HOPE_DATA_DIR)
    mkdir -p "$root${_data_path:-/var/lib/hope}"
    chown -R hope:hope "$root${_data_path:-/var/lib/hope}"
    chmod 700 "$root${_data_path:-/var/lib/hope}"
    url=$(python3 "$_activation" value --env "$staging/hope.env" --key HOPE_SERVER_URL)
    [ "$HOPE_INSTALL_PROXY" = caddy ] && install_caddy
    info 'Activating server with database backup and rollback'
    python3 "$_activation" install --root "$root" --package "$staging/server" \
        --candidate-env "$staging/hope.env" --candidate-unit "$staging/hope.service" \
        || die 'Server activation failed; inspect the recovery result above before retrying.'
    ok 'hope-server activation verified'
    sign_agents
    _token=$(journalctl -u hope -b --no-pager 2>/dev/null | sed -n 's/.*setup token: \([A-Za-z0-9_-]*\).*/\1/p' | tail -n 1)
    printf '\n%s%s%s\n' "$c_bold" 'H.O.P.E is installed.' "$c_off"
    printf '  Console:        %s\n' "$url"
    if [ -n "$_token" ]; then printf '  Setup token:    %s\n' "$_token"
    else printf '  Setup token:    journalctl -u hope | grep "setup token"\n'; fi
    printf '  Settings:       /etc/hope/hope.env   (systemctl restart hope)\n'
    printf '  Logs:           journalctl -fu hope\n'
    if [ -n "$agents_signed" ]; then
        printf '  Agent releases: /opt/hope/releases   (%s, signed with /etc/hope/release-signing.pem; back it up)\n' "$agents_signed"
    elif [ -e "$root/opt/hope/releases/current" ]; then
        printf '  Agent releases: /opt/hope/releases   (previous signed release kept)\n'
    else
        printf '  Agent releases: /opt/hope/releases   (none signed: publish a signed release before adding agents)\n'
    fi
    [ "$ca_exported" = 1 ] && printf '  Agents must trust the internal CA: copy /etc/hope/ca.crt to the same path on each agent host.\n'
    [ "$HOPE_INSTALL_PROXY" = none ] && printf '  Point your reverse proxy at http://127.0.0.1:8080 with Host and Origin unchanged and SSE unbuffered.\n'
    printf '\n'
}

# ---------- Proxmox VE ----------
# Container creation is the community-scripts engine's job: ct/hope.sh in this repository sources
# community-scripts/core build.func, which builds the container (defaults or advanced settings)
# and runs install/hope-install.sh inside it. Nothing here duplicates that.
proxmox_install() {
    command -v bash >/dev/null 2>&1 || die 'bash is required on the Proxmox host.'
    _ct=${HOPE_INSTALL_CT_SCRIPT_URL:-https://raw.githubusercontent.com/$repo/main/ct/hope.sh}
    info "Handing over to the community-scripts engine ($_ct)"
    _script=$(curl -fsSL --max-time 60 "$_ct") || die "Cannot download $_ct"
    [ -n "$_script" ] || die "Empty response from $_ct"
    exec bash -c "$_script"
}

# ---------- entry ----------
main() {
    banner
    _target=${HOPE_INSTALL_TARGET:-}
    if [ -z "$_target" ]; then
        if is_proxmox; then
            choose HOPE_INSTALL_TARGET 'This is a Proxmox VE host. Where should H.O.P.E run?' lxc 'lxc|In a new LXC container via the community-scripts engine (recommended)' 'host|Directly on this Proxmox host'
            _target=$HOPE_INSTALL_TARGET
        else
            _target=host
        fi
    fi
    case "$_target" in
    lxc) is_proxmox || die 'HOPE_INSTALL_TARGET=lxc needs a Proxmox VE host.'; proxmox_install ;;
    host) host_install ;;
    *) die 'HOPE_INSTALL_TARGET must be host or lxc.' ;;
    esac
}
main "$@"
