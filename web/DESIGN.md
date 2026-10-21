# Console design

The approved direction is the dark console in
`../docs/design/network-console/`, with restrained neutral surfaces and the
network graph on its own page. Light and automatic appearance remain supported;
existing theme preferences are preserved.

## Visual system

Tokens live in `app/globals.css`; console composition lives in `app/console.css`.
Scoped shell, overview and map refinements live in the corresponding
`app/*-refinements.css` files, imported after it. Use system sans-serif for interface text and monospace
for addresses, identifiers and measurements. Primary actions use foreground-on-
background contrast. Color communicates actual status, chart series or manual
relationships. Navigation and borders should recede behind the working content.

Overview starts with fleet counts, attention grouped by host and actionable resource
rankings. Show up to five affected hosts, three issue messages per host, and an
accessible disclosure for additional issues. Addresses disambiguate repeated names.
Memory ranks by bytes used; bars compare used bytes relative to the largest
consumer, with percentage of capacity secondary and explicitly labelled.
Stale measurements do not participate in rankings or fleet resource totals.
On phones attention and rankings precede fleet resources and network settings.

## Network map

`lib/network-topology.ts` derives a logical forest from the visible inventory:
reported `runs_on` relationships take precedence, followed by inferred parents
and reported agent gateways. Source identities resolve to their canonical host
cards. Missing parents remain unresolved. Cycles are broken deterministically,
discarding inferred links before reported links. Large groups of leaf hosts wrap
into a grid. Subnets group by configured network membership and carry no implied
connectivity edges.

The default topology displays health on each card at a readable scale. Small
inventories fit naturally; larger ones start on a selected or attention host and
its immediate relationships. Pan or use the explicit Fit all control to see the
whole inventory. Readable view restores legible cards. Fit all reserves space
above the controls, and live reports do not reset pan or zoom. The optional Show metrics
control expands current CPU, memory and storage readings in the same layout.
The shared clock expires stale health and measurements even when no report
arrives. Metrics expansion preserves host positions. Reported, inferred and manual edges remain distinct. None describes a
physical switch port or access point without evidence.

Saved positions and manual links retain the existing map API. Metrics expansion
does not change the saved position namespace. The new hierarchy uses a versioned
`topology:v2` namespace because the old topology coordinates were local to subnet
containers. Existing topology positions remain stored but are not applied to the
new coordinate system. Subnet positions and manual links remain in use.

Phones use explicit Navigation and More actions menus. Search remains visible;
Filters discloses advanced controls through tablet widths (1000px), with an active
count and full reset. Status filters scroll in one row. The minimap is reserved
for wider desktops. Discovery limitations are neutral, expandable context below
the inventory rather than a permanent warning above the map.

## Verification

The Playwright suite exercises authentication, discovery, filters, navigation,
host and container history, live reporting, manual map edits and retry, saved
positions, reported guest links, default health, metrics expansion, stale data,
memory ranking, and a 250-host graph with missing/cyclic parent observations.
Capture desktop and phone sizes in both appearances when changing the layout.
Browser fixtures validate console behavior, not physical LAN discovery accuracy.

Verified on 2026-10-21: production build and TypeScript passed; all 73 Playwright
tests passed, including a touch-enabled mobile context, readable framing,
filter disclosure, issue grouping, byte-scaled bars, and viewport retention across
live reports. Desktop captures use 1512 × 982, tablet 768 × 1024 and phone
390 × 844. Report fixtures enter through the real server API;
adversarial snapshots exercise missing/cyclic parents and merged source aliases.
Updated console screenshots are in `../docs/screenshots/`; dark redesign captures
are in `../docs/design/network-console/screenshots/refined-*.png`.
See the [refinement review](../docs/design/network-console/refinement-review.md)
for the bounded visual review and remaining tradeoffs.
