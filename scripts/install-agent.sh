#!/bin/sh
# H.O.P.E agent installer. Served over HTTPS with the release verification key embedded.
#
# Trust: the key below is trusted because this script was fetched over verified HTTPS
# (custom CA via HOPE_CA_CERT when needed). The installer verifies every downloaded
# binary against it before executing anything it downloaded.
#
# Operations: install | upgrade | uninstall
# Environment:
#   HOPE_SERVER, HOPE_ENROLLMENT_TOKEN, HOPE_CA_CERT, HOPE_AGENT_DIR, HOPE_RESET_CREDENTIAL
#   HOPE_INSTALL_MODE=auto|user|system  (auto detects an existing install, else root/sudo decides)
#   HOPE_PURGE_IDENTITY=1               (uninstall: also delete the enrolled identity; revoke it in H.O.P.E)
#   HOPE_FIRST_REPORT_TIMEOUT=120       (seconds to wait for the service's first accepted report)
#   HOPE_POLL_INTERVAL=2                (seconds between acceptance checks; test knob)
#   HOPE_INSTALL_ROOT                   (TEST ONLY: prefix for absolute system paths)
#
# Output: progress lines "hope-install: phase=NAME" and failures "hope-install: error=CODE message"
# on stderr. Enrollment tokens and credentials are never printed.
#
# Phases: preflight -> stage -> verify -> stop -> place-binary -> configure -> write-unit
#         -> activate -> wait-report -> complete. Everything from "stop" onward is journaled in
#         STATE/.install-transaction; failure, a signal, or an interrupted earlier run restores the
#         previous binary, service definition and settings and restarts the previous service.
#
# Service accounts (hope-agent / _hopeagent) created by the installer are kept on rollback and
# uninstall; remove them yourself if no longer wanted. A pre-existing account is reused only when
# it is a dedicated, no-login account with its own group.
# The system-mode transaction directory is root-owned and outside the service-writable state
# directory. Signals are handled when the current foreground command returns, so a long-running
# child (for example a slow download) delays interruption handling until it finishes.
# User services: systemd user services stop at logout unless an administrator enables lingering
# (loginctl enable-linger USER). macOS LaunchAgents run only while that user has a GUI session.
set -eu
umask 077

operation=${1:-install}
temporary=
txn=
txn_active=0
rb_ok=1
no_manager=0
preserve=HOPE_SERVER,HOPE_ENROLLMENT_TOKEN,HOPE_CA_CERT,HOPE_INSTALL_MODE,HOPE_AGENT_DIR,HOPE_RESET_CREDENTIAL,HOPE_PURGE_IDENTITY,HOPE_FIRST_REPORT_TIMEOUT,HOPE_POLL_INTERVAL,HOPE_INSTALL_ROOT
root=${HOPE_INSTALL_ROOT:-}
root=${root%/}

# Keep a private copy of stderr so progress/errors survive redirections active when a signal trap runs.
exec 3>&2
phase() { printf 'hope-install: phase=%s\n' "$1" >&3; }
note() { printf 'hope-install: %s\n' "$*" >&3; }
err() { printf 'hope-install: error=%s %s\n' "$1" "$2" >&3; }
fail() { err "$1" "$2"; exit 1; }

journal() { [ -d "$txn" ] && printf '%s\n' "$1" >> "$txn/journal" || true; }
jget() { sed -n "s/^$1=//p" "$txn/journal" 2>/dev/null | tail -n 1; }

is_system() { [ "$mode" = system ]; }
is_linux() { [ "$platform" = linux ]; }

paths_for() {
    case "$1" in
    system)
        binary=$root/usr/local/bin/hope-agent
        state_default=$root/var/lib/hope-agent
        if [ "$platform" = linux ]; then unit=$root/etc/systemd/system/hope-agent.service
        else unit=$root/Library/LaunchDaemons/io.hope.agent.plist; fi;;
    *)
        binary=${HOME:-/nonexistent}/.local/bin/hope-agent
        state_default=${HOME:-/nonexistent}/.local/state/hope-agent
        if [ "$platform" = linux ]; then unit=${HOME:-/nonexistent}/.config/systemd/user/hope-agent.service
        else unit=${HOME:-/nonexistent}/Library/LaunchAgents/io.hope.agent.plist; fi;;
    esac
}
txn_for() {
    if [ "$1" = system ]; then
        if [ "$platform" = linux ]; then printf '%s\n' "$root/var/lib/hope-agent-install"
        else printf '%s\n' "$root/Library/Application Support/hope-agent-install"; fi
    else printf '%s\n' "$state_default/.install-transaction"; fi
}
present_for() {
    paths_for "$1"
    [ -e "$unit" ] || [ -e "$binary" ] || [ -e "$state_default/config.json" ] || [ -d "$(txn_for "$1")" ]
}
mkdir_public() { ( umask 022; mkdir -p "$1" ); }
state_safe() { [ ! -L "$state" ] && [ ! -L "$state/config.json" ]; }

