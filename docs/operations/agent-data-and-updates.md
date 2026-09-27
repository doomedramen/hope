# Agent data, logs, and managed updates

## Upgrade order

Deploy the server and migrations 0036–0038 before publishing an agent with log-streaming and managed-update capabilities. Existing agents keep their current protocol behavior.

For an existing installation, run the signed installer again using the existing server address and TLS pin. Enrollment files in `/var/lib/hope` are reused; no new enrollment is needed. The installer adds `hope-agent-update.service` and `hope-agent-update.timer`. The previous SSH updater remains a recovery option, but replacing only the executable does not install the new helper. The UI reports **Bootstrap required** until the agent advertises managed updates.

Publish verified bundles under `HOPE_AGENT_RELEASE_DIR`, using the release-signing procedure. Linux amd64 and arm64 are supported when the corresponding artifact is present. The running agent and updater helper must trust the signing key; follow the dual-key rotation procedure in ADR-0017.

In Agents, open an agent's Overview to update now. Settings contains notify/manual/automatic policy, release channel, optional version pin, rollout percentage, and UTC update window. Equal start/end hours permit updates all day. Notify is the default. An offline update remains pending. Automatic expansion needs a healthy canary and ten-minute observation period; a small fleet may need an explicit first update.

The helper keeps its previous executable and status under `/var/lib/hope-updater`. Staged downloads are under `/var/lib/hope/update`. Inspect these services when an operation stops progressing:

```sh
systemctl status hope-agent hope-agent-update.timer hope-agent-update.service
journalctl -u hope-agent -u hope-agent-update.service
```

An `awaiting_health` operation means local collection resumed but server verification is incomplete. It is not evidence of success or a reason to discard the previous executable. An overdue operation pauses automatic expansion. Restore connectivity or inspect collector diagnostics. Failed and rolled-back operations can be explicitly retried.

## Enable selected logs

In Settings, enter exact systemd unit or Docker container names, one per line. Empty lists disable host/container log collection. Agent diagnostics remain available. Literal redaction strings are applied before writing messages to disk. Settings show the revision acknowledged by the agent.

The service account must already have permission to read each selected source. For journal access, an administrator can add `hope-agent` to `systemd-journal`, then restart `hope-agent`. Docker socket access is powerful: add the account to the Docker socket's group only if that access is appropriate for the host. The installer does not grant either permission automatically. Permission failures are visible in Agent diagnostics and do not stop metrics.

Sources are polled every five seconds with a four-second command deadline and 64 KiB capture per output stream. There are at most 16 configured sources and 32 redaction strings. Messages are limited to 2 KiB locally and carry truncation metadata. Each source checkpoint is saved after its batch is durable. Journal expiration and exhausted Docker replay boundaries emit collection-gap events. Quiet sources can be healthy without producing messages.

Logs & activity supports source, severity, full-text search, time-bounded deep links, older-page navigation, and bounded export of loaded entries. Pausing or focusing a log entry freezes visible rows; new entries are counted until resume. Export is NDJSON and includes only loaded results. Log text is rendered as text.

## Storage and interpretation

Metrics and logs each retain up to 64 MiB or 24 hours in local outboxes. Collection continues while disconnected and after process restart. Server acknowledgements follow transaction commit; replays retain the same identities. Overview shows delivery counters from the last received metric sample, so offline queue sizes are necessarily stale. Loss counters include expiration, capacity eviction, corrupt records, and permanent rejection.

The server limits log ingestion to 4 MiB per connection per minute; excess batches receive retryable acknowledgements. Host/container logs default to seven-day retention, configurable from one to 90 days per agent. Agent diagnostics retain 30 days. Search windows are limited to 90 days and pages to 500 entries.

Raw metric retention keeps the existing setting (default seven days). Five-minute rollups retain 30 days; hourly rollups retain 180 days. Retention waits for required rollups. One-hour raw queries read at most 4,096 rows; longer UI ranges use at most 720 rollup buckets. Charts use fixed time buckets, show gaps, averages, and min/max context. Current UI ranges stop at seven days. The background worker must run for retention and legacy backfill.

## Validation boundaries

Automated checks cover database migrations, replay/deduplication, revocation, configuration conflicts, rollups with late samples, update health checks, and log pause/search behavior. The design plan tracks further operational acceptance. Production 500-agent mixed-load capacity, a real 30-minute network outage, and five-operator usability sessions are not implied by unit-test success. No production capacity guarantee is made by this implementation.
