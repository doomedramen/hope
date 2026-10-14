# Operations notes

Full operator documentation is tracked separately (Ticket 11). This file records the operational behavior that is already implemented and tested: shutdown, backup/restore, response headers, and request limits.

## Shutdown

`hope-server` handles SIGTERM (Docker, systemd) and SIGINT (Ctrl-C) identically. On either signal it:

1. stops accepting scan and SSH-job requests and stops the periodic monitoring and retention timers;
2. cancels in-flight scans and SSH installation sessions, and ends open event streams (browsers reconnect on their own);
3. waits for connections to drain, then persists unfinished work: scans become `interrupted`, SSH jobs become `interrupted` with code `SERVER_RESTARTED` (the remote host may be partly installed; inspect it before retrying);
4. checkpoints and truncates the SQLite write-ahead log and closes the database.

The internal deadline is 8 seconds from the signal, inside Docker's default 10 second stop grace period. If connections do not close in time the server logs a warning and exits anyway; unfinished rows are still marked interrupted. A hard kill (SIGKILL, power loss) skips this; the next start marks any `queued`/`running` scans and jobs as interrupted, so no state is left claiming to run.

## Backup and restore

The database is `$HOPE_DATA_DIR/hope.db` (`/var/lib/hope/hope.db` in the image). It runs in WAL mode, so committed data may live in `hope.db-wal` rather than the main file. **Never copy `hope.db` alone while the server runs.**

Create a consistent backup of the live database (SQLite `VACUUM INTO`, one snapshot, safe during writes):

```sh
docker compose exec hope hope-server backup /var/lib/hope/backup-$(date +%Y%m%d-%H%M%S).db
# or, outside Docker:
HOPE_DATA_DIR=/path/to/data hope-server backup /path/to/backup.db
```

The command refuses to overwrite an existing file, creates the copy with mode 0600 (it holds password, session and credential hashes), checks it with `PRAGMA integrity_check`, and produces a standalone file with no `-wal`/`-shm` sidecars. Move backups off the host (`docker compose cp hope:/var/lib/hope/backup-….db .`) and treat them as secrets.

What is not in the database: agent release artifacts and signatures (`./releases`), the optional custom CA file referenced by `HOPE_AGENT_CA`, and environment configuration (`.env`). Back those up separately.

Restore:

1. Stop the server (`docker compose stop hope`) so nothing holds the database.
2. Replace `hope.db` with the backup and **delete stale `hope.db-wal` and `hope.db-shm`**. A leftover WAL from the previous database would be replayed against the restored file and corrupt it.
3. Ensure the file is owned by the service user (uid 10001 in the image).
4. Start the server. Migrations run on open; a backup from a newer server version is refused rather than modified.

In Docker the data lives in the `hope-data` volume; for example, with the server stopped:

```sh
docker run --rm -v hope_hope-data:/data -v "$PWD":/backup debian:bookworm-slim \
  sh -c 'rm -f /data/hope.db-wal /data/hope.db-shm && cp /backup/backup.db /data/hope.db && chown 10001:10001 /data/hope.db'
```

Agents keep reporting with their existing credentials after a restore; reports newer than the backup are simply absent from history.

## Response headers

Every response carries `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`, a restrictive `Content-Security-Policy` (`frame-ancestors 'none'`, `object-src 'none'`, `base-uri 'none'`, everything else `'self'`) and a deny-all `Permissions-Policy`. `Strict-Transport-Security: max-age=31536000` is sent only when `HOPE_SERVER_URL` is `https://`.

Every `/api/` response (authentication, enrollment, install commands, SSH jobs, event stream, errors) also carries `Cache-Control: no-store` and `Pragma: no-cache`. Static console files are left cacheable.

The CSP allows inline scripts (`script-src 'self' 'unsafe-inline'`). The Next.js static export embeds inline bootstrap scripts whose contents differ per page and build, so they cannot be pinned by a fixed hash without a post-build hashing step. Styles allow inline for React Flow and chart style attributes. All other sources, including `connect-src`, are `'self'`. Do not weaken or override these at the reverse proxy.

