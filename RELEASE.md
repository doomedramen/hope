# Release process

This repository currently builds and tests server, web, and agent artifacts in
GitHub Actions. It does not publish a production server image or agent release
automatically. A release operator must verify artifacts and place the approved
agent bundle in the server-side release repository.

## Prepare

1. Merge code, migration, test, and documentation changes to the release
   commit. Record the source commit and intended version.
2. Run `just fmt`, `just lint`, and `just test`, plus required DB- and
   Docker-gated tests.
3. Review migrations and [upgrade/recovery guidance](docs/operations/upgrade-recovery.md).
4. Confirm the backup and restore procedure was exercised for the deployment
   class being upgraded.
5. Build server and web artifacts from the same source commit. For production,
   publish or pin the resulting image by immutable digest; the example Compose
   file builds locally.

## Sign agent releases

Keep the Ed25519 signing seed offline. Generate it with:

```sh
just release-keygen
```

Build and sign the supported Linux artifacts:

```sh
just release-build-and-sign VERSION=0.1.0
```

Or sign an existing build with `just release-sign`. Review `manifest.json`,
`manifest.json.sig`, `SHA256SUMS`, platform/architecture, protocol floor, and
channel. Verify the bundle independently:

```sh
( cd dist/release && sha256sum -c SHA256SUMS )
agent verify-release \
  --manifest dist/release/manifest.json \
  --binary dist/release/agent-linux-amd64
```

The CI job uses configured repository signing secrets when available. Without
them, CI generates an ephemeral key only to exercise the pipeline; those
artifacts are not production-trusted.

## Publish and trust

Copy only the approved, verified bundle into the read-only release repository:

```text
agent-releases/
  public-key.hex
  <version>/
    manifest.json
    manifest.json.sig
    SHA256SUMS
    agent-linux-amd64
    agent-linux-arm64
```

Keep `public-key.hex` or `HOPE_RELEASE_PUBLIC_KEYS` aligned with the public key
embedded in supported agents. During rotation, retain both keys until every
supported agent has crossed the dual-trust release boundary. Never copy the
private signing seed into the server, worker, container image, or release
repository. See [Signed agent releases](docs/release-signing.md).

## Rollout

1. Back up PostgreSQL, credential master key, server PKI, release repository,
   and deployment configuration.
2. Upgrade server and worker as a lockstep pair using the migration procedure.
3. Check API liveness/readiness, login, worker jobs, maintenance reconciliation,
   notifications, and agent gateway reconnect.
4. Start with a canary or manually pinned agent update. Confirm the target
   agent checks in before widening rollout.
5. Keep the previous signed artifact and database backup for the documented
   recovery window.

Agent update rollback is bounded to the target agent binary. It does not roll
back control-plane migrations or server PKI. If a migration has run, follow
the [upgrade rollback boundary](docs/operations/upgrade-recovery.md#rollback-boundary).

## Support target

The operational target is support for the current release and one previous
release (`N` and `N-1`). This target is not a claim that all mixed-version or
rolling upgrades are currently proven. Record tested version pairs, migration
ranges, agent protocol floors, trust-key changes, and restore-test evidence
with each release.
