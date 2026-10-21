# Console design

The approved direction is the dark console in
`../docs/design/network-console/`, with restrained neutral surfaces and the
network graph on its own page. Light and automatic appearance remain supported;
existing theme preferences are preserved.

## Visual system

Tokens live in `app/globals.css`; console composition and component refinements
live in `app/console.css`. Use system sans-serif for interface text and monospace
for addresses, identifiers and measurements. Primary actions use foreground-on-
background contrast. Color communicates actual status, chart series or manual
relationships. Navigation and borders should recede behind the working content.

Overview starts with fleet counts, attention items and actionable resource
rankings. Memory ranks by bytes used, showing capacity and percentage alongside.
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

The default topology displays health on each card. The optional Show metrics
control expands current CPU, memory and storage readings in the same layout.
The shared clock expires stale health and measurements even when no report
arrives. Reported, inferred and manual edges remain distinct. None describes a
physical switch port or access point without evidence.

Saved positions and manual links retain the existing map API. Metrics expansion
does not change the saved position namespace. The new hierarchy uses a versioned
`topology:v2` namespace because the old topology coordinates were local to subnet
containers. Existing topology positions remain stored but are not applied to the
new coordinate system. Subnet positions and manual links remain in use.

## Verification

The Playwright suite exercises authentication, discovery, filters, navigation,
host and container history, live reporting, manual map edits and retry, saved
positions, reported guest links, default health, metrics expansion, stale data,
memory ranking, and a 250-host graph with missing/cyclic parent observations.
Capture desktop and phone sizes in both appearances when changing the layout.
Browser fixtures validate console behavior, not physical LAN discovery accuracy.

Verified on 2026-10-20: production build, TypeScript, ESLint and formatting checks
passed; all 70 Playwright tests passed. Desktop captures use 1512 × 982 and phone
captures include 390 × 844. Report fixtures enter through the real server API;
adversarial snapshots exercise missing/cyclic parents and merged source aliases.
Updated console screenshots are in `../docs/screenshots/`; dark redesign captures
are in `../docs/design/network-console/screenshots/implemented-*.png`.
