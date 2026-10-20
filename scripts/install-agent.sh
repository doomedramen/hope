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
#   HOPE_EXPECT_AGENT_ID                (SSH uninstall: refuse to remove a different enrolled identity)
#   HOPE_FIRST_REPORT_TIMEOUT=120       (seconds to wait for the service's first accepted report)
#   HOPE_POLL_INTERVAL=2                (seconds between acceptance checks; test knob)
#   HOPE_ALLOW_DOWNGRADE=1              (permit installing a release older than the installed agent)
#   HOPE_REQUIRE_PERSISTENT_SERVICE=1   (fail LINGER_REQUIRED instead of installing a Linux user service that stops at logout; set by SSH jobs)
#   HOPE_ONLY_NEWER=1                   (change nothing unless the server offers a newer agent than the installed one)
#   HOPE_UPDATER=1                      (set by hope-agent-update: leave the installed updater as it is)
#   HOPE_INSTALL_ROOT                   (TEST ONLY: prefix for absolute system paths)
#
# Output: progress lines "hope-install: phase=NAME" and failures "hope-install: error=CODE message"
# on stderr, and on success "hope-install: result=updated VERSION" (or "result=current VERSION" when
# HOPE_ONLY_NEWER found nothing newer). Enrollment tokens and credentials are never printed.
#
# Updater (system services only): after a successful install the installer keeps a root-owned copy
# of itself, whose embedded key is the one this host now trusts, and installs hope-agent-update, which
# runs that copy in upgrade mode. The agent, which cannot write its own binary, asks for an update by
# creating update-request in its state directory when the server says one is wanted; a systemd path
# unit (launchd WatchPaths on macOS) then starts hope-agent-update as root. A later release is
# therefore checked against the key pinned here, never against one the server sends at update time.
# An administrator can also run it by hand: sudo hope-agent-update
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
proxmox_enabled=0
temporary=
txn=
txn_active=0
rb_ok=1
no_manager=0
preserve=HOPE_SERVER,HOPE_ENROLLMENT_TOKEN,HOPE_CA_CERT,HOPE_INSTALL_MODE,HOPE_AGENT_DIR,HOPE_RESET_CREDENTIAL,HOPE_PURGE_IDENTITY,HOPE_FIRST_REPORT_TIMEOUT,HOPE_POLL_INTERVAL,HOPE_INSTALL_ROOT,HOPE_REQUIRE_PERSISTENT_SERVICE,HOPE_ONLY_NEWER,HOPE_UPDATER,HOPE_EXPECT_AGENT_ID
root=${HOPE_INSTALL_ROOT:-}
root=${root%/}

# Keep a private copy of stderr so progress/errors survive redirections active when a signal trap runs.
exec 3>&2
# Progress goes through cat so that a dropped reader (a closed SSH session or pipe) fails this pipeline, which
# set -e turns into a normal rollback exit. Writing with the shell's own printf would instead kill the shell with
# SIGPIPE before its EXIT trap runs, stranding a half-finished transaction (and, on bash 3.2, leave the unflushed
# line in the buffer of the next command substitution).
emit() { printf '%s\n' "$1" | cat >&3; }
phase() { emit "hope-install: phase=$1"; }
note() { emit "hope-install: $*"; }
err() { emit "hope-install: error=$1 $2"; }
fail() { err "$1" "$2"; exit 1; }