# --- service manager adapters -------------------------------------------------------------
sc() { if is_system; then systemctl "$@"; else systemctl --user "$@"; fi; }
svc_active() {
    [ "$no_manager" = 0 ] || return 1
    if is_linux; then sc is-active --quiet hope-agent >/dev/null 2>&1
    else launchctl print "$domain/io.hope.agent" 2>/dev/null | awk '$1 == "pid" && $2 == "=" && $3 + 0 > 0 { found = 1 } END { exit !found }'; fi
}
svc_loaded() {
    if is_linux; then sc is-active --quiet hope-agent >/dev/null 2>&1
    else launchctl print "$domain/io.hope.agent" >/dev/null 2>&1; fi
}
svc_stop() {
    [ "$no_manager" = 0 ] || return 0
    if is_linux; then sc stop hope-agent >/dev/null 2>&1 || true
    else launchctl bootout "$domain/io.hope.agent" >/dev/null 2>&1 || true; fi
}
svc_disable() { [ "$no_manager" = 0 ] || return 0; if is_linux; then sc disable hope-agent >/dev/null 2>&1 || true; fi; }
svc_reload() { [ "$no_manager" = 0 ] || return 0; if is_linux; then sc daemon-reload; fi; }
svc_start() {
    if is_linux; then sc start hope-agent; else launchctl bootstrap "$domain" "$unit"; fi
}
svc_enable_now() {
    if is_linux; then sc enable --now hope-agent; else launchctl bootstrap "$domain" "$unit"; fi
}
svc_pid() {
    if is_linux; then sc show -p MainPID hope-agent 2>/dev/null | sed 's/^MainPID=//' | head -n 1
    else launchctl print "$domain/io.hope.agent" 2>/dev/null | awk '$1 == "pid" && $2 == "=" { print $3; exit }'; fi
}
chown_state() {
    if is_system && [ -d "$state" ]; then chown -R "$service_user:$service_group" "$state"; fi
}

# --- transaction rollback ------------------------------------------------------------------
rollback() {
    trap '' HUP INT TERM QUIT
    txn_active=0
    rb_ok=1
    if [ ! -d "$txn" ]; then rm -rf "$txn.tmp"; return 0; fi
    phase rollback
    svc_stop
    if [ "$(jget had_binary)" = 1 ]; then
        if [ -f "$txn/binary.prev" ] && cp "$txn/binary.prev" "$binary.new" && chmod 755 "$binary.new" && mv -f "$binary.new" "$binary"; then :; else rb_ok=0; fi
    else rm -f "$binary" "$binary.new"; fi
    if [ "$(jget had_unit)" = 1 ]; then
        if [ -f "$txn/unit.prev" ] && cp "$txn/unit.prev" "$unit.new" && chmod 644 "$unit.new" && mv -f "$unit.new" "$unit"; then :; else rb_ok=0; fi
    else svc_disable; rm -f "$unit" "$unit.new"; fi
    # Settings: the previous binary restores them while keeping any credential already issued.
    # Without a previous installation the freshly enrolled identity is retained for the retry.
    if [ "$(jget configure)" = started ] && [ "$(jget had_config)" = 1 ] && [ "$(jget had_binary)" = 1 ]; then
        if ! state_safe; then
            note 'warning=state directory or config.json is a symbolic link; settings were not restored'
        elif ! HOPE_AGENT_DIR="$state" "$binary" restore-config "$txn/config.json.prev" >/dev/null 2>&1; then
            note 'warning=previous settings could not be restored automatically; current settings were kept'
        fi
    fi
    chown_state || rb_ok=0
    svc_reload || true
    if [ "$(jget was_active)" = 1 ] && [ "$rb_ok" = 1 ]; then
        svc_start || { note 'warning=the previous service could not be restarted'; rb_ok=0; }
    fi
    if [ "$rb_ok" = 1 ]; then rm -rf "$txn"
    else err ROLLBACK_INCOMPLETE "Some files could not be restored; the backup was kept in $txn for the next run."; fi
    return 0
}

