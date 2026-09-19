# Hope web UI design specification: overview dashboard

**Status:** Approved visual direction; implementation-ready specification  
**Scope:** Light-theme web application shell and overview dashboard  
**Primary route:** `/`  
**Implementation stack:** React, TanStack Router, Tailwind CSS v4, shadcn components, Lucide icons, Recharts  
**Source of truth:** shadcn CSS variables in `apps/web/src/index.css`; Tailwind utilities compose layout and spacing

This document translates the approved Hope dashboard concept into a buildable UI system. It describes the visual language, information hierarchy, component composition, semantic tokens, chart rules, responsive behavior, and acceptance criteria.

## 1. Design intent

Hope is a homelab operations console. The overview should answer, in order:

1. Is the infrastructure broadly healthy?
2. What needs an operator’s attention now?
3. What changed recently?
4. Where should the operator investigate next?

The design is a calm operational command centre: compact enough for daily scanning, rich enough for investigation, and quiet when everything is normal.

### 1.1 Visual principles

- **Neutral by default.** Surfaces, navigation, normal metrics, topology, and most chart series use slate, blue-gray, and cool neutrals.
- **Color is semantic.** Teal means healthy, ochre means attention/degraded, dusty rose means critical, and powder blue means informational. These colors are reserved for state.
- **Pastel status language.** Status backgrounds use the same low-saturation pastel treatment as the approved `Success` and `Needs review` badges. Avoid saturated traffic-light UI.
- **One clear focal region.** The `Needs attention` queue should be the strongest actionable region through typography, grouping, and modest contrast—not a large red or yellow panel.
- **Charts are analytical, not decorative.** Use low-opacity fills, quiet gridlines, direct labels, tooltips, and line styles. Do not use a rainbow palette to make charts feel rich.
- **Swiss-style structure.** Use a disciplined grid, strong alignment, consistent spacing, restrained borders, and a clear type scale.
- **Evidence over ornament.** Every icon, color, badge, and chart mark should communicate state, context, or an available action.

### 1.2 Explicit anti-patterns

- No raw hex, RGB, or OKLCH values in route/component files.
- No new page-specific color palette in Tailwind class strings.
- No saturated green/yellow/red/blue circles by default.
- No colored backgrounds on every card.
- No emoji as UI icons.
- No chart series distinguished by hue alone.
- No new primitive implementation when a shadcn wrapper already exists.
- No layout changes hidden inside chart components.

## 2. App shell and page layout

### 2.1 Desktop shell

At `md` and above, use a two-column application shell:

- **Sidebar:** `w-60`, fixed-width, full viewport height, right border, light sidebar surface.
- **Main column:** `min-w-0 flex-1`, containing the application top bar and scrollable page content.
- **Main content gutter:** `p-5 md:p-8`; content max width `max-w-7xl mx-auto`.
- **Page background:** cool off-white; cards remain white and are separated with a thin neutral ring/border.

The existing `RootLayout` in `apps/web/src/routes/__root.tsx` owns this shell. Keep the shell structure there rather than duplicating it in individual routes.

### 2.2 Sidebar

The sidebar should retain the current navigation model and use the shadcn sidebar variables:

- Brand block at the top: Hope shield/checkmark mark, product name, and optional short descriptor.
- Navigation order: Overview, Infrastructure, Monitoring, Maintenance, Changes, Agents, Settings.
- Row height: approximately 40px; use `rounded-lg`, `px-2.5`, and a consistent 8px icon/text gap.
- Inactive rows: `text-sidebar-foreground/70`; hover uses `bg-sidebar-accent`.
- Active row: `bg-sidebar-accent text-sidebar-accent-foreground`; the active state is a muted indigo surface, not a saturated blue pill.
- Bottom block: separator, health indicator, and current site/estate context.

Use `lucide-react` icons at a consistent 16–18px size. Decorative icons next to visible labels should be `aria-hidden`; standalone controls require an accessible name.