journal() { { [ -d "$txn" ] && printf '%s\n' "$1" >> "$txn/journal"; } || true; }
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
# The updater lives outside the service-writable state directory, owned by root.
updater_paths() {
    updater_bin=$root/usr/local/sbin/hope-agent-update
    if [ "$platform" = linux ]; then
        updater_dir=$root/var/lib/hope-agent-update
        updater_path_unit=$root/etc/systemd/system/hope-agent-update.path
        updater_service_unit=$root/etc/systemd/system/hope-agent-update.service
    else
        updater_dir="$root/Library/Application Support/hope-agent-update"
        updater_plist=$root/Library/LaunchDaemons/io.hope.agent.update.plist
    fi
}
proxmox_paths() {
    proxmox_service=$root/etc/systemd/system/hope-agent-proxmox-collect.service
    proxmox_timer=$root/etc/systemd/system/hope-agent-proxmox-collect.timer
}
stop_proxmox_collector() {
    if is_system && is_linux; then
        systemctl stop hope-agent-proxmox-collect.timer hope-agent-proxmox-collect.service >/dev/null 2>&1 || true
    fi
}
remove_proxmox_collector() {
    if is_system && is_linux; then
        systemctl disable --now hope-agent-proxmox-collect.timer >/dev/null 2>&1 || true
        systemctl stop hope-agent-proxmox-collect.service >/dev/null 2>&1 || true
        proxmox_paths
        rm -f "$proxmox_service" "$proxmox_timer"
        systemctl daemon-reload >/dev/null 2>&1 || true
    fi
}
install_proxmox_collector() {
    [ "$proxmox_enabled" = 1 ] || { remove_proxmox_collector; return 0; }
    proxmox_paths
    mkdir_public "$(dirname "$proxmox_service")" || return 1
    mkdir_public "$root/etc/hope-agent" || return 1
    chmod 750 "$root/etc/hope-agent" || return 1
    if [ -d /etc/pve/priv ]; then
        mkdir -p /etc/pve/priv/hope-agent /etc/pve/priv/lock || return 1
    fi
    cp "$temporary/proxmox.service.new" "$proxmox_service.new" && chmod 644 "$proxmox_service.new" \
        && mv -f "$proxmox_service.new" "$proxmox_service" || return 1
    cp "$temporary/proxmox.timer.new" "$proxmox_timer.new" && chmod 644 "$proxmox_timer.new" \
        && mv -f "$proxmox_timer.new" "$proxmox_timer" || return 1
    systemctl daemon-reload && systemctl enable --now hope-agent-proxmox-collect.timer
}
mkdir_public() { ( umask 022; mkdir -p "$1" ); }
state_safe() { [ ! -L "$state" ] && [ ! -L "$state/config.json" ]; }
install_lock_held=0
lock_installation() {
    # Share the updater's process-owned lock before recovering or starting a
    # manual transaction. The updater's installer child already inherits fd 9.
    if is_system && is_linux && [ "${HOPE_UPDATER:-}" != 1 ] && [ "$install_lock_held" = 0 ]; then
        updater_paths
        command -v flock >/dev/null 2>&1 || fail LOCK_UNAVAILABLE 'Install flock (util-linux) before installing a system agent.'
        mkdir_public "$updater_dir"
        exec 8>"$updater_dir/update.lock"
        flock -n 8 || fail INSTALL_BUSY 'Another agent installation or update is running. Retry after it finishes.'
        install_lock_held=1
    fi
}

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
    stop_proxmox_collector
    svc_stop
    if [ "$(jget had_binary)" = 1 ]; then
        if [ -f "$txn/binary.prev" ] && cp "$txn/binary.prev" "$binary.new" && chmod 755 "$binary.new" && mv -f "$binary.new" "$binary"; then :; else rb_ok=0; fi
    else rm -f "$binary" "$binary.new"; fi
    if [ "$(jget had_unit)" = 1 ]; then
        if [ -f "$txn/unit.prev" ] && cp "$txn/unit.prev" "$unit.new" && chmod 644 "$unit.new" && mv -f "$unit.new" "$unit"; then :; else rb_ok=0; fi
    else svc_disable; rm -f "$unit" "$unit.new"; fi
    proxmox_paths
    for component in service timer; do
        if [ "$(jget had_proxmox_$component)" = 1 ]; then
            previous="$txn/proxmox.$component.prev"
            destination=$proxmox_service
            [ "$component" = timer ] && destination=$proxmox_timer
            if [ -f "$previous" ] && cp "$previous" "$destination.new" && chmod 644 "$destination.new" && mv -f "$destination.new" "$destination"; then :; else rb_ok=0; fi
        else
            destination=$proxmox_service
            [ "$component" = timer ] && destination=$proxmox_timer
            rm -f "$destination" "$destination.new"
        fi
    done
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
    if is_system && is_linux; then
        systemctl daemon-reload >/dev/null 2>&1 || rb_ok=0
        if [ "$(jget proxmox_was_active)" = 1 ] && [ "$rb_ok" = 1 ]; then
            systemctl enable --now hope-agent-proxmox-collect.timer >/dev/null 2>&1 || rb_ok=0
        fi
    fi
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