on_exit() {
    rc=$?
    trap '' HUP INT TERM QUIT
    set +e
    if [ "$txn_active" = 1 ]; then
        rollback
        [ "$rb_ok" = 1 ] && err ROLLED_BACK 'The previous installation was restored. Nothing new is running.'
        [ "$rc" -eq 0 ] && rc=1
    fi
    [ -n "$temporary" ] && rm -rf "$temporary"
    [ -n "$txn" ] && [ "$txn_active" = 0 ] && rm -rf "$txn.tmp"
    exit "$rc"
}
on_signal() { trap '' HUP INT TERM QUIT; err INTERRUPTED 'The installer was interrupted.'; exit 1; }
trap on_exit EXIT
trap on_signal HUP INT TERM QUIT

# =================================== phase: preflight =========================================
phase preflight
case "$operation" in install|upgrade|uninstall) ;; *) err USAGE 'Usage: install-agent.sh [install|upgrade|uninstall]'; exit 2;; esac
case "$(uname -s)" in Linux) platform=linux;; Darwin) platform=macos;; *) fail UNSUPPORTED_PLATFORM 'H.O.P.E supports Linux and macOS.';; esac
case "$(uname -m)" in x86_64|amd64) architecture=x86_64;; arm64|aarch64) architecture=aarch64;; *) fail UNSUPPORTED_ARCH 'Unsupported CPU architecture.';; esac
case "$root" in ''|/*) ;; *) fail PATH_UNWRITABLE 'HOPE_INSTALL_ROOT must be absolute.';; esac

requested=${HOPE_INSTALL_MODE:-auto}
case "$requested" in auto|user|system) ;; *) fail INVALID_MODE 'HOPE_INSTALL_MODE must be auto, user, or system.';; esac
uid=$(id -u)
sudo_ok() { command -v sudo >/dev/null 2>&1 && sudo -n --preserve-env="$preserve" true >/dev/null 2>&1; }

mode=$requested
fresh_auto=0
if [ "$requested" = auto ]; then
    has_system=0; has_user=0
    present_for system && has_system=1
    present_for user && has_user=1
    if [ "$has_system" = 1 ] && [ "$has_user" = 1 ]; then
        fail EXISTING_INSTALL_MODE "Both a system and a user installation exist. Set HOPE_INSTALL_MODE=system or HOPE_INSTALL_MODE=user to choose which one to $operation."
    elif [ "$has_system" = 1 ]; then mode=system
    elif [ "$has_user" = 1 ]; then mode=user
    else
        if [ "$operation" = uninstall ]; then echo 'No H.O.P.E agent installation was found. Nothing to remove.'; exit 0; fi
        if [ "$operation" = upgrade ]; then fail NOT_INSTALLED 'No existing installation was found to upgrade. Use install.'; fi
        fresh_auto=1
        if [ "$uid" -eq 0 ]; then mode=system
        elif sudo_ok; then mode=system
        else
            mode=user
            note 'notice=Non-interactive sudo is unavailable, so a user service will be installed instead of a system service.'
            note 'notice=A systemd user service stops at logout unless an administrator enables lingering for your account; a macOS LaunchAgent runs only during your GUI login session.'
        fi
    fi
fi
if [ "$mode" = user ] && [ "$uid" -eq 0 ]; then
    fail INVALID_MODE 'A user service must be managed by its own user, not root. Run the installer as that user, or use HOPE_INSTALL_MODE=system.'
fi
if [ "$requested" != auto ]; then
    if [ "$mode" = system ]; then other=user; else other=system; fi
    paths_for "$other"
    if [ -d "$(txn_for "$other")" ]; then
        fail EXISTING_INSTALL_MODE "An interrupted installation exists in $other mode. Rerun with HOPE_INSTALL_MODE=$other first so it can be rolled back."
    fi
fi
if [ "$mode" = system ] && [ "$uid" -ne 0 ]; then
    sudo_ok || fail SUDO_UNAVAILABLE 'System mode needs root or sudo that works without a password prompt (sudo -n). Run as root, or use HOPE_INSTALL_MODE=user.'
    note 'notice=Using non-interactive sudo for a system service.'
    export HOPE_INSTALL_MODE=system
    exec sudo -n --preserve-env="$preserve" sh "$0" "$operation"
fi
if [ "$mode" = user ]; then
    case "${HOME:-}" in /*) ;; *) fail PATH_UNWRITABLE 'HOME must be an absolute path for a user service.';; esac
    if [ "$fresh_auto" = 0 ] || [ "$uid" -ne 0 ]; then
        note 'notice=Installing a user service. Reporting may stop after logout (systemd lingering) or outside a macOS GUI session.'
    fi
fi
HOPE_INSTALL_PID=$$
export HOPE_INSTALL_PID

paths_for "$mode"
state=${HOPE_AGENT_DIR:-$state_default}
case "$state" in /*) ;; *) fail PATH_UNWRITABLE 'HOPE_AGENT_DIR must be an absolute path.';; esac
case "$state$binary$unit" in *'%'*|*'"'*|*'\'*|*'
'*) fail PATH_UNWRITABLE 'Installation paths cannot contain percent signs, quotes, backslashes, or newlines.';; esac
export HOPE_AGENT_DIR="$state"
if is_system; then txn=$(txn_for system); else txn=$state/.install-transaction; fi
if is_linux; then service_user=hope-agent; service_group=hope-agent; else service_user=_hopeagent; service_group=_hopeagent; fi
if is_system; then domain=system; else domain="gui/$uid"; fi

temporary=$(mktemp -d "${TMPDIR:-/tmp}/hope-install.XXXXXX") || fail PATH_UNWRITABLE 'Cannot create a private temporary directory.'

# Service manager must be running, not merely installed.
manager_error=
if is_linux; then
    if ! command -v systemctl >/dev/null 2>&1; then manager_error='systemd is required; no service was installed.'
    else
        manager_state=$(sc is-system-running 2>/dev/null || true)
        case "$manager_state" in running|degraded|starting) ;;
            *) if is_system; then manager_error="systemd is not running (state: ${manager_state:-unknown}); no service was installed."
               else manager_error='No running systemd user manager. Log in normally, or run as root/sudo for a system service.'; fi;;
        esac
    fi
else
    if ! command -v launchctl >/dev/null 2>&1; then manager_error='launchd is required.'
    elif is_system; then launchctl print system >/dev/null 2>&1 || manager_error='launchd system domain is unavailable.'
    else launchctl print "$domain" >/dev/null 2>&1 || manager_error='No graphical user session. Log in at the Mac or use administrator access for a system daemon.'; fi
fi
if [ -n "$manager_error" ]; then
    if [ "$operation" = uninstall ]; then
        note "warning=$manager_error Files will be removed, but any running service must be stopped manually."
        no_manager=1
    else fail NO_SERVICE_MANAGER "$manager_error"; fi
fi

writable_target() {
    target=$1
    while [ ! -e "$target" ] && [ ! -L "$target" ]; do target=$(dirname "$target"); done
    [ -d "$target" ] && [ -w "$target" ]
}
for target in "$(dirname "$binary")" "$(dirname "$unit")" "$state" "$(dirname "$txn")"; do
    writable_target "$target" || fail PATH_UNWRITABLE "Cannot write to $target."
done
state_safe || fail PATH_UNWRITABLE "$state or its config.json is a symbolic link; refusing to run privileged operations on it."

# Converge after an interrupted run: restore the previous installation first.
if [ -d "$txn" ]; then
    phase recover
    if [ -f "$txn/journal" ]; then
        rollback
        [ "$rb_ok" = 1 ] || fail ROLLBACK_INCOMPLETE 'An interrupted installation could not be restored automatically.'
        note 'notice=An interrupted earlier run was rolled back; continuing.'
    else rm -rf "$txn"; fi
fi
rm -rf "$txn.tmp"

if [ "$operation" = uninstall ]; then
    phase uninstall
    svc_stop
    svc_disable
    rm -f "$unit" "$binary" "$binary.new"
    svc_reload || true
    if [ "${HOPE_PURGE_IDENTITY:-}" = 1 ]; then
        state_safe || fail PATH_UNWRITABLE 'Refusing to purge through a symbolic link.'
        # Remove only files the agent creates; never the whole (possibly shared) directory.
        for name in config.json spool.json last-acceptance.json previous.json sample.json last-report agent.lock agent.log bootstrap-ca.pem .install-transaction; do
            rm -rf "$state/$name"
        done
        rm -f "$state"/.hope-*.tmp
        rmdir "$state" 2>/dev/null || note "warning=$state contains other files and was kept"
        echo "Service removed and identity deleted from $state. Revoke its credential in H.O.P.E."
    else
        echo "Service removed. Identity retained at $state. Revoke its credential in H.O.P.E before deleting that directory, or set HOPE_PURGE_IDENTITY=1."
    fi
    phase complete
    exit 0
fi

: "${HOPE_SERVER:?Set HOPE_SERVER to your H.O.P.E HTTPS URL}"
HOPE_SERVER=${HOPE_SERVER%/}
case "$HOPE_SERVER" in
    http://*) fail INSECURE_URL 'The installer requires an https:// server URL.';;
    https://*) ;;
    *) fail MALFORMED_URL 'HOPE_SERVER must look like https://host[:port].';;
esac
case "$HOPE_SERVER" in *[!]A-Za-z0-9._~:/%+=,[-]*) fail MALFORMED_URL 'HOPE_SERVER contains characters that are not allowed in a server URL.';; esac
server_host=${HOPE_SERVER#https://}; server_host=${server_host%%/*}
case "$server_host" in ''|:*) fail MALFORMED_URL 'HOPE_SERVER has no host name.';; esac
export HOPE_SERVER
if [ -n "${HOPE_CA_CERT:-}" ] && { [ ! -f "$HOPE_CA_CERT" ] || [ ! -r "$HOPE_CA_CERT" ]; }; then
    fail CA_UNREADABLE 'HOPE_CA_CERT is not a readable file.'
fi
command -v curl >/dev/null 2>&1 || fail DOWNLOAD_FAILED 'Install curl first.'
command -v openssl >/dev/null 2>&1 || fail BAD_SIGNATURE 'Install OpenSSL first to verify the signed release.'
if [ ! -f "$state/config.json" ] && [ -z "${HOPE_ENROLLMENT_TOKEN:-}" ]; then
    fail CONFIGURE_FAILED 'HOPE_ENROLLMENT_TOKEN is required for a first installation.'
fi
case "${HOPE_FIRST_REPORT_TIMEOUT:-120}" in ''|*[!0-9]*) fail INVALID_SETTING 'HOPE_FIRST_REPORT_TIMEOUT must be a number of seconds.';; esac

cat > "$temporary/release-public.pem" <<'HOPE_PUBLIC_KEY'
@@HOPE_PUBLIC_KEY@@
HOPE_PUBLIC_KEY
first_line=$(sed -n '1p' "$temporary/release-public.pem")
last_line=$(sed -n '$p' "$temporary/release-public.pem")
[ "$first_line" = '-----BEGIN PUBLIC KEY-----' ] && [ "$last_line" = '-----END PUBLIC KEY-----' ] \
    && ! grep -q '@@' "$temporary/release-public.pem" \
    && openssl pkey -pubin -in "$temporary/release-public.pem" -noout >/dev/null 2>&1 \
    || fail BAD_SIGNATURE 'The embedded release verification key is missing or invalid; fetch the installer from your H.O.P.E server.'

fetch() {
    if [ -n "${HOPE_CA_CERT:-}" ]; then
        ( unset CURL_CA_BUNDLE SSL_CERT_FILE SSL_CERT_DIR; curl -q --fail --silent --show-error --location --connect-timeout 20 --max-time 120 --proto '=https' --proto-redir '=https' --cacert "$HOPE_CA_CERT" "$@" )
    else
        ( unset CURL_CA_BUNDLE SSL_CERT_FILE SSL_CERT_DIR; curl -q --fail --silent --show-error --location --connect-timeout 20 --max-time 120 --proto '=https' --proto-redir '=https' "$@" )
    fi
}
artifact="hope-agent-$platform-$architecture"
# The availability API needs a login, so confirm the public artifacts exist before stopping anything.
for name in "$artifact" "$artifact.sig"; do
    code=$(fetch --head -o /dev/null --write-out '%{http_code}' "$HOPE_SERVER/agent/releases/$name" 2>/dev/null || true)
    case "$code" in
        2??) ;;
        404) fail RELEASE_UNAVAILABLE "No signed agent is packaged for $platform/$architecture on this server." ;;
        *) fail DOWNLOAD_FAILED "Cannot check $name on $HOPE_SERVER (HTTP status ${code:-none}). Check the URL, network, and HOPE_CA_CERT." ;;
    esac
done

# =================================== phase: stage =============================================
phase stage
echo "Downloading signed $platform/$architecture agent."
fetch -o "$temporary/hope-agent" "$HOPE_SERVER/agent/releases/$artifact" || fail DOWNLOAD_FAILED 'Downloading the agent binary failed.'
fetch -o "$temporary/hope-agent.sig" "$HOPE_SERVER/agent/releases/$artifact.sig" || fail DOWNLOAD_FAILED 'Downloading the agent signature failed.'
openssl dgst -sha256 -verify "$temporary/release-public.pem" -signature "$temporary/hope-agent.sig" "$temporary/hope-agent" >/dev/null 2>&1 \
    || fail BAD_SIGNATURE 'Release signature verification failed. Nothing was installed.'
chmod 755 "$temporary/hope-agent"
"$temporary/hope-agent" --version || fail BINARY_NOT_EXECUTABLE 'The verified agent cannot run from the temporary directory (noexec mount or wrong platform?). Set TMPDIR to an executable directory.'

if is_system; then
    if is_linux; then
        if entry=$(getent passwd "$service_user" 2>/dev/null); then
            account_uid=$(printf '%s' "$entry" | cut -d: -f3)
            account_gid=$(printf '%s' "$entry" | cut -d: -f4)
            account_home=$(printf '%s' "$entry" | cut -d: -f6)
            account_shell=$(printf '%s' "$entry" | cut -d: -f7)
            case "$account_uid" in ''|*[!0-9]*) account_uid=0;; esac
            case "$account_shell" in /usr/sbin/nologin|/sbin/nologin|/usr/bin/false|/bin/false) shell_ok=1;; *) shell_ok=0;; esac
            group_entry=$(getent group "$account_gid" 2>/dev/null || true)
            group_name=$(printf '%s' "$group_entry" | cut -d: -f1)
            group_members=$(printf '%s' "$group_entry" | cut -d: -f4)
            if [ "$account_uid" -lt 1 ] || [ "$account_uid" -ge 1000 ] || [ "$account_home" != "$state" ] || [ "$shell_ok" != 1 ] \
                || [ "$group_name" != "$service_group" ] || { [ -n "$group_members" ] && [ "$group_members" != "$service_user" ]; }; then
                fail SERVICE_ACCOUNT_INVALID "Existing account $service_user must be a system account (UID 1-999) with home $state, a no-login shell, and its own dedicated $service_group group with no other members."
            fi
        else
            useradd --system --user-group --no-create-home --home-dir "$state" --shell /usr/sbin/nologin "$service_user" \
                || fail SERVICE_ACCOUNT_INVALID "Cannot create the $service_user service account."
        fi
    else
        dread() { dscl . -read "$1" "$2" 2>/dev/null | awk -v key="$2:" '$1 == key { print $2; exit }'; }
        count_id() { dscl . -list "$1" "$2" 2>/dev/null | awk -v id="$3" '$2 == id { n++ } END { print n + 0 }'; }
        if dscl . -read "/Users/$service_user" UniqueID >/dev/null 2>&1; then
            account_uid=$(dread "/Users/$service_user" UniqueID)
            account_gid=$(dread "/Users/$service_user" PrimaryGroupID)
            account_shell=$(dread "/Users/$service_user" UserShell)
            account_home=$(dread "/Users/$service_user" NFSHomeDirectory)
            group_gid=$(dread "/Groups/$service_group" PrimaryGroupID)
            members=$(dscl . -read "/Groups/$service_group" GroupMembership 2>/dev/null | sed 's/^GroupMembership:[[:space:]]*//' | tr -d '\n')
            case "$account_uid" in ''|*[!0-9]*) account_uid=0;; esac
            case "$account_gid" in ''|*[!0-9]*) account_gid=20;; esac
            if [ "$account_uid" -lt 400 ] || [ "$account_uid" -gt 499 ] || [ "$account_shell" != /usr/bin/false ] || [ "$account_home" != "$state" ] \
                || [ "$account_gid" -eq 20 ] || [ "$account_gid" != "$group_gid" ] \
                || [ "$(count_id /Users UniqueID "$account_uid")" -ne 1 ] || [ "$(count_id /Groups PrimaryGroupID "$account_gid")" -ne 1 ] \
                || { [ -n "$members" ] && [ "$members" != "$service_user" ]; }; then
                fail SERVICE_ACCOUNT_INVALID "Existing account $service_user must have a unique UID in 400-499, shell /usr/bin/false, home $state, and its own dedicated group (not staff) with no other members."
            fi
        else
            used=$( { dscl . -list /Users UniqueID; dscl . -list /Groups PrimaryGroupID; } | awk '{print $2}')
            group_gid=$(dread "/Groups/$service_group" PrimaryGroupID)
            if [ -n "$group_gid" ]; then
                case "$group_gid" in *[!0-9]*) group_gid=0;; esac
                { [ "$group_gid" -ge 400 ] && [ "$group_gid" -le 499 ]; } || fail SERVICE_ACCOUNT_INVALID "Existing group $service_group must have a GID in 400-499."
                service_id=$group_gid
                members=$(dscl . -read "/Groups/$service_group" GroupMembership 2>/dev/null | sed 's/^GroupMembership:[[:space:]]*//' | tr -d '\n')
                [ -z "$members" ] && [ "$(count_id /Users UniqueID "$service_id")" -eq 0 ] \
                    || fail SERVICE_ACCOUNT_INVALID "Existing group $service_group must have no members and its ID must not be used by another user."
            else
                service_id=400
                while printf '%s\n' "$used" | grep -qx "$service_id"; do service_id=$((service_id + 1)); done
                [ "$service_id" -lt 500 ] || fail SERVICE_ACCOUNT_INVALID 'No free service UID/GID in 400-499.'
                dscl . -create "/Groups/$service_group" || fail SERVICE_ACCOUNT_INVALID 'Cannot create the service group.'
                dscl . -create "/Groups/$service_group" PrimaryGroupID "$service_id"
            fi
            dscl . -create "/Users/$service_user" || fail SERVICE_ACCOUNT_INVALID 'Cannot create the service account.'
            dscl . -create "/Users/$service_user" UniqueID "$service_id"
            dscl . -create "/Users/$service_user" PrimaryGroupID "$service_id"
            dscl . -create "/Users/$service_user" UserShell /usr/bin/false
            dscl . -create "/Users/$service_user" NFSHomeDirectory "$state"
            dscl . -create "/Users/$service_user" IsHidden 1
        fi
    fi