### 2.3 Application top bar

On desktop, add a compact top bar above the page content:

- Height: approximately 56px.
- Left side: reserved for page context or empty breathing room.
- Right side: global search field, keyboard shortcut hint, notifications button, and operator avatar/menu.
- Search is visually quiet: neutral border, white surface, muted placeholder, no permanent accent fill.
- Notification button remains neutral in the idle state. If there is a critical incident, use the critical pastel token—not a saturated red icon.

On mobile, the current compact header remains the entry point. It should expose the product name, health indicator, and a menu trigger that opens the sidebar in a shadcn `Sheet` or equivalent existing component.

### 2.4 Overview page grid

The page uses a high-density dashboard grid with clear vertical rhythm:

```text
┌──────────────────────────────────────────────────────────────────────┐
│ Page heading, description, last-updated                              │
├──────────────┬──────────────┬──────────────┬────────────────────────┤
│ Devices      │ Healthy      │ Pending      │ Recent changes          │
│              │ services     │ review       │                         │
├───────────────────────────────────────────┬──────────────────────────┤
│ Service health                            │ Needs attention          │
│                                           │ Identity review          │
│                                           │ Monitor proposals        │
├───────────────────────────┬───────────────┤                          │
│ Infrastructure signals     │ Topology      │                          │
├───────────────────────────┴───────────────┴──────────────────────────┤
│ Recent changes table                                                   │
└──────────────────────────────────────────────────────────────────────┘
```

Implementation shape:

- KPI row: four equal columns at `xl`, two columns at `sm`, one column below `sm`.
- Main row: a flexible left region and a narrower right region.
- Left region: service-health card above a two-card row containing infrastructure signals and topology.
- Right region: `Needs attention` spans the height of the service-health and lower chart row.
- Recent changes: full-width card below the dashboard grid.
- Use nested CSS grids and Tailwind utilities; do not hardcode pixel positions.

Suggested utility composition:

```text
page:       mx-auto flex max-w-7xl flex-col gap-6
kpis:       grid gap-4 sm:grid-cols-2 xl:grid-cols-4
main-grid:  grid gap-4 xl:grid-cols-[minmax(0,1.75fr)_minmax(19rem,0.85fr)]
left-grid:  grid gap-4
lower-row:  grid gap-4 lg:grid-cols-[minmax(0,1.35fr)_minmax(16rem,0.8fr)]
```

These are layout recommendations, not a requirement to copy literal class strings when the existing component structure provides a better composition.

## 3. Typography and density

- Continue using `Geist Variable` through the existing `--font-sans` token.
- Use the existing font for headings and body copy; do not introduce a second font family for the first implementation.
- Page title: approximately 30–32px desktop, 24px mobile, semibold, tight tracking.
- Card titles: 15–16px, medium/semibold.
- Body and labels: 14px default; supporting copy 12–13px only when it is genuinely secondary.
- Numeric KPI values: 28–32px, semibold, tabular-looking where possible.
- Chart axes and helper labels: 11–12px, muted but still readable.
- Monospace remains appropriate for IDs, addresses, and technical values only.
- Keep a visible type hierarchy; do not make every label bold or colored.

Spacing follows the existing Tailwind/shadcn scale and an 8px rhythm:

- Card internal padding: 16px by default; 20–24px for major analytical cards.
- Section gap: 24px.
- Card grid gap: 16px.
- List row gap: 8–12px.
- Icon-to-label gap: 8px.
- Use `gap-*`, `p-*`, and `space-*` utilities rather than one-off CSS values.

## 4. Color and token system

### 4.1 Token ownership

All colors live in `apps/web/src/index.css` as shadcn-compatible CSS variables. Add custom semantic aliases to the existing `@theme inline` block so they can be used through Tailwind utilities, for example:

```css
@theme inline {
  --color-status-healthy-bg: var(--status-healthy-bg);
  --color-status-healthy-fg: var(--status-healthy-fg);
  --color-status-healthy-border: var(--status-healthy-border);
  --color-status-attention-bg: var(--status-attention-bg);
  --color-status-attention-fg: var(--status-attention-fg);
  --color-status-attention-border: var(--status-attention-border);
  --color-status-critical-bg: var(--status-critical-bg);
  --color-status-critical-fg: var(--status-critical-fg);
  --color-status-critical-border: var(--status-critical-border);
  --color-status-info-bg: var(--status-info-bg);
  --color-status-info-fg: var(--status-info-fg);
  --color-status-info-border: var(--status-info-border);
  --color-status-neutral-bg: var(--status-neutral-bg);
  --color-status-neutral-fg: var(--status-neutral-fg);
  --color-status-neutral-border: var(--status-neutral-border);
  --color-chart-health: var(--chart-health);
  --color-chart-health-fill: var(--chart-health-fill);
}
```

The component layer should consume classes such as `bg-status-healthy-bg` and `text-status-attention-fg`. It should not contain literal palette values.

### 4.2 Base light-theme tokens

These values are the intended direction. Prefer OKLCH declarations in CSS; the hex values are human-readable references for the approved mockup.

| shadcn token | Reference | Role |
|---|---|---|
| `--background` | `#F8FAFC` | Cool off-white page canvas |
| `--foreground` | `#172033` | Primary text and headings |
| `--card` | `#FFFFFF` | Card and panel surface |
| `--card-foreground` | `#172033` | Card text |
| `--muted` | `#EEF2F5` | Quiet surface, skeleton, hover |
| `--muted-foreground` | `#5A6675` | Supporting text and chart labels |
| `--border` | `#DEE5EB` | Card rings, dividers, inputs |
| `--input` | `#D9E1E8` | Input borders |
| `--primary` | `#5873B0` | Selected navigation, selected range, links |
| `--primary-foreground` | `#FFFFFF` | Text on primary controls |
| `--secondary` | `#EDF1F6` | Secondary control surface |
| `--secondary-foreground` | `#34435B` | Text on secondary surface |
| `--ring` | `#5873B0` | Keyboard focus ring |
| `--sidebar` | `#F5F7FA` | Sidebar surface |
| `--sidebar-foreground` | `#334155` | Sidebar text |
| `--sidebar-accent` | `#E9EFFB` | Active/hover sidebar surface |
| `--sidebar-accent-foreground` | `#4565A5` | Active sidebar text |
| `--sidebar-border` | `#DEE5EB` | Sidebar divider |

`--destructive` remains available for form/error semantics, but normal monitoring states should use the pastel status tokens below rather than the default saturated destructive color.

### 4.3 Semantic status tokens

The status pair is always a light background plus a muted foreground. Use the foreground for the small marker/icon and the background for the surrounding chip or marker halo.

| Token group | Background | Foreground | Border | Meaning |
|---|---|---|---|---|
| `status-healthy` | `#DCEFE8` | `#347B70` | `#C3E0D6` | Healthy, recovered, successful |
| `status-attention` | `#F4E6C9` | `#8A672D` | `#E8D6AC` | Pending, degraded, review needed |
| `status-critical` | `#F3DADD` | `#9C515B` | `#E8BCC2` | Down, critical, failed |
| `status-info` | `#DDE8F5` | `#4D72A0` | `#C7D8EC` | Informational, discovered, neutral action |
| `status-neutral` | `#EEF2F5` | `#5B6877` | `#DCE3E9` | No state, disabled, ordinary |

Rules:

- Status badges use the background/foreground pair, for example `Success`, `Needs review`, `Resolved`.
- Tiny status dots use the muted foreground with a soft background halo when space allows.
- Do not use a saturated red/yellow/green fill for a dot, icon, or badge.
- Do not color a whole card based on status.
- Always include text, a label, a count, a shape, or a position cue so color is not the only signal.
- A healthy state should be visually quiet. It does not need to compete with a review item.