# --- updater (system services only) --------------------------------------------------------
# Each step returns 1 on failure: the callers run these as `f || note warning`, where set -e is off.
install_updater() {
    updater_paths
    if is_linux && ! command -v flock >/dev/null 2>&1; then
        note 'warning=UPDATER_NOT_INSTALLED Install flock (util-linux) to enable safe updater locking.'
        return 1
    fi
    [ ! -L "$state/update-queue" ] || return 1
    mkdir -p "$state/update-queue" && chmod 700 "$state/update-queue" \
        && chown "$service_user:$service_group" "$state/update-queue" || return 1
    [ -f "$0" ] || { note 'warning=UPDATER_NOT_INSTALLED The installer was not run from a file, so it cannot be kept for updates.'; return 1; }
    pinned_ca=
    if [ -n "${HOPE_CA_CERT:-}" ]; then
        case "$HOPE_CA_CERT" in
            /*) pinned_ca=$HOPE_CA_CERT;;
            *) pinned_ca=$(cd "$(dirname "$HOPE_CA_CERT")" 2>/dev/null && pwd)/$(basename "$HOPE_CA_CERT") || return 1;;
        esac
    fi
    mkdir_public "$(dirname "$updater_bin")" && mkdir_public "$updater_dir" || return 1
    # The installer text carries the verification key this host now trusts. It is replaced only by
    # an administrator running a full installation again, never by an update.
    cp "$0" "$updater_dir/install-agent.sh.new" && chmod 755 "$updater_dir/install-agent.sh.new" \
        && mv -f "$updater_dir/install-agent.sh.new" "$updater_dir/install-agent.sh" || return 1
    printf '%s\n' "$HOPE_SERVER" > "$updater_dir/server.new" && mv -f "$updater_dir/server.new" "$updater_dir/server" || return 1
    if [ -n "$pinned_ca" ]; then
        printf '%s\n' "$pinned_ca" > "$updater_dir/ca.new" && mv -f "$updater_dir/ca.new" "$updater_dir/ca" || return 1
    else rm -f "$updater_dir/ca"; fi
    {
        printf '#!/bin/sh\n# H.O.P.E agent updater, written by install-agent.sh and owned by root.\n'
        printf "dir='%s'\nstate='%s'\nroot='%s'\nplatform='%s'\n" "$updater_dir" "$state" "$root" "$platform"
        cat <<'UPDATER_SCRIPT_END'
# Runs the installer kept in $dir in upgrade mode when the server offers a newer agent. Started as
# root by the service manager when the agent leaves a request, or by an administrator: sudo hope-agent-update
set -eu
umask 077
PATH=$PATH:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export PATH
queue=$state/update-queue
# Linux uses a process-owned lock: SIGKILL cannot leave it held. Keep the
# legacy directory interlock during migration, and retain requests when busy.
lock=$dir/lock
if [ "$platform" = linux ]; then
    exec 9>"$dir/update.lock"
    flock -n 9 || { echo 'hope-agent-update: another update is already running' >&2; exit 75; }
    if [ -d "$lock" ]; then
        find "$lock" -maxdepth 0 -mmin +30 -exec rm -rf {} + 2>/dev/null || true
        [ ! -d "$lock" ] || { echo 'hope-agent-update: another update is already running' >&2; exit 75; }
    fi
    lock=
else
    if ! mkdir "$lock" 2>/dev/null; then
        find "$lock" -maxdepth 0 -mmin +30 -exec rm -rf {} + 2>/dev/null || true
        mkdir "$lock" 2>/dev/null || { echo 'hope-agent-update: another update is already running' >&2; exit 75; }
    fi
fi
log=$(mktemp)
trap 'if [ -n "$lock" ]; then rm -rf "$lock"; fi; rm -f "$log"' EXIT
# The request only wakes this script; nothing in it is read or followed.
if [ -d "$queue" ] && [ ! -L "$queue" ] && [ ! -L "$state" ]; then rm -f "$queue/request"; fi
HOPE_SERVER=$(cat "$dir/server")
HOPE_AGENT_DIR=$state
HOPE_INSTALL_MODE=system
HOPE_ONLY_NEWER=1
HOPE_UPDATER=1
export HOPE_SERVER HOPE_AGENT_DIR HOPE_INSTALL_MODE HOPE_ONLY_NEWER HOPE_UPDATER
if [ -f "$dir/ca" ]; then HOPE_CA_CERT=$(cat "$dir/ca"); export HOPE_CA_CERT; fi
if [ -n "$root" ]; then HOPE_INSTALL_ROOT=$root; export HOPE_INSTALL_ROOT; fi
rc=0
outcome=
code=
# The agent never creates symbolic links, and this runs as root over files it can write: a link
# anywhere near its state is tampering, so stop before the installer or the agent binary open it.
if [ -L "$state" ] || { [ -d "$state" ] && [ -n "$(find "$state" -maxdepth 2 -type l 2>/dev/null | head -n 1)" ]; }; then
    echo 'hope-agent-update: refusing to run: a symbolic link was found in the agent state directory' >&2
    rc=1
    code=STATE_UNSAFE
else
    # Progress goes to the journal as it happens (an update can wait two minutes for the new agent's
    # first report); the same text is kept to find the outcome in.
    rcfile=$(mktemp)
    { code=0; sh "$dir/install-agent.sh" upgrade 2>&1 || code=$?; echo "$code" > "$rcfile"; } | tee "$log" >&2
    rc=$(cat "$rcfile" 2>/dev/null || echo 1)
    rm -f "$rcfile"
    outcome=$(sed -En 's/^hope-install: result=(updated|current) ([A-Za-z0-9._+-]{1,64})$/\1 \2/p' "$log" | tail -n 1)
    code=$(sed -n 's/^hope-install: error=\([A-Z_]\{1,40\}\) .*/\1/p' "$log" | head -n 1)
fi
if [ "$rc" -eq 0 ] && [ -n "$outcome" ]; then
    status="status=${outcome%% *}
version=${outcome#* }"
else
    status="status=failed
code=${code:-UNKNOWN}"
fi
# Tell the agent what happened; it reports this to the server. A new file in a private name, so a
# link left by the agent cannot redirect the write.
if [ -d "$state" ] && [ ! -L "$state" ]; then
    out=$(mktemp "$state/.update-result.XXXXXX") && {
        printf '%s\nat=%s\n' "$status" "$(date +%s)" > "$out"
        chmod 644 "$out"
        mv -f "$out" "$state/update-result"
    } || true
fi
exit_rc=$rc
# Retry only downloads, which can fail transiently. Signature, state and installer failures need an
# operator or a new server request; restarting those automatically can delay a later valid update.
if [ "$rc" -ne 0 ] && [ "$code" = DOWNLOAD_FAILED ]; then exit_rc=75; fi
exit "$exit_rc"
UPDATER_SCRIPT_END
    } > "$updater_bin.new" && chmod 755 "$updater_bin.new" && mv -f "$updater_bin.new" "$updater_bin" || return 1
    if is_linux; then
        cat > "$updater_path_unit.new" <<EOF
[Unit]
Description=H.O.P.E agent update requests
[Path]
PathExists=$state/update-queue/request
PathExists=$txn
Unit=hope-agent-update.service
[Install]
WantedBy=multi-user.target
EOF
        cat > "$updater_service_unit.new" <<EOF
[Unit]
Description=H.O.P.E agent update
StartLimitIntervalSec=1h
StartLimitBurst=12
[Service]
Type=oneshot
ExecStart="$updater_bin"
TimeoutStartSec=1800
Restart=on-failure
RestartSec=5min
RestartPreventExitStatus=1
EOF
        mv -f "$updater_path_unit.new" "$updater_path_unit" && mv -f "$updater_service_unit.new" "$updater_service_unit" || return 1
        [ "$no_manager" = 0 ] || return 0
        sc daemon-reload && sc enable --now hope-agent-update.path || return 1
    else
        xml_text() { printf '%s' "$1" | sed 's/\&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g'; }
        cat > "$updater_plist.new" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>io.hope.agent.update</string>
<key>ProgramArguments</key><array><string>$(xml_text "$updater_bin")</string></array>
<key>QueueDirectories</key><array><string>$(xml_text "$state/update-queue")</string></array>
</dict></plist>
EOF
        mv -f "$updater_plist.new" "$updater_plist" || return 1
        launchctl bootout system/io.hope.agent.update >/dev/null 2>&1 || true
        launchctl bootstrap system "$updater_plist" || return 1
    fi
}
remove_updater() {
    updater_paths
    if [ "$no_manager" = 0 ]; then
        if is_linux; then sc disable --now hope-agent-update.path >/dev/null 2>&1 || true
        else launchctl bootout system/io.hope.agent.update >/dev/null 2>&1 || true; fi
    fi
    if is_linux; then rm -f "$updater_path_unit" "$updater_service_unit"; else rm -f "$updater_plist"; fi
    rm -f "$updater_bin" "$updater_bin.new"
    rm -rf "${updater_dir:?}"
    if is_linux; then svc_reload || true; fi
}