fi

gen_uuid() {
    if command -v uuidgen >/dev/null 2>&1; then uuidgen | tr 'A-Z' 'a-z'
    elif [ -r /proc/sys/kernel/random/uuid ]; then cat /proc/sys/kernel/random/uuid
    else
        raw=$(openssl rand -hex 16)
        printf '%s-%s-%s-%s-%s\n' "$(printf '%s' "$raw" | cut -c1-8)" "$(printf '%s' "$raw" | cut -c9-12)" "$(printf '%s' "$raw" | cut -c13-16)" "$(printf '%s' "$raw" | cut -c17-20)" "$(printf '%s' "$raw" | cut -c21-32)"
    fi
}
activation_id=$(gen_uuid)
printf '%s\n' "$activation_id" | grep -Eq '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' || fail CONFIGURE_FAILED 'Cannot generate an activation identifier.'

# Render the service definition now; it is written only inside the transaction.
if is_linux; then
    {
        cat <<EOF
[Unit]
Description=H.O.P.E host reporting agent
After=network-online.target
Wants=network-online.target
[Service]
Type=simple
ExecStart="$binary" run
Environment="HOPE_AGENT_DIR=$state"
Environment="HOPE_ACTIVATION_ID=$activation_id"
Restart=on-failure
RestartSec=30
UMask=0077
NoNewPrivileges=true
Nice=10
EOF
        if is_system; then cat <<EOF
User=$service_user
Group=$service_group
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths="$state"
CapabilityBoundingSet=
EOF
        fi
        printf '\n[Install]\nWantedBy=default.target\n'
    } > "$temporary/unit.new"
