# Milestone 1 design: canonical inventory and evidence model

Spec refs: §2.1, §4, §6.6, §9, §11, §12.5, §14.1, §17 M1. Conventions: UUID PKs
(`gen_random_uuid()`, ADR-0004), `sqlx` runtime-checked queries, `jiff` for
time, Postgres `inet`/`cidr`/`tstzrange` (ADR-0004), migrations numbered
sequentially in `migrations/`.

## 1. Table schema sketch

All entity tables carry `id uuid primary key default gen_random_uuid()`,
`created_at`/`updated_at timestamptz not null default now()`, and a
`version integer not null default 1` for optimistic concurrency (§14.1) on
anything user-editable. Triggers bump `updated_at`; `version` is incremented
in the same `UPDATE` that the API issues (`SET version = version + 1 WHERE
version = $expected`), 0 rows affected ⇒ 409 Conflict.

```sql
-- Structural / location
sites(id, name, timezone, version, created_at, updated_at)

networks(id, site_id nullable fk, cidr cidr not null, vlan integer,
         gateway inet, scan_policy jsonb not null default '{}',
         name text, version, created_at, updated_at)

-- Identity spine
devices(id, device_type text check in
          ('physical_host','vm','lxc','container_host','appliance','unknown'),
        name text, status text check in ('active','stale','archived','merged'),
        identity_confidence real not null default 0,
        canonical_of uuid null references devices(id),  -- see §4 merge model
        version, created_at, updated_at)

interfaces(id, device_id fk devices, mac macaddr, description text,
           first_seen timestamptz, last_seen timestamptz,
           version, created_at, updated_at)

addresses(id, interface_id fk interfaces, ip inet not null,
          address_type text check in ('static','dhcp','unknown'),
          first_seen timestamptz not null, last_seen timestamptz not null,
          is_current boolean not null default true,
          version, created_at, updated_at)
  -- IP history: never delete a row when an IP moves; close it
  -- (is_current=false, last_seen=now()) and insert the new one.
  -- unique partial index (ip) where is_current -- at most one live owner/IP.

workloads(id, workload_type text check in ('vm','lxc','docker_container','other'),
          host_device_id fk devices,       -- containment: runs-on
          runtime_id text,                 -- e.g. docker container id, Proxmox VMID
          image_or_template text,
          name text, status text,
          version, created_at, updated_at)

services(id, name text, protocol text, product text, product_version text,
         owner_kind text check in ('device','workload'),
         owner_id uuid not null,           -- polymorphic FK, no db-level FK
         version, created_at, updated_at)

endpoints(id, service_id fk services,
          endpoint_type text check in ('socket','published_port','url','dns_name'),
          address inet, port integer, url text, dns_name text,
          first_seen timestamptz, last_seen timestamptz, is_current boolean,
          version, created_at, updated_at)

-- Graph primitives (§9)
containment_edges(id, parent_kind text, parent_id uuid,
                   child_kind text, child_id uuid,
                   relation text check in
                     ('hosts_vm','hosts_container','has_interface','runs_service'),
                   version, created_at, updated_at)
  unique (parent_kind, parent_id, child_kind, child_id, relation)

dependency_edges(id, provider_kind text, provider_id uuid,
                  consumer_kind text, consumer_id uuid,
                  dependency_kind text,           -- dns, network, storage, app-db, ...
                  criticality text check in ('hard','soft'),
                  origin text check in ('manual','inferred'),
                  confidence real, inference_rule text null,
                  health_propagation text check in ('none','propagate','suppress_only'),
                  version, created_at, updated_at)
  unique (provider_kind, provider_id, consumer_kind, consumer_id, dependency_kind)

-- Identity rules / pins (§4.2)
identity_rules(id, device_id fk devices, rule_type text check in
                 ('agent_id','machine_id','hardware_uuid','mac','serial',
                  'proxmox_vmid','docker_container_id','hostname','ssh_host_key',
                  'snmp_engine_id','tls_cert_identity'),
               value text not null, pinned boolean not null default false,
               pinned_by uuid null references users(id), pinned_at timestamptz,
               version, created_at, updated_at)
  unique (rule_type, value) where pinned  -- a pinned identifier can't be reused

-- Merge/split history (§4.2, gate: undo without losing observations)
merge_events(id, kind text check in ('merge','split','undo_merge'),
             survivor_device_id fk devices,
             absorbed_device_id fk devices,        -- device row kept as tombstone
             performed_by uuid null references users(id),  -- null = automatic
             reason text, score real, explanation jsonb,
             undone_at timestamptz, undone_by uuid null references users(id),
             created_at timestamptz not null default now())

-- Evidence (see §2 below for shape rationale)
evidence(id, subject_table text not null, subject_id uuid not null,
         source_type text check in
           ('network_scan','agent','docker','mdns','snmp','api','manual'),
         source_instance text,               -- e.g. agent id, scan job id
         attribute text not null,            -- e.g. 'hostname', 'open_port', 'image'
         value jsonb not null,
         confidence real not null,
         first_seen timestamptz not null, last_seen timestamptz not null,
         expires_at timestamptz null,
         absent boolean not null default false,   -- marks expected-but-missing
         confirmed_by uuid null references users(id),
         overridden boolean not null default false,
         version, created_at, updated_at)
  index (subject_table, subject_id)
  index (source_type, source_instance)

-- Change / audit (§11, §12.5)
change_events(id, entity_kind text, entity_id uuid,
              category text,              -- port, service, device, dependency, ...
              severity text check in ('info','notice','warning','critical'),
              before jsonb, after jsonb,
              evidence_source text, occurred_at timestamptz not null default now(),
              acknowledged boolean not null default false)
  index (entity_kind, entity_id, occurred_at desc)
  index (occurred_at desc)  -- global feed

audit_events(id, actor_user_id uuid null references users(id),
             actor_kind text check in ('operator','agent','worker','system'),
             action text not null, target_kind text, target_id uuid,
             result text check in ('success','failure'),
             detail jsonb, ip inet null,
             occurred_at timestamptz not null default now())
  index (occurred_at desc)
  index (actor_user_id, occurred_at desc)
```

