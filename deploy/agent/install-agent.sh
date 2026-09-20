#!/usr/bin/env bash

set -Eeuo pipefail
umask 077

readonly SERVICE_NAME="hope-agent"
readonly SERVICE_USER="hope-agent"
readonly STATE_DIR="/var/lib/hope"
readonly AGENT_PATH="/usr/local/libexec/hope-agent"
readonly UNIT_PATH="/etc/systemd/system/hope-agent.service"
readonly LIBEXEC_DIR="/usr/local/libexec"

OPERATION="install"
ENROLL_URL=""
GATEWAY_URL=""
RELEASE_BASE_URL=""
RELEASE_URL=""
VERSION="latest"
CODE_STDIN=false
YES=false
ARCH=""
SERVICE_GROUP=""
TEMP_DIR=""
STAGE_PATH=""
UNIT_STAGE_PATH=""
RELEASE_ROOT=""
RELEASE_PATH=""

log() {
    printf '%s\n' "$*" >&2
}

die() {
    log "error: $*"
    exit 1
}

cleanup() {
    local status=$?
    trap - EXIT
    if [[ -n "$STAGE_PATH" ]]; then
        rm -f -- "$STAGE_PATH" || true
    fi
    if [[ -n "$UNIT_STAGE_PATH" ]]; then
        rm -f -- "$UNIT_STAGE_PATH" || true
    fi
    if [[ -n "$TEMP_DIR" && -d "$TEMP_DIR" ]]; then
        rm -rf -- "$TEMP_DIR" || true
    fi
    exit "$status"
}

trap cleanup EXIT

usage() {
    cat <<'EOF'
Usage:
  install-agent.sh --enroll-url URL --gateway-url URL \
    (--release-base-url URL | --release-url URL) [--version VERSION]
  install-agent.sh --uninstall --yes [--purge]

Install options:
  --enroll-url URL        HTTPS enrollment endpoint.
  --gateway-url URL       WSS/WS gateway endpoint written to systemd.
  --release-base-url URL  Hope server base URL; appends /agent-download.
  --release-url URL       Explicit /agent-download route root.
  --version VERSION       latest (default) or exact semantic version.
  --code-stdin            Read enrollment code from stdin; requires --yes.
  --yes                   Confirm noninteractive or destructive operation.

Uninstall options:
  --uninstall              Stop service and remove binary/unit; keep state/user.
  --purge                  With --uninstall, remove state and service user.
  --yes                    Required for uninstall and purge.

The release source must be supplied explicitly. No release host is inferred.
EOF
}