else
    xml_escape() { printf '%s' "$1" | sed 's/\&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g'; }
    {
        cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>io.hope.agent</string>
<key>ProgramArguments</key><array><string>$(xml_escape "$binary")</string><string>run</string></array>
<key>EnvironmentVariables</key><dict><key>HOPE_AGENT_DIR</key><string>$(xml_escape "$state")</string><key>HOPE_ACTIVATION_ID</key><string>$activation_id</string></dict>
<key>RunAtLoad</key><true/><key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict><key>ThrottleInterval</key><integer>30</integer>
<key>StandardErrorPath</key><string>$(xml_escape "$state")/agent.log</string>
EOF
        if is_system; then printf '<key>UserName</key><string>%s</string>\n<key>GroupName</key><string>%s</string>\n' "$service_user" "$service_group"; fi
        printf '</dict></plist>\n'
    } > "$temporary/unit.new"
fi

# =================================== phase: verify ============================================
phase verify
if [ -f "$state/config.json" ]; then
    HOPE_AGENT_DIR="$state" "$temporary/hope-agent" check-config >/dev/null \
        || fail EXISTING_IDENTITY_MISMATCH "The existing identity in $state cannot be used with $HOPE_SERVER. Uninstall with HOPE_PURGE_IDENTITY=1 or use the original server."