### 4.4 Chart tokens

Keep the existing `--chart-1` through `--chart-5` names for shadcn chart compatibility, but map them to a quieter palette. Add named aliases for charts that need stable meaning:

| Token | Reference | Use |
|---|---|---|
| `--chart-health` | `#5BA99E` | Healthy service line |
| `--chart-health-fill` | `#B9DED8` | Service-health area fill, low opacity |
| `--chart-primary` | `#5B79BA` | Selected/highlighted metric series |
| `--chart-slate` | `#66758A` | Secondary metric series |
| `--chart-cool-gray` | `#9AA7B5` | Low-emphasis metric series |
| `--chart-attention` | `#B58A3B` | Degraded marker or warning band |
| `--chart-critical` | `#B8676C` | Critical marker only |

Chart color is intentionally more expressive than the rest of the UI, but still uses desaturated tones and low-opacity fills. The Service health chart may use a soft teal-blue field; other cards should not inherit that fill.

### 4.5 Dark mode

The first implementation targets the approved light theme. Keep the existing `.dark` token structure valid and provide semantic dark equivalents rather than reusing light pastel values directly. Preserve the same meanings and hierarchy:

- dark neutral surfaces instead of white cards;
- lighter, desaturated status foregrounds on dark status backgrounds;
- no saturated neon states;
- at least 4.5:1 contrast for normal text and visible focus states.

## 5. Component composition

Use the existing shadcn wrappers under `apps/web/src/components/ui/`. Tailwind utilities are appropriate for composition; shadcn remains the source of truth for primitives and interaction states.

### 5.1 Required primitives

| UI area | Preferred components |
|---|---|
| Dashboard panels | `Card`, `CardHeader`, `CardTitle`, `CardDescription`, `CardAction`, `CardContent` |
| Status chips | `Badge` with semantic variants |
| Review/add actions | `Button` with `outline`, `ghost`, and `sm`/`icon` sizes |
| Time range control | `ToggleGroup`, `ToggleGroupItem` |
| Recent changes | `Table`, `TableHeader`, `TableBody`, `TableRow`, `TableCell` |
| Group separators | `Separator` or card/list dividers |
| Loading states | `Skeleton` matching final card geometry |
| Errors/unavailable data | `Alert` with semantic pastel classes |
| Mobile navigation | `Sheet` or existing sidebar primitive |
| Chart surface | existing shadcn `ChartContainer`/chart utilities plus Recharts |
| Hover/focus detail | `Tooltip`, with persistent text/table fallback for important values |

If a semantic status variant does not exist, extend the existing shadcn `Badge`/`Alert` variant definitions. Do not create one-off replacement primitives in a route.

### 5.2 Shared status components

Create small shared components rather than repeating status classes:

- `StatusBadge`: renders `Success`, `Needs review`, `Resolved`, or equivalent semantic labels.
- `StatusDot`: renders a compact dot with optional pastel halo and accessible text/label.
- `TrendSparkline`: renders a quiet, non-interactive KPI trend; color only changes when the trend represents a meaningful state.
- `ChartLegendItem`: pairs marker/line style with label and value; supports keyboard/hover detail where interactive.

These components should accept a semantic status/type, not arbitrary color strings.

## 6. Overview screen sections

### 6.1 Page heading

- Optional eyebrow: `Overview / M1 inventory` or a time-aware greeting.
- Title: `Infrastructure overview`.
- Description: `A live view of your infrastructure, services and overall health.`
- Right-side metadata: a small healthy/last-updated indicator such as `Last updated 2 minutes ago`.
- Avoid making the heading area colorful; the title and whitespace provide the emphasis.

### 6.2 KPI cards

Four cards:

