/* Standalone design prototype. Inventory and relationships below are synthetic. */
const paths = {
  home: '<path d="m3 10 9-7 9 7v10H3Z"/><path d="M9 20v-7h6v7"/>',
  hosts:
    '<rect x="4" y="3" width="16" height="7" rx="1.5"/><rect x="4" y="14" width="16" height="7" rx="1.5"/><path d="M7 6.5h.01M7 17.5h.01M11 6.5h6M11 17.5h6"/>',
  map: '<circle cx="12" cy="5" r="3"/><circle cx="5" cy="19" r="3"/><circle cx="19" cy="19" r="3"/><path d="M12 8v4M5 16v-4h14v4"/>',
  network:
    '<ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v14c0 4 16 4 16 0V5M4 12c0 4 16 4 16 0"/>',
  shield: '<path d="m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6Z"/>',
  search: '<circle cx="10.5" cy="10.5" r="7"/><path d="m16 16 5 5"/>',
  chevron: '<path d="m9 5 7 7-7 7"/>',
  down: '<path d="m7 10 5 5 5-5"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  minus: '<path d="M5 12h14"/>',
  x: '<path d="m6 6 12 12M18 6 6 18"/>',
  cpu: '<rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"/><rect x="9" y="9" width="6" height="6"/>',
  memory:
    '<rect x="3" y="5" width="18" height="13" rx="2"/><path d="M7 9v4M12 9v4M17 9v4M7 18v3M12 18v3M17 18v3"/>',
  warning: '<path d="m12 3 10 18H2Z"/><path d="M12 9v5M12 17h.01"/>',
  settings:
    '<path d="m9 3-.5 3-2 1-3-.5-2 4L4 13v2l-1.5 2.5 3 3L8 19h2l2 2 4-1 .5-3 2-1 3 .5 1-4L20 11V9l1-2-3-3-3 1-2-2Z"/><circle cx="12" cy="12" r="3"/>',
  monitor:
    '<rect x="3" y="3" width="18" height="13" rx="2"/><path d="M12 16v5M7 21h10"/>',
  router:
    '<rect x="3" y="12" width="18" height="8" rx="2"/><path d="M6 12 5 3M18 12l1-9M7 16h.01M11 16h.01M15 16h2"/>',
  box: '<path d="m12 2 9 5v10l-9 5-9-5V7Z M3 7l9 5 9-5M12 12v10M7 4.8l9 5"/>',
  wifi: '<path d="M2 8a16 16 0 0 1 20 0M5 12a11 11 0 0 1 14 0M8 16a6 6 0 0 1 8 0M12 20h.01"/>',
  fit: '<path d="M8 3H3v5M16 3h5v5M21 16v5h-5M8 21H3v-5"/>',
  arrow: '<path d="M4 12h16m-6-6 6 6-6 6"/>',
};
const icon = (name) =>
  `<svg class="icon" viewBox="0 0 24 24" aria-hidden="true">${paths[name] || paths.hosts}</svg>`;
const escapeHtml = (value) =>
  String(value).replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