Notes:
- `owner_id`/`subject_id`/edge endpoints are polymorphic (kind + uuid) rather
  than per-entity FKs — see §2 for the evidence rationale; the same argument
  applies to containment/dependency edges, which must span devices,
  workloads, services, and interfaces uniformly. Referential integrity for
  these is enforced in the service layer plus periodic consistency jobs, not
  the FK constraint system.
- `devices.canonical_of` implements the merge redirect cheaply for the common
  read path (see §4).

## 2. Evidence model

**Recommendation: one polymorphic `evidence` table**, not per-entity evidence
tables.

Rationale: §4.4 requires every inferred fact — on devices, interfaces,
addresses, services, endpoints, and (later) dependency edges — to carry the
same fields (source, confidence, first/last seen, expiry, confirmed/override).
A single table means:
- one reconciliation/expiry engine, one review-queue query, one explainability
  renderer, instead of N near-duplicate implementations;
- new subject types (dependency edges in M1, checks in M4) need no schema
  migration, only a new `subject_table` value;
- the "Why?" view (§13.4) is `select * from evidence where subject_table = $1
  and subject_id = $2 order by last_seen desc`, always.

Cost: no DB-level FK from `evidence.subject_id`, so orphan cleanup is a
worker job (`evidence_gc`) rather than `ON DELETE CASCADE`. Given devices are
never hard-deleted in v1 (archived, not dropped — §12.4 "removal does not
delete historical inventory"), this is low risk. A partial index per common
`subject_table` value keeps per-entity evidence lookups fast.

Confidence & precedence:
- Each evidence row has its own `confidence` (0–1) set by the producing
  source (deterministic sources like agent/machine-id near 1.0, weak sources
  like mDNS name guesses lower).
- The *entity's* displayed value is resolved at read time: prefer the
  highest-confidence non-absent, non-expired evidence; a row with
  `confirmed_by` set always wins regardless of confidence, and is never
  overwritten by conflicting automatic evidence unless the user re-confirms.
- Absence: when a source no longer reports a fact it previously reported
  (e.g. a port that stopped responding), the reconciler inserts a **new**
  evidence row with `absent = true` rather than deleting or mutating the
  affirmative row. The affirmative row's history is retained; the entity's
  "is this port open" projection considers the most recent row per
  `(subject_id, attribute, source_type)`. Expiry (`expires_at`) is a
  scheduled downgrade of confidence to 0 for display purposes, not a delete.

## 3. Identity matching

Scoring: weighted sum over identity-rule types matched between the
incoming observation and each candidate device, normalized to 0–1.
Suggested weights (tunable, stored in config not code):

| identifier | weight |
|---|---|
| agent_id | 1.0 (deterministic, auto-match alone) |
| hardware_uuid | 1.0 |
| machine_id | 0.9 |
| tls_cert_identity | 0.8 |
| ssh_host_key | 0.8 |
| docker_container_id + engine identity | 0.8 |
| proxmox_vmid + host identity | 0.8 |
| mac | 0.6 |
| serial | 0.6 |
| snmp_engine_id | 0.5 |
| hostname | 0.3 |
| interface/IP history overlap | 0.2 |

Thresholds: score ≥ 0.85 (or any single weight-1.0 exact match) ⇒ **automatic
match**, merged/attached with an explanation record. 0.4 ≤ score < 0.85 ⇒
**suggested match**, written to a review queue, no graph mutation. score <
0.4 ⇒ treated as a new device. A `pinned` identity_rule always short-circuits
scoring to 1.0 for that identifier and can never itself be outscored — pins
exist precisely so an operator's manual correction sticks (§4.2).

Explainability record (`merge_events.explanation` jsonb, also surfaced from
`evidence`/`identity_rules` for suggestions not yet acted on):

```json
{
  "score": 0.95,
  "matched": [
    {"rule_type": "agent_id", "weight": 1.0, "candidate_value": "...", "observed_value": "..."},
    {"rule_type": "hostname", "weight": 0.3, "candidate_value": "docker01", "observed_value": "docker01"}
  ],
  "conflicting": [],
  "threshold": 0.85,
  "decision": "auto_match"
}
```

This is the literal payload the §13.4 "Why?" view renders ("Matched to
existing device because agent ID and machine ID agree").

Review queue: a plain query (`identity_suggestions` view or table) of
pending, non-superseded suggested matches, actioned via `POST
/api/v1/identity-suggestions/{id}:confirm|reject`.

## 4. Merge / split / undo

Evidence rows are **never re-pointed or deleted** on merge — only
`subject_id` linkage from the *device* side changes, via a redirect:

- `devices.canonical_of` — on merge, the absorbed device row is kept
  (tombstoned, `status='merged'`, `canonical_of = survivor_id`); its `id` is
  never reused nor deleted, and every evidence/interface/address row that had
  `subject_id`/`device_id = absorbed.id` **stays exactly as recorded**.
- Read paths resolve through the redirect: any lookup of a merged device's id
  transparently returns the survivor's aggregate view (evidence from both
  ids), via a `resolve_device(id)` helper that follows `canonical_of` before
  querying dependents. `interfaces.device_id` is *not* rewritten to the
  survivor at merge time — this is the key to non-destructive undo.
- A `merge_events` row records `survivor_device_id`, `absorbed_device_id`,
  score/explanation, and who/what performed it.
- **Undo**: set `devices.canonical_of = null`, `status='active'` on the
  absorbed row, and `merge_events.undone_at/undone_by`. Because nothing was
  physically re-pointed, all interfaces/addresses/evidence that belonged to
  the absorbed device were never moved — undo is an O(1) metadata flip, and
  no observation is lost. This directly satisfies the M1 gate: "a merge can
  be undone without losing observations."
- **Split**: operator selects a subset of interfaces/evidence to move from
  device A to a newly created device B. Implemented as an explicit
  re-pointing of only the selected `interfaces.device_id` /
  `evidence.subject_id` rows (this *is* destructive to A's aggregate, by
  operator intent) plus a `merge_events` row with `kind='split'` for audit;
  original evidence timestamps/sources are preserved on the rows, just
  reassigned.
- Consequence: any code that queries "this device's interfaces" must call
  `resolve_device` / a canonical-id-aware view, never assume `device_id`
  equality alone covers the merged set. This is the one deliberate
  complexity cost of the non-destructive design; it is centralized in one
  repository function.

## 5. Change-event and audit framework

- Both `change_events` and `audit_events` are written **in the same
  transaction** as the state mutation that caused them (application-level,
  not DB triggers — keeps logic in Rust/testable, matches ADR-0006's
  transaction-per-job-step style). A shared `Recorder` passed through service
  functions exposes `record_change(...)`/`record_audit(...)`, called just
  before commit.
- `change_events` shape: entity_kind/id, category (port/service/device/
  dependency/agent/...), severity, `before`/`after` jsonb snapshots (narrow —
  only the changed fields, not full entity dumps), evidence_source,
  occurred_at. Noisy numeric samples (CPU%, latency) never go here — they are
  `observations` (M4 territory), not part of M1 scope, but the table
  boundary is fixed now to prevent scope creep.
- `audit_events` shape: actor (user or agent/worker/system), action string
  (`device.merge`, `credential.use`, `monitor.update`, ...), target, result,
  jsonb detail, timestamp. Append-only; no update/delete path in the API.
- Both are exposed as cursor-paginated `/api/v1/changes` (per §14.1) and an
  internal-only audit view (no public audit API in M1 — operator-only, read
  via the UI's authenticated session).

## 6. API surface and UI (M1 slice)

```text
GET/POST        /api/v1/networks
GET/POST/PATCH  /api/v1/devices
GET             /api/v1/devices/{id}                (identity, interfaces, evidence, "Why?")
POST            /api/v1/devices/{id}:merge
POST            /api/v1/devices/{id}:split
POST            /api/v1/devices/{id}:undo-merge
GET/POST/PATCH  /api/v1/interfaces, /addresses
GET/POST/PATCH  /api/v1/workloads
GET/POST/PATCH  /api/v1/services, /endpoints
GET/POST/PATCH  /api/v1/dependencies
GET             /api/v1/identity-suggestions
POST            /api/v1/identity-suggestions/{id}:confirm|:reject
GET             /api/v1/changes                      (global feed, cursor pagination)
GET             /api/v1/evidence?subject_table=&subject_id=
```

All list endpoints: cursor pagination (`?cursor=&limit=`), `version` field on
mutable resources, `PATCH` requires `If-Match`-style `version` in body ⇒ 409
on mismatch (§14.1).

UI (minimal M1 set, under **Infrastructure**):
- Device list + device detail page (identity/confidence, interfaces/IP
  history, evidence "Why?" panel, merge/split/undo actions) — §13.3 subset.
- Service/endpoint list + detail.
- Manual create/edit forms for device, interface, address, workload, service,
  endpoint, dependency edge.
- Identity review queue (suggested matches, confirm/reject).
- Global change feed (under **Changes**), filterable by category/severity.

## 7. Acceptance-gate mapping

| Gate | Design element | Test |
|---|---|---|
| Device can change IP without duplicate | `addresses` history table, `is_current` flip instead of row mutation; identity matching keyed off non-IP identifiers first | Integration: agent reports same `machine_id` with new IP → same device id, old address row closed, new one current |
| Strong evidence reconciles two observations to one device | Identity scoring ≥0.85 auto-match path | Integration: two evidence submissions sharing `agent_id` → one `devices` row, `merge_events` or direct-attach recorded |
| Ambiguous evidence → review suggestion, not silent merge | Score 0.4–0.85 branch writes to suggestion queue only | Integration: hostname-only match (weight 0.3) never triggers auto-merge; appears in `GET /identity-suggestions` |
| Merge undoable without losing observations | `canonical_of` redirect, non-destructive merge (§4) | Integration: merge two devices, undo, assert every original `evidence`/`interfaces` row unchanged (row count and content) |
| Manual facts survive expiry of automatic evidence | `confirmed_by` precedence in read-resolution; expiry only affects automatic rows | Integration: confirm a hostname manually, let conflicting automatic evidence expire/mark absent, assert displayed value still the confirmed one |

## 8. Decisions

Operator-approved; all as recommended above, with the specifics below.

1. **Evidence/edge subject modelling: polymorphic.** One `evidence` table
   (`subject_table` + `subject_id`) and polymorphic containment/dependency
   edges, not per-entity FK tables — one evidence engine, one explainability
   renderer, no migration needed for future subject kinds.

2. **Auto-merge threshold.** Auto-merge on a weight-1.0 deterministic
   identifier (agent_id, hardware_uuid) alone, or combined score ≥0.85 from
   ≥2 independent identifier types. Score 0.4–0.85 always goes to the review
   queue. MAC address alone never drives an auto-merge, regardless of score,
   since it's the weakest/most spoofable signal in the table (§3).

3. **Split granularity: per-interface.** Split moves whole interfaces (with
   their addresses and evidence) to the new device row; not per-evidence-row,
   not deferred past M1.

4. **Dependency-edge provenance: reuse `evidence`.** Dependency edges get
   evidence rows (`subject_table = 'dependency_edges'`) instead of a bespoke
   provenance table, so §9.2's inferred-edge provenance uses the same
   explainability machinery as device identity.

5. **`change_events` retention: plain table + scheduled bounded-batch delete
   job.** No partitioning in M1. A worker job (registered in the existing job
   handler registry, spec §5.1/ADR-0006) runs on a recurring schedule and
   deletes rows older than a configurable retention window (default 1 year)
   in bounded batches (`DELETE ... WHERE occurred_at < $cutoff LIMIT n`, via
   `ctid`/subquery batching) to avoid long-running transactions or table
   locks. Revisit partitioning once real volume from M2+ scanning is known.

6. **Optimistic concurrency: `version` everywhere, 409 enforced on
   user-edited tables.** Every entity table (including evidence-adjacent
   ones) carries `version` for schema uniformity and to avoid a later ALTER
   sweep. The API layer enforces the version-match/409 check only on tables
   the spec identifies as user-edited (devices, workloads, services,
   endpoints, dependency_edges); evidence/change/audit rows are append-only
   and never go through the conflict path.

7. **Confidence resolution: computed at read time.** `devices
   .identity_confidence`/service confidence are not materialized columns
   updated by triggers; they're computed from `evidence` at query time,
   avoiding a second write path that can drift. Revisit materialization
   (trigger or background job) only if read-time aggregation proves too slow
   once evidence volume is high post-M2.

## 9. Suggested implementation slices

1. Migration: core entity tables (sites, networks, devices, interfaces,
   addresses, workloads, services, endpoints) with `version` columns.
2. Migration: `evidence`, `identity_rules`, `merge_events`.
3. Migration: `containment_edges`, `dependency_edges`.
4. Migration: `change_events`, `audit_events`.
5. `crates/domain`: Rust types + `sqlx` query layer for entities above
   (repository pattern, one module per entity family).
6. Identity scoring service (pure function over evidence + identity_rules,
   unit-testable without DB) + review-queue persistence.
7. Merge/split/undo service functions (`resolve_device`, transactional
   merge/undo/split) with the `Recorder` wired for change/audit events.
8. `/api/v1` handlers (CRUD + merge/split/undo/confirm-suggestion) with
   cursor pagination and version-conflict handling.
9. Minimal UI: device list/detail, identity review queue, global change
   feed, manual create/edit forms.
10. `change_events` retention job: register in the worker job handler
    registry, recurring schedule, bounded-batch delete, configurable window
    (default 1y).
11. Seed/demo topology fixture + integration tests mapped to §7's gate table.
