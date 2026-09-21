# Contributing

Contributions should preserve the security, data-boundary, and operational
contracts in [`spec.md`](spec.md), the ADRs, and the current
[threat model](docs/threat-model.md).

## Development setup

Install the Rust toolchain selected by [`rust-toolchain.toml`](rust-toolchain.toml),
Node.js and pnpm, Docker with Compose v2, and PostgreSQL for DB-gated tests.
Use `.env` for local values; never commit `.env`, credentials, private keys,
agent state, release signing keys, or production data.

Install Lefthook and enable the repository hooks before committing:

```sh
just hooks
```

The pre-commit hook runs formatting, Clippy, and web lint. The pre-push hook
also cross-builds the Linux agent with `cargo-zigbuild`, then runs the Rust and
web tests/build. Install Zig and `cargo-zigbuild` for the Linux agent check.

Useful commands:

```sh
just fmt
just lint
just test
just dev
just web
```

`just test` runs the Rust and web suites. Tests that need PostgreSQL use
`DATABASE_URL`; tests that need Docker are explicitly ignored. Run those gates
when changing the related subsystem.

The browser smoke test needs a running server with a disposable database. Set
`E2E_BASE_URL` when the server is not at the default address:

```sh
E2E_BASE_URL=http://127.0.0.1:8080 pnpm -C apps/web test:e2e
```

CI provisions PostgreSQL, the server PKI, the server, and Chromium before
running this test.

## Change rules

- Keep changes focused. Preserve concurrent work and do not rewrite unrelated
  files.
- Add or update tests for behavior changes. Include migration-upgrade coverage
  when a schema change affects an existing database.
- Treat migrations as forward-only after merge. Add a new numbered migration;
  do not edit an applied migration.
- Keep API mutations authenticated and behind the CSRF custom-header
  middleware. Do not return credential plaintext, private keys, enrollment
  tokens, or release signing keys in API responses or logs.
- Keep release verification and agent trust boundaries intact. Do not add a
  bypass for signature, CA fingerprint, or SSH host-key checks.
- Update user-facing and operational documentation when commands, environment
  variables, API behavior, backup requirements, or recovery boundaries change.
- Record architecture or security decisions in an ADR when the change cannot
  be explained by an existing decision.

## Pull requests

Describe behavior, data migrations, security impact, and operational rollout or
rollback steps. List commands run and their result. Call out skipped DB,
Docker, browser, or platform-specific checks.

Use a focused Conventional Commit subject, for example:
`fix(server): reject stale maintenance versions`. Keep commits independently
reviewable. Do not combine unrelated features, formatting churn, or generated
artifacts.

Reviewers should check changed-file scope, migration safety, secret handling,
authorization and CSRF coverage, logs, tests, and documentation links.
