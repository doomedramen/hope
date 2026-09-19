# Milestone 3 design: fingerprinting and service proposals

Spec refs: §4.3–4.4, §6.5–6.6, §8.2, §17 M3. M2 owns socket discovery,
safe protocol classification, and the canonical service/endpoint record. M3
adds explainable product evidence without replacing M2 records.

## 1. Boundary

M2 creates one `services` row and current socket endpoint for an open
device/address/port. Its `network_scan` evidence contains bounded HTTP, TLS,
SSH, or generic-TCP protocol facts. M3 consumes those facts and adds product
guesses, confidence, and reasons. It does not create a second service model.

Manual service fields and confirmed evidence win over every automatic rule.
An automatic result may enrich or revise automatic data only when its evidence
is at least as strong and is not contradicted by a confirmed user fact.

## 2. Fingerprint rule interface

Rules are pure functions over bounded normalized inputs. Each result contains:

- rule identifier and fixture version;
- protocol/product/version guess;
- confidence in `[0, 1]`;
- the exact evidence fields it used; and
- a stable explanation suitable for the UI's **Why?** view.

Rules never fetch extra URLs, authenticate, follow cross-endpoint redirects,
or inspect unbounded body data. Protocol collectors remain separately bounded.
The first set covers generic HTTP/TLS/SSH facts produced by M2, with generic
fallbacks when no signature matches.

### 2.1 Slice 1 implementation

The pure engine lives in `domain::fingerprinting`. `FingerprintInput` accepts
only normalized M2 fields and validates the same bounded shape before a rule
can read it: allowlisted HTTP headers, status, title, body sample, banner,
same-endpoint redirects, TLS ALPN, and reduced certificate metadata. The
`from_m2_evidence` adapter parses stored M2 JSON only; it does not probe or
persist anything.

`FingerprintRule` is the extension point. A rule returns a protocol, product,
and version match, confidence, exact `EvidenceField` paths, and stable reasons.
`FingerprintEngine` orders competing candidates by confidence and rule ID.
Every candidate carries `rule_id` and `fixture_version`. If no rule matches,
the engine returns `fallback.<protocol>` with a generic protocol candidate,
so unknown HTTP, TLS, SSH, and TCP services remain visible.

Built-in HTTP signatures cover AdGuard Home, Grafana, Home Assistant, Immich,
Jellyfin, Lidarr, Nextcloud, OPNsense, Pi-hole, Plex, Portainer, Proxmox VE,
Radarr, Readarr, Sonarr, TrueNAS, and UniFi. Product signatures require two
independent fields, normally title plus body or server. A single title or
body marker cannot create a product guess. OpenSSH and Dropbear use their
structured SSH implementation banner as a high-specificity exception and
still expose the exact banner field and parsed version when present.

Checked-in fixtures live under
`crates/domain/fixtures/fingerprinting/v1/`. Each JSON file has
`format_version`, normalized `input`, and `expected` candidate fields. The
fixture envelope version is independent from each rule's stamped fixture
version so fixture readers and signatures can evolve separately.

### 2.2 Slice 2 implementation

`apps/server/src/inventory/fingerprinting.rs` consumes the append-only M2
`protocol_classification` row for the canonical service and runs the pure
engine without opening another connection. It appends a `network_scan` /
`fingerprint` row containing the selected candidate, all ranked candidates,
the rule metadata, reasons, evidence fields, confidence, and the source M2
evidence ID. Repeating the same scan instance and value reuses the existing
row; a new scan instance remains a separate observation.

The reconciler projects only automatic `product`, `product_version`, and
`protocol` values onto the existing service. It takes a service advisory lock,
uses the strongest automatic support before replacing an automatic value, and
records one `service.fingerprint_changed` event for a real projection change.
Generic fallback candidates remain evidence without inventing a product.
Confirmed manual evidence protects the corresponding service field, including
manual fingerprint evidence that confirms the whole classification.

The DB gate covers repeated discoveries enriching one service, generic TCP
remaining visible, automatic projection idempotence, and protection of manual
values. Review items for conflicting or low-confidence candidates remain the
next M3 slice.

## 3. Reconciliation and review

Automatic fingerprint evidence is append-only. A reconciliation transaction
selects the best supported automatic classification for a canonical service,
updates only automatic fields, and emits one material change event. Conflicting
or low-confidence guesses create a service-review item rather than silently
overwriting a service. The item lists competing candidates and their evidence.