1. **Devices** — count, online/offline detail, quiet neutral sparkline.
2. **Healthy services** — healthy/total count, percentage, muted teal trend.
3. **Pending review** — count, identity/monitor breakdown, muted ochre trend.
4. **Recent changes** — count and time window, neutral/indigo trend.

Each card contains:

- 32–40px icon slot with a neutral or very subtle semantic surface;
- label and large value;
- supporting detail;
- optional right-aligned sparkline;
- optional arrow/link affordance only when the entire card is navigable.

Do not fill all four icon slots with different saturated colors.

### 6.3 Service health card

This is the primary analytical card.

- Header: activity icon, title, short description, and a right-aligned range `ToggleGroup` (`1h`, `6h`, `24h`, `7d`, `30d`).
- Main chart: 24-hour area chart with subtle gridlines and a muted teal health baseline.
- Area fill: soft teal-to-blue, low opacity; never a solid green rectangle.
- Degraded dips: small muted ochre markers/segments.
- Critical/down event: small muted brick marker/segment.
- Right rail or aligned summary: `95.8% Healthy now`, plus text legend for healthy/degraded/down counts.
- Legend markers use pastel-backed status dots and text; the chart must remain understandable without hue alone.

The chart needs an accessible summary and a non-hover fallback. Tooltips are useful but must not be the only way to read exact values.

### 6.4 Needs attention queue

This is the primary action queue, not a full-screen alert.

- Header: muted critical/attention bell icon, `Needs attention`, and `View all`.
- Section one: `Identity review` with a count badge and rows for unrecognized devices, unknown agents, or newly detected services.
- Section two: `Monitor proposals` with a count badge and rows for high disk usage, new services, or suggested monitors.
- Each row includes a pastel `StatusDot`, title, secondary technical detail, and an `outline` action button (`Review` or `Add`).
- Use `Separator` between queue groups.
- Keep the panel surface white/neutral. Use pastel markers and typography to distinguish severity.

### 6.5 Infrastructure signals

- Header: chart icon, title, description, and the same range control pattern.
- Multi-line time-series chart for CPU, memory, network in/out.
- One highlighted series may use muted indigo/blue; other lines use slate and cool-gray tones.
- Distinguish series with line style and direct labels as well as color.
- Keep legend values visible and align them to the chart edge.
- Avoid more than four simultaneous visible series without a toggle or aggregation.

### 6.6 Infrastructure topology

- Small companion card titled `Infrastructure topology` with `View map` action.
- Show a compact, non-interactive preview: Internet → Core → location/device clusters.
- Nodes are neutral circles with Lucide icons and labels; the selected/core node can use muted indigo.
- Connection lines are cool gray and low opacity.
- Do not use status colors for node type unless a node actually carries a health state.
- Provide a text/adjacency fallback in the full topology view.

### 6.7 Recent changes

- Full-width card with title, description, and `View all changes` action.
- Table columns: Time, Device, Event, Details, Changed by, Status.
- Normal event markers are pastel/neutral; warning and critical rows receive the corresponding pastel badge.
- Use compact table row height and visible row dividers.
- On narrow screens, use a shadcn `ScrollArea` or a responsive stacked representation; do not let the entire page create uncontrolled horizontal overflow.

## 7. Data, loading, empty, and error states

- Continue using the existing TanStack Query data sources and API types. This design spec does not authorize API or schema changes.
- Loading state: use `Skeleton` blocks that match the final card/chart dimensions. Avoid a blank dashboard.
- Empty state: use the existing `Empty` component with neutral iconography and a clear next action.
- Error state: use `Alert` with pastel critical tokens and an explicit retry action. Do not use an all-red panel.
- Stale/unavailable metric: retain the card structure, lower emphasis, and explain the state in text.
- Zero attention items: show a quiet healthy/neutral message such as `Nothing needs your attention` rather than an empty white void.

## 8. Interaction and accessibility requirements

