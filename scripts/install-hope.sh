#!/bin/sh
# H.O.P.E server installer: one command, interactive, idempotent.
#
#   bash -c "$(curl -fsSL https://raw.githubusercontent.com/doomedramen/hope/main/scripts/install-hope.sh)"
#
# On a Proxmox VE host it creates a Debian LXC container and installs H.O.P.E inside it
# (community-scripts style). On any other systemd Linux host it installs H.O.P.E directly:
# a dedicated `hope` account, /opt/hope, /var/lib/hope, /etc/hope/hope.env, a hardened
# systemd unit with CAP_NET_RAW for discovery, and optionally Caddy as the HTTPS reverse
# proxy (public certificate for a real domain, or Caddy's internal CA for LAN-only names,
# whose root certificate is exported for agents at /etc/hope/ca.crt).
#
# The server binary comes from the latest GitHub release when one is published for this
# architecture (checksum verified) and otherwise from a source build (Rust and Node are
# installed under /opt/hope/build; nothing else on the system is touched).
# Re-running upgrades the binary and console and keeps /etc/hope/hope.env and the database.
#
# Non-interactive use (every prompt has an environment answer; unset values prompt):
#   HOPE_INSTALL_TARGET=host|lxc        where to install (lxc only on a Proxmox host; default lxc there)
#   HOPE_INSTALL_URL=https://hope.lan   canonical HTTPS URL (host name or IP; default https://<host IP>)
#   HOPE_INSTALL_PROXY=caddy|none       install Caddy in front (default caddy)
#   HOPE_INSTALL_SOURCE=auto|release|source
#   HOPE_INSTALL_VERSION=vX.Y.Z         pin a release tag (default: latest)
#   HOPE_INSTALL_REF=main               git ref for source builds
#   HOPE_INSTALL_NONINTERACTIVE=1       never prompt; missing answers use the defaults above
#   HOPE_INSTALL_CT_ID / _HOSTNAME / _STORAGE / _BRIDGE / _DISK_GB / _CORES / _MEMORY_MB / _IP
#                                       LXC settings (IP: dhcp or CIDR,gateway e.g. 192.168.1.50/24,192.168.1.1)
#   HOPE_INSTALL_REPO=owner/repo        (default doomedramen/hope)
#   HOPE_INSTALL_RELEASE_URL=...        TEST ONLY: base URL holding hope-server-<arch>-linux.tar.gz
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

# ---------- output ----------
if [ -t 1 ] && [ "${NO_COLOR:-}" = "" ]; then
    c_bold=$(printf '\033[1m'); c_blue=$(printf '\033[34m'); c_green=$(printf '\033[32m')
    c_yellow=$(printf '\033[33m'); c_red=$(printf '\033[31m'); c_off=$(printf '\033[0m')
else
    c_bold=; c_blue=; c_green=; c_yellow=; c_red=; c_off=
fi
info() { printf '%s  ›%s %s\n' "$c_blue" "$c_off" "$*"; }
ok() { printf '%s  ✓%s %s\n' "$c_green" "$c_off" "$*"; }
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
cleanup() { [ -n "$staging" ] && rm -rf "$staging" || true; }
trap cleanup EXIT HUP INT TERM

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
    aarch64|arm64) echo aarch64 ;;
    *) die "Unsupported architecture $(uname -m); H.O.P.E ships for x86_64 and aarch64." ;;
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
    debian) case "$1" in base) echo "curl ca-certificates tar xz-utils libcap2-bin" ;; build) echo "git build-essential pkg-config cmake" ;; caddy) echo caddy ;; esac ;;
    fedora) case "$1" in base) echo "curl ca-certificates tar xz libcap" ;; build) echo "git gcc gcc-c++ make pkgconf-pkg-config cmake" ;; caddy) echo caddy ;; esac ;;
    arch) case "$1" in base) echo "curl ca-certificates tar xz libcap" ;; build) echo "git base-devel cmake" ;; caddy) echo caddy ;; esac ;;
    esac
}

