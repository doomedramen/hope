# Hope web UI direction

**Status:** Approved and implementation-ready
**Scope:** The authenticated web console and its primary Devices and Monitoring workflows
**Stack:** React, TanStack Router, Tailwind CSS v4, shadcn components, Lucide icons
**Related decision record:** [simple-ux.md](./simple-ux.md)

Hope is a homelab operations console. Its interface starts with recognizable
objects and reveals the inventory and reconciliation machinery when it helps
someone answer a question about that object. The default screen is the device
list. A separate overview dashboard is not part of the working navigation.

## Shell and navigation

The root route redirects to `/devices`. The authenticated shell uses a compact
desktop header with three primary destinations:

- Devices
- Monitoring
- Maintenance

Activity (`/changes`) and Settings remain secondary destinations in the same
header. Networks and Agents are administration tools exposed from the Devices
page and their own routes. They are not competing top-level work surfaces on
every screen. Existing bookmarks continue to work, including `/infrastructure`
as an inventory-compatible route and object links with search parameters.

The header keeps the main navigation semantic and keyboard reachable. On small
screens the primary navigation occupies its own full-width row, while the
secondary links remain available in the first row. Route changes focus the main
landmark. A skip link, visible focus rings, labeled controls, and reduced-motion
CSS are required.

## Visual language

Hope uses a cool off-white canvas, white content surfaces, and quiet neutral
separators. Geist remains the only typeface. The layout is dense enough for
repeated scanning but does not turn every value into a card.

| Token group                    | Use                                               |
| ------------------------------ | ------------------------------------------------- |
| Neutral canvas and card tokens | Page background, surfaces, text, borders, inputs  |
| `status-healthy`               | Passing, recovered, successful observations       |
| `status-attention`             | Degraded, stale, pending, or review-needed states |
| `status-critical`              | Down, failed, or critical observations            |
| `status-info`                  | Discovered or informational state                 |
| `status-neutral`               | Unknown, disabled, or unavailable state           |

Status always has text or another non-color cue. A normal device record is not
called healthy merely because it exists. Agent contact is not service health,
and stale observation is not proof that a device is offline.

Use consistent row heights, left alignment, and an 8px spacing rhythm. Use
Lucide vector icons with `aria-hidden="true"` beside visible labels. Standalone
icon buttons need accessible names. Keep normal text at least 4.5:1 contrast.

## Devices

`/devices` is the default working surface. It opens with a full-width,
searchable list and no automatically selected device. The list gives each row
the information needed for a decision:

```text
Device                         Address / record       Services       Condition
nas                            192.168.1.20          1 needs review  Warning
mini-pc                        192.168.1.30          3 services      Active
router                         Record updated 2m ago No observations Unknown
```

Addresses come from canonical interface and address associations. If that
source is loading or unavailable, say so locally; do not manufacture an
address from a service endpoint. A record timestamp is labeled as a record
timestamp, never as live contact.

The Devices page keeps Add device available for manual inventory creation. The
action clearly names the result: a manual record does not promise agent data or
a passing service result. Networks and Agents links sit beside this action so
the supporting views remain discoverable without becoming permanent dashboard
panels.

Selecting a row adds `device=<id>` to the URL and changes the layout. Wide
screens retain the list as a compact navigator beside the detail. At tablet and
mobile widths, the list is replaced by the detail with a Devices back control.
Search and selection are represented in validated route search state so browser
Back and bookmarked object views work.

The detail header shows the device name, type, current address when available,
record freshness, and current condition. Its main actions are Edit and a
contextual More actions menu for agent deployment and a confirmed full port
scan. The full scan keeps its existing explicit scope, progress, cancellation,
and error handling.

The detail has three named sections:

- **Overview** shows current exceptions and services. Each service is joined to
  monitors by canonical service ID. Multiple checks remain visible; a single
  check must not hide another endpoint's failure. Missing checks read “No active
  check,” while unavailable check data reports the local query problem.
- **Activity** shows the latest monitor state and result time for the selected
  device. Links preserve the monitor ID when opening Monitoring.
- **Details** contains network interfaces and address history, identity
  confidence, evidence, and merge/split/restore actions. Evidence is available
  for identity decisions but does not compete with the service condition in the
  default view.

The identity review queue remains available below the list as a collapsed,
labeled disclosure with a pending count. It opens only when the operator asks
for it. Confirm/reject actions retain their existing server-side review and
error boundaries.

## Monitoring

Monitoring uses a persistent list/detail structure. The selected monitor stays
in context while its state, target, recent results, and incident history are
read. Monitor-specific actions must use real API contracts; the UI must not
invent pause, edit, delete, direct URL creation, uptime, latency, or “healthy”
claims when the data is absent.

Suggested checks, service classification reviews, and monitor proposals remain
secondary views with explicit approval. A proposal is not an incident or an
active passing monitor. After approval, show the target as waiting for its first
result. Partial bulk work keeps successful decisions visible and names failed
targets.

## Discovery and forms

Discovery keeps its safe scope boundary visible. A network scan distinguishes
the configured range, exclusions, scan profile, and the explicit confirmation
before launch. Target and probe arithmetic stays server-side rather than adding
an extra calculation step to the page. Advanced concurrency, scheduling, and
pressure options are disclosed beside the summary that explains their current
effective values. Scan progress, zero-result runs, cancellation, and technical
logs remain available in the selected network context.

Forms use progressive disclosure without hiding decisions that change target or
scope. Dialogs are for bounded changes; sustained reading belongs in a detail
view. Do not nest settings dialogs. Preserve entered values after local or API
errors and return focus when a dialog closes.

## Required states and validation

Every object view covers loading, empty, filtered-empty, unavailable, selected,
success, and recovery states. A failed section does not blank the whole page.
Refreshes do not move focus or announce every poll.

Before release, review at 375px, tablet width, and desktop width; use keyboard
only for selection and dialogs; test reduced motion; and verify browser Back on
`/devices?device=<id>`, `/devices?q=<term>`, and
`/monitoring?monitor=<id>`. Confirm that identity evidence and discovery
approval boundaries remain visible when advanced disclosures are opened.
