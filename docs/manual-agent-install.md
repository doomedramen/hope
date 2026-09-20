# Manual agent installation

The installer supports Linux `amd64` and `arm64`. It downloads the selected
agent from the Hope server, verifies the signed release bundle, enrolls the
host with a single-use code, and creates a hardened systemd service.

The server must already be running with its CA and agent listeners available.
The normal endpoints are:

- enrollment: `https://<hope-host>:8444`
- gateway: `wss://<hope-host>:8443`
- HTTP/API origin: `https://<hope-host>`

## Install

Create a short-lived enrollment code on the Hope server:

```sh
server enroll-token create --ttl-minutes 15
```

When running from a checkout, use `cargo run -p server --` before the
subcommand. Transfer the printed `code=<token>.<ca-fingerprint>` value to the
target host through a trusted channel. It is single-use and must be treated as
a password. Do not put it in shell history, a unit file, or logs.

On the target Linux host, download the installer from the same Hope origin:

```sh
curl -fsSL https://hope.example/install-agent.sh -o /tmp/hope-install-agent.sh
chmod 0700 /tmp/hope-install-agent.sh
sudo /tmp/hope-install-agent.sh \
  --enroll-url https://hope.example:8444 \
  --gateway-url wss://hope.example:8443 \
  --release-base-url https://hope.example
```

The installer detects `x86_64`/`aarch64`, selects the matching Linux artifact,
and prompts for the enrollment code without echoing it. `latest` is the default
release selection. To supply the code through a pipe without exposing it in
the process list, use `--code-stdin --yes`:

```sh
printf '%s\n' '<token>.<ca-fingerprint>' | \
  sudo /tmp/hope-install-agent.sh \
    --enroll-url https://hope.example:8444 \
    --gateway-url wss://hope.example:8443 \
    --release-base-url https://hope.example \
    --code-stdin --yes
```

The UI should generate these values from the configured public URL. The
installer URL is always the current Hope origin with `/install-agent.sh`
appended; `hope.example` above is only an example.

For a pinned release, add `--version 0.1.0`. For an explicit release
repository root, use `--release-url https://hope.example/agent-download` with
an exact `--version`; `--release-base-url` is required when using `latest`.

The installer:

1. downloads `manifest.json`, its detached signature, and the architecture-
   specific binary from the server's verified release repository;
2. runs the downloaded binary's `verify-release` command before replacing an
   installed binary;
3. installs `/usr/local/libexec/hope-agent` as a root-owned executable;
4. creates the non-login `hope-agent` service account and `/var/lib/hope` with
   mode `0700`;
5. enrolls the host using the code and writes the private identity only in
   that state directory; and
6. enables and starts `hope-agent.service` with systemd hardening.

Re-running the installer is safe. Existing enrollment state is kept and the
binary/unit are replaced only after release verification succeeds.

Check the service:

```sh
sudo systemctl status hope-agent
sudo journalctl -u hope-agent -f
```

## Uninstall

Remove the unit and binary but preserve the enrolled identity for recovery:

```sh
sudo /tmp/hope-install-agent.sh --uninstall --yes
```

To also remove the service account and enrolled identity:

```sh
sudo /tmp/hope-install-agent.sh --uninstall --purge --yes
```

`--purge` is intentionally separate and requires `--yes`.

## Security and recovery

- Use the HTTPS enrollment URL and the exact CA fingerprint printed with the
  enrollment code. The agent refuses enrollment when the fingerprint does not
  match the Hope CA.
- The enrollment code never appears in the systemd unit, command-line
  arguments, or installer logs when using the interactive prompt or
  `--code-stdin`.
- The service account has no login shell and cannot write the installed binary.
- Docker socket access is root-equivalent. Do not add `hope-agent` to the
  Docker group unless that trust is intended.
- If enrollment expires or has already been used, create a new code. Do not
  disable TLS or reuse an old code.
- The current agent certificate lifetime is 30 days. Re-enroll before expiry
  until certificate renewal is implemented.

Automatic self-update is not exposed by this installer yet. The update and
rollback path must be complete before an unattended update switch is offered.

Automated SSH installation and repair are documented in the API section below
and use the same verified release repository.

## Automated SSH install and repair

The API exposes `POST /api/v1/devices/<device-id>/agent-install` and
`POST /api/v1/devices/<device-id>/agent-repair`. Both require an
`Idempotency-Key` header and a JSON body containing `host`, `port`, and the
selected `credential_id`. Set `disassociate_after_enrollment` to `true` when
the SSH credential should be detached after successful enrollment.

The server and worker deployment must provide:

```text
HOPE_CREDENTIAL_MASTER_KEY_FILE=/app/data/pki/credential-master-key
HOPE_AGENT_ENROLL_URL=https://hope.example:8444
HOPE_AGENT_GATEWAY_URL=wss://hope.example:8443
HOPE_AGENT_BINARY_X86_64=/app/agent-releases/agent-linux-amd64
HOPE_AGENT_BINARY_AARCH64=/app/agent-releases/agent-linux-arm64
```

The worker detects Linux architecture over SSH, verifies the saved SSH host
key, uploads the matching verified artifact, creates the dedicated service
account, and enrolls the agent. First use and host-key changes deliberately
fail with a pending/changed trust record; review the fingerprint through
`GET /api/v1/ssh-host-keys` and explicitly call its `/trust` action before
retrying. The worker executes only the fixed install/repair sequence; it is not
a general-purpose shell runner.
