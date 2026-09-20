# ADR-0017: signed agent updates and rollback

## Status

Accepted for Milestone 7.

## Decision

The server owns a filesystem-backed release repository. PostgreSQL stores only
verified manifest metadata, update policy, and update-operation state. The
server and worker never accept an operator-supplied binary path or shell
command from the API.

Before a release is exposed or used, the repository verifies:

- the detached Ed25519 signature over the exact `manifest.json` bytes;
- that every manifest artifact has a trusted Ed25519 record signature;
- that `agent-<platform>-<arch>` exists as a regular, bounded file;
- the signed size and SHA-256 digest; and
- platform, architecture, release-version, and minimum-protocol consistency;
- signed release channel (`stable` or `canary`) and bounded release notes.

Trusted public keys are configured with `HOPE_RELEASE_PUBLIC_KEYS` or
`HOPE_RELEASE_PUBLIC_KEY(_FILE)`. Multiple keys are accepted during a key
rotation window. The private signing key never enters the server or worker.

An update job uses the existing M6 SSH credential association and host-key
trust boundary. It uploads the verified bytes to a bounded temporary path,
installs them as a root-owned staged file, copies the current binary to a
rollback path, and atomically renames the staged file into the service path.
The worker restarts `hope-agent`, then waits for a fresh gateway hello with
the target version. If the service fails or the deadline expires, the worker
restores the rollback copy and waits for the previous version to check in. If
that recovery also fails and the policy permits it, the existing M6 repair
flow is attempted.

Update operations are durable and expose `pending`, `verifying`,
`installing`, `restarting`, `succeeded`, `failed`, `rolled_back`, `pinned`,
and `incompatible` states. A partial unique index prevents two active updates
for one agent. Policies support `manual`, `notify`, and `automatic` modes;
each carries a channel, optional pin, rollout percentage, SSH target, and
repair preference. The scheduler enqueues the bounded automatic-reconcile
job every five minutes.

## Repository layout

The configured directory can contain a single bundle at its root or versioned
subdirectories:

```text
agent-releases/
  public-key.hex
  0.7.0/
    manifest.json
    manifest.json.sig
    agent-linux-amd64
    agent-linux-arm64
```

The current `cargo xtask sign` output can be copied into a version directory.
The channel and optional release notes live in the signed manifest; automatic
selection filters by the policy's channel, while an explicit pinned or manual
version remains an operator choice.
The public-key file is a trust configuration file, not a release artifact
signature and must contain only public keys.

## Rotation and recovery

1. Generate a new key without replacing the old private key.
2. Add the new public key to `HOPE_RELEASE_PUBLIC_KEYS_FILE` while retaining
   the old key.
3. Ship and verify a release signed by the old key that embeds both keys in
   the agent build. Keep both server keys configured during the fleet
   transition.
4. Begin signing releases with the new key and verify the repository accepts
   them before removing the old key.
5. Retire the old key only after all supported agents have crossed the dual-key
   release boundary. Keep an offline copy of the old public key and the
   corresponding signed bundles for rollback and incident response.

If the repository or signing key is unavailable, automatic reconciliation
does not enqueue updates. Existing agents continue to run; an operator can
restore the release directory and retry. If an update fails its check-in,
the update operation records `rolled_back`; SSH repair remains available as a
separate explicit operation.