# ---------- obtaining the server ----------
release_url() {
    # Prints the tarball URL and, on a second line, the checksum file URL; nothing when no release fits.
    if [ -n "${HOPE_INSTALL_RELEASE_URL:-}" ]; then
        printf '%s/hope-server-%s-linux.tar.gz\n%s/SHA256SUMS-server\n' "$HOPE_INSTALL_RELEASE_URL" "$(arch)" "$HOPE_INSTALL_RELEASE_URL"
        return
    fi
    if [ -n "${HOPE_INSTALL_VERSION:-}" ]; then _api="https://api.github.com/repos/$repo/releases/tags/$HOPE_INSTALL_VERSION"
    else _api="https://api.github.com/repos/$repo/releases/latest"; fi
    _json=$(curl -fsSL --max-time 20 -H 'Accept: application/vnd.github+json' "$_api" 2>/dev/null) || return 0
    _tar=$(printf '%s' "$_json" | tr ',' '\n' | sed -n 's/.*"browser_download_url": *"\([^"]*hope-server-'"$(arch)"'-linux\.tar\.gz\)".*/\1/p' | head -n 1)
    [ -n "$_tar" ] || return 0
    _sums=$(printf '%s' "$_json" | tr ',' '\n' | sed -n 's/.*"browser_download_url": *"\([^"]*SHA256SUMS-server\)".*/\1/p' | head -n 1)
    printf '%s\n%s\n' "$_tar" "$_sums"
}
fetch_release() {
    _urls=$(release_url) || true
    [ -n "$_urls" ] || return 1
    _tar=$(printf '%s\n' "$_urls" | sed -n 1p)
    _sums=$(printf '%s\n' "$_urls" | sed -n 2p)
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
    [ -x "$staging/server/hope-server" ] && [ -f "$staging/server/web/index.html" ] || die 'The release archive is incomplete (hope-server and web/index.html expected).'
    ok "Release $(basename "$_tar") ready"
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
    case "$(arch)" in x86_64) _node_arch=x64 ;; aarch64) _node_arch=arm64 ;; esac
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
write_unit() {
    cat > "$root/etc/systemd/system/hope.service" <<EOF
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
    _env=$root/etc/hope/hope.env
    if [ -f "$_env" ]; then
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
    _env=$root/etc/hope/hope.env
    if grep -q "^$1=" "$_env"; then sed -i "s|^$1=.*|$1=$2|" "$_env"; else printf '%s=%s\n' "$1" "$2" >> "$_env"; fi
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
    _running=0
    systemctl is-active --quiet hope 2>/dev/null && _running=1
    install -m 755 "$staging/server/hope-server" "$root/opt/hope/bin/hope-server.new"
    mv -f "$root/opt/hope/bin/hope-server.new" "$root/opt/hope/bin/hope-server"
    rm -rf "$root/opt/hope/web.new"
    cp -R "$staging/server/web" "$root/opt/hope/web.new"
    rm -rf "$root/opt/hope/web.old"
    [ -d "$root/opt/hope/web" ] && mv "$root/opt/hope/web" "$root/opt/hope/web.old"
    mv "$root/opt/hope/web.new" "$root/opt/hope/web"
    rm -rf "$root/opt/hope/web.old"
    chown -R hope:hope "$root/var/lib/hope"
    chmod 700 "$root/var/lib/hope"
    setcap cap_net_raw=ep "$root/opt/hope/bin/hope-server" || warn 'setcap failed: ICMP and ARP discovery will be unavailable (TCP probing continues).'
    write_env
    write_unit
    systemctl daemon-reload
    if [ "$_running" = 1 ]; then systemctl restart hope; else systemctl enable --now hope >/dev/null 2>&1 || systemctl start hope; fi
    [ "$HOPE_INSTALL_PROXY" = caddy ] && install_caddy
    [ "$ca_exported" = 1 ] && systemctl restart hope
    _n=0
    while [ $_n -lt 30 ]; do
        curl -fsS --max-time 2 http://127.0.0.1:8080/healthz >/dev/null 2>&1 && break
        sleep 1; _n=$((_n + 1))
    done
    if [ $_n -lt 30 ]; then ok 'hope-server is healthy'
    else warn 'hope-server did not answer on 127.0.0.1:8080 yet; check: journalctl -u hope'; fi
    _token=$(journalctl -u hope -b --no-pager 2>/dev/null | sed -n 's/.*setup token: \([A-Za-z0-9_-]*\).*/\1/p' | tail -n 1)
    printf '\n%s%s%s\n' "$c_bold" 'H.O.P.E is installed.' "$c_off"
    printf '  Console:        %s\n' "$url"
    if [ -n "$_token" ]; then printf '  Setup token:    %s\n' "$_token"
    else printf '  Setup token:    journalctl -u hope | grep "setup token"\n'; fi
    printf '  Settings:       /etc/hope/hope.env   (systemctl restart hope)\n'
    printf '  Logs:           journalctl -fu hope\n'
    printf '  Agent releases: /opt/hope/releases   (publish a signed release before adding agents)\n'
    [ "$ca_exported" = 1 ] && printf '  Agents must trust the internal CA: copy /etc/hope/ca.crt to the same path on each agent host.\n'
    [ "$HOPE_INSTALL_PROXY" = none ] && printf '  Point your reverse proxy at http://127.0.0.1:8080 with Host and Origin unchanged and SSE unbuffered.\n'
    printf '\n'
}

