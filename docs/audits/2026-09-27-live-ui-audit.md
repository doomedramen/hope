# Live application audit — 27 September 2026

## Scope and method

Read-only inspection of Devices, device details, Networks, all six enrolled agents, agent inventory/metrics/logs/settings, Monitoring, Maintenance, Activity, and notification Settings. The audit inspected the deployed application and cross-checked selected findings against the current source. No scans, approvals, installations, configuration saves, or record changes were performed on the live server.

Representative desktop (1440 × 1000) and mobile (390 × 844) views were inspected. Local fixes were checked with synthetic data, including a long hostname and missing inventory. This is a page and data audit, not a validation of write workflows, notification delivery, update execution, or maintenance conflict handling. Production payloads, hostnames, addresses, identifiers, and credentials are intentionally omitted from this report.

## Highest-priority unresolved findings

### 1. Discovery repeatedly fails before completing the network — P1

The Networks page shows seven failed jobs, including initial discovery and six subsequent daily scans. Each reports 196 of 254 targets. The latest scan reports 7,644 of 9,906 ports, five exhausted attempts, and `invalid persisted M2 protocol classification`. Its activity panel has no recorded activity events.

Impact: discovery cannot establish a complete current inventory. Repeated retries do not repair the failing input. The UI correctly says partial results are not authoritative, but this failure is only visible on Networks.

Next investigation: obtain the full server error chain and a sanitized failing classification, then replay it through `FingerprintInput::from_m2_evidence`. The current UI exposes only the outer error context. The exact rejected field and a safe persistence/retry strategy remain unproven; no speculative parser change was made.

### 2. Two host agents have metrics but no inventory or device link — P1

Both hosts are online, have current CPU/memory readings, and advertise inventory capabilities. Their detail pages show zero interfaces, filesystems, processes, sockets, containers, and evidence; reconciliation is unmatched. Neither has an agent-linked entry in Devices. Guest discovery therefore cannot be assessed through these host records.

A third agent has inventory reconciled more than an hour ago while its metrics remain current. Other agents show newer inventory activity. A healthy transport and current metrics do not prove inventory delivery.

Next investigation: correlate agent inventory collection diagnostics, pending snapshot state, server acknowledgements/rejections, and the current snapshot row. Check the Proxmox export timer/cache only after establishing that host inventory reaches the server. Do not assume that reinstalling or deleting pending inventory will fix this.

The misleading healthy overview is fixed in this change; the missing production inventory itself is not fixed.

### 3. All four agent-linked devices lack addresses — P1

The device list contains 35 records, including four agent-linked records with `No address reported`. Separate address-based records remain, including a known address belonging to an agent host. The inspected agent inventory has interface names/MACs and routes but no interface addresses. All four non-Proxmox agent overviews report limited network and Docker inventory coverage.

Impact: duplicate or disconnected records, missing host identity/address context, and incomplete container inventory. On the inspected container host, process inventory contains Docker processes while container inventory is empty. Collector failures, stale data, and reconciliation must be distinguished before any merge.

Next investigation: capture the effective running service sandbox/PATH and current collector errors, compare a fresh snapshot with the persisted inventory, and verify the server/web deployment includes the existing device identity fixes. Do not merge based on hostname or IP alone.

## Other confirmed findings

