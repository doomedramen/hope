#!/bin/sh
# H.O.P.E agent installer. Served over trusted HTTPS with the release verification key embedded.
set -eu
umask 077
operation=${1:-install}
case "$operation" in install|upgrade|uninstall) ;; *) echo 'Usage: install-agent.sh [install|upgrade|uninstall]' >&2; exit 2;; esac
platform=$(uname -s)
case "$platform" in Linux) platform=linux;; Darwin) platform=macos;; *) echo 'H.O.P.E supports Linux and macOS.' >&2; exit 1;; esac
case "$(uname -m)" in x86_64|amd64) architecture=x86_64;; arm64|aarch64) architecture=aarch64;; *) echo 'Unsupported CPU architecture.' >&2; exit 1;; esac

if [ "$(id -u)" -ne 0 ] && [ "${HOPE_INSTALL_MODE:-auto}" != user ] && command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
    echo 'Using available non-interactive sudo for a system service.'
    exec sudo -n --preserve-env=HOPE_SERVER,HOPE_ENROLLMENT_TOKEN,HOPE_CA_CERT,HOPE_INSTALL_MODE,HOPE_AGENT_DIR,HOPE_RESET_CREDENTIAL sh "$0" "$operation"
fi
system=false
if [ "$(id -u)" -eq 0 ]; then system=true; fi
if "$system"; then
    binary=/usr/local/bin/hope-agent
    state=${HOPE_AGENT_DIR:-/var/lib/hope-agent}
else
    binary="$HOME/.local/bin/hope-agent"
    state=${HOPE_AGENT_DIR:-$HOME/.local/state/hope-agent}
    echo 'Installing a user service. Reporting may stop after logout or before your next login.'
fi
case "$state$binary" in *'%'*|*'"'*|*'\'*|*'
'*) echo 'Installation paths cannot contain percent signs, quotes, backslashes, or newlines.' >&2; exit 1;; esac
export HOPE_AGENT_DIR="$state"

if [ "$platform" = linux ]; then
    command -v systemctl >/dev/null 2>&1 || { echo 'systemd is required; no service was installed.' >&2; exit 1; }
    if "$system"; then unit=/etc/systemd/system/hope-agent.service; else
        unit="$HOME/.config/systemd/user/hope-agent.service"
        systemctl --user show-environment >/dev/null 2>&1 || { echo 'No systemd user manager. Log in normally or run with administrator access.' >&2; exit 1; }
    fi
else
    command -v launchctl >/dev/null 2>&1 || { echo 'launchd is required.' >&2; exit 1; }
    if "$system"; then unit=/Library/LaunchDaemons/io.hope.agent.plist; domain=system; else
        unit="$HOME/Library/LaunchAgents/io.hope.agent.plist"; domain="gui/$(id -u)"
        launchctl print "$domain" >/dev/null 2>&1 || { echo 'No graphical user session. Log in or use administrator access for a system daemon.' >&2; exit 1; }
    fi
fi
stop_service() {
    if [ "$platform" = linux ]; then
        if "$system"; then systemctl stop hope-agent 2>/dev/null || true; else systemctl --user stop hope-agent 2>/dev/null || true; fi
    else launchctl bootout "$domain/io.hope.agent" 2>/dev/null || true; fi
}
if [ "$operation" = uninstall ]; then
    stop_service
    if [ "$platform" = linux ]; then
        if "$system"; then systemctl disable hope-agent 2>/dev/null || true; else systemctl --user disable hope-agent 2>/dev/null || true; fi
    fi
    rm -f "$unit" "$binary"
    if [ "$platform" = linux ]; then if "$system"; then systemctl daemon-reload; else systemctl --user daemon-reload; fi; fi
    echo "Service removed. Identity retained at $state. Revoke its credential in H.O.P.E before deleting that directory."
    exit 0
fi