fi

# =================================== transaction ==============================================
mkdir_public "$(dirname "$binary")"
mkdir_public "$(dirname "$unit")"
mkdir_public "$(dirname "$state")"
mkdir -p "$state"
chmod 700 "$state"
mkdir_public "$(dirname "$txn")"
rm -rf "$txn.tmp"
mkdir "$txn.tmp"
was_active=0
svc_active && was_active=1
{
    printf 'mode=%s\nwas_active=%s\n' "$mode" "$was_active"
    if [ -f "$binary" ]; then cp "$binary" "$txn.tmp/binary.prev"; echo had_binary=1; else echo had_binary=0; fi
    if [ -f "$unit" ]; then cp "$unit" "$txn.tmp/unit.prev"; echo had_unit=1; else echo had_unit=0; fi
    if [ -f "$state/config.json" ]; then cp "$state/config.json" "$txn.tmp/config.json.prev"; echo had_config=1; else echo had_config=0; fi
    echo step=begin
} > "$txn.tmp/journal"
txn_active=1
mv "$txn.tmp" "$txn"

phase stop
journal step=stop
svc_stop

phase place-binary
journal step=place-binary
cp "$temporary/hope-agent" "$binary.new"
chmod 755 "$binary.new"
mv -f "$binary.new" "$binary"
"$binary" --version >/dev/null || fail BINARY_INVALID 'The installed agent does not run.'