# =================================== phase: preflight =========================================
phase preflight
case "$operation" in install|upgrade|uninstall) ;; *) err USAGE 'Usage: install-agent.sh [install|upgrade|uninstall]'; exit 2;; esac
case "$(uname -s)" in
    Linux) platform=linux ;;
    Darwin)
        [ "$operation" = uninstall ] || fail UNSUPPORTED_PLATFORM 'H.O.P.E agents currently support Linux x86_64 only.'
        platform=macos
        ;;
    *) fail UNSUPPORTED_PLATFORM 'H.O.P.E agents currently support Linux x86_64 only.' ;;
esac
case "$(uname -m)" in
    x86_64|amd64) architecture=x86_64 ;;
    arm64|aarch64)
        [ "$operation" = uninstall ] || fail UNSUPPORTED_ARCH 'H.O.P.E agents currently support Linux x86_64 only.'
        architecture=aarch64
        ;;
    *) fail UNSUPPORTED_ARCH 'H.O.P.E agents currently support Linux x86_64 only.' ;;
esac
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
    if is_linux && [ "$operation" != uninstall ]; then
        linger_user=$(id -un 2>/dev/null || true)
        linger=$(loginctl show-user "$linger_user" -p Linger --value 2>/dev/null || true)
        if [ "$linger" != yes ] && [ ! -e "$root/var/lib/systemd/linger/$linger_user" ]; then
            if [ "${HOPE_REQUIRE_PERSISTENT_SERVICE:-}" = 1 ]; then
                fail LINGER_REQUIRED "A user service stops soon after the last session of $linger_user ends because lingering is off, so nothing was installed. Run 'sudo loginctl enable-linger $linger_user' (or install with administrator access) and retry."
            fi
            note 'notice=USER_SERVICE_NOT_PERSISTENT'
            emit "Warning: lingering is off for $linger_user, so the user service will stop when the last session ends. An administrator can run 'sudo loginctl enable-linger $linger_user'."
        fi
    fi
