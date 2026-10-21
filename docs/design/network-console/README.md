# H.O.P.E network console demo

Interactive design proposal. Open `index.html` directly, or serve this folder with
`python3 -m http.server 8091 --bind 127.0.0.1`.

The selected direction combines the dark initial concept with the hierarchical
map concept, then applies the user's Vercel + Linear direction: restrained neutral
surfaces, compact navigation, fine borders, and status colors with specific jobs.
This folder remains a standalone prototype with synthetic data. The production
implementation now lives in `web/`; see [the console design notes](../../../web/DESIGN.md)
for its data model and verification. Captures named `implemented-*` show that app.

## Design contract

- Overview: attention first, then fleet resources and ranked memory, CPU and
  filesystem usage. Memory is ranked by GiB in use; utilization remains visible.
- Network map: a separate page with topology and subnet modes. Health appears
  directly on topology nodes: status dots, issue labels, and muted unobserved
  hosts. A Show metrics toggle expands CPU, memory and storage measurements in
  the same hierarchy. Hosts remain one click away; phones scroll the canvas.
- Near-black ground, subdued sidebar, neutral controls, off-white primary action.
- Working topology, subnets and metrics toggle, node inspection, host search/filter/sort,
  network filtering, simulated discovery, command search, and sample empty/error states.
- All data is synthetic. A stale agent is critical; an unobserved device is neutral.
  Missing and stale measurements are not shown as current values. Inferred map
  relationships are dashed; guest relationships are reported fixture relationships.
- The demo uses the system sans-serif font; no external font or asset dependency.
- Reference viewport: 1505 × 1045. Verify desktop and 390px mobile, controls,
  keyboard operation, focus, empty/error states and reduced-motion behavior.

References consulted:
[Geist colors](https://vercel.com/geist/colors) and
[Linear's interface refresh](https://linear.app/now/behind-the-latest-design-refresh).
These inform hierarchy and component treatment, rather than copying their branding.

Generated concept targets and implementation screenshots are in `references/` and
`screenshots/`. Targets were produced with the built-in image generation tool.
The implementation is a responsive interpretation; it is not a pixel-perfect claim.
The generated overview contains illustrative rankings; the working demo calculates
rankings and weighted resource totals from its sample inventory instead.

## Verification

Chromium, desktop 1505 × 1045 and mobile 390 × 844. Inspected captures for both
pages at both sizes. Exercised map modes, health labels, metrics toggle and zoom, host inspection and history,
keyboard command search, host search/filter/sort, network filtering, range
validation, simulated discovery, empty inventory and disconnected/recovery states.
No page overflow or runtime errors in those checks. The graph has its own scrolling
viewport at phone width. This does not validate any live server or agent behavior.

The final generation instructions are preserved in [references/prompts.md](references/prompts.md).
