# ADR-0016: Bounded SSH agent deployment

## Status

Accepted for Milestone 6.

## Decision

Agent install and repair run as leased background jobs. The API accepts a
credential ID, target device, host, port, and an idempotency key; it never puts
secret material in the job payload. The worker decrypts the selected SSH
credential for one operation, connects with `russh`, verifies the presented
host key through the M6 trust table, and rejects first-use, changed, and
revoked keys until an operator explicitly trusts the record.

The worker executes a fixed sequence only:

1. detect Linux and `x86_64`/`aarch64`;
2. verify local binary size and upload it to a random temporary path;
3. install a root-owned binary and a least-privilege `hope-agent` service user;
4. enroll with a one-time token sent over SSH stdin, never a command-line
   argument;
5. write a hardened systemd unit and start it;
6. verify the service is active.

Repair repeats the binary/service steps and re-enrolls only when the state
files are missing. Remote output is capped at 64 KiB, operations have a
bounded timeout, shell arguments are quoted, and errors are generic so
passwords, private keys, and enrollment codes cannot enter logs or API
responses.

The server must be configured with `HOPE_AGENT_ENROLL_URL`,
`HOPE_AGENT_GATEWAY_URL`, and read-only paths to the signed agent binaries:
`HOPE_AGENT_BINARY_X86_64` and `HOPE_AGENT_BINARY_AARCH64`. The CA path is
read from `HOPE_CA_CERT_PATH` or the existing default. The install path is
deliberately not a general remote-shell feature.

## Consequences

- Linux hosts need `useradd`, `install`, `systemctl`, and passwordless root
  escalation when the SSH account is not root.
- First-use host keys produce a pending record and a failed job; the operator
  must review the fingerprint before retrying.
- A saved SSH credential can be disassociated after successful enrollment.
  The mTLS agent identity remains independent, so deleting the credential does
  not break an enrolled agent.
- Hosts without systemd are rejected in this milestone instead of receiving
  an untracked background process.