# ---------- Proxmox LXC ----------
lxc_install() {
    [ "$(id -u)" = 0 ] || die 'Run this installer as root on the Proxmox host.'
    _next=$(pvesh get /cluster/nextid 2>/dev/null || echo 200)
    ask HOPE_INSTALL_CT_ID 'Container ID' "$_next"
    ask HOPE_INSTALL_CT_HOSTNAME 'Container host name' hope
    _storages=$(pvesm status --content rootdir 2>/dev/null | awk 'NR>1 {print $1}' | head -n 8)
    ask HOPE_INSTALL_CT_STORAGE 'Storage for the container disk' "$(printf '%s\n' "$_storages" | head -n 1)"
    ask HOPE_INSTALL_CT_DISK_GB 'Disk size (GB)' 8
    ask HOPE_INSTALL_CT_CORES 'CPU cores' 2
    ask HOPE_INSTALL_CT_MEMORY_MB 'Memory (MB)' 1024
    ask HOPE_INSTALL_CT_BRIDGE 'Network bridge' vmbr0
    ask HOPE_INSTALL_CT_IP 'IPv4 (dhcp, or CIDR,gateway such as 192.168.1.50/24,192.168.1.1)' dhcp
    choose HOPE_INSTALL_PROXY 'Reverse proxy inside the container' caddy 'caddy|Caddy with automatic HTTPS (recommended)' 'none|I run my own reverse proxy'
    _tmpl_storage=$(pvesm status --content vztmpl 2>/dev/null | awk 'NR>1 {print $1}' | head -n 1)
    [ -n "$_tmpl_storage" ] || die 'No storage accepts container templates (content vztmpl).'
    info 'Updating the template catalogue'
    pveam update >/dev/null 2>&1 || true
    _template=$(pveam available --section system 2>/dev/null | awk '{print $2}' | grep '^debian-1[23]-standard' | sort -V | tail -n 1)
    [ -n "$_template" ] || die 'No Debian template is available in the Proxmox catalogue.'
    pveam list "$_tmpl_storage" 2>/dev/null | grep -q "$_template" || { info "Downloading $_template"; pveam download "$_tmpl_storage" "$_template" >/dev/null || die 'Template download failed.'; }
    case "$HOPE_INSTALL_CT_IP" in
    dhcp) _net="name=eth0,bridge=$HOPE_INSTALL_CT_BRIDGE,ip=dhcp" ;;
    *) _net="name=eth0,bridge=$HOPE_INSTALL_CT_BRIDGE,ip=${HOPE_INSTALL_CT_IP%%,*},gw=${HOPE_INSTALL_CT_IP#*,}" ;;
    esac
    info "Creating container $HOPE_INSTALL_CT_ID ($HOPE_INSTALL_CT_HOSTNAME)"
    pct create "$HOPE_INSTALL_CT_ID" "$_tmpl_storage:vztmpl/$_template" \
        --hostname "$HOPE_INSTALL_CT_HOSTNAME" --cores "$HOPE_INSTALL_CT_CORES" --memory "$HOPE_INSTALL_CT_MEMORY_MB" \
        --rootfs "$HOPE_INSTALL_CT_STORAGE:$HOPE_INSTALL_CT_DISK_GB" --net0 "$_net" \
        --unprivileged 1 --features nesting=1 --onboot 1 --ostype debian \
        --description "H.O.P.E network observatory (install-hope.sh)" >/dev/null || die 'pct create failed.'
    pct start "$HOPE_INSTALL_CT_ID" >/dev/null || die 'pct start failed.'
    info 'Waiting for the container network'
    _n=0; _ip=
    while [ $_n -lt 60 ]; do
        _ip=$(pct exec "$HOPE_INSTALL_CT_ID" -- hostname -I 2>/dev/null | awk '{print $1}')
        [ -n "$_ip" ] && pct exec "$HOPE_INSTALL_CT_ID" -- getent hosts deb.debian.org >/dev/null 2>&1 && break
        sleep 2; _n=$((_n + 1))
    done
    [ -n "$_ip" ] || die 'The container did not get an IP address.'
    ask HOPE_INSTALL_URL 'Canonical HTTPS URL for H.O.P.E' "https://$_ip"
    info 'Installing H.O.P.E inside the container'
    pct exec "$HOPE_INSTALL_CT_ID" -- sh -c "export DEBIAN_FRONTEND=noninteractive; apt-get update -qq >/dev/null && apt-get install -y -qq --no-install-recommends curl ca-certificates >/dev/null && curl -fsSL '$script_url' -o /root/install-hope.sh" || die 'Could not fetch the installer inside the container.'
    pct exec "$HOPE_INSTALL_CT_ID" -- env HOPE_INSTALL_TARGET=host HOPE_INSTALL_NONINTERACTIVE=1 \
        HOPE_INSTALL_URL="$HOPE_INSTALL_URL" HOPE_INSTALL_PROXY="$HOPE_INSTALL_PROXY" \
        HOPE_INSTALL_SOURCE="${HOPE_INSTALL_SOURCE:-auto}" HOPE_INSTALL_VERSION="${HOPE_INSTALL_VERSION:-}" \
        HOPE_INSTALL_REF="${HOPE_INSTALL_REF:-main}" HOPE_INSTALL_REPO="$repo" NO_COLOR=1 \
        sh /root/install-hope.sh || die 'Installation inside the container failed; inspect with: pct enter '"$HOPE_INSTALL_CT_ID"
    printf '\n%sContainer %s is running at %s.%s\n' "$c_bold" "$HOPE_INSTALL_CT_ID" "$_ip" "$c_off"
    printf '  Enter it with: pct enter %s\n\n' "$HOPE_INSTALL_CT_ID"
}

# ---------- entry ----------
main() {
    banner
    _target=${HOPE_INSTALL_TARGET:-}
    if [ -z "$_target" ]; then
        if is_proxmox; then
            choose HOPE_INSTALL_TARGET 'This is a Proxmox VE host. Where should H.O.P.E run?' lxc 'lxc|In a new Debian LXC container (recommended)' 'host|Directly on this Proxmox host'
            _target=$HOPE_INSTALL_TARGET
        else
            _target=host
        fi
    fi
    case "$_target" in
    lxc) is_proxmox || die 'HOPE_INSTALL_TARGET=lxc needs a Proxmox VE host.'; lxc_install ;;
    host) host_install ;;
    *) die 'HOPE_INSTALL_TARGET must be host or lxc.' ;;
    esac
}
main "$@"