The current policy uses a product auto-apply threshold of `0.90`. A product
candidate at or above `0.90` may fill an empty automatic product field. A
candidate below `0.90` enters review; the engine's normal two-field product
candidate is `0.86`, so it is intentionally reviewable. Any candidate that
differs from an existing non-manual product is also a conflict and enters
review, even when its confidence is above the threshold. Competing product
candidates in one report are a conflict. These paths do not change product or
version projections. Confirmed manual service evidence suppresses both review
creation and automatic projection for its protected fields.

Review rows retain the selected candidate, all ranked candidates, M2 input,
fingerprint evidence ID, rule ID, fixture version, confidence, reason, and
threshold. Candidate identity is a stable hash of the candidate alone, so a
later scan with unchanged candidate data does not create another row. Rejecting
a row records the decision and suppresses that unchanged candidate. Confirming
a row appends confirmed manual fingerprint evidence and applies the candidate
transactionally; a later automatic result cannot overwrite it.

The API exposes `GET /api/v1/service-reviews` (pending by default, with
`status`, cursor, and limit filters), `GET /api/v1/service-reviews/{id}`, and
`POST /api/v1/service-reviews/{id}/confirm` or `/reject`. These routes use the
existing session-authenticated inventory router, including its custom-header
CSRF check. Monitor proposals and mDNS/UPnP collectors remain outside this
slice.

The review UI must show the source observations, scores, and rules. Confirming
a classification writes manual evidence; rejecting it records the decision and
prevents the same unchanged automatic candidate from repeatedly resurfacing.

## 4. Monitor proposals

Rules propose monitors from resolved service protocol/product fields and current
canonical endpoint rows, but do not execute checks. Migration
`0013_monitor_proposals.sql` stores one durable proposal per stable
`(rule_id, endpoint_id)` identity. The row retains the endpoint target
snapshot, policy rule version, resolved protocol/product, source fingerprint
evidence when available, confidence, generated check configuration, and whether
policy permits automatic creation. Historical endpoints and arbitrary
user-supplied targets cannot create proposals.

The current bounded policy emits `monitor.http.generic` for unresolved HTTP,
`monitor.https.generic` for unresolved HTTPS, `monitor.tcp.generic` for
unresolved TCP, and a stable `monitor.<protocol>.product.<slug>` rule for a
resolved HTTP/HTTPS product. Generic HTTP and TCP proposals remain
representable even when product fingerprinting finds no signature. The policy
only persists intent; M4 owns monitor creation, scheduling, and health checks.

Fingerprint reconciliation, confirmed service-review decisions, and manual
service edits refresh proposals in the same transaction. Refresh updates only
automatic policy fields. It preserves proposal status, manual decisions, and
user overrides. User override patches cannot change service, endpoint, rule,
target, or resolved identity.

The server exposes authenticated, custom-header-CSRF-protected routes:

- `GET /api/v1/monitor-proposals` (pending by default; `status`, service, and
  cursor filters) and `GET /api/v1/monitor-proposals/{id}`;
- `POST /api/v1/monitor-proposals/{id}/approve` and `/reject`;
- `PATCH /api/v1/monitor-proposals/{id}` for versioned user overrides; and
- `POST /api/v1/monitor-proposals/generate` or
  `POST /api/v1/services/{id}/monitor-proposals` for an explicit policy
  refresh.

Approval and rejection record manual decision provenance on the proposal and
write matching change and audit records. Override changes write the same
records. Repeating the same decision is idempotent; an opposite decision
returns a conflict. No route creates a monitor or runs a check.

M3 supports proposal persistence and review only. Continuous scheduling and
health execution are M4 work.

## 5. Delivery sequence

1. Pure rule engine, checked-in fixtures, and score/explanation tests.
2. Fingerprint evidence persistence and canonical service reconciliation.
3. Service-review queue and **Why?** API/UI.
4. Monitor-proposal policy, persistence, and server review API. Web review UI
   remains outside this slice.
5. Safe collector adapters for mDNS/DNS-SD and UPnP, then controlled SNMP.

## 6. Acceptance evidence

M3 is complete only when repeated discoveries enrich one service; unknown
HTTP/TCP remain visible and receive generic proposals; every fingerprint shows
evidence and confidence; and weaker scans cannot overwrite user confirmation.
