#!/bin/sh
# H.O.P.E agent starter, served at /install with this server's address (and custom CA path) filled in:
#
#   curl -fsS https://hope.example.com/install | sh -s -- TOKEN       install with an enrollment token
#   curl -fsS https://hope.example.com/install | sh -s -- upgrade     update an installed agent
#   curl -fsS https://hope.example.com/install | sh -s -- uninstall   remove the agent (identity kept)
#
# It downloads the full installer (scripts/install-agent.sh) to a private temporary file and runs it from
# there: the installer keeps a copy of itself for the updater and re-runs itself under sudo, so it cannot
# run from a pipe. Every download stays on HTTPS and every binary is still verified by the installer.
# Everything happens inside main, called on the last line, so a download cut off part-way runs nothing.
# Other installer settings pass through the environment, e.g. ... | HOPE_INSTALL_MODE=user sh -s -- TOKEN
main() {
    set -eu
    umask 077
    server=@@HOPE_SERVER@@
    ca=@@HOPE_AGENT_CA@@
    operation=install
    case "${1:-}" in
        upgrade|uninstall) operation=$1 ;;
        '')
            [ -n "${HOPE_ENROLLMENT_TOKEN:-}" ] || {
                echo 'Usage: curl -fsS <server>/install | sh -s -- <enrollment token> | upgrade | uninstall' >&2
                exit 2
            } ;;
        -*) echo "hope-install: unknown option $1" >&2; exit 2 ;;
        *) HOPE_ENROLLMENT_TOKEN=$1; export HOPE_ENROLLMENT_TOKEN ;;
    esac
    path=/agent/install
    [ "$operation" != uninstall ] || path=/agent/uninstall
    installer=$(mktemp) || exit 1
    trap 'rm -f "$installer"' EXIT
    if [ -n "$ca" ]; then
        curl -q --fail --silent --show-error --location --max-time 30 --proto '=https' --proto-redir '=https' \
            --cacert "$ca" "$server$path" -o "$installer"
        HOPE_CA_CERT=$ca
        export HOPE_CA_CERT
    else
        curl -q --fail --silent --show-error --location --max-time 30 --proto '=https' --proto-redir '=https' \
            "$server$path" -o "$installer"
    fi
    HOPE_SERVER=$server
    export HOPE_SERVER
    if [ "$operation" = upgrade ]; then
        HOPE_ONLY_NEWER=1
        export HOPE_ONLY_NEWER
    fi
    # Not exec: the trap removes the temporary installer once it has finished.
    sh "$installer" "$operation" < /dev/null
}
main "$@"