- All interactive controls use shadcn primitives and retain visible `focus-visible` rings.
- Buttons and icon controls must have at least a practical 44px hit area where space permits; icon-only controls require an accessible label.
- Color is never the only status cue; pair it with text, labels, shapes, line styles, or table values.
- Normal body text maintains at least 4.5:1 contrast against its surface.
- Do not rely on hover-only information. Important chart values must be available through focus, tooltip, or a visible data summary.
- Range controls expose the selected value to assistive technology.
- Decorative icons adjacent to visible text use `aria-hidden="true"`; status icons that communicate meaning have an accessible label or accompanying text.
- Respect `prefers-reduced-motion`; chart updates must not flash or continuously steal focus.
- Keep heading order sequential (`h1` page title, `h2` card sections).

## 9. Responsive behavior

### 9.1 Breakpoints

- **375px:** single-column content, mobile header, cards stack, charts retain readable minimum height, recent changes use scroll/stacked layout.
- **768px (`md`):** two-column KPI grid, wider page gutters, compact sidebar may appear according to the existing shell.
- **1024px (`lg`):** lower chart row may split signals and topology; main dashboard remains readable.
- **1280px (`xl`):** full shell, four KPI cards, two-region main grid, attention queue spanning the analytical rows.
- **1440px:** use the available width for chart breathing room but keep the content max width; do not stretch text measures indefinitely.

### 9.2 Mobile priorities

On small screens, preserve the order:

1. Page heading and last-updated state.
2. KPI cards.
3. Needs attention.
4. Service health.
5. Infrastructure signals.
6. Topology.
7. Recent changes.

The attention queue moves above the charts on mobile because actionability outranks topology context.

## 10. Implementation plan

Suggested implementation slices:

1. **Token layer:** refine `apps/web/src/index.css`; add semantic status and chart aliases to `@theme inline`; preserve dark variables.
2. **Component variants:** extend `Badge` and, if needed, `Alert` with semantic pastel variants; add `StatusDot`/`StatusBadge` shared components.
3. **Shell:** add the desktop top bar and refine sidebar active/idle states in `__root.tsx`.
4. **Overview composition:** split the home route into maintainable overview sections while retaining existing query functions and loading/error behavior.
5. **Charts:** use the existing shadcn chart wrapper and Recharts; add accessible summaries and direct legends.
6. **Responsive pass:** verify the four target widths and the mobile information order.
7. **Verification:** run focused web tests, lint, typecheck, and build; inspect a desktop and mobile screenshot before handoff.

Suggested component locations:

```text
apps/web/src/components/overview/
  OverviewHeader.tsx
  OverviewMetrics.tsx
  ServiceHealthCard.tsx
  InfrastructureSignalsCard.tsx
  InfrastructureTopologyCard.tsx
  NeedsAttentionCard.tsx
  RecentChangesCard.tsx
  StatusBadge.tsx
  StatusDot.tsx
  TrendSparkline.tsx
```

Keep route-level data fetching in the route or a dedicated query hook. Section components should receive typed data and callbacks rather than fetching unrelated resources independently.

## 11. Acceptance criteria

The implementation is aligned when:

- The `/` route matches the documented grid and preserves the current inventory/review/change data.
- The light theme is predominantly neutral, with pastel semantic status accents.
- The Service health chart has a soft teal-blue field and useful status markers without saturated traffic-light colors.
- The `Needs attention` queue is the clearest actionable region without a large colored panel.
- The chart palette uses direct labels and line styles in addition to hue.
- All status chips, dots, and alerts use shared CSS variables and semantic component variants.
- No TSX file contains raw color literals for this design.
- Existing shadcn primitives remain the only source of interaction primitives.
- Loading, empty, error, and stale states are visually intentional.
- Keyboard focus, text contrast, reduced motion, and non-color status cues are preserved.
- The layout is usable at 375px, 768px, 1024px, 1280px, and 1440px widths.
- Verification passes at minimum: `pnpm lint`, `pnpm test`, `pnpm build`, and `git diff --check`.

