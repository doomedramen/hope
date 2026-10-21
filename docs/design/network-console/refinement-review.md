# Console refinement review — 21 October 2026

The approved dark Vercel/Linear direction remains the target. This pass refines
the implemented application rather than replacing its visual identity. The
original `implemented-*` captures are the baseline; `refined-*` captures show the
revised application with representative synthetic inventory.

Three agents independently reviewed the map, Overview and responsive shell,
implemented separate areas, then reviewed one another's rendered work. The parent
integrated the changes, exercised the application and inspected final captures.

## Findings resolved

| Finding | Change | Evidence |
| --- | --- | --- |
| Fit-to-all made cards unreadable, especially on phones | Default framing keeps a readable scale and relevant host family; explicit Fit all and Readable view controls | [Desktop map](screenshots/refined-map-desktop.png), [phone map](screenshots/refined-map-mobile-dark.png) |
| Phone controls buried the graph at approximately 832px | Compact Navigation/More actions menus, visible search, advanced Filters disclosure, one scrolling status row; graph begins around 307px | [Phone map](screenshots/refined-map-mobile-dark.png) |
| Tablet filters and minimap crowded the canvas | Collapse advanced filters through 1000px and hide the minimap there; graph starts around 348px | [Tablet map](screenshots/refined-map-tablet.png) |
| Floating controls could block a host after Fit all | Reserve 76px under fitted bounds; regression exercises the previously blocked host click | Playwright health/expiry and map tests |
| Attention repeated the same host for each issue | One group per host, count and three messages, accessible additional-issue disclosure | [Busy overview](screenshots/refined-overview-busy-desktop.png) |
| Memory bars contradicted byte-based ordering | Primary values and relative bars both encode bytes used; capacity percentage is secondary | [Overview](screenshots/refined-overview-desktop.png) |
| Repeated host names looked like duplicate rows | Display identifying addresses in attention and rankings | [Busy phone overview](screenshots/refined-overview-busy-mobile.png) |
| Phone attention panel did not fill available width | Stretch mobile Overview sections consistently | [Phone overview](screenshots/refined-overview-mobile.png) |
| Fleet caption included reporting agents whose measurements were excluded | Describe current measurements without claiming an inaccurate population | [Overview](screenshots/refined-overview-desktop.png) |

## Verification and limits

The production build and TypeScript checks passed. All 73 Playwright tests passed,
including real report ingestion, guest relationships, manual map edits, saved
positions, live health expiry and viewport retention, 250-host layout, byte-scaled
rankings, issue disclosure, tablet filters, and touch navigation/host inspection.

Visual review covered 1512 × 982 desktop, 768 × 1024 tablet, and 390 × 844 phone
viewports, with full-page captures. The representative fleet contains 12 hosts;
the busy state has five affected hosts and ten issues. Phone map captures cover
dark and light appearances. Captures were inspected against the original renders
and the approved direction; this is not a pixel-for-pixel fidelity claim.

Saved PNG dimensions: map desktop 1512 × 1053, tablet 768 × 1105, phone
390 × 1003; Overview desktop 1512 × 1507, phone 390 × 2816, and busy phone
390 × 3221. These are full-page images, so their heights exceed the viewports.

Readable view intentionally shows a local branch when the whole inventory cannot
fit legibly. Pan, search or Fit all reveals the rest. A busy phone Overview remains
long because attention precedes resource rankings. These are deliberate tradeoffs,
not claims that every host or report fits on the first screen.

Browser fixtures prove these UI paths, not physical LAN discovery accuracy or a
complete accessibility/device matrix. Reported, inferred and manual relationships
retain their existing provenance; no physical switch-port evidence is invented.