fi
HOPE_INSTALL_PID=$$
export HOPE_INSTALL_PID

paths_for "$mode"
state=${HOPE_AGENT_DIR:-$state_default}
case "$state" in /*) ;; *) fail PATH_UNWRITABLE 'HOPE_AGENT_DIR must be an absolute path.';; esac
# shellcheck disable=SC1003 # the backslash pattern is intentional
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
        fail NO_SERVICE_MANAGER "$manager_error Cannot confirm the service is stopped; installation files were kept."
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

if [ "$operation" = uninstall ]; then
    lock_installation
    if [ -n "${HOPE_EXPECT_AGENT_ID:-}" ]; then
        # Config is written by serde_json, including on older agents. Read only the canonical
        # UUID field, without executing the installed binary or requiring Python/jq on the host.
        case "$HOPE_EXPECT_AGENT_ID" in *[!0-9a-fA-F-]*) fail INVALID_SETTING 'Invalid expected agent identity.';; esac
        [ "${#HOPE_EXPECT_AGENT_ID}" = 36 ] || fail INVALID_SETTING 'Invalid expected agent identity.'
        [ -f "$state/config.json" ] || fail EXISTING_IDENTITY_MISMATCH 'No enrolled identity was found for the selected agent.'
        [ "$(wc -c < "$state/config.json")" -le 131072 ] || fail EXISTING_IDENTITY_MISMATCH 'Agent configuration exceeds the supported size.'
        actual_agent=$(tr -d '\r\n' < "$state/config.json" | sed -n 's/.*"agent_id"[[:space:]]*:[[:space:]]*"\([0-9a-fA-F-]*\)".*/\1/p')
        [ "$actual_agent" = "$HOPE_EXPECT_AGENT_ID" ] || fail EXISTING_IDENTITY_MISMATCH 'The installed agent does not match the selected host. Nothing was removed.'
    fi
fi

# Converge after an interrupted run: restore the previous installation first.
if [ -d "$txn" ]; then
    lock_installation
    phase recover
    if [ -f "$txn/journal" ]; then
        rollback
        [ "$rb_ok" = 1 ] || fail ROLLBACK_INCOMPLETE 'An interrupted installation could not be restored automatically.'
        note 'notice=An interrupted earlier run was rolled back; continuing.'
    else rm -rf "$txn"; fi
fi
rm -rf "$txn.tmp"

if [ "$operation" = uninstall ]; then
    lock_installation
    phase uninstall
    svc_stop
    if svc_loaded; then
        fail UNINSTALL_STOP_FAILED 'The service is still loaded or running. Installation files were kept; stop it before retrying.'
    fi
    svc_disable
    remove_proxmox_collector
    rm -f "$unit" "$binary" "$binary.new"
    svc_reload || true
    if [ "${HOPE_PURGE_IDENTITY:-}" = 1 ]; then
        state_safe || fail PATH_UNWRITABLE 'Refusing to purge through a symbolic link.'
        # Remove only files the agent creates; never the whole (possibly shared) directory.
        for name in config.json spool.json child-spool.json child-sequence.json last-acceptance.json previous.json sample.json last-report agent.lock agent.log bootstrap-ca.pem update-queue update-result update-asked .install-transaction; do
            rm -rf "${state:?}/$name"
        done
        rm -f "$state"/.hope-*.tmp "$state"/.update-result.*
        rmdir "$state" 2>/dev/null || note "warning=$state contains other files and was kept"
        echo "Service removed and identity deleted from $state. Revoke its credential in H.O.P.E."
    else
        echo "Service removed. Identity retained at $state. Revoke its credential in H.O.P.E before deleting that directory, or set HOPE_PURGE_IDENTITY=1."
    fi
    # This removes the updater lock's directory too. Keep it until all agent-state changes
    # finish, so another installer cannot create a new lock inode during a purge.
    if is_system; then remove_updater; fi
    if is_system && is_linux; then
        rm -f "$root/run/hope-agent/proxmox.json" "$root"/run/hope-agent/proxmox.json.tmp-*
        rmdir "$root/run/hope-agent" 2>/dev/null || true
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
# shellcheck disable=SC2015 # fail exits, so A && B || fail is intended
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
# The server binds a freshly fetched installer to its release. The root-owned saved updater
# retains this verification key but selects the currently published release once per upgrade.
release_id='@@HOPE_RELEASE_ID@@'
if [ "$operation" = upgrade ]; then
    release_id=$(fetch --max-filesize 128 "$HOPE_SERVER/agent/releases/current-id") \
        || fail RELEASE_UNAVAILABLE 'No immutable agent release could be selected. The running agent was kept.'