const networks = [
  {
    id: "main",
    name: "Main network",
    range: "192.168.1.0/24",
    description: "Primary home network",
  },
  {
    id: "lab",
    name: "Lab",
    range: "192.168.10.0/24",
    description: "Homelab and test systems",
  },
  {
    id: "iot",
    name: "IoT",
    range: "192.168.20.0/24",
    description: "Home automation and devices",
  },
];
const hosts = [
  ["nas-01", "192.168.1.10", "main", "network", 18, 64, 91, "agent", "storage"],
  ["proxmox-01", "192.168.1.20", "main", "hosts", 32, 56, 48, "agent"],
  [
    "build-01",
    "192.168.1.30",
    "main",
    "monitor",
    null,
    null,
    null,
    "stale",
    "agent",
  ],
  ["home-assistant", "192.168.1.40", "main", "home", 8, 34, 32, "agent"],
  ["media-server", "192.168.1.50", "main", "hosts", 24, 42, 71, "agent"],
  ["workstation", "192.168.1.100", "main", "monitor", 28, 48, 61, "agent"],
  ["backup-01", "192.168.1.60", "main", "network", 14, 36, 55, "agent"],
  ["lab-node", "192.168.10.10", "lab", "hosts", 30, 58, 43, "agent"],
  ["lab-mini", "192.168.10.11", "lab", "hosts", 30, 46, 38, "agent"],
  ["router", "192.168.1.1", "main", "router", null, null, null, "observed"],
  ["docker-01", "192.168.1.21", "main", "box", null, null, null, "observed"],
  [
    "vm-ubuntu",
    "192.168.1.22",
    "main",
    "monitor",
    null,
    null,
    null,
    "observed",
  ],
  ["media", "192.168.1.11", "main", "hosts", null, null, null, "observed"],
  ["backups", "192.168.1.12", "main", "network", null, null, null, "observed"],
  ["zigbee2mqtt", "192.168.1.41", "main", "cpu", null, null, null, "observed"],
  ["esphome", "192.168.1.42", "main", "wifi", null, null, null, "observed"],
  ["printer", "192.168.1.70", "main", "hosts", null, null, null, "observed"],
  [
    "laptop",
    "192.168.1.101",
    "main",
    "monitor",
    null,
    null,
    null,
    "unobserved",
  ],
  ["test-vm", "192.168.10.12", "lab", "box", null, null, null, "observed"],
  ["dev-pi", "192.168.10.13", "lab", "cpu", null, null, null, "observed"],
  ["sandbox", "192.168.10.14", "lab", "box", null, null, null, "observed"],
  [
    "hallway-sensor",
    "192.168.20.10",
    "iot",
    "cpu",
    null,
    null,
    null,
    "observed",
  ],
  ["living-room", "192.168.20.11", "iot", "wifi", null, null, null, "observed"],
  ["thermostat", "192.168.20.12", "iot", "cpu", null, null, null, "observed"],
  [
    "office-display",
    "192.168.1.80",
    "main",
    "monitor",
    null,
    null,
    null,
    "observed",
  ],
  ["phone", "192.168.1.90", "main", "wifi", null, null, null, "observed"],
].map(
  ([name, ip, network, type, cpu, memory, storage, state, issue], index) => ({
    name,
    ip,
    network,
    type,
    cpu,
    memory,
    storage,
    state,
    issue,
    memoryTotal: [32, 64, 16, 8, 16, 32, 8, 16, 8][index] ?? null,
    storageTotal: [8.9, 4, 0, 0.125, 8, 2, 12, 2, 1][index] ?? null,
  }),
);
const pageNames = {
  overview: "Overview",
  hosts: "Hosts",
  map: "Network map",
  networks: "Networks",
  agents: "Agents",
};
const params = new URLSearchParams(location.search);
const state = {
  page: pageNames[params.get("page")] ? params.get("page") : "overview",
  mode: "topology",
  showMetrics: false,
  query: "",
  filter: "all",
  network: "all",
  sort: "default",
  scenario: "normal",
  scanning: false,
  discovered: false,
  zoom: 1,
};
const app = document.querySelector("#app");
const detail = document.querySelector("#detail");
const command = document.querySelector("#command");
let toastTimer;
function inventory() {
  return state.scenario === "empty" ? [] : hosts;
}
function tone(host) {
  return host.state === "stale"
    ? "critical"
    : host.issue
      ? "warning"
      : host.state === "unobserved"
        ? "neutral"
        : "";
}
function status(host) {
  return host.state === "stale"
    ? "Agent stale"
    : host.state === "agent"
      ? "Reporting"
      : host.state === "unobserved"
        ? "Not recently seen"
        : "Observed";
}
function statusHtml(host) {
  return `<span class="status"><i class="dot ${tone(host)}"></i>${status(host)}</span>`;
}
function meter(value, warning = false) {
  return value == null
    ? '<span class="muted">—</span>'
    : `<span class="meter ${warning ? "warning" : ""}"><span style="width:${value}%"></span></span>`;
}
function say(text) {
  document.querySelector("#announcement").textContent = text;
}
function toast(text) {
  clearTimeout(toastTimer);
  const el = document.querySelector("#toast");
  el.textContent = text;
  el.classList.add("visible");
  toastTimer = setTimeout(() => el.classList.remove("visible"), 4000);
}
function summary() {
  const list = inventory();
  return `<section class="summary" aria-label="Fleet summary">
  <button data-page="hosts">${icon("monitor")}<div><strong>${list.length ? 24 : 0} <small>/ ${list.length}</small></strong><p>Recently seen</p></div></button>
  <button data-page="agents">${icon("shield")}<div><strong>${list.length ? 8 : 0} <small>/ ${list.length ? 9 : 0}</small></strong><p>Agents reporting</p></div></button>
  <button class="attention" data-action="attention">${icon("warning")}<div><strong>${list.length ? 2 : 0}</strong><p>Need attention</p></div></button>
  <button data-page="networks">${icon("map")}<div><strong>${list.length ? 3 : 0}</strong><p>Networks</p></div></button>
</section>`;
}
function shell() {
  app.innerHTML = `<aside class="sidebar"><div class="brand"><span class="brand-mark">H</span><div><strong>H.O.P.E</strong><p>Home Network Monitor</p></div></div>
  <div class="workspace-label">${icon("home")}Home network</div>
  <button class="search-trigger" data-action="command">${icon("search")} Search… <kbd>⌘ K</kbd></button>
  <nav class="nav" aria-label="Main navigation">${Object.entries(pageNames)
    .map(
      ([id, name]) =>
        `<button data-page="${id}" class="${state.page === id ? "active" : ""}" ${state.page === id ? 'aria-current="page"' : ""}>${icon({ overview: "home", hosts: "monitor", map: "map", networks: "network", agents: "shield" }[id])}${name}${["hosts", "networks", "agents"].includes(id) ? `<span class="nav-count">${inventory().length ? { hosts: 26, networks: 3, agents: 9 }[id] : 0}</span>` : ""}</button>`,
    )
    .join("")}</nav>
  <div class="sidebar-bottom"><div class="nav"><button data-action="settings">${icon("settings")}Preview settings</button></div><div class="self-hosted"><i class="dot"></i>Self-hosted</div></div></aside>
  <div class="workspace"><header class="topbar"><div class="breadcrumb">Home network ${icon("chevron")}<strong>${pageNames[state.page]}</strong></div><div class="top-actions"><button class="demo-badge" data-action="settings"><i class="dot"></i>Demo data</button><button data-action="install">${icon("plus")}Add agent</button><button class="primary" data-action="discover" ${state.scanning ? "disabled" : ""}>${icon(state.scanning ? "router" : "search")}${state.scanning ? "Discovering…" : "Discover"}</button></div></header>${state.scanning ? '<div class="scan-progress" role="progressbar" aria-label="Simulated discovery running"></div>' : ""}
  <main class="main" id="main" tabindex="-1">${state.scenario === "disconnected" ? `<div class="notice">${icon("warning")}<span>Connection interrupted. Showing the last sample inventory.</span><button data-action="reconnect">Reconnect</button></div>` : ""}
  <div class="page-heading"><div><h1>${pageNames[state.page]}</h1><p>${{ overview: inventory().length ? "24 of 26 hosts recently seen. 2 hosts need attention." : "Start with a network range or a reporting agent.", hosts: "Your discovered devices and reporting hosts.", map: "Router hierarchy, guest relationships and link provenance.", networks: "The IPv4 ranges watched by discovery.", agents: "Reporting agents and their latest measurements." }[state.page]}</p></div><span class="muted small">${state.discovered ? "Last discovery just now" : "Last discovery 2 min ago"}</span></div>
  ${state.page === "overview" ? overview() : state.page === "map" ? mapPage() : state.page === "networks" ? networkTable() : hostTable()}
  <footer class="footer"><span>Self-hosted · Local observations</span><span>Interactive design demo · Sample inventory</span></footer></main></div>`;
  fitMap();
}
function issues() {
  return `<section class="panel"><div class="panel-heading"><h2>Needs attention <span class="count-badge">${inventory().length ? 2 : 0}</span></h2><button class="quiet" data-action="attention">View hosts ${icon("arrow")}</button></div>${
    inventory().length
      ? `
  <button class="issue-row" data-host="build-01"><span class="issue-symbol critical">${icon("warning")}</span><div><h3>build-01</h3><p class="critical">Agent not reporting</p><small>Last report 12 minutes ago</small></div>${icon("chevron")}</button>
  <button class="issue-row" data-host="nas-01"><span class="issue-symbol">${icon("network")}</span><div><h3>nas-01</h3><p class="warning">/data is 91% full</p><small>8.1 TiB used of 8.9 TiB</small></div>${icon("chevron")}</button>`
      : '<div class="empty"><h2>No hosts to assess</h2><p>Discover devices to see their status here.</p></div>'
  }</section>`;
}
function resources() {
  const agents = inventory().filter((h) => h.state === "agent");
  const average = agents.length
    ? Math.round(agents.reduce((sum, h) => sum + h.cpu, 0) / agents.length)
    : 0;
  const weighted = (key, capacity) =>
    Math.round(
      (agents.reduce((sum, h) => sum + (h[key] / 100) * h[capacity], 0) /
        agents.reduce((sum, h) => sum + h[capacity], 0)) *
        100,
    ) || 0;
  return `<section class="panel"><div class="panel-heading"><h2>Fleet resources</h2><span class="muted small">${agents.length ? agents.length + " reporting agents" : "No samples"}</span></div><div class="resource-list">${[
    ["cpu", "Average CPU", average],
    ["memory", "Memory", weighted("memory", "memoryTotal")],
    ["network", "Storage", weighted("storage", "storageTotal")],
  ]
    .map(
      ([type, label, value]) =>
        `<div class="resource"><div class="resource-label"><span>${icon(type)}${label}</span><strong>${inventory().length ? value + "%" : "—"}</strong></div>${meter(inventory().length ? value : 0)}</div>`,
    )
    .join(
      "",
    )}</div><p class="panel-note">Latest samples from reporting agents. Discovery-only devices and stale readings are excluded.</p></section>`;
}
function ranking(title, key) {
  const used = (h) => (h.memory * h.memoryTotal) / 100;
  const list = inventory()
    .filter((h) => h.state === "agent")
    .sort((a, b) => (key === "memory" ? used(b) - used(a) : b[key] - a[key]))
    .slice(0, 4);
  return `<section class="panel"><div class="panel-heading"><h2>${title}</h2><span class="muted small">${key === "storage" ? "By filesystem" : key === "memory" ? "By memory in use" : "Latest sample"}</span></div><div class="rank-list">${list.length ? list.map((h) => `<button class="rank-row ${key === "memory" ? "memory-ranking" : ""}" data-host="${h.name}" aria-label="Inspect ${h.name}, ${key} ${h[key]} percent"><span class="rank-name">${h.name}${key === "storage" ? `<small>${h.name === "nas-01" ? "/data" : "/"}</small>` : key === "memory" ? `<small class="memory-used">${used(h).toFixed(1)} GiB / ${h.memoryTotal} GiB</small>` : ""}</span>${meter(h[key], key === "storage" && h[key] >= 90)}<span class="rank-value ${key === "storage" && h[key] >= 90 ? "warning" : ""}">${h[key]}%</span></button>`).join("") : '<p class="panel-note">No reporting agents yet.</p>'}</div></section>`;
}
function networkList() {
  return `<section class="panel"><div class="panel-heading"><h2>Networks</h2><button class="quiet" data-page="map">Open network map ${icon("arrow")}</button></div><div class="net-list">${inventory().length ? networks.map((n) => `<button class="network-mini" data-network="${n.id}"><i class="dot neutral"></i><span>${n.name}</span><span class="mono">${n.range}</span>${icon("chevron")}</button>`).join("") : '<p class="panel-note">No networks configured.</p>'}</div></section>`;
}
function overview() {
  return `${summary()}<div class="overview-grid"><div class="column">${issues()}${resources()}${networkList()}</div><div class="column">${ranking("Most memory used by", "memory")}${ranking("Busiest CPUs", "cpu")}${ranking("Fullest filesystems", "storage")}</div></div>`;
}
const nodePositions = [
  ["router", 500, 55],
  ["proxmox-01", 230, 190],
  ["nas-01", 565, 190],
  ["home-assistant", 825, 190],
  ["build-01", 80, 340],
  ["docker-01", 215, 340],
  ["vm-ubuntu", 350, 340],
  ["media", 500, 340],
  ["backups", 635, 340],
  ["zigbee2mqtt", 785, 340],
  ["esphome", 920, 340],
];
function graph() {
  const height = state.showMetrics ? 590 : 430;
  const halfNode = state.showMetrics ? 78 : 48;
  const rows = state.showMetrics ? [90, 280, 490] : [55, 190, 340];
  const branch = (source, targets, row, nextRow) => {
    const start = rows[row] + halfNode;
    const end = rows[nextRow] - halfNode;
    const junction = (start + end) / 2;
    return targets
      .map((target) => `M${source} ${start}V${junction}H${target}V${end}`)
      .join("");
  };
  const reported = branch(500, [230, 565, 825], 0, 1);
  const inferred =
    branch(230, [80, 215, 350], 1, 2) +
    branch(565, [500, 635], 1, 2) +
    branch(825, [785, 920], 1, 2);
  return `<div class="map-content"><div class="map-world ${state.showMetrics ? "with-metrics" : ""}" style="height:${height}px" data-height="${height}"><svg class="edges" viewBox="0 0 1000 ${height}" aria-hidden="true"><path d="${reported}"/><path class="inferred" d="${inferred}"/></svg>${nodePositions
    .map(([name, x, y]) => {
      const h = hosts.find((h) => h.name === name);
      const label =
        h.state === "stale"
          ? "Agent stale"
          : h.issue === "storage"
            ? `Disk ${h.storage}%`
            : status(h);
      const row = y === 55 ? 0 : y === 190 ? 1 : 2;
      const metrics =
        h.state === "agent"
          ? `<span class="node-metrics">${[
              ["CPU", "cpu"],
              ["Mem", "memory"],
              ["Disk", "storage"],
            ]
              .map(
                ([label, key]) =>
                  `<span class="node-metric"><span>${label}</span>${meter(h[key], h[key] >= 90)}<span>${h[key]}%</span></span>`,
              )
              .join("")}</span>`
          : `<span class="node-metrics-unavailable">${h.state === "stale" ? "No current readings" : "No agent metrics"}</span>`;
      return `<button class="node ${y === 340 ? "leaf" : ""} ${tone(h) ? "health-" + tone(h) : ""}" style="left:${x}px;top:${rows[row]}px" data-host="${name}" aria-label="Inspect ${name}, ${status(h)}${h.issue ? ", " + label : ""}">${icon(h.type)}<span class="node-name"><i class="dot ${tone(h)}"></i>${name}</span><span class="mono">${h.ip}</span><span class="node-status ${tone(h)}">${label}</span>${state.showMetrics ? metrics : ""}</button>`;
    })
    .join("")}</div></div>`;
}
function subnets() {
  return `<div class="subnets">${networks
    .map(
      (n) =>
        `<section class="subnet"><h3>${n.name}</h3><p class="muted mono">${n.range}</p><div class="subnet-list">${inventory()
          .filter((h) => h.network === n.id)
          .map(
            (h) =>
              `<button data-host="${h.name}"><i class="dot ${tone(h)}"></i>${h.name}</button>`,
          )
          .join("")}</div></section>`,
    )
    .join("")}</div>`;
}
function mapPage() {
  return `${summary()}<div class="map-layout"><section class="panel map-panel"><div class="panel-heading"><h2>Network overview</h2><div class="map-view-controls"><div class="tabs" aria-label="Map mode">${["topology", "subnets"].map((m) => `<button data-mode="${m}" aria-pressed="${state.mode === m}">${m[0].toUpperCase() + m.slice(1)}</button>`).join("")}</div>${state.mode === "topology" ? `<button class="metrics-toggle" data-action="toggle-metrics" aria-pressed="${state.showMetrics}">${icon("cpu")}Show metrics</button>` : ""}</div></div><div class="map-scroll">${!inventory().length ? emptyState("No devices on the map", "Discover hosts to start exploring your network.") : state.mode === "subnets" ? subnets() : graph()}</div><div class="map-footer"><div class="legend">${state.mode === "subnets" ? "<span>Grouped by configured IPv4 range</span>" : '<span><i class="line-key"></i>Reported</span><span><i class="line-key dashed"></i>Inferred</span>'}</div>${state.mode !== "subnets" ? `<div class="map-tools"><button data-zoom="out" aria-label="Zoom out">${icon("minus")}</button><button data-zoom="in" aria-label="Zoom in">${icon("plus")}</button><button data-zoom="fit" aria-label="Fit map">${icon("fit")}</button></div>` : ""}</div><p class="map-hint">${state.mode === "subnets" ? "Select a host to inspect it." : "Health shown on every host · Inferred links do not establish physical connectivity."}</p></section><div class="column map-side">${issues()}${resources()}</div></div><div style="margin-top:18px">${networkTable()}</div>`;
}
function fitMap() {
  const scroll = document.querySelector(".map-scroll");
  const content = document.querySelector(".map-content");
  const world = document.querySelector(".map-world");
  if (!scroll || !content || !world) return;
  const width = Math.max(800, scroll.clientWidth);
  const scale = (width / 1000) * state.zoom;
  world.style.transform = `scale(${scale})`;
  content.style.width = `${Math.max(width, 1000 * scale)}px`;
  content.style.height = `${Number(world.dataset.height) * scale}px`;
  if (scroll.clientWidth < 800)
    scroll.scrollLeft = (1000 * scale - scroll.clientWidth) / 2;
}
function emptyState(title, message) {
  return `<div class="empty">${icon("router")}<h2>${title}</h2><p>${message}</p><button class="primary" data-action="discover">${icon("search")}Discover devices</button></div>`;
}
function matchingHosts() {
  return inventory()
    .filter(
      (h) =>
        (state.page !== "agents" || ["agent", "stale"].includes(h.state)) &&
        (state.network === "all" || h.network === state.network) &&
        (state.filter !== "attention" || h.issue) &&
        `${h.name} ${h.ip}`.toLowerCase().includes(state.query.toLowerCase()),
    )
    .sort((a, b) =>
      state.sort === "name"
        ? a.name.localeCompare(b.name)
        : state.sort === "cpu"
          ? (b.cpu ?? -1) - (a.cpu ?? -1)
          : state.sort === "memory"
            ? (b.memory ?? -1) - (a.memory ?? -1)
            : 0,
    );
}
function tableRows() {
  const list = matchingHosts();
  return (
    list
      .map(
        (h) =>
          `<tr><td><button class="host-link" data-host="${h.name}">${icon(h.type)}${h.name}</button></td><td class="mono muted">${h.ip}</td><td>${statusHtml(h)}</td>${["cpu", "memory", "storage"].map((k) => `<td>${h[k] == null ? '<span class="muted">—</span>' : `<div class="cell-meter"><span class="${k === "storage" && h[k] >= 90 ? "warning" : ""}">${h[k]}%</span>${meter(h[k], k === "storage" && h[k] >= 90)}</div>`}</td>`).join("")}<td><button class="quiet icon-button" data-host="${h.name}" aria-label="Inspect ${h.name}">${icon("chevron")}</button></td></tr>`,
      )
      .join("") ||
    '<tr><td colspan="7"><div class="empty"><h2>No matching hosts</h2><p>Try another name, address or filter.</p><button data-action="clear-filters">Clear filters</button></div></td></tr>'
  );
}
function hostTable() {
  if (!inventory().length)
    return `<section class="panel">${emptyState("No hosts yet", "Run discovery to find devices, or install a reporting agent for live metrics.")}</section>`;
  return `<section class="panel"><div class="panel-heading"><h2>${state.page === "agents" ? "Reporting agents" : "Host inventory"}</h2><span class="muted small">${state.network === "all" ? "All networks" : networks.find((n) => n.id === state.network).name}</span></div><div class="toolbar"><div class="tabs" aria-label="Host filter"><button data-filter="all" aria-pressed="${state.filter === "all"}">All ${state.page === "agents" ? "agents" : "hosts"}</button><button data-filter="attention" aria-pressed="${state.filter === "attention"}">Needs attention <span class="count-badge">2</span></button></div><label class="search-field">${icon("search")}<input id="host-search" aria-label="Search hosts" placeholder="Search hosts or IP addresses…" value="${escapeHtml(state.query)}" /></label>${state.network !== "all" ? '<button class="quiet" data-action="clear-filters">All networks</button>' : ""}</div><div class="table-scroll"><table><thead><tr><th><button data-sort="name">Host ${icon("down")}</button></th><th>Address</th><th>State</th><th><button data-sort="cpu">CPU ${icon("down")}</button></th><th><button data-sort="memory">Memory ${icon("down")}</button></th><th>Storage</th><th><span class="sr-only">Inspect</span></th></tr></thead><tbody id="host-rows">${tableRows()}</tbody></table></div><div class="table-footer"><span id="host-count">${matchingHosts().length} hosts shown</span><span>Stale measurements are excluded</span></div></section>`;
}
function networkTable() {
  return `<section class="panel"><div class="panel-heading"><h2>Networks</h2><button data-action="add-network">${icon("plus")}Add network</button></div>${
    !inventory().length
      ? emptyState(
          "No networks configured",
          "Add an IPv4 range to tell discovery where to look.",
        )
      : `<div class="table-scroll"><table><thead><tr><th>Name</th><th>Range</th><th>Hosts</th><th>Agents</th><th>Description</th><th></th></tr></thead><tbody>${networks
          .map((n) => {
            const items = inventory().filter((h) => h.network === n.id);
            const agents = items.filter((h) =>
              ["agent", "stale"].includes(h.state),
            );
            return `<tr><td><button class="host-link" data-network="${n.id}"><i class="dot neutral"></i>${n.name}</button></td><td class="mono muted">${n.range}</td><td>${items.length}</td><td>${agents.filter((h) => h.state === "agent").length} / ${agents.length}</td><td class="muted">${n.description}</td><td><button class="quiet icon-button" data-network="${n.id}" aria-label="View ${n.name} hosts">${icon("chevron")}</button></td></tr>`;
          })
          .join("")}</tbody></table></div>`
  }</section>`;
}
function navigate(page) {
  state.page = page;
  state.query = "";
  state.filter = "all";
  state.network = "all";
  const url = new URL(location.href);
  url.searchParams.set("page", page);
  history.replaceState(null, "", url);
  shell();
  document.querySelector("#main").focus({ preventScroll: true });
  window.scrollTo(0, 0);
}
function dialogFrame(title, subtitle, body, type = "hosts") {
  detail.innerHTML = `<div class="dialog-heading">${icon(type)}<div><h2 id="dialog-title">${title}</h2><p>${subtitle}</p></div><button class="icon-button" data-action="close" aria-label="Close dialog">${icon("x")}</button></div><div class="dialog-body">${body}</div>`;
  if (!detail.open) detail.showModal();
}
function inspect(name, historyTab = false) {
  const h = hosts.find((h) => h.name === name);
  if (!h) return;
  const stale = h.state === "stale";
  const measured = h.state === "agent";
  dialogFrame(
    h.name,
    `${h.ip} · ${networks.find((n) => n.id === h.network).name}`,
    `${h.issue ? `<div class="notice ${stale ? "critical" : ""}">${icon("warning")}<span>${stale ? "Agent not reporting. Last report was 12 minutes ago. Live readings are unavailable." : "/data is 91% full. 8.1 TiB used of 8.9 TiB."}</span></div>` : ""}<div class="tabs" aria-label="Host detail view"><button data-detail-tab="current" data-name="${name}" aria-pressed="${!historyTab}">Current readings</button><button data-detail-tab="history" data-name="${name}" aria-pressed="${historyTab}">History</button></div>${
      historyTab
        ? `<p class="muted small" style="margin-top:20px">CPU · Illustrative one-hour history${stale ? " · Missing readings remain gaps" : ""}</p><svg class="history" viewBox="0 0 480 150" role="img" aria-label="Illustrative CPU history${stale ? ", with a gap for the last twelve minutes" : ""}"><path class="grid" d="M0 30H480M0 75H480M0 120H480"/><path d="M0 120 12 116 24 122 36 104 48 110 60 102 72 108 84 88 96 98 108 95 120 100 132 78 144 84 156 70 168 80 180 76 192 96 204 100 216 88 228 99 240 91 252 100 264 105 276 95 288 100 300 92 312 105 324 101 336 110 348 103 360 107 ${stale ? "" : "372 99 384 104 396 94 408 105 420 102 432 111 444 103 456 110 480 106"}"/></svg><div class="history-labels"><span>60 min ago</span><span>Now</span></div>`
        : `<div class="detail-grid" style="margin-top:20px">${[
            ["CPU", "cpu"],
            ["Memory", "memory"],
            ["Storage", "storage"],
          ]
            .map(
              ([label, key]) =>
                `<div class="detail-metric"><span>${label}</span><strong class="${h[key] >= 90 ? "warning" : ""}">${h[key] == null ? "—" : h[key] + "%"}</strong>${meter(h[key] ?? 0, h[key] >= 90)}</div>`,
            )
            .join(
              "",
            )}</div>${!measured ? `<p class="panel-note" style="padding:14px 0 0">${stale ? "Current readings are unavailable while the agent is stale." : "Discovery provides identity and reachability. A reporting agent is needed for resource measurements."}</p>` : ""}<dl class="detail-meta"><div><dt>State</dt><dd>${statusHtml(h)}</dd></div><div><dt>Latest observation</dt><dd>${stale ? "12 minutes ago" : h.state === "unobserved" ? "35 minutes ago" : "Just now"}</dd></div><div><dt>Source</dt><dd>${["agent", "stale"].includes(h.state) ? "Reporting agent" : "Network discovery"}</dd></div><div><dt>Inventory</dt><dd>Synthetic demo data</dd></div></dl>`
    }<div class="detail-actions"><button data-action="close">Close</button><button class="primary" data-inspect-network="${h.network}">View network hosts ${icon("arrow")}</button></div>`,
    h.type,
  );
}
function settings() {
  dialogFrame(
    "Preview settings",
    "Explore the design with sample states.",
    `<label class="form-field">Inventory state<select id="scenario"><option value="normal" ${state.scenario === "normal" ? "selected" : ""}>Normal · two hosts need attention</option><option value="empty" ${state.scenario === "empty" ? "selected" : ""}>Empty · no hosts or networks</option><option value="disconnected" ${state.scenario === "disconnected" ? "selected" : ""}>Disconnected · last known inventory</option></select></label><p class="muted small">This prototype uses sample inventory. Actions do not contact a server or modify your network.</p><div class="detail-actions"><button class="primary" data-action="apply-settings">Apply preview state</button></div>`,
    "settings",
  );
}
function openCommand() {
  command.innerHTML = `<label class="command-search">${icon("search")}<input id="command-input" aria-label="Search hosts and pages" placeholder="Search hosts and pages…" autocomplete="off"/><kbd>Esc</kbd></label><div class="command-results" id="command-results"></div><div class="command-foot">↑ ↓ navigate · Enter to open · Esc to close</div>`;
  command.showModal();
  renderCommand("");
}
function renderCommand(query) {
  const q = query.toLowerCase();
  document.querySelector("#command-results").innerHTML =
    Object.entries(pageNames)
      .filter(([, name]) => name.toLowerCase().includes(q))
      .map(
        ([page, name]) =>
          `<button data-command-page="${page}">${icon(page === "map" ? "map" : "home")}${name}</button>`,
      )
      .join("") +
    inventory()
      .filter((h) => `${h.name} ${h.ip}`.toLowerCase().includes(q))
      .slice(0, 8)
      .map(
        (h) =>
          `<button data-command-host="${h.name}">${icon(h.type)}${h.name}<span class="mono">${h.ip}</span></button>`,
      )
      .join("");
  if (!document.querySelector("#command-results").children.length)
    document.querySelector("#command-results").innerHTML =
      '<div class="empty" style="min-height:120px">No matching hosts or pages</div>';
}
function discover() {
  if (state.scanning) return;
  state.scanning = true;
  shell();
  say("Simulated discovery started.");
  setTimeout(() => {
    state.scanning = false;
    state.discovered = true;
    if (state.scenario === "empty") state.scenario = "normal";
    shell();
    toast("Demo discovery complete. Sample inventory refreshed.");
  }, 1800);
}
document.addEventListener("click", (e) => {
  const b = e.target.closest("button");
  if (!b) return;
  if (b.dataset.page) {
    navigate(b.dataset.page);
    return;
  }
  if (b.dataset.host) {
    inspect(b.dataset.host);
    return;
  }
  if (b.dataset.network) {
    navigate("hosts");
    state.network = b.dataset.network;
    shell();
    return;
  }
  if (b.dataset.mode) {
    state.mode = b.dataset.mode;
    state.zoom = 1;
    shell();
    document.querySelector(`[data-mode="${state.mode}"]`).focus();
    return;
  }
  if (b.dataset.zoom) {
    state.zoom =
      b.dataset.zoom === "fit"
        ? 1
        : Math.max(
            0.75,
            Math.min(
              1.75,
              state.zoom + (b.dataset.zoom === "in" ? 0.15 : -0.15),
            ),
          );
    fitMap();
    say(`Map zoom ${Math.round(state.zoom * 100)} percent.`);
    return;
  }
  if (b.dataset.filter) {
    state.filter = b.dataset.filter;
    shell();
    document.querySelector(`[data-filter="${state.filter}"]`).focus();
    return;
  }
  if (b.dataset.sort) {
    state.sort = b.dataset.sort;
    shell();
    document.querySelector(`[data-sort="${state.sort}"]`).focus();
    return;
  }
  if (b.dataset.detailTab) {
    inspect(b.dataset.name, b.dataset.detailTab === "history");
    detail.querySelector(`[data-detail-tab="${b.dataset.detailTab}"]`).focus();
    return;
  }
  if (b.dataset.inspectNetwork) {
    detail.close();
    navigate("hosts");
    state.network = b.dataset.inspectNetwork;
    shell();
    return;
  }
  if (b.dataset.commandPage) {
    command.close();
    navigate(b.dataset.commandPage);
    return;
  }
  if (b.dataset.commandHost) {
    command.close();
    inspect(b.dataset.commandHost);
    return;
  }
  switch (b.dataset.action) {
    case "toggle-metrics": {
      const scroll = document.querySelector(".map-scroll");
      const left = scroll.scrollLeft;
      state.showMetrics = !state.showMetrics;
      shell();
      document.querySelector(".map-scroll").scrollLeft = left;
      document
        .querySelector('[data-action="toggle-metrics"]')
        .focus({ preventScroll: true });
      say(
        state.showMetrics
          ? "Host metrics shown. Health remains visible on every host."
          : "Host metrics hidden. Health remains visible on every host.",
      );
      break;
    }
    case "attention":
      navigate("hosts");
      state.filter = "attention";
      shell();
      break;
    case "close":
      detail.close();
      break;
    case "settings":
      settings();
      break;
    case "apply-settings":
      state.scenario = document.querySelector("#scenario").value;
      detail.close();
      shell();
      say("Preview state updated.");
      break;
    case "reconnect":
      state.scenario = "normal";
      shell();
      toast("Demo connection restored.");
      break;
    case "command":
      openCommand();
      break;
    case "discover":
      discover();
      break;
    case "clear-filters":
      state.query = "";
      state.network = "all";
      state.filter = "all";
      shell();
      break;
    case "install":
      dialogFrame(
        "Add a reporting agent",
        "Live CPU, memory, storage and host history.",
        `<p class="muted">Install H.O.P.E's signed agent on a Linux or macOS host to receive its measurements.</p><div class="notice" style="margin-top:20px">This is a design demo. Your live console provides the installer and agent credentials for your server.</div><div class="detail-actions"><button data-action="close">Close</button><button class="primary" data-command-page="agents">View sample agents</button></div>`,
        "shield",
      );
      break;
    case "add-network":
      dialogFrame(
        "Add a network",
        "Preview a discovery range.",
        `<form id="network-form"><label class="form-field">Name<input name="name" value="Workshop" required maxlength="40" /></label><label class="form-field">IPv4 range<input name="range" value="192.168.30.0/24" required inputmode="text" /></label><p id="network-error" class="form-error" role="alert"></p><p class="muted small">The demo validates this range and previews it here. It does not save or scan a real network.</p><div class="detail-actions"><button type="button" data-action="close">Cancel</button><button type="submit" class="primary">Preview range</button></div></form>`,
        "network",
      );
      break;
  }
});
document.addEventListener("input", (e) => {
  if (e.target.id === "host-search") {
    state.query = e.target.value;
    document.querySelector("#host-rows").innerHTML = tableRows();
    document.querySelector("#host-count").textContent =
      `${matchingHosts().length} hosts shown`;
    say(`${matchingHosts().length} matching hosts.`);
  }
  if (e.target.id === "command-input") renderCommand(e.target.value);
});
document.addEventListener("submit", (e) => {
  if (e.target.id !== "network-form") return;
  e.preventDefault();
  const form = new FormData(e.target);
  const range = String(form.get("range")).trim();
  const match = range.match(
    /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\/(\d{1,2})$/,
  );
  if (
    !match ||
    match.slice(1, 5).some((v) => Number(v) > 255) ||
    Number(match[5]) < 20 ||
    Number(match[5]) > 32
  ) {
    document.querySelector("#network-error").textContent =
      "Enter a valid IPv4 range from /20 to /32, for example 192.168.30.0/24.";
    return;
  }
  dialogFrame(
    escapeHtml(form.get("name")),
    "Network range preview",
    `<p class="mono" style="font-size:17px">${escapeHtml(range)}</p><p class="muted" style="margin-top:14px">Valid IPv4 range. In the live console, discovery would probe addresses in this range and record responsive devices.</p><p class="panel-note" style="padding:16px 0">Preview only · No network was saved or scanned.</p><div class="detail-actions"><button class="primary" data-action="close">Done</button></div>`,
    "network",
  );
});
document.addEventListener("keydown", (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
    e.preventDefault();
    if (command.open) command.close();
    else if (!detail.open) openCommand();
  }
  if (!command.open) return;
  const buttons = [...command.querySelectorAll(".command-results button")];
  const index = buttons.indexOf(document.activeElement);
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    const next =
      e.key === "ArrowDown"
        ? index + 1
        : index <= 0
          ? buttons.length - 1
          : index - 1;
    buttons[(next + buttons.length) % buttons.length]?.focus();
  }
  if (e.key === "Enter" && document.activeElement.id === "command-input") {
    e.preventDefault();
    buttons[0]?.click();
  }
});
for (const dialog of [detail, command])
  dialog.addEventListener("click", (e) => {
    if (e.target !== dialog) return;
    const r = dialog.getBoundingClientRect();
    if (
      e.clientX < r.left ||
      e.clientX > r.right ||
      e.clientY < r.top ||
      e.clientY > r.bottom
    )
      dialog.close();
  });
detail.addEventListener("click", (e) => {
  if (e.target.closest("[data-command-page]")) detail.close();
});
window.addEventListener("resize", fitMap);
shell();