Session cookies are `HttpOnly; SameSite=Strict`, plus `Secure` when `HOPE_SERVER_URL` is `https://`. State-changing requests (anything but GET/HEAD/OPTIONS) are rejected when `Origin` differs from the canonical URL or `Sec-Fetch-Site` is anything other than `same-origin`/`none`; agents and `curl` send neither header.

## Request limits and reverse proxies

Limits count **failures per client** and never use a shared (global) budget, because any shared ceiling can be filled by an attacker from many addresses to lock out the administrator or legitimate agents. Successful logins, enrollments and accepted reports consume nothing from the failure budget; a successful login clears the client's failures.

| Endpoint | Per-client limit | Notes |
| --- | --- | --- |
| Login and first-run setup | 10 failures/minute, then 429 | No global ceiling. Password hashing concurrency is capped at 2; a flood gets a transient 503 with `Retry-After: 2` (not counted as a failure), never a lockout |
| Agent enrollment | 20 failed requests/minute | Successful enrollments do not count |
| Agent reports with bad credentials | 30 rejections/minute | Checked before the global write lock is taken |
| Authenticated reports | 120/minute per agent identity | Keyed by the authenticated identity only; no global ceiling |

The key table holds 4096 clients. When full it drops expired entries, then the least recently active one; keys are never pooled, so a new client always starts with its own budget. IPv6 clients are keyed by /64 and IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) are treated as IPv4.

"Client" is the TCP peer. Behind a reverse proxy every request arrives from the proxy, so the proxy must be trusted to supply `X-Forwarded-For` (the right-most entry that is not itself a trusted proxy is used; left-hand entries are client supplied and ignored):

- `HOPE_TRUSTED_PROXIES` (comma-separated addresses/CIDRs) sets the list explicitly.
- When it is unset and `HOPE_BIND` is loopback-only (the default, and the documented same-host proxy setup), loopback (`127.0.0.0/8`, `::1`) is trusted automatically: only local processes can reach such a listener.
- For any other bind (the Docker image binds `0.0.0.0`) nothing is trusted by default; set the variable to the proxy address as the server sees it.

The server logs a warning once if an untrusted peer sends `X-Forwarded-For`, and once if a trusted proxy sends an unparseable hop. Without a correct trusted list all clients share one key, so ten failures from anyone would block everyone for a minute.

Rejected request bodies (malformed JSON, wrong content type, oversize) receive fixed messages that never quote the submitted data.

Container capability note: the `hope-server` binary has a file capability (`cap_net_raw`), so the container's bounding set must contain `NET_RAW` (compose `cap_add`); without it the binary fails to execute. Do not enable `no-new-privileges`. Run backups with `docker compose exec hope hope-server backup /var/lib/hope/<name>.db`: the root filesystem is read-only, so the destination must be on the data volume or another writable mount, and the file is fsynced (with its directory) before the command reports success.

## Signed agent releases

`$HOPE_RELEASE_DIR` (`./releases` in compose) is a root directory written only by `scripts/assemble-release.sh`. It holds versioned release directories `<version>-<random>/` and the symlink `current` pointing at the live one (plus `previous`). The server resolves `current` on every request, so a publish takes effect immediately, without a restart. If there is no `current`, the directory itself is treated as the release (tests, hand-built trees). Each release directory contains:

```
hope-agent-{linux,macos}-{x86_64,aarch64}   four binaries
hope-agent-….sig                            per-artifact RSA/SHA-256 signature
manifest.json, manifest.json.sig            signed version / key id / sha256 / size / minimum OS per artifact
release-public.pem                          the public key embedded into /agent/install
```

The installer downloads `manifest.json`, verifies its signature against the key embedded in the installer it was served by, picks its platform entry, then checks the artifact's size, SHA-256 and own signature, and finally that `--version` equals the manifest version. Any mismatch aborts before the service is touched (`MANIFEST_INVALID`, `CHECKSUM_MISMATCH`, `BAD_SIGNATURE`, `RELEASE_UNAVAILABLE`). An upgrade to an older version than the installed agent is refused (`DOWNGRADE_REFUSED`) unless the host runs the installer with `HOPE_ALLOW_DOWNGRADE=1`. The server lists a platform as available only if the manifest is well formed, its `key_id` equals the SHA-256 of the SPKI DER of `release-public.pem`, it lists the platform, and the artifact and signature exist with the listed size. With no manifest nothing is available (no per-file fallback); the console and SSH jobs then report that no signed release manifest is published. The downgrade check compares only with the agent installed on that host: a fresh install, or a host whose agent is already old, can receive any older release the server's key signed, so publish only intended releases (rollback is a deliberate act, see below).

