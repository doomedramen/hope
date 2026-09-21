# Hope UX: keep the object in focus

Status: revised proposal, informed by visual reference inspection on 21 September 2026. No application changes. This replaces the earlier navigation-focused proposal.

## The underlying problem

Hope asks users to interpret its internal model before they can use it: devices, agents, interfaces, evidence, identity confidence, service reviews, proposals, monitors, and incidents. Moving those concepts behind fewer navigation links does not solve that problem.

Start with things the user recognizes: a server, router, service, or planned maintenance window. Supporting concepts should appear when they answer a question about that thing. There is no tutorial, setup checklist, wizard, or beginner mode.

## What the reference images actually show

These are publisher-provided images, inspected visually. They demonstrate structure, not every behavior of the latest releases. Beszel's dashboard image contains agent version 0.8.0; Kuma's example contains historical dates. Kuma's temporary demo opened at database setup, so no authenticated live interaction is claimed.

| Reference | Visible observation | Implication for Hope |
| --- | --- | --- |
| [Beszel systems](https://www.beszel.dev/image/home-dashboard.png) | One table dominates. Names, CPU, memory, disk, network, and agent information share consistent rows. Filter and Columns sit above it. Add System is distinct. | Useful density can be simple. Repeated structure is easier to scan than unrelated panels. Give the list the space. |
| [Beszel system details](https://www.beszel.dev/image/home-system.png) | A named system and compact metadata strip frame CPU, memory, Docker, and disk charts. A time selector sits at the top. | Rich detail becomes appropriate after the user selects its subject. Charts need a clear scope. |
| [Beszel alerts](https://www.beszel.dev/image/home-alerts.webp) | An Alerts dialog overlays the system table. System/all-system scope is explicit. Repeated rules pair switches with thresholds and durations. | Configuration belongs beside the affected object. Keep context and scope visible; repeat a simple form pattern. |
| [Kuma monitor details](https://user-images.githubusercontent.com/1336778/212262296-e6205815-ad62-488c-83ec-a5b0d0689f7c.jpg) | A searchable monitor list stays on the left. The selected target, status, heartbeat strip, measurements, and history occupy the right. Pause and Edit sit beside the target. | The list is navigation; the detail area answers questions about one selection. Investigating does not require losing the list. |
| [Kuma settings](https://louislam.net/uptimekuma/2.jpg) | General/account controls and notification destinations occupy a settings screen, away from live status. | Configuration complexity need not be visible during observation. |

Neither product contains little information. Each keeps information within recognizable structures. Users understand the subject before interpreting values.

Do not copy everything. Small icons, dense charts, and strongly emphasized destructive controls in the examples are not necessary to reproduce. Preserve accessible labels and deliberate destructive actions.

## A concrete diagnosis of Hope

`routes/infrastructure.tsx` already has a list/detail layout. But its detail area leads with Edit, Deploy agent, Merge, Split, identity confidence, interfaces/IP history, and evidence. The list repeats internal IDs and record versions. Four metric cards and an identity review queue compete with the inventory.

The first detail is effectively an inventory diagnostic view. A user selecting their NAS probably wants its services and current condition before identity reconciliation. Merely reducing columns would miss the problem.

`components/overview/OverviewDashboard.tsx` combines device counts, health, reviews, changes, charts, topology, and attention. Each section introduces another question. Suggestions also resemble operational problems.

`routes/monitoring.tsx` separates monitors, incidents, service reviews, and proposals into top-level tabs. Investigating one service requires knowing which internal category contains the next answer.

`components/AgentsPage.tsx` repeats device inventory from a technical perspective, with another set of summary cards and details. Agents need not be a second everyday representation of the same computer.

These Hope findings are based on code inspection, not a live usability test.

## Three levels of information

| Level | Question | Visible | Deferred |
| --- | --- | --- | --- |
| Scan | Which thing needs me? | Names, useful addresses, current condition, compact recent history where supported | Settings, evidence, logs, internal IDs |
| Inspect | What is happening to this thing? | Current result, meaningful history, related services, relevant incident, freshness | Full configuration and raw diagnostics |
| Change or diagnose | What should I change, or why did Hope decide this? | Scoped settings, validation, evidence, detailed logs | Unrelated objects and global configuration |

The third level has named entry points: Check settings, Notifications, Network interfaces, Agent details, Identity history. It is not one generic Advanced destination. Users should be able to predict where information lives.

## Remove the redundant dashboard

The previous proposal retained Home plus Devices. The stronger recommendation is to land directly on **Devices**, with **Monitoring** and **Maintenance** as the other primary views. Home is not justified if it repeats fragments from those pages.

Use compact desktop top navigation. A permanent feature sidebar plus another object sidebar would consume space without helping orientation. Keep Settings and Activity secondary in the header. Devices exposes visible Networks and Agents links for administration.

Keep navigation stable between empty and populated installations. Preserve bookmarked routes with redirects or contextual destinations. This is an information architecture proposal, not a requirement to remove every technical route.

## Devices: the default working surface

Start with a full-width searchable list. No detail placeholder consumes half the screen. No device is selected merely because it is first in an API response.

Illustrative layout, with fictional data:

```text
Hope      Devices   Monitoring   Maintenance           Activity  Settings

Devices                                                   [Add device]
Search devices…           All  Needs attention          Networks  Agents

Device                       Contact             Monitored services
nas       192.168.1.20        Seen 12s ago         1 failing · 3 passing
mini-pc   192.168.1.30        Seen 18s ago         6 passing
router    192.168.1.1         Seen 2h ago          No checks
```

Contact reports the freshness/source of observations; service results describe checks. Do not call a device healthy solely because it exists in inventory or its agent reports in.

Place one address beneath the name instead of an ID. Additional addresses live in Network interfaces. Occasional states such as Maintenance or Identity needs review appear on the affected row, without permanent columns for every exception.

Keep ordering stable, with a visible Needs attention filter. Live updates must not make rows jump while someone is selecting one. A compact exceptional-state count can act as a filter; four permanent summary cards are unnecessary.

Offer Columns for optional Type, Network, Last contact, and supported host metrics. Persist preferences. A simple default should not force experts into repeated detail visits or require customization before use.

### Selecting a device

On a wide screen, selection turns the list into a compact navigator and opens a substantial detail pane. Keep name, contact, and service condition in the navigator; do not squeeze the complete table into a narrow column.

The detail begins with:

1. **Identity:** name, current address, observation freshness/source, and a restrained Edit action.
2. **Current exception, if any:** a service failure or missing agent contact. Report observations without inventing root causes.
3. **Services:** names, current checks, recent results, and direct actions.
4. **Resources, when available:** compact host measurements; richer history on request. No empty CPU charts for an agentless router.

Use Overview, Activity, and Details as named sections or tabs. Details groups Network interfaces, Agent details, and Identity history. Merge and Split belong inside Identity history, not beside everyday actions.

An identity conflict creates a contextual Review identity link near the name. Open the evidence needed for that decision. Retain a secondary fleet review view for resolving many conflicts.

On mobile, selection opens a full detail page with a Devices back link. Preserve search, filters, scroll position, and selection. Support deep links and browser Back on desktop too.

## Monitoring: inspect one target without losing the list

Use Kuma's list/detail structure. The list contains a name, text status, and compact recent history supported by actual results. Defer interval, next-run timestamp, UUIDs, and consecutive-failure counters.

The selected monitor reads from top to bottom:

```text
Jellyfin                          [Pause] [Check settings] [More]
nas · http://192.168.1.20:8096

Not responding
Last check 30 seconds ago · Connection timed out

Recent checks       | | | | | | | | |    [time range]
Response time history

Current incident / recent state changes
Notifications: Not configured                          [Configure]
```

This fictional example illustrates hierarchy, not implemented pause/edit contracts. Only ship actions backed by working APIs. Show uptime and latency summaries only with sufficient data; do not fill metric tiles for symmetry.

An incident is part of its monitor's story. Inspect it without switching to a global tab. Retain secondary cross-monitor incident history for broader investigation.

Keep common actions labeled. Put deletion in More with appropriate confirmation. The screenshot's prominent Delete action is not a pattern to copy.

### Suggested checks

Show a restrained Suggested checks (3) link above the list when applicable, not a permanent fourth tab and dashboard task queue. The review view shows a target, proposed check, and Start monitoring. Check settings remain available beside each proposal.

On a device, place its suggestions in Services. Global and contextual views operate on the same proposals. Preserve explicit approval and access to classification evidence. A proposal is neither a failure nor an active monitor.

After approval, keep the target visible as Waiting for first result. On partial bulk failure, retain successful work and identify failed targets. Starting monitoring does not imply a passing result.

## Forms: expose decisions when they matter

Do not collapse everything technical. A field belongs in the initial form when omitting it could change the intended target or outcome. Less common tuning can be disclosed later.

| Action | Initially visible | Reveal on request | Completion |
| --- | --- | --- | --- |
| Install agent | Target platform, command, server address summary | Change address; troubleshooting | Intended device connected, or waiting/error |
| Scan network | Range, exclusions, scan profile, explicit launch confirmation | Scheduling and pressure details | Progress and results, including zero results |
| Configure check | Name, type, target, applicable required fields | Timing, thresholds, assertions, protocol options | Saved settings distinct from the next result |
| Notifications | Object/scope, destination, relevant events | Delivery-specific options | Saved configuration; delivery test if supported |
| Maintenance | Target, start, duration | Recurrence and dependencies | Confirmed schedule or actionable conflict |

Prefer a collapsed summary such as “Every 60 seconds · timeout 10 seconds” over an unexplained Advanced label. Show actual effective values and overrides. Scope stays visible for multi-target or global changes.

Use dialogs for bounded changes, detail pages for sustained reading, and inline disclosure for a few optional fields. Do not nest settings dialogs. Restore focus after closing.

Add device must distinguish installing an agent, scanning a network, and creating a manual record. A manual record does not promise live data. Preserve it as a secondary option. SSH installation belongs to the selected device where prerequisites are known.

Direct URL monitoring remains a useful later capability. The proposal-based creation path cannot merely be relabeled; target creation and validation need a backend contract.

## Visual discipline and exceptional states

- Use aligned rows and quiet separators. Avoid a card for every number and heading.
- Reserve emphasis for the main action and meaningful status. Suggestions must not look like alarms.
- Use consistent row heights, value alignment, and text hierarchy. Repetition reduces scanning effort without deleting useful information.
- Scope charts to their subjects. Compact histories can remain in lists when useful; large charts belong in details.
- Omit empty optional sections. Explain missing data where it is expected, rather than displaying walls of No data cards.
- Keep essential controls labeled and keyboard accessible. Nothing essential depends on hover or color alone.
- Keep errors and retries local, retain inputs, and preserve successful partial work.

An API failure is not an empty estate. Stale observation is not proof of offline status. Agent contact does not prove service availability. An approved check without results is not healthy.

For a failed service, show the concrete result and check time before technical logs. For maintenance, explain expected effects near the device or service while keeping actual observations accessible. Notification suppression must not turn a failure into a healthy badge.

Refresh quietly without moving focus or announcing every heartbeat. A partial error affects its section, not the entire detail view.

## Where advanced work remains available

| Capability | Contextual entry | Broader access |
| --- | --- | --- |
| Agent inventory and connection | Device > Details > Agent | Devices > Agents |
| Interfaces and address history | Device > Details > Network interfaces | Devices > Networks |
| Evidence, merge, split | Device > Details > Identity history | Identity review link when relevant |
| Service classification | Affected service > Details | Suggested checks/review view |
| Check scheduling and thresholds | Monitor > Check settings | Bulk controls when selected |
| Incident history | Monitor > Activity | Monitoring > incident history |
| Change events | Object > Activity | Global Activity |
| Recurrence and dependencies | Maintenance editor > named options | Maintenance view |

These are discoverability requirements. Removing a navigation item is incomplete unless its replacement path exists and is understandable.

## Implementation implications and delivery

Keep backend entities intact while composing the UI around devices and services. Do not silently resolve uncertain identities to make the interface look simpler.

Validate associations before showing combined rows. Ambiguous or unassociated agents remain visible in Agents and identity review. Multi-endpoint services need a drill-down rather than an ambiguous single healthy badge.

Check contracts for fleet summaries, canonical associations, enrollment-to-agent correlation, monitor changes, histories, and pagination. Never derive estate totals from a bounded first page or a small sample.

Deliver a coherent Devices list and detail experience first, then Monitoring, then their forms. Reducing sidebar items alone would ship the least important part. If adopted, replace the dense hierarchy in ui-overview.md while retaining its shared tokens and accessibility guidance.

## Validation

Review the normal list, selected device, failed service, and settings dialog in sequence. Include mobile, empty, and unavailable states. At every level, the user should recognize the subject, current state, and relevant next action.

Ask new users to find a failing service and explain what happened. Ask experienced users to change a threshold, inspect an interface, and resolve an identity conflict. Compare wrong destinations, backtracking, completion, and requests for help with the current UI.

Do not measure simplicity by fewest clicks alone. One deliberate click into named details can be easier than reading ten unrelated sections. Each level should answer its question without requiring knowledge of the next.

Research note: the UI/UX skill's local search did not return relevant progressive-disclosure guidance. These recommendations use the inspected images and Hope's code, not an asserted database match.