| Finding | Evidence and impact | Status |
| --- | --- | --- |
| Missing/stale/partial inventory looks healthy | Overview says “Data arriving normally” when the host has no inventory, old inventory, or failed collectors. | Fixed locally: the main condition reflects absent, delayed, or partial inventory. Inventory age uses collection time, so replay does not make old data fresh. |
| “View reasons” does not reveal reasons | It switches to Settings, leaving the relevant disclosure closed below log/update configuration. | Fixed locally: an inline disclosure shows each collector's actual reason. |
| Evidence source and time are missing | Agent evidence says `from Unknown` and `—`; server rows use `source_type` and `last_seen`, while the view expects `source` and `observed_at`. | Fixed at the client API boundary, including confirmation state. |
| Missing values produce misleading inventory cells | Socket owners show `pid undefined`; inode values render a bare slash; process CPU cells are blank; unknown filesystem write status is displayed as `Writable`. | Fixed locally: missing ownership is normalized, absent readings show dashes, and unknown write status stays unknown. |
| Charts default to idle virtual devices | A host with active network/disk readings opens the network chart on `lo` and disk chart on `loop0`, producing near-empty charts. | Fixed locally: prefer a non-loopback interface and a non-loop/RAM disk when present; preserve explicit user choices. |
| Suggested monitoring work is hidden | There are zero active monitors, 38 pending checks, and one classification review. Initial Monitoring view conceals counts behind disclosures. | Open. Show pending counts and a direct review action above the empty monitor list. |
| Proposals lack recognizable targets | Proposal rows repeat labels such as “Http Http” plus short IDs. Target address/port appears only in the detail below the list. | Open. Put device name and endpoint in each row and ahead of technical metadata. |
| Device services cannot be distinguished | The inspected device shows six “Unnamed service” rows, only a protocol and “No active check”; no address/port appears. | Open. Display endpoint and product/name fallback; link directly to relevant suggested checks. |
| Interface inventory accumulates noise | Device details show historical/repeated virtual interfaces and a `bonding_masters` pseudo-entry. Identity details also list an all-zero MAC and many transient MACs. | Open. Exclude non-interface sysfs entries; distinguish current versus historical interfaces and review identity-rule eligibility. Do not delete existing identities without reconciliation evidence. |
| IPv6 sockets are not decoded | Agent socket details display 32-character `/proc/net/tcp6` hex strings instead of IPv6 addresses, and raw state codes such as `0A`/`07`. | Open. Decode addresses and state names in the collector with fixture coverage. |
| Activity is noisy and links lose context | Routine inventory arrivals dominate the latest 100 events with “Complete updated · Device Id updated · Sequence updated”. Agent links go to the fleet, not the affected agent. | Open. Retain audit evidence but summarize actual changes and deep-link to the subject. The page title “Changes” also differs from navigation “Activity”. |
| Default log configuration looks stuck | Empty, never-enabled log configuration shows “Configuration 0: Waiting for agent acknowledgement”. | Open. Distinguish disabled/default settings from a pending requested revision. |
| Update status mixes success and a blocker | An up-to-date agent also displays “Outside selected rollout percentage”. | Open. Show rollout restrictions when they affect an available update, or clearly label future policy. |
| Generic partial telemetry warning is overbroad | Metric pages with working CPU/memory/network/disk/pressure also warn “Partial telemetry”; GPU is the only unavailable collector shown. | Open UX issue. Name missing collectors and distinguish unsupported optional hardware from failed collection. No evidence establishes that these machines should provide GPU telemetry. |

## Expected empty states and working behavior

- Zero active monitors explains unknown service/device health. An online agent is not an independent service check. Do not label all devices healthy solely because an agent is connected.
- Host/container logs are explicitly not enabled. Their absence is expected until sources are selected.
- Maintenance has no events; its empty state and creation action are clear. Conflict and lifecycle workflows were not exercised.
- No notification channels or routes are configured. The empty state explains the next step; delivery was not tested.
- Current metric graphs contain real samples and valid timestamps. The earlier “Invalid date” symptom was not reproduced.
- All six agents were online and showed version 0.0.98 by the end of inspection. No browser console errors were observed during the inspected navigation.
- Desktop and mobile layouts remain readable. The larger UX problems concern missing context and hidden exceptions rather than horizontal overflow.

## Validation of this change

The evidence mapping and overview health regressions were first reproduced by failing tests. Targeted tests cover persisted evidence field names, omitted socket ownership, missing/stale/partial inventory, chart defaults, unknown inode counts, and unknown filesystem permissions. The local overview was inspected at desktop and mobile widths with synthetic data. Missing-inventory status and its diagnostic action fit in the first viewport without horizontal overflow.

These changes improve presentation and diagnosis. They do not demonstrate recovery of production discovery, inventory ingestion, device reconciliation, or Proxmox guest discovery. Those remain the first operational follow-ups.

## Follow-up diagnosis after deployment

The updated UI correctly exposes delayed inventory and the inline collector reasons. The missing-inventory warning was also verified on an affected host after the new client loaded.

### Confirmed inventory validation failure

An operator inspected the queued snapshot on an affected Debian host and found 127 empty process command fields. The Linux collector can legitimately emit an empty command string, including for kernel threads. Server inventory validation incorrectly required every JSON string value to be nonempty, rejecting the complete snapshot before ingestion. This explains a concrete blocker on that host while independent metric delivery continues.

The server now permits empty JSON string values while retaining nonempty keys, envelope identity validation, maximum string length, node count, and depth limits. A regression reproduced rejection before the fix and passes after it. This server change allows existing queued snapshots to pass this validation; agents already retry pending inventory automatically. Production recovery still needs verification after deploying this follow-up server change. Do not delete pending snapshots or reenroll agents.

The empty journal output did not mean the agent was absent: subsequent process/service inspection confirmed the host service was running. Guest agent processes can also be visible from the host. Inventory protocol rejection details are not currently surfaced by the agent's diagnostic handler, which explains the limited information available in the UI; broader diagnostic reporting remains open.

### Reproduced discovery input mismatch

A local HTTP probe demonstrated that M2 stores an empty `Server` header, but M3 rejects it with `header_value is invalid: must not be empty`. M3 now ignores empty header values when adapting stored M2 evidence, preserving other signals and the existing input bounds. This also handles previously persisted evidence without rewriting it. Scanner-to-fingerprinting and domain tests cover the case.

The fingerprinting error now includes the specific validation failure instead of only the outer context. The production scan's exact rejected field remains unverified; the empty-header mismatch is a reproduced defect, not proof that every observed scan failure has this cause. Run a fresh standard scan after deployment and inspect the more specific error if it still fails. Historical failed runs will remain failed.