### Platforms

| Platform | Target | Minimum |
| --- | --- | --- |
| Linux x86_64, aarch64 | `*-unknown-linux-musl` | Static binary, no glibc; any distribution. Kernel 3.2 (x86_64) / 4.1 (aarch64), as documented by Rust |
| macOS x86_64, aarch64 | `*-apple-darwin` | macOS 11.0 (`MACOSX_DEPLOYMENT_TARGET`, checked against the binary's `minos`) |

Support is not claimed for any platform until the installation matrix in `docs/validation.md` is evidenced for it.

### The signing key

One RSA key (at least 2048 bits; use 3072), generated and stored **outside the repository and outside the container image**:

```sh
umask 077
openssl genrsa -out ~/secure/hope-release-signing.pem 3072
```

`assemble-release.sh` refuses a key path inside the repository. Keep an offline backup; losing the key means every host must be re-installed from a new installer. In CI the key is the `HOPE_SIGNING_KEY` secret of the protected `release` GitHub environment, available only to the `publish` job of `release.yml` (tag pushes `v*`); pull-request workflows reference no secrets. Configure required reviewers and restrict the environment to `v*` tags.

### Publishing

1. Set the workspace version in `Cargo.toml`, merge, tag `vX.Y.Z` (the tag must equal the crate version). `release.yml` builds the four targets on matching runners, uploads them, and the `publish` job assembles, signs and verifies; it fails if any platform is missing.
2. To publish to a server by hand: download the four `agent-*` artifacts into one directory and run
   `HOPE_SIGNING_KEY=~/secure/hope-release-signing.pem scripts/assemble-release.sh --staging DIR --version X.Y.Z --out releases`.
   It checks that all artifacts share the version and git commit and that each ELF/Mach-O header matches its platform label, builds `releases/X.Y.Z-<random>/`, verifies every signature, checksum, size and the key id there, then atomically repoints `releases/current` (a temporary symlink renamed over it with `mv -T`/`mv -h`). A failure leaves `current` untouched. The two newest older releases, always including the one just replaced, are kept; the live one is never deleted. Version must be plain `X.Y.Z` or `X.Y.Z-pre` (no leading `v`).
3. Mount the **root** (`./releases:/opt/hope/releases:ro`, as the compose files do), never a versioned directory: the root keeps its identity, and the relative `current` symlink resolves inside the container, so no restart is needed after publishing.
4. Roll back by repointing: `ln -s <older-dir> releases/.c && mv -fh releases/.c releases/current` (BSD/macOS; use `mv -fT` on GNU). Hosts that already upgraded keep the newer agent; re-running the installer on them will refuse to downgrade unless `HOPE_ALLOW_DOWNGRADE=1`. Re-publishing with `--allow-downgrade` also works.

Local development: `HOPE_SIGNING_KEY=… scripts/package-agent.sh` builds this machine's target and publishes a **partial** release (manifest `partial:true`, one platform) into `./releases`. Its staging directory must be under `./dist` or the temp directory. To change development key, delete `./releases` first; `--rotate-key` requires a complete release. Linux targets on a Mac: `scripts/build-agent-docker.sh <triple> --out DIR`.

### Rotation and revocation

Publishing a different public key next to old artifacts would break verification for some hosts, so the script refuses it. To rotate: `assemble-release.sh … --rotate-key` with the new key and all four artifacts (it cannot be combined with `--allow-partial`); every artifact and the manifest are signed with the new key in that run and the old release stays available as `previous`. Installers already fetched embed the old public key and will reject the new release with `MANIFEST_INVALID`; hosts must fetch `/agent/install` again from the server. Rotation does not revoke agent credentials; revoke those in the console. A compromised signing key requires rotating, restarting the server, and re-installing from freshly fetched installers; there is no automatic revocation of binaries already installed.