: "${HOPE_SERVER:?Set HOPE_SERVER to your H.O.P.E HTTPS URL}"
HOPE_SERVER=${HOPE_SERVER%/}
case "$HOPE_SERVER" in https://*) ;; *) echo 'The installer requires HTTPS.' >&2; exit 1;; esac
command -v curl >/dev/null 2>&1 || { echo 'Install curl first.' >&2; exit 1; }
command -v openssl >/dev/null 2>&1 || { echo 'Install OpenSSL first to verify the signed release.' >&2; exit 1; }
temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT HUP INT TERM
fetch() {
    if [ -n "${HOPE_CA_CERT:-}" ]; then
        curl --fail --silent --show-error --location --max-time 120 --proto '=https' --proto-redir '=https' --cacert "$HOPE_CA_CERT" "$1" -o "$2"
    else curl --fail --silent --show-error --location --max-time 120 --proto '=https' --proto-redir '=https' "$1" -o "$2"; fi
}
cat > "$temporary/release-public.pem" <<'HOPE_PUBLIC_KEY'
@@HOPE_PUBLIC_KEY@@
HOPE_PUBLIC_KEY
artifact="hope-agent-$platform-$architecture"
echo "Downloading signed $platform/$architecture agent."
fetch "$HOPE_SERVER/agent/releases/$artifact" "$temporary/hope-agent"
fetch "$HOPE_SERVER/agent/releases/$artifact.sig" "$temporary/hope-agent.sig"
openssl dgst -sha256 -verify "$temporary/release-public.pem" -signature "$temporary/hope-agent.sig" "$temporary/hope-agent" >/dev/null 2>&1 || { echo 'Release signature verification failed. Nothing was installed.' >&2; exit 1; }
chmod 755 "$temporary/hope-agent"
"$temporary/hope-agent" --version

service_user=
if "$system"; then
    if [ "$platform" = linux ]; then
        service_user=hope-agent
        id "$service_user" >/dev/null 2>&1 || useradd --system --home-dir "$state" --shell /usr/sbin/nologin "$service_user"
    else
        service_user=_hopeagent
        if ! id "$service_user" >/dev/null 2>&1; then
            used=$(dscl . -list /Users UniqueID | awk '{print $2}')
            service_uid=400
            while echo "$used" | grep -qx "$service_uid"; do service_uid=$((service_uid + 1)); done
            [ "$service_uid" -lt 500 ] || { echo 'No free service UID in 400–499.' >&2; exit 1; }
            dscl . -create /Users/_hopeagent
            dscl . -create /Users/_hopeagent UniqueID "$service_uid"
            dscl . -create /Users/_hopeagent PrimaryGroupID 20
            dscl . -create /Users/_hopeagent UserShell /usr/bin/false
            dscl . -create /Users/_hopeagent NFSHomeDirectory "$state"
            dscl . -create /Users/_hopeagent IsHidden 1
        fi
    fi
fi
mkdir -p "$(dirname "$binary")" "$state" "$(dirname "$unit")"
chmod 700 "$state"
stop_service
if [ -f "$binary" ]; then cp "$binary" "$temporary/previous-agent"; fi
# Replace on the same filesystem, preserving identity and allowing a retry after interruptions.
cp "$temporary/hope-agent" "$binary.new"
chmod 755 "$binary.new"
mv "$binary.new" "$binary"
if ! "$binary" enroll; then
    if [ -f "$temporary/previous-agent" ]; then cp "$temporary/previous-agent" "$binary"; fi
    echo 'Enrollment or first report failed. Identity was retained; fix connectivity and rerun the installer.' >&2
    exit 1
fi
if "$system"; then chown -R "$service_user" "$state"; fi
unset HOPE_ENROLLMENT_TOKEN
if [ "$platform" = linux ]; then
    cat > "$unit" <<EOF
[Unit]
Description=H.O.P.E host reporting agent
After=network-online.target
Wants=network-online.target
[Service]
Type=simple
ExecStart="$binary" run
Environment="HOPE_AGENT_DIR=$state"
Restart=on-failure
RestartSec=30
UMask=0077
NoNewPrivileges=true
Nice=10
EOF
    if "$system"; then cat >> "$unit" <<EOF
User=$service_user
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths="$state"
CapabilityBoundingSet=
EOF
    fi
    printf '\n[Install]\nWantedBy=default.target\n' >> "$unit"
    if "$system"; then systemctl daemon-reload; systemctl enable --now hope-agent; systemctl is-active --quiet hope-agent; else
        systemctl --user daemon-reload; systemctl --user enable --now hope-agent; systemctl --user is-active --quiet hope-agent
        echo 'To report after logout, ask an administrator to enable lingering for your account.'
    fi
else
    xml_escape() { printf '%s' "$1" | sed 's/\&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g'; }
    cat > "$unit" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>io.hope.agent</string>
<key>ProgramArguments</key><array><string>$(xml_escape "$binary")</string><string>run</string></array>
<key>EnvironmentVariables</key><dict><key>HOPE_AGENT_DIR</key><string>$(xml_escape "$state")</string></dict>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>30</integer>
<key>StandardErrorPath</key><string>$(xml_escape "$state")/agent.log</string>
EOF
    if "$system"; then printf '<key>UserName</key><string>%s</string>\n' "$service_user" >> "$unit"; fi
    printf '</dict></plist>\n' >> "$unit"
    chmod 644 "$unit"
    launchctl bootstrap "$domain" "$unit"
    launchctl print "$domain/io.hope.agent" >/dev/null
fi
echo 'H.O.P.E agent installed. Enrollment and first report confirmed.'
