# Simple by default, capable when needed

Status: UX proposal for review. No application changes are included.

## Product direction

Hope should be understandable through its everyday interface. There should be no onboarding wizard, tutorial, setup checklist, or separate beginner mode. First-time and experienced users should use the same screens.

The default view shows recognizable things, their status, and the actions people use most. Technical detail and uncommon controls remain available on the relevant object. Simplicity comes from fewer concepts and better hierarchy, not from removing capabilities or adding explanatory copy.

## Current friction

Code inspection identifies these obstacles:

- `routes/__root.tsx` presents eight equally prominent destinations. Users must understand the relationship between devices, networks, agents, and monitoring themselves.
- `components/overview/OverviewDashboard.tsx` leads with counts and analytical panels before users have useful data. It also mixes setup suggestions with operational attention.
- `routes/monitoring.tsx` exposes four internal categories: monitors, incidents, service reviews, and monitor proposals.
- `components/MonitorList.tsx` has an empty state with no action. Its normal table includes internal IDs, scheduling details, and failure counters alongside the target and its health.
- `components/AgentsPage.tsx` makes agent administration a separate destination from devices and ends the install dialog with “Done” without verifying connection.
- `components/DiscoveryScopeSetup.tsx` exposes scan configuration and calculation mechanics before a scan can start.

These findings come from code inspection, not a completed browser audit or user test.

## Reference principles

[Beszel](https://www.beszel.dev/guide/getting-started) keeps adding a system and its install instructions together, then presents the result in a systems table. Borrow that close relationship between action and result.

[Uptime Kuma](https://github.com/louislam/uptime-kuma) centers its product on monitors and their status. Borrow the focus on recognizable targets and readable health.

For Hope, the proposed equivalent is: **see devices, see what is monitored, act on a problem**. Retain the current typography, shared components, and semantic colors. Change information hierarchy before adding visual styling.

## Navigation

Keep four stable primary destinations:

| Destination | Purpose | Secondary access |
| --- | --- | --- |
| Home | What needs attention, and what is being watched? | Recent activity |
| Devices | Computers, appliances, and their details | Networks, agent management |
| Monitoring | Checks and their current results | Suggested checks, incident history |
| Maintenance | Planned work and affected devices | Recurrence, dependencies, conflicts |

Keep Settings at the bottom of the sidebar. Networks and agent management have labeled links on Devices; the agent belonging to a device is accessible from that device's details. Preserve existing route URLs and direct links.

Do not put all advanced features in a generic “Advanced” destination. Network settings belong to networks; check thresholds belong to monitors. Secondary destinations should remain visible links, not hidden gestures or hover-only menus.

## Home

Use a calm status page with a compact health summary and a useful list. A typical populated view could look like this; names and values below are illustrative:

```text
Home                                      [Add device]

1 service down · 11 checks passing · 2 waiting

Needs attention
Jellyfin       Not responding       Checked 30s ago  [View]

Devices                                   [View all]
nas            Connected           Last seen 12s ago
mini-pc        Connected           Last seen 18s ago

3 suggested checks                        [Review]
Recent activity                           [View]
```

Display “Connected” for agent contact and check results for service availability. Neither implies the other. Missing, stale, and unavailable data must remain distinguishable from healthy data.

On an empty installation, keep the same page structure and replace the empty list with “No devices yet” and “Add device.” Omit charts and meaningless zero-value panels. Do not insert a tour or a sequence of tasks.

Move analytical charts into device or monitor details. Show actual problems prominently; show configuration suggestions quietly and separately. Users should not interpret an unapproved suggestion as an outage.

## Devices

Default columns: **Name, Address, Status, Last seen**. Provide search and a direct “Add device” action. Clicking a row opens details with services, useful host information, and available actions.

“Add device” offers clearly named methods inside one dialog: “Install agent” and “Scan network.” Show only the controls for the selected method. This is a normal action dialog, not a wizard.

The agent method reuses the existing install command. Keep the proposed server address visible as an editable summary. Show connection status in the same dialog, with troubleshooting available on failure. A copied command is not success. SSH installation remains available where its existing device prerequisites are met.

The scan method asks for the network and shows the target range and count before launch. Put exclusions and scan profile under “Scan options.” Calculate targets automatically after valid input while retaining explicit scope confirmation. Show results where the action began.

Raw evidence, confidence scores, identifiers, process/socket inventories, and reconciliation belong in labeled detail sections. Agent administration remains available for fleet operations; normal device use should not require visiting it.

## Monitoring

Default columns: **Name, Status, Response time, Last checked**. Show an honest placeholder when response time is not available. Keep search and simple state filters. Clicking a monitor opens history, incidents, and settings.

Present proposals as “Suggested checks” beside the list, with a count only when there are suggestions. Offer “Start monitoring” for a recognizable target. Keep service classification and identity review attached to the affected object with a clear explanation of the decision needed.

Intervals, timeouts, thresholds, assertions, and technical IDs belong in monitor details or “Check options.” Preserve explicit review where the backend needs it; fewer tabs must not mean silently approving proposals.

An empty list needs a relevant action: review existing suggestions or find services through Devices. A filtered list with no matches offers “Clear filters.”

A future “Add monitor” dialog should accept a URL or address directly. This needs backend work: the current web API creates monitors through proposal approval and canonical service/endpoint associations. Do not ship a button promising a direct path before that path exists.

## Maintenance and settings

Default maintenance creation should ask for the affected device or service, start time, and duration. Put recurrence and dependency configuration in labeled secondary sections. Show conflicts when relevant, with an understandable resolution.

Group Settings by user intent, such as notifications and access. Keep technical health and diagnostics accessible without placing them in everyday status summaries. Clearly indicate whether notifications are configured; a check running does not imply an alert will be delivered.

## Interaction rules

- Give each page one clear primary action; use secondary actions for less common work.
- Use names and addresses before internal identifiers.
- Put status and actions beside the object they affect.
- Keep defaults visible in summaries, with controls available to change them.
- Keep advanced controls in the relevant form or detail view, without a global mode switch.
- Use short labels and contextual help. Avoid explanatory paragraphs compensating for unclear structure.
- Preserve keyboard access, readable text statuses, mobile layouts, and direct links.
- Keep errors and retries in context, retaining entered values and successful partial work.

## Delivery order

1. **Home and navigation:** reduce default density, distinguish problems from suggestions, and establish secondary access to existing pages.
2. **Lists and details:** simplify Devices and Monitoring tables; move technical fields into detail sections; add useful empty-state actions.
3. **Action dialogs:** bring installation and scan results into their originating views; reduce exposed configuration; retain confirmation and validation.
4. **Direct monitoring:** design and implement URL/address monitor creation, including canonical targets, duplicate handling, authorization, and bounded checks.

Review the existing `ui-overview.md` hierarchy when this proposal is accepted. Its component, accessibility, and semantic-color guidance can remain useful while its dashboard layout changes.

## Validation

Evaluate the ordinary interface, with no coaching: ask new users to add a device, find a failed check, and change a check setting. Ask experienced users to find scan exclusions, agent details, and maintenance conflicts.

Measure task completion, wrong destinations, requests for help, and time to a verified result. Establish the current baseline before claiming improvement. Success means fewer decisions for routine work while advanced tasks remain discoverable.

Before implementation, review desktop and mobile mockups for an empty installation, a healthy populated installation, and an installation with a failure. Include loading and unavailable-data states. No tutorial overlays or mandatory setup journey should appear in any version.
