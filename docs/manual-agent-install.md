# Manual agent installation

Use this procedure before automated SSH installation. It installs one Linux
agent, enrolls it with a single-use token, and starts its outbound gateway
connection.

Current supported binaries:

- Linux `amd64` for `x86_64` hosts: `agent-linux-amd64`
- Linux `arm64` for `aarch64` hosts: `agent-linux-arm64`

The server must already have its internal CA and agent listeners configured.
The enrollment listener defaults to `https://<server>:8444`; the mTLS gateway
defaults to `wss://<server>:8443`.

## 1. Create enrollment material

Run this on the server:

```sh
server enroll-token create --ttl-minutes 15
```

When running from this repository, use `cargo run -p server --` before the
subcommand.

The command prints:

```text
token=<single-use secret>
ca_fingerprint_sha256=<CA SHA-256 fingerprint>
code=<token>.<fingerprint>
```

Transfer `code` to the target host through a trusted channel. The token is
single-use, expires after its TTL, and must be treated like a password. The CA
fingerprint is not secret, but it pins the enrollment TLS connection and must
also come from a trusted channel. Do not put either value in shell history or
logs.

## 2. Select and verify the binary

Select the binary from the signed release bundle. Verify its checksum and
signature before copying it to the host:

```sh
sha256sum -c SHA256SUMS
agent verify-release \
  --manifest manifest.json \
  --binary agent-linux-amd64
```

Use `agent-linux-arm64` and `aarch64` when the target architecture is ARM64.
Do not run an artifact for a different architecture. Release signing details
are in [Signed agent releases](release-signing.md).

Install the selected binary as a root-owned executable. Replace the source
filename when using ARM64:

```sh
sudo install -o root -g root -m 0755 agent-linux-amd64 /usr/local/libexec/hope-agent
```

## 3. Enroll the host

Create a dedicated service account and private state directory. The state
directory contains the agent private key, client certificate, and CA
certificate.

```sh
sudo useradd --system --home-dir /var/lib/hope --shell /usr/sbin/nologin hope-agent
sudo install -d -o hope-agent -g hope-agent -m 0700 /var/lib/hope

sudo -u hope-agent /usr/local/libexec/hope-agent enroll \
  --server https://hope.example:8444 \
  --code '<token>.<ca-fingerprint>' \
  --state-dir /var/lib/hope
```

Enrollment generates the key locally. The SSH credential used to copy the
binary never reaches the agent.

## 4. Start the outbound connection

Run the gateway process as the dedicated account:

```sh
sudo -u hope-agent /usr/local/libexec/hope-agent run \
  --gateway wss://hope.example:8443 \
  --state-dir /var/lib/hope
```

For a persistent host, run the same command under the host's service manager.
Example systemd unit:

```ini
[Unit]
Description=Hope agent
After=network-online.target
Wants=network-online.target

[Service]
User=hope-agent
Group=hope-agent
ExecStart=/usr/local/libexec/hope-agent run --gateway wss://hope.example:8443 --state-dir /var/lib/hope
Restart=always
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
ProtectHome=true
ProtectSystem=strict
ReadWritePaths=/var/lib/hope

[Install]
WantedBy=multi-user.target
```

Enable it only after manual enrollment succeeds:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now hope-agent
sudo journalctl -u hope-agent -f
```

## Least privilege

- Run the agent as a dedicated non-login user. Keep `/var/lib/hope` mode `0700`.
- Keep the binary root-owned and non-writable by the service account.
- Grant read access only to collectors that need it. Do not grant `sudo` or
  arbitrary command execution.
- Docker socket access is root-equivalent. Do not add the service account to
  the Docker group unless container inventory is required and that trust is
  accepted.
- If a collector needs root for socket ownership, SMART, package, or system
  data, use the smallest host-specific exception. Review it when collector
  capabilities change.
- Keep enrollment code out of unit files, environment files, process arguments,
  and logs after enrollment. Remove it from shell history.

## Reconnect and recovery

`agent run` keeps its enrolled identity on disk and reconnects after a dropped
gateway connection. Retry delay uses full jitter over exponential backoff:
1, 2, 4, 8, 16, 32, then at most 60 seconds. A successful session resets the
backoff.

The agent must be enrolled before `run`. If the state files are missing or
unreadable, fix ownership and permissions; do not copy a private key from
another host. A revoked certificate is rejected on its next gateway
connection. The current implementation issues a 30-day client certificate
without renewal, so plan a new enrollment before expiry.

If the enrollment token is expired or already used, create a new token. If the
CA fingerprint is wrong, stop and obtain the fingerprint again through a
trusted channel; do not disable TLS verification.