fi
# Reject slashes, whitespace, shell syntax and unresolved template markers before constructing URLs.
case "$release_id" in
    ''|*[!0-9A-Za-z.-]*) fail RELEASE_UNAVAILABLE 'The server did not select a valid immutable release.' ;;
esac
printf '%s\n' "$release_id" | grep -Eq '^[0-9][0-9A-Za-z.-]{0,63}-[0-9a-f]{12}$' \
    || fail RELEASE_UNAVAILABLE 'The server did not select a valid immutable release.'
release_url=$HOPE_SERVER/agent/releases/by-id/$release_id
artifact="hope-agent-$platform-$architecture"
# The availability API needs a login, so confirm the public files exist before stopping anything.
for name in manifest.json manifest.json.sig "$artifact" "$artifact.sig"; do
    code=$(fetch --head -o /dev/null --write-out '%{http_code}' "$release_url/$name" 2>/dev/null || true)
    case "$code" in
        2??) ;;
        404)
            case "$name" in
                manifest.json*) fail RELEASE_UNAVAILABLE "This server publishes no signed release manifest ($name). An administrator must publish a release with scripts/assemble-release.sh." ;;
                *) fail RELEASE_UNAVAILABLE "No signed agent is packaged for $platform/$architecture on this server." ;;
            esac ;;
        *) fail DOWNLOAD_FAILED "Cannot check $name on $HOPE_SERVER (HTTP status ${code:-none}). Check the URL, network, and HOPE_CA_CERT." ;;
    esac
done

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d ' ' -f 1
    else openssl dgst -sha256 -r "$1" | cut -d ' ' -f 1; fi
}
# Dotted numeric versions ("v1", "0.1.0", "1.2.3-rc1" compares as 1.2.3). version_lt: true when $1 < $2.
version_ok() { case "$1" in v[0-9]*|[0-9]*) return 0;; esac; return 1; }
version_lt() {
    awk -v a="$1" -v b="$2" 'function norm(s) { sub(/^v/, "", s); sub(/[^0-9.].*$/, "", s); return s }
        BEGIN { n = split(norm(a), x, "."); m = split(norm(b), y, "."); k = (n > m) ? n : m
            for (i = 1; i <= k; i++) { if (x[i] + 0 < y[i] + 0) exit 0; if (x[i] + 0 > y[i] + 0) exit 1 }
            exit 1 }'
}
single() { # single NAME VALUE: exactly one non-empty line, else MANIFEST_INVALID
    { [ -n "$2" ] && [ "$(printf '%s\n' "$2" | wc -l | tr -d ' ')" = 1 ]; } || fail MANIFEST_INVALID "The release manifest has no single valid $1."
}

# =================================== phase: stage =============================================
phase stage
echo "Downloading signed $platform/$architecture agent."
manifest="$temporary/manifest.json"
fetch --max-filesize 65536 -o "$manifest" "$release_url/manifest.json" || fail DOWNLOAD_FAILED 'Downloading the release manifest failed.'
fetch --max-filesize 16384 -o "$manifest.sig" "$release_url/manifest.json.sig" || fail DOWNLOAD_FAILED 'Downloading the release manifest signature failed.'
[ "$(wc -c < "$manifest" | tr -d ' ')" -le 65536 ] || fail MANIFEST_INVALID 'The release manifest is implausibly large.'
openssl dgst -sha256 -verify "$temporary/release-public.pem" -signature "$manifest.sig" "$manifest" >/dev/null 2>&1 \
    || fail MANIFEST_INVALID 'Release manifest signature verification failed. Nothing was installed.'