require_value() {
    local option=$1
    if (( $# < 2 )) || [[ -z "${2-}" ]]; then
        die "$option requires a value"
    fi
}

while (( $# > 0 )); do
    case "$1" in
        --enroll-url)
            require_value "$1" "${2-}"
            ENROLL_URL=$2
            shift 2
            ;;
        --enroll-url=*)
            ENROLL_URL=${1#*=}
            [[ -n "$ENROLL_URL" ]] || die "--enroll-url requires a value"
            shift
            ;;
        --gateway-url)
            require_value "$1" "${2-}"
            GATEWAY_URL=$2
            shift 2
            ;;
        --gateway-url=*)
            GATEWAY_URL=${1#*=}
            [[ -n "$GATEWAY_URL" ]] || die "--gateway-url requires a value"
            shift
            ;;
        --release-base-url)
            require_value "$1" "${2-}"
            RELEASE_BASE_URL=$2
            shift 2
            ;;
        --release-base-url=*)
            RELEASE_BASE_URL=${1#*=}
            [[ -n "$RELEASE_BASE_URL" ]] || die "--release-base-url requires a value"
            shift
            ;;
        --release-url)
            require_value "$1" "${2-}"
            RELEASE_URL=$2
            shift 2
            ;;
        --release-url=*)
            RELEASE_URL=${1#*=}
            [[ -n "$RELEASE_URL" ]] || die "--release-url requires a value"
            shift
            ;;
        --version)
            require_value "$1" "${2-}"
            VERSION=$2
            shift 2
            ;;
        --version=*)
            VERSION=${1#*=}
            [[ -n "$VERSION" ]] || die "--version requires a value"
            shift
            ;;
        --code-stdin)
            CODE_STDIN=true
            shift
            ;;
        --uninstall)
            OPERATION="uninstall"
            shift
            ;;
        --purge)
            PURGE=true
            shift
            ;;
        --yes)
            YES=true
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        --)
            shift
            (( $# == 0 )) || die "unexpected positional argument: $1"
            ;;
        *)
            die "unknown option: $1"
            ;;
    esac
done

PURGE=${PURGE:-false}

require_root() {
    [[ "$(id -u)" == "0" ]] || die "run installer as root or through sudo"
}

require_command() {
    local command_name
    for command_name in "$@"; do
        command -v "$command_name" >/dev/null 2>&1 || die "required command not found: $command_name"
    done
}

validate_url_chars() {
    local label=$1
    local value=$2
    [[ -n "$value" ]] || die "$label must not be empty"
    [[ "$value" != *$'\n'* && "$value" != *$'\r'* && "$value" != *$'\t'* && "$value" != *' '* ]] \
        || die "$label must not contain whitespace"
    [[ "$value" != *"'"* && "$value" != *'"'* && "$value" != *'\\'* && "$value" != *';'* && "$value" != *'#'* ]] \
        || die "$label contains a character unsafe for systemd or URL joining"
    [[ "$value" != *"?"* && "$value" != *"#"* ]] \
        || die "$label must not contain a query or fragment"
    [[ "$value" != *"@"* ]] || die "$label must not contain URL credentials"
}

validate_enroll_url() {
    validate_url_chars "--enroll-url" "$ENROLL_URL"
    [[ "$ENROLL_URL" == https://* ]] || die "--enroll-url must use https://"
    [[ "$ENROLL_URL" =~ ^https://[^/]+(/[^/]*)*$ ]] || die "--enroll-url is not a valid HTTPS URL"
}

validate_gateway_url() {
    validate_url_chars "--gateway-url" "$GATEWAY_URL"
    [[ "$GATEWAY_URL" == wss://* || "$GATEWAY_URL" == ws://* ]] \
        || die "--gateway-url must use wss:// or ws://"
    [[ "$GATEWAY_URL" =~ ^(wss|ws)://[^/]+(/[^/]*)*$ ]] || die "--gateway-url is not a valid WebSocket URL"
}

validate_release_url() {
    local option=$1
    local value=$2
    validate_url_chars "$option" "$value"
    [[ "$value" == https://* || "$value" == http://* ]] \
        || die "$option must use https:// or http://"
    [[ "$value" =~ ^https?://[^/]+(/[^/]*)*$ ]] || die "$option is not a valid HTTP URL"
}

validate_exact_version() {
    local value=$1
    local core
    local major
    local minor
    local patch
    local component

    [[ "$value" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$ ]] \
        || die "--version must be latest or semantic version MAJOR.MINOR.PATCH[-PRERELEASE]"

    core=${value%%-*}
    IFS=. read -r major minor patch <<< "$core"
    for component in "$major" "$minor" "$patch"; do
        if [[ "$component" == 0* && ${#component} -gt 1 ]]; then
            die "--version has a leading zero in its semantic version"
        fi
    done
}

trim_trailing_slashes() {
    local value=$1
    while [[ "$value" == */ ]]; do
        value=${value%/}
    done
    printf '%s' "$value"
}

detect_architecture() {
    [[ "$(uname -s)" == "Linux" ]] || die "unsupported operating system: installer requires Linux"
    case "$(uname -m)" in
        x86_64|amd64)
            ARCH="amd64"
            ;;
        aarch64|arm64)
            ARCH="arm64"
            ;;
        *)
            die "unsupported Linux architecture: $(uname -m); supported architectures are x86_64 and aarch64"
            ;;
    esac
}

check_path_not_symlink() {
    local path=$1
    local label=$2
    [[ ! -L "$path" ]] || die "$label is a symlink; refusing to follow it: $path"
}

file_size() {
    local path=$1
    local size
    size=$(wc -c < "$path") || return 1
    size=${size//[[:space:]]/}
    [[ "$size" =~ ^[0-9]+$ ]] || return 1
    printf '%s' "$size"
}

download_file() {
    local url=$1
    local destination=$2
    local maximum_bytes=$3
    local size

    rm -f -- "$destination"
    if command -v curl >/dev/null 2>&1; then
        if ! curl \
            --fail --silent --show-error --location \
            --proto '=http,https' --proto-redir '=http,https' \
            --connect-timeout 10 --max-time 120 --retry 2 \
            --max-filesize "$maximum_bytes" \
            --output "$destination" "$url"; then
            rm -f -- "$destination"
            return 1
        fi
    elif command -v wget >/dev/null 2>&1; then
        if ! wget \
            --quiet --max-redirect=5 --timeout=30 --tries=3 \
            --output-document="$destination" "$url"; then
            rm -f -- "$destination"
            return 1
        fi
    else
        return 1
    fi

    [[ -f "$destination" && ! -L "$destination" ]] || return 1
    size=$(file_size "$destination") || return 1
    (( size > 0 && size <= maximum_bytes )) || return 1
    return 0
}

release_contract_error() {
    local requested=$1
    die "release endpoint did not provide $requested; expected Hope routes <release-root>/<version>/linux/$ARCH/manifest.json, manifest.json.sig, and agent. Latest resolution expects <release-root>/latest/linux/$ARCH to return one exact semantic version. Check that server API contract is deployed."
}

resolve_release_path() {
    local base_url
    local selector

    if [[ -n "$RELEASE_URL" ]]; then
        RELEASE_ROOT=$(trim_trailing_slashes "$RELEASE_URL")
    else
        base_url=$(trim_trailing_slashes "$RELEASE_BASE_URL")
        RELEASE_ROOT="$base_url/agent-download"
    fi

    if [[ "$VERSION" == "latest" ]]; then
        if ! download_file "$RELEASE_ROOT/latest/linux/$ARCH" "$TEMP_DIR/latest" 4096; then
            die "release endpoint did not provide $RELEASE_ROOT/latest/linux/$ARCH; expected one exact semantic version as plain text. Check that latest server route is deployed or pass an exact --version."
        fi
        selector=$(tr -d '\r\n' < "$TEMP_DIR/latest")
        [[ -n "$selector" ]] || die "latest release selector is empty"
        validate_exact_version "$selector"
        VERSION=$selector
    else
        validate_exact_version "$VERSION"
    fi
    RELEASE_PATH="$RELEASE_ROOT/$VERSION/linux/$ARCH"
}

ensure_nologin_shell() {
    local candidate
    for candidate in /usr/sbin/nologin /sbin/nologin; do
        if [[ -x "$candidate" ]]; then
            printf '%s' "$candidate"
            return
        fi
    done
    die "could not find /usr/sbin/nologin or /sbin/nologin"
}

ensure_service_account() {
    local nologin_shell
    local current_shell
    local current_home
    local uid
    local gid

    nologin_shell=$(ensure_nologin_shell)
    if ! getent passwd "$SERVICE_USER" >/dev/null; then
        useradd --system --home-dir "$STATE_DIR" --shell "$nologin_shell" --user-group "$SERVICE_USER" \
            || die "could not create dedicated service user $SERVICE_USER"
    else
        uid=$(id -u "$SERVICE_USER")
        [[ "$uid" != "0" ]] || die "refusing to use root as $SERVICE_USER"
        current_shell=$(getent passwd "$SERVICE_USER" | awk -F: '{print $7}')
        if [[ "$current_shell" != "$nologin_shell" ]]; then
            usermod --shell "$nologin_shell" "$SERVICE_USER" \
                || die "could not set non-login shell for $SERVICE_USER"
        fi
        current_home=$(getent passwd "$SERVICE_USER" | awk -F: '{print $6}')
        if [[ "$current_home" != "$STATE_DIR" ]]; then
            usermod --home "$STATE_DIR" "$SERVICE_USER" \
                || die "could not set service home for $SERVICE_USER"
        fi
    fi

    uid=$(id -u "$SERVICE_USER")
    gid=$(id -g "$SERVICE_USER")
    [[ "$uid" != "0" && "$gid" != "0" ]] || die "refusing root-owned service identity for $SERVICE_USER"
    SERVICE_GROUP=$(id -gn "$SERVICE_USER")
    [[ "$SERVICE_GROUP" =~ ^[A-Za-z0-9_.-]+$ ]] || die "service group name is unsafe: $SERVICE_GROUP"

    check_path_not_symlink "$STATE_DIR" "state directory"
    install -d -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 0700 "$STATE_DIR" \
        || die "could not create $STATE_DIR"
    chown "$SERVICE_USER:$SERVICE_GROUP" "$STATE_DIR" || die "could not set state directory owner"
    chmod 0700 "$STATE_DIR" || die "could not set state directory mode"

    find "$STATE_DIR" -mindepth 1 -maxdepth 1 -type f \
        -exec chown "$SERVICE_USER:$SERVICE_GROUP" {} + \
        -exec chmod 0600 {} + \
        || die "could not secure existing state files"
}

state_is_enrolled() {
    local name
    for name in agent-key.pem agent-cert.pem ca-cert.pem; do
        [[ ! -L "$STATE_DIR/$name" && -s "$STATE_DIR/$name" ]] || return 1
    done
    return 0
}

run_as_service() {
    if command -v runuser >/dev/null 2>&1; then
        runuser -u "$SERVICE_USER" -- "$@"
    elif command -v sudo >/dev/null 2>&1; then
        sudo -n -u "$SERVICE_USER" -- "$@"
    else
        die "required command not found: runuser or sudo"
    fi
}

verify_downloaded_release() {
    local manifest_path=$1
    local binary_path=$2
    local verification_output="$TEMP_DIR/verify.log"
    local verifier_owner
    local expected_line="OK: $VERSION linux/$ARCH verified"

    chmod 0644 "$manifest_path" "$manifest_path.sig" "$binary_path" \
        || die "could not prepare downloaded release for verification"
    chmod 0755 "$TEMP_DIR" || die "could not prepare release temporary directory"

    if [[ -x "$AGENT_PATH" && ! -L "$AGENT_PATH" ]]; then
        verifier_owner=$(stat -c '%u' "$AGENT_PATH" 2>/dev/null || printf 'unknown')
    else
        verifier_owner="unknown"
    fi

    if [[ "$verifier_owner" == "0" ]]; then
        if ! "$AGENT_PATH" verify-release --manifest "$manifest_path" --binary "$binary_path" \
            > "$verification_output" 2>&1; then
            die "installed agent rejected signed release $VERSION; binary was not installed"
        fi
    else
        require_command runuser getent
        getent passwd nobody >/dev/null \
            || die "fresh release verification requires an unprivileged nobody account"
        if ! runuser -u nobody -- "$binary_path" verify-release \
            --manifest "$manifest_path" --binary "$binary_path" \
            > "$verification_output" 2>&1; then
            die "downloaded agent could not verify signed release $VERSION; binary was not installed"
        fi
    fi

    grep -Fq -- "$expected_line" "$verification_output" \
        || die "signed release verifier did not confirm exact version $VERSION for linux/$ARCH; binary was not installed"
    log "verified signed release $VERSION for Linux/$ARCH"
}

install_agent_binary() {
    local existing_type

    install -d -o root -g root -m 0755 "$LIBEXEC_DIR" \
        || die "could not create $LIBEXEC_DIR"
    chown root:root "$LIBEXEC_DIR" || die "could not set $LIBEXEC_DIR owner"
    chmod 0755 "$LIBEXEC_DIR" || die "could not set $LIBEXEC_DIR mode"

    if [[ -e "$AGENT_PATH" || -L "$AGENT_PATH" ]]; then
        check_path_not_symlink "$AGENT_PATH" "agent path"
        [[ -f "$AGENT_PATH" ]] || die "agent path is not a regular file: $AGENT_PATH"
    fi

    STAGE_PATH=$(mktemp "$LIBEXEC_DIR/.hope-agent.XXXXXX") \
        || die "could not create root-owned agent staging file"
    if ! install -o root -g root -m 0755 -- "$TEMP_DIR/agent-linux-$ARCH" "$STAGE_PATH"; then
        die "could not stage verified agent binary"
    fi
    existing_type=$(stat -c '%u:%a' "$STAGE_PATH" 2>/dev/null || true)
    [[ "$existing_type" == 0:* ]] || die "staged agent binary is not root-owned"
    mv -f -- "$STAGE_PATH" "$AGENT_PATH" || die "could not install verified agent binary"
    STAGE_PATH=""
    log "installed $AGENT_PATH (root-owned, mode 0755)"
}

enroll_agent() {
    local enrollment_log="$TEMP_DIR/enroll.log"
    local code

    if state_is_enrolled; then
        log "existing enrollment found; preserving $STATE_DIR"
        return
    fi

    if [[ "$CODE_STDIN" == true ]]; then
        if ! run_as_service "$AGENT_PATH" enroll \
            --server "$ENROLL_URL" --code-stdin --state-dir "$STATE_DIR" \
            > "$enrollment_log" 2>&1; then
            die "enrollment failed; enrollment code was not logged"
        fi
    else
        [[ -r /dev/tty ]] || die "interactive enrollment needs a terminal; use --code-stdin --yes"
        log "enter enrollment code (input hidden)"
        IFS= read -r -s code < /dev/tty || die "could not read enrollment code"
        printf '\n' >&2
        [[ -n "$code" ]] || die "enrollment code must not be empty"
        if ! printf '%s\n' "$code" | run_as_service "$AGENT_PATH" enroll \
            --server "$ENROLL_URL" --code-stdin --state-dir "$STATE_DIR" \
            > "$enrollment_log" 2>&1; then
            unset code
            die "enrollment failed; enrollment code was not logged"
        fi
        unset code
    fi

    state_is_enrolled || die "enrollment command completed without all state files"
    log "enrollment complete; code was not written to unit, arguments, or logs"
}

write_service_unit() {
    local gateway_for_unit

    gateway_for_unit=${GATEWAY_URL//%/%%}
    UNIT_STAGE_PATH=$(mktemp "/etc/systemd/system/.hope-agent.service.XXXXXX") \
        || die "could not create systemd unit staging file"
    cat > "$UNIT_STAGE_PATH" <<EOF
[Unit]
Description=Hope agent
After=network-online.target
Wants=network-online.target
StartLimitIntervalSec=60
StartLimitBurst=5

[Service]
Type=simple
User=$SERVICE_USER
Group=$SERVICE_GROUP
WorkingDirectory=$STATE_DIR
ExecStart=$AGENT_PATH run --gateway $gateway_for_unit --state-dir $STATE_DIR
Restart=on-failure
RestartSec=5s
NoNewPrivileges=true
PrivateTmp=true
ProtectHome=true
ProtectSystem=strict
ReadWritePaths=$STATE_DIR
CapabilityBoundingSet=
AmbientCapabilities=
RestrictSUIDSGID=true
LockPersonality=true
RestrictRealtime=true
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
SystemCallArchitectures=native
UMask=0077
TimeoutStopSec=30s

[Install]
WantedBy=multi-user.target
EOF
    chown root:root "$UNIT_STAGE_PATH" || die "could not set systemd unit owner"
    chmod 0644 "$UNIT_STAGE_PATH" || die "could not set systemd unit mode"
    mv -f -- "$UNIT_STAGE_PATH" "$UNIT_PATH" || die "could not install systemd unit"
    UNIT_STAGE_PATH=""
}

start_service() {
    systemctl daemon-reload || die "systemd daemon-reload failed"
    systemctl enable "$SERVICE_NAME" >/dev/null || die "could not enable $SERVICE_NAME"
    if ! systemctl restart "$SERVICE_NAME"; then
        systemctl --no-pager --full status "$SERVICE_NAME" || true
        die "$SERVICE_NAME failed to start"
    fi
    systemctl is-active --quiet "$SERVICE_NAME" \
        || die "$SERVICE_NAME is not active; inspect: systemctl status $SERVICE_NAME"
    log "$SERVICE_NAME active and enabled"
}

uninstall() {
    local active_status

    require_root
    require_command id rm getent systemctl
    if [[ "$PURGE" == true ]]; then
        require_command userdel groupdel
    fi
    [[ "$YES" == true ]] || die "--uninstall is destructive; pass --yes"

    if systemctl is-active --quiet "$SERVICE_NAME"; then
        systemctl stop "$SERVICE_NAME" || die "could not stop active $SERVICE_NAME"
        systemctl is-active --quiet "$SERVICE_NAME" \
            && die "$SERVICE_NAME remained active; refusing to remove its binary"
        log "stopped $SERVICE_NAME"
    else
        active_status=$?
        [[ "$active_status" == "3" ]] \
            || die "could not determine $SERVICE_NAME state; refusing to remove its binary"
        log "$SERVICE_NAME already inactive"
    fi

    systemctl disable "$SERVICE_NAME" >/dev/null 2>&1 || true
    if [[ -e "$UNIT_PATH" || -L "$UNIT_PATH" ]]; then
        check_path_not_symlink "$UNIT_PATH" "systemd unit"
        [[ -f "$UNIT_PATH" ]] || die "systemd unit path is not a regular file: $UNIT_PATH"
        rm -f -- "$UNIT_PATH" || die "could not remove systemd unit"
    fi
    systemctl daemon-reload || die "systemd daemon-reload failed after unit removal"

    if [[ -e "$AGENT_PATH" || -L "$AGENT_PATH" ]]; then
        check_path_not_symlink "$AGENT_PATH" "agent path"
        [[ -f "$AGENT_PATH" ]] || die "agent path is not a regular file: $AGENT_PATH"
        rm -f -- "$AGENT_PATH" || die "could not remove $AGENT_PATH"
    fi

    if [[ "$PURGE" == true ]]; then
        [[ "$STATE_DIR" == "/var/lib/hope" ]] || die "refusing unsafe state path"
        check_path_not_symlink "$STATE_DIR" "state directory"
        if [[ -e "$STATE_DIR" ]]; then
            rm -rf -- "$STATE_DIR" || die "could not purge $STATE_DIR"
        fi
        if getent passwd "$SERVICE_USER" >/dev/null; then
            userdel "$SERVICE_USER" || die "could not remove service user $SERVICE_USER"
        fi
        if getent group "$SERVICE_USER" >/dev/null; then
            groupdel "$SERVICE_USER" \
                || log "warning: service group $SERVICE_USER remains because it is still in use"
        fi
        log "purged $STATE_DIR and service user $SERVICE_USER"
    else
        log "preserved $STATE_DIR and service user $SERVICE_USER"
    fi

    log "uninstall complete"
}

if [[ "$OPERATION" == "uninstall" ]]; then
    [[ -z "$ENROLL_URL" && -z "$GATEWAY_URL" && -z "$RELEASE_BASE_URL" && -z "$RELEASE_URL" ]] \
        || die "installation URLs cannot be used with --uninstall"
    [[ "$VERSION" == "latest" ]] || die "--version cannot be used with --uninstall"
    [[ "$CODE_STDIN" == false ]] || die "--code-stdin cannot be used with --uninstall"
    [[ "$PURGE" == true || "$PURGE" == false ]] || die "invalid purge state"
    uninstall
    exit 0
fi

[[ "$PURGE" == false ]] || die "--purge requires --uninstall"
[[ -n "$ENROLL_URL" ]] || die "--enroll-url is required"
[[ -n "$GATEWAY_URL" ]] || die "--gateway-url is required"
[[ -n "$RELEASE_BASE_URL" || -n "$RELEASE_URL" ]] \
    || die "one explicit release source is required: --release-base-url or --release-url"
[[ -z "$RELEASE_BASE_URL" || -z "$RELEASE_URL" ]] \
    || die "use only one of --release-base-url and --release-url"
[[ "$CODE_STDIN" == false || "$YES" == true ]] \
    || die "--code-stdin is noninteractive; pass --yes"

require_root
require_command id uname mktemp rm chmod chown install mv find awk grep wc tr stat getent useradd usermod systemctl
if ! command -v curl >/dev/null 2>&1 && ! command -v wget >/dev/null 2>&1; then
    die "required command not found: curl or wget"
fi

validate_enroll_url
validate_gateway_url
if [[ -n "$RELEASE_BASE_URL" ]]; then
    validate_release_url "--release-base-url" "$RELEASE_BASE_URL"
else
    validate_release_url "--release-url" "$RELEASE_URL"
fi
if [[ "$VERSION" != "latest" ]]; then
    validate_exact_version "$VERSION"
fi
if [[ -n "$RELEASE_URL" && "$VERSION" == "latest" ]]; then
    die "--release-url requires an exact --version; use --release-base-url for latest"
fi

detect_architecture
TEMP_DIR=$(mktemp -d /tmp/hope-agent-install.XXXXXX) \
    || die "could not create secure temporary directory"
chmod 0755 "$TEMP_DIR" || die "could not prepare temporary directory"

resolve_release_path
MANIFEST_PATH="$TEMP_DIR/manifest.json"
BINARY_PATH="$TEMP_DIR/agent-linux-$ARCH"

if ! download_file "$RELEASE_PATH/manifest.json" "$MANIFEST_PATH" 524288; then
    release_contract_error "$RELEASE_PATH/manifest.json"
fi
if ! download_file "$RELEASE_PATH/manifest.json.sig" "$MANIFEST_PATH.sig" 4096; then
    release_contract_error "$RELEASE_PATH/manifest.json.sig"
fi
if ! download_file "$RELEASE_PATH/agent" "$BINARY_PATH" 67108864; then
    release_contract_error "$RELEASE_PATH/agent"
fi

verify_downloaded_release "$MANIFEST_PATH" "$BINARY_PATH"
ensure_service_account
install_agent_binary
enroll_agent
write_service_unit
start_service

log "installed Hope agent $VERSION for Linux/$ARCH"
log "state: $STATE_DIR (mode 0700, preserved on reinstall)"
log "service: systemctl status $SERVICE_NAME"
