# Linux agent installation

The installer supports Linux `amd64` and `arm64`. It downloads the selected
agent from the Hope server, verifies the signed release bundle, enrolls the
host with a single-use code, and creates a hardened systemd service.

The server must already be running with its CA and agent listeners available.
The normal endpoints are:

- enrollment: `https://<hope-host>:8444`
- gateway: `wss://<hope-host>:8443`
- HTTP/API origin: `https://<hope-host>`

## Install

The Agents page generates a short-lived, single-use bootstrap code. Copy the
resulting command to the target Linux host:

```sh
curl -fsSL 'https://hope.example/agent/install.sh' | \
  sudo env \
    HOPE_SERVER='https://hope.example' \
    HOPE_ENROLLMENT_CODE='<token>.<ca-fingerprint>' \
    bash
```

The command downloads the signed installer, derives the enrollment and gateway
URLs from `HOPE_SERVER`, enrolls the host, installs the matching `amd64` or
`arm64` release, and enables the hardened systemd service. The code expires
after 15 minutes and must be treated like a password.

The installer also accepts the shorter compatibility aliases `HSERV`, `HPKEY`,
and `HHKEY` (server, token, and CA fingerprint respectively). Explicit
`--enroll-url`, `--gateway-url`, and `--release-base-url` flags remain available
for recovery and unusual topologies:

```sh
curl -fsSL https://hope.example/agent/install.sh -o /tmp/hope-install-agent.sh
chmod 0700 /tmp/hope-install-agent.sh
sudo /tmp/hope-install-agent.sh \
  --enroll-url https://hope.example:8444 \
  --gateway-url wss://hope.example:8443 \
  --release-base-url https://hope.example
```

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

- Use HTTPS for `HOPE_SERVER` and only paste the generated command into the
  intended target host. The code never appears in the systemd unit or
  installer logs, but it is visible briefly in the shell command and process
  environment while bootstrapping.
- The agent refuses enrollment when the CA fingerprint in the code does not
  match the Hope CA.
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