m_format=$(sed -n 's/^ *"format": *\([0-9][0-9]*\),\{0,1\} *$/\1/p' "$manifest")
m_version=$(sed -n 's/^ *"version": *"\([A-Za-z0-9._+-]\{1,64\}\)",\{0,1\} *$/\1/p' "$manifest")
single 'format' "$m_format"
single 'version' "$m_version"
[ "$m_format" = 1 ] || fail MANIFEST_INVALID "Unsupported release manifest format $m_format."
version_ok "$m_version" || fail MANIFEST_INVALID "Release version '$m_version' is not a dotted number."
entries=$(grep -F "\"platform\":\"$platform\",\"arch\":\"$architecture\"," "$manifest" || true)
[ -n "$entries" ] || fail RELEASE_UNAVAILABLE "The signed manifest (version $m_version) lists no agent for $platform/$architecture."
[ "$(printf '%s\n' "$entries" | wc -l | tr -d ' ')" = 1 ] || fail MANIFEST_INVALID "The manifest lists $platform/$architecture more than once."
m_file=$(printf '%s\n' "$entries" | sed -n 's/.*"file":"\([^"]*\)".*/\1/p')
m_sha=$(printf '%s\n' "$entries" | sed -n 's/.*"sha256":"\([0-9a-f]\{64\}\)".*/\1/p')
m_size=$(printf '%s\n' "$entries" | sed -n 's/.*"size":\([0-9]\{1,12\}\)[,}].*/\1/p')
{ [ "$m_file" = "$artifact" ] && [ -n "$m_sha" ] && [ -n "$m_size" ]; } \
    || fail MANIFEST_INVALID "The manifest entry for $platform/$architecture is malformed."

if [ "${HOPE_ONLY_NEWER:-}" = 1 ]; then
    [ -x "$binary" ] || fail NOT_INSTALLED 'No installed agent was found to update.'
    installed_version=$("$binary" --version 2>/dev/null | sed -n 's/^hope-agent //p' | head -n 1 || true)
    if version_ok "$installed_version" && ! version_lt "$installed_version" "$m_version"; then
        echo "hope-agent $installed_version is current; the server offers $m_version."
        if is_system && [ "${HOPE_UPDATER:-}" != 1 ]; then
            lock_installation
            install_updater || note 'warning=UPDATER_NOT_INSTALLED The current agent was kept, but updater repair failed.'
        fi
        note "result=current $installed_version"
        phase complete
        exit 0
    fi
fi
# Refuse to replace a newer installed agent with an older release. This compares only with the agent
# installed on this host: a fresh install, or a host whose agent is already old, can still receive any
# older release the server's key signed. Rely on the server publishing only the intended release.
if [ -x "$binary" ]; then
    installed_version=$("$binary" --version 2>/dev/null | sed -n 's/^hope-agent //p' | head -n 1 || true)
    if version_ok "$installed_version" && version_lt "$m_version" "$installed_version"; then
        if [ "${HOPE_ALLOW_DOWNGRADE:-}" = 1 ]; then
            note "warning=Downgrading hope-agent $installed_version to $m_version (HOPE_ALLOW_DOWNGRADE=1)."
        else
            fail DOWNGRADE_REFUSED "The server offers hope-agent $m_version but $installed_version is installed. Set HOPE_ALLOW_DOWNGRADE=1 to downgrade deliberately."
        fi
    fi
fi

fetch --max-filesize "$m_size" -o "$temporary/hope-agent" "$release_url/$artifact" || fail DOWNLOAD_FAILED 'Downloading the agent binary failed.'
fetch --max-filesize 16384 -o "$temporary/hope-agent.sig" "$release_url/$artifact.sig" || fail DOWNLOAD_FAILED 'Downloading the agent signature failed.'
[ "$(wc -c < "$temporary/hope-agent" | tr -d ' ')" = "$m_size" ] \
    || fail CHECKSUM_MISMATCH "The downloaded agent size differs from the signed manifest ($m_size bytes). Nothing was installed."
[ "$(sha256_of "$temporary/hope-agent")" = "$m_sha" ] \
    || fail CHECKSUM_MISMATCH 'The downloaded agent checksum differs from the signed manifest. Nothing was installed.'
openssl dgst -sha256 -verify "$temporary/release-public.pem" -signature "$temporary/hope-agent.sig" "$temporary/hope-agent" >/dev/null 2>&1 \
    || fail BAD_SIGNATURE 'Release signature verification failed. Nothing was installed.'
