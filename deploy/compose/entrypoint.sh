#!/bin/sh
set -eu

default_key_path=/app/data/pki/credential-master-key
key_path="${HOPE_CREDENTIAL_MASTER_KEY_FILE:-$default_key_path}"

# An explicitly supplied inline key remains an escape hatch for deployments
# that manage secrets outside the Compose volume.
if [ -n "${HOPE_CREDENTIAL_MASTER_KEY:-}" ]; then
    unset HOPE_CREDENTIAL_MASTER_KEY_FILE
elif [ "$key_path" = "$default_key_path" ]; then
    mkdir -p "$(dirname "$key_path")"
    if [ ! -e "$key_path" ]; then
        umask 077
        temporary_key="${key_path}.tmp.$$"
        od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"$temporary_key"
        mv "$temporary_key" "$key_path"
        chmod 0400 "$key_path"
    fi
fi

if [ -z "${HOPE_CREDENTIAL_MASTER_KEY:-}" ] && [ ! -f "$key_path" ]; then
    echo "credential master key file not found: $key_path" >&2
    exit 1
fi

if [ "${1:-serve}" = "serve" ]; then
    pki_dir=/app/data/pki
    pki_files="ca-cert.pem ca-key.pem server-cert.pem server-key.pem"
    pki_count=0

    for file in $pki_files; do
        if [ -e "$pki_dir/$file" ]; then
            pki_count=$((pki_count + 1))
        fi
    done

    case "$pki_count" in
        0)
            /app/server ca init
            ;;
        4)
            for file in $pki_files; do
                if [ ! -s "$pki_dir/$file" ]; then
                    echo "refusing incomplete server PKI: $pki_dir/$file" >&2
                    exit 1
                fi
            done
            ;;
        *)
            echo "refusing partial server PKI; restore the complete server-pki volume" >&2
            exit 1
            ;;
    esac
fi

exec /app/server "$@"