phase configure
state_safe || fail PATH_UNWRITABLE "$state or its config.json became a symbolic link; refusing to continue."
journal step=configure
journal configure=started
HOPE_AGENT_DIR="$state" "$binary" configure \
    || fail CONFIGURE_FAILED 'Enrollment or configuration failed. The token may have expired or been used already; create a new one in H.O.P.E. Identity data, if any, was retained.'
unset HOPE_ENROLLMENT_TOKEN
chown_state

phase write-unit
journal step=write-unit
cp "$temporary/unit.new" "$unit.new"
chmod 644 "$unit.new"
mv -f "$unit.new" "$unit"
svc_reload || fail SERVICE_START_FAILED 'The service manager rejected the new service definition.'

phase activate
journal step=activate
svc_enable_now || fail SERVICE_START_FAILED 'The service could not be started.'
svc_loaded || fail SERVICE_START_FAILED 'The service is not running after activation.'

phase wait-report
journal step=wait-report
deadline=$(( $(date +%s) + ${HOPE_FIRST_REPORT_TIMEOUT:-120} ))
interval=${HOPE_POLL_INTERVAL:-2}
confirmed=0
while :; do
    pid=$(svc_pid || true)
    case "$pid" in ''|*[!0-9]*) pid=0;; esac
    if [ "$pid" -gt 0 ] && HOPE_AGENT_DIR="$state" "$binary" status --activation-id "$activation_id" --service-pid "$pid" --json >/dev/null 2>&1; then confirmed=1; break; fi
    [ "$(date +%s)" -lt "$deadline" ] || break
    sleep "$interval"
done
[ "$confirmed" = 1 ] || fail FIRST_REPORT_TIMEOUT "The installed service did not send an accepted report within ${HOPE_FIRST_REPORT_TIMEOUT:-120}s. Check connectivity to $HOPE_SERVER and the service logs."

# Success: discard the previous binary/unit/config backup.
txn_active=0
rm -rf "$txn"
phase complete
if ! is_system; then
    if is_linux; then echo 'To report after logout, ask an administrator to enable lingering for your account (loginctl enable-linger).'
    else echo 'This LaunchAgent reports only while you are logged in to a graphical session.'; fi
fi
echo 'H.O.P.E agent installed. The running service sent an accepted report.'