chmod 755 "$temporary/hope-agent"
reported=$("$temporary/hope-agent" --version) || fail BINARY_NOT_EXECUTABLE 'The verified agent cannot run from the temporary directory (noexec mount or wrong platform?). Set TMPDIR to an executable directory.'
printf '%s\n' "$reported"
[ "${reported#hope-agent }" = "$m_version" ] \
    || fail MANIFEST_INVALID "The agent reports version '${reported#hope-agent }' but the signed manifest says $m_version."

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
                { [ -z "$members" ] && [ "$(count_id /Users UniqueID "$service_id")" -eq 0 ]; } \
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
    if command -v uuidgen >/dev/null 2>&1; then uuidgen | tr '[:upper:]' '[:lower:]'
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
    if is_system && command -v pvesh >/dev/null 2>&1; then
        proxmox_enabled=1
        cat > "$temporary/proxmox.service.new" <<EOF
[Unit]
Description=H.O.P.E read-only Proxmox inventory collector
After=local-fs.target
ConditionPathExists=$(command -v pvesh)
[Service]
Type=oneshot
# Leave headroom for bounded pvesh reads and guest enrichment while keeping a hard
# upper bound on this root-owned oneshot.
TimeoutStartSec=45s
User=root
Group=root
Environment="PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
ExecStart="$binary" collect-proxmox
UMask=0027
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
RuntimeDirectory=hope-agent
RuntimeDirectoryMode=0750
RuntimeDirectoryPreserve=yes
ReadWritePaths=/run/hope-agent /etc/hope-agent /etc/pve/priv/hope-agent /etc/pve/priv/lock
EOF
        cat > "$temporary/proxmox.timer.new" <<'EOF'
[Unit]
Description=Refresh H.O.P.E Proxmox inventory
[Timer]
OnBootSec=30s
OnUnitActiveSec=30s
AccuracySec=2s
Persistent=true
Unit=hope-agent-proxmox-collect.service
[Install]
WantedBy=timers.target
EOF
    fi
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
lock_installation
mkdir_public "$(dirname "$binary")"
mkdir_public "$(dirname "$unit")"
mkdir_public "$(dirname "$state")"
mkdir -p "$state"
chmod 700 "$state"
# Where the agent leaves an update request for the updater (system services only).
if is_system; then
    # The service account owns the state directory, so a link may have been left here; replace it
    # rather than change the mode of whatever it points at.
    [ ! -L "$state/update-queue" ] || rm -f "$state/update-queue"
    mkdir -p "$state/update-queue"
    chmod 700 "$state/update-queue"
fi
mkdir_public "$(dirname "$txn")"
rm -rf "$txn.tmp"
mkdir "$txn.tmp"
was_active=0
svc_active && was_active=1
pve_was_active=0
if is_system && is_linux; then
    proxmox_paths
    if [ -f "$proxmox_timer" ]; then
        if systemctl is-active --quiet hope-agent-proxmox-collect.timer; then
            pve_was_active=1
        fi
    fi
fi
{
    printf 'mode=%s\nwas_active=%s\nproxmox_was_active=%s\n' "$mode" "$was_active" "$pve_was_active"
    if [ -f "$binary" ]; then cp "$binary" "$txn.tmp/binary.prev"; echo had_binary=1; else echo had_binary=0; fi
    if [ -f "$unit" ]; then cp "$unit" "$txn.tmp/unit.prev"; echo had_unit=1; else echo had_unit=0; fi
    if is_system && is_linux; then
        if [ -f "$proxmox_service" ]; then cp "$proxmox_service" "$txn.tmp/proxmox.service.prev"; echo had_proxmox_service=1; else echo had_proxmox_service=0; fi
        if [ -f "$proxmox_timer" ]; then cp "$proxmox_timer" "$txn.tmp/proxmox.timer.prev"; echo had_proxmox_timer=1; else echo had_proxmox_timer=0; fi
    fi
    if [ -f "$state/config.json" ]; then cp "$state/config.json" "$txn.tmp/config.json.prev"; echo had_config=1; else echo had_config=0; fi
    echo step=begin
} > "$txn.tmp/journal"
txn_active=1
mv "$txn.tmp" "$txn"

phase stop
journal step=stop
stop_proxmox_collector
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
install_proxmox_collector || fail SERVICE_START_FAILED 'The Proxmox collector service could not be installed safely.'

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
if [ "$operation" = upgrade ]; then note "result=updated $m_version"; else note "result=installed $m_version"; fi
if is_system && [ "${HOPE_UPDATER:-}" != 1 ]; then
    install_updater || note 'warning=UPDATER_NOT_INSTALLED The agent is installed and reporting, but updates from the console need a new installation command.'
fi
if ! is_system; then
    if is_linux; then echo 'To report after logout, ask an administrator to enable lingering for your account (loginctl enable-linger).'
    else echo 'This LaunchAgent reports only while you are logged in to a graphical session.'; fi
fi
echo 'H.O.P.E agent installed. The running service sent an accepted report.'
