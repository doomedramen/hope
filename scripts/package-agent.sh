#!/bin/sh
set -eu
# Supply the same offline RSA release-signing key for every platform. Never commit it.
: "${HOPE_SIGNING_KEY:?Set HOPE_SIGNING_KEY to your RSA private key file}"
cd "$(dirname "$0")/.."
case "$(uname -s)" in Linux) platform=linux;; Darwin) platform=macos;; *) exit 1;; esac
case "$(uname -m)" in x86_64|amd64) architecture=x86_64;; arm64|aarch64) architecture=aarch64;; *) exit 1;; esac
cargo build --release --locked -p hope-agent
mkdir -p releases
artifact="releases/hope-agent-$platform-$architecture"
cp target/release/hope-agent "$artifact"
openssl pkey -in "$HOPE_SIGNING_KEY" -pubout -out releases/release-public.pem
openssl dgst -sha256 -sign "$HOPE_SIGNING_KEY" -out "$artifact.sig" "$artifact"
openssl dgst -sha256 -verify releases/release-public.pem -signature "$artifact.sig" "$artifact"
echo "Packaged $artifact"
