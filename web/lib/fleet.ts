import type { Disk, Host, ScanJob } from "@/lib/protocol";
import { memoryPercent } from "@/lib/history";

export type Tone = "good" | "warning" | "critical" | "neutral";
export type StateKey =
  | "reporting"
  | "stale"
  | "revoked"
  | "observed"
  | "unobserved"
  | "stopped"
  | "archived";
export type HostState = { key: StateKey; label: string; tone: Tone };

/** Agents report every 30 s; two missed reports mean the agent is stale. */
export const AGENT_STALE_SECONDS = 120;
/** Discovery runs every five minutes; two missed passes mean a host is unobserved. */
export const OBSERVED_SECONDS = 600;

export function hostState(
  host: Pick<Host, "agent_id" | "agent_revoked" | "last_report" | "last_seen"> &
    Partial<
      Pick<
        Host,
        "agent_uninstalled" | "source" | "lifecycle" | "source_observed_at"
      >
    >,
  now = Date.now() / 1000,
): HostState {
  if (host.agent_revoked && !host.agent_uninstalled)
    return { key: "revoked", label: "Agent revoked", tone: "neutral" };
  if (host.agent_id && !host.agent_uninstalled)
    return host.last_report && now - host.last_report < AGENT_STALE_SECONDS
      ? { key: "reporting", label: "Reporting", tone: "good" }
      : { key: "stale", label: "Agent stale", tone: "critical" };
  if (host.source) {
    if (host.lifecycle === "archived")
      return { key: "archived", label: "Archived", tone: "neutral" };
    if (host.lifecycle === "stopped")
      return { key: "stopped", label: "Stopped", tone: "neutral" };
    if (
      host.lifecycle === "stale" ||
      now - (host.source_observed_at ?? 0) >= OBSERVED_SECONDS
    )
      return { key: "unobserved", label: "Source stale", tone: "warning" };
    return { key: "observed", label: "Running", tone: "good" };
  }
  return now - host.last_seen < OBSERVED_SECONDS
    ? { key: "observed", label: "Observed", tone: "good" }
    : { key: "unobserved", label: "Not recently seen", tone: "neutral" };
}

export function sensorTone(celsius: number): Tone {
  if (celsius >= 95) return "critical";
  if (celsius >= 85) return "warning";
  return "good";
}

export function usageTone(percent: number | null | undefined): Tone {
  if (percent == null || !Number.isFinite(percent)) return "neutral";
  if (percent >= 95) return "critical";
  if (percent >= 85) return "warning";
  return "good";
}

export function diskPercent(disk: Disk): number | null {
  return disk.total_bytes > 0
    ? (1 - disk.available_bytes / disk.total_bytes) * 100
    : null;
}

const GUEST_VOLUME = /(?:^|\/)(?:subvol|basevol)-\d+-disk-\d+\/?$/;

/**
 * Whether a mount is a guest's root filesystem exposed on its hypervisor, such
 * as a Proxmox LXC subvolume. Its usage belongs to the guest, not the host.
 * Mirrors `Disk::is_guest_storage` in hope-protocol.
 */
export function isGuestStorage(disk: Disk): boolean {
  // A dataset named by the hypervisor is still this machine's own root
  // filesystem when the agent is running inside the guest.
  if (disk.mount === "/") return false;
  return (
    GUEST_VOLUME.test(disk.mount) ||
    GUEST_VOLUME.test(disk.name) ||
    /^\/var\/lib\/lxc\/.+\/rootfs\/?$/.test(disk.mount)
  );
}

/** The fullest filesystem a host reports, or null without any usable mount. */
export function worstDisk(host: Host): { disk: Disk; percent: number } | null {
  const byMount = new Map<string, Disk>();
  for (const disk of host.inventory?.disks ?? []) byMount.set(disk.mount, disk);
  // A metrics sample can omit mounts if the OS mount list is truncated. Keep
  // inventory mounts as a fallback, while letting the latest reading replace
  // the same mount when both sources contain it.
  for (const disk of host.metrics?.disks ?? []) byMount.set(disk.mount, disk);
  let worst: { disk: Disk; percent: number } | null = null;
  for (const disk of byMount.values()) {
    if (isGuestStorage(disk)) continue;
    const percent = diskPercent(disk);
    if (percent != null && (!worst || percent > worst.percent))
      worst = { disk, percent };
  }
  return worst;
}

export function cpuPercent(host: Host): number | null {
  const value = host.metrics?.cpu_percent ?? host.child?.metrics.cpu_percent;
  return value != null && Number.isFinite(value) ? value : null;
}
export function memoryUsage(host: Host): number | null {
  if (host.metrics) return memoryPercent(host.metrics);
  const child = host.child?.metrics;
  return child?.memory_used_bytes != null &&
    child.memory_total_bytes != null &&
    child.memory_total_bytes > 0
    ? (child.memory_used_bytes / child.memory_total_bytes) * 100
    : null;
}

export type Issue = {
  host: Host;
  severity: "critical" | "warning";
  kind:
    | "agent"
    | "cpu"
    | "memory"
    | "disk"
    | "pool"
    | "sensor"
    | "service"
    | "monitor";
  message: string;
};

function duration(seconds: number) {
  if (seconds < 3600) return `${Math.max(1, Math.floor(seconds / 60))} min`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)} h`;
  return `${Math.floor(seconds / 86400)} d`;
}

/** Everything that deserves an operator's attention on a host, most severe first. */
export function hostIssues(host: Host, now = Date.now() / 1000): Issue[] {
  const issues: Issue[] = [];
  const state = hostState(host, now);
  if (state.key === "stale")
    issues.push({
      host,
      severity: "critical",
      kind: "agent",
      message: host.last_report
        ? `Agent silent for ${duration(now - host.last_report)}`
        : "Agent has never reported",
    });
  if (state.key === "reporting") {
    const cpu = cpuPercent(host);
    if (cpu != null && cpu >= 90)
      issues.push({
        host,
        severity: cpu >= 98 ? "critical" : "warning",
        kind: "cpu",
        message: `CPU at ${cpu.toFixed(0)}%`,
      });
    const memory = memoryUsage(host);
    if (memory != null && memory >= 90)
      issues.push({
        host,
        severity: memory >= 97 ? "critical" : "warning",
        kind: "memory",
        message: `Memory at ${memory.toFixed(0)}%`,
      });
    const disk = worstDisk(host);
    if (disk && disk.percent >= 90)
      issues.push({
        host,
        severity: disk.percent >= 97 ? "critical" : "warning",
        kind: "disk",
        message: `${disk.disk.mount} is ${disk.percent.toFixed(0)}% full`,
      });
    const metrics = host.metrics;
    for (const pool of metrics?.pools ?? []) {
      const percent =
        pool.size_bytes > 0
          ? (pool.allocated_bytes / pool.size_bytes) * 100
          : 0;
      if (pool.health.toUpperCase() !== "ONLINE")
        issues.push({
          host,
          severity: "critical",
          kind: "pool",
          message: `Pool ${pool.name} is ${pool.health}`,
        });
      else if (percent >= 90)
        issues.push({
          host,
          severity: percent >= 97 ? "critical" : "warning",
          kind: "pool",
          message: `Pool ${pool.name} is ${percent.toFixed(0)}% allocated`,
        });
    }
    const hottest = [...(metrics?.temperatures ?? [])].sort(
      (a, b) => b.celsius - a.celsius,
    )[0];
    if (hottest && hottest.celsius >= 85)
      issues.push({
        host,
        severity: hottest.celsius >= 95 ? "critical" : "warning",
        kind: "sensor",
        message: `${hottest.label} at ${hottest.celsius.toFixed(0)}°C`,
      });
    const failed = (metrics?.services ?? []).filter(
      (unit) => unit.state === "failed",
    );
    const failedCount = Math.max(
      metrics?.service_summary?.failed ?? 0,
      failed.length,
    );
    if (failedCount)
      issues.push({
        host,
        severity: "warning",
        kind: "service",
        message:
          failedCount === 1 && failed.length === 1
            ? `${failed[0].name} failed`
            : `${failedCount} service ${failedCount === 1 ? "unit" : "units"} failed`,
      });
    const down = (metrics?.monitors ?? []).filter((monitor) => !monitor.up);
    if (down.length)
      issues.push({
        host,
        severity: "critical",
        kind: "monitor",
        message:
          down.length === 1
            ? `${down[0].target} is down`
            : `${down.length} monitors are down`,
      });
  }
  return issues.sort((a, b) => rank(a.severity) - rank(b.severity));
}
const rank = (severity: Issue["severity"]) => (severity === "critical" ? 0 : 1);

export function fleetIssues(hosts: Host[], now = Date.now() / 1000): Issue[] {
  return hosts
    .flatMap((host) => hostIssues(host, now))
    .sort(
      (a, b) =>
        rank(a.severity) - rank(b.severity) ||
        (a.host.name_override || a.host.name).localeCompare(
          b.host.name_override || b.host.name,
        ),
    );
}

export type FleetSummary = {
  total: number;
  online: number;
  agents: number;
  reporting: number;
  issues: Issue[];
  cpuAverage: number | null;
  memoryUsed: number;
  memoryTotal: number;
  diskUsed: number;
  diskTotal: number;
  rx: number | null;
  tx: number | null;
  runningScans: ScanJob[];
  lastScan: ScanJob | null;
};

/** Fleet-wide figures from the latest sample of every host; nothing is invented for missing data. */
export function fleetSummary(
  hosts: Host[],
  scans: ScanJob[],
  now = Date.now() / 1000,
): FleetSummary {
  let online = 0;
  let agents = 0;
  let reporting = 0;
  let cpuSum = 0;
  let cpuCount = 0;
  let memoryUsed = 0;
  let memoryTotal = 0;
  let diskUsed = 0;
  let diskTotal = 0;
  let rx: number | null = null;
  let tx: number | null = null;
  for (const host of hosts) {
    const state = hostState(host, now);
    if (state.tone === "good") online++;
    if (host.agent_id && !host.agent_revoked && !host.agent_uninstalled)
      agents++;
    if (state.key !== "reporting") continue;
    reporting++;
    const cpu = cpuPercent(host);
    if (cpu != null) {
      cpuSum += cpu;
      cpuCount++;
    }
    const metrics = host.metrics;
    if (
      metrics?.memory_used_bytes != null &&
      metrics.memory_total_bytes != null &&
      metrics.memory_total_bytes > 0
    ) {
      memoryUsed += metrics.memory_used_bytes;
      memoryTotal += metrics.memory_total_bytes;
    }
    for (const disk of metrics?.disks ?? [])
      if (disk.total_bytes > 0 && !isGuestStorage(disk)) {
        diskUsed += disk.total_bytes - disk.available_bytes;
        diskTotal += disk.total_bytes;
      }
    if (metrics?.network_rx_bytes_per_second != null)
      rx = (rx ?? 0) + metrics.network_rx_bytes_per_second;
    if (metrics?.network_tx_bytes_per_second != null)
      tx = (tx ?? 0) + metrics.network_tx_bytes_per_second;
  }
  const runningScans = scans.filter(
    (scan) => scan.status === "running" || scan.status === "queued",
  );
  const lastScan = [...scans].sort(
    (a, b) => (b.finished_at ?? b.started_at) - (a.finished_at ?? a.started_at),
  )[0];
  return {
    total: hosts.length,
    online,
    agents,
    reporting,
    issues: fleetIssues(hosts, now),
    cpuAverage: cpuCount ? cpuSum / cpuCount : null,
    memoryUsed,
    memoryTotal,
    diskUsed,
    diskTotal,
    rx,
    tx,
    runningScans,
    lastScan: lastScan ?? null,
  };
}

export type HostFilter = "all" | "attention" | "monitored" | StateKey;
export const HOST_FILTERS: { key: HostFilter; label: string }[] = [
  { key: "all", label: "All" },
  { key: "attention", label: "Needs attention" },
  { key: "reporting", label: "Reporting" },
  { key: "observed", label: "Observed" },
  { key: "stopped", label: "Stopped" },
  { key: "archived", label: "Archived" },
  { key: "stale", label: "Agent stale" },
  { key: "unobserved", label: "Not recently seen" },
  { key: "revoked", label: "Revoked" },
];
export const HOST_FILTER_HINTS: Partial<Record<HostFilter, string>> = {
  monitored:
    "Devices opted in to uptime monitoring. Configure checks in each device’s Settings tab.",
  attention:
    "Agents not reporting, and reported resource, service or monitor problems.",
  unobserved: `Devices without an agent that have not been seen for ${OBSERVED_SECONDS / 60} minutes or more. This does not mean the device is offline; it may be asleep or away from Wi-Fi.`,
};
export function matchesFilter(
  host: Host,
  filter: HostFilter,
  now = Date.now() / 1000,
) {
  if (filter === "all") return true;
  if (filter === "monitored") return host.uptime?.enabled ?? false;
  if (filter === "attention") return hostIssues(host, now).length > 0;
  return hostState(host, now).key === filter;
}

export type SortKey =
  "name" | "address" | "status" | "cpu" | "memory" | "disk" | "seen";
export function compareHosts(
  key: SortKey,
  now = Date.now() / 1000,
): (a: Host, b: Host) => number {
  const number = (value: number | null) =>
    value == null ? Number.NEGATIVE_INFINITY : value;
  const toneRank: Record<Tone, number> = {
    critical: 0,
    warning: 1,
    neutral: 2,
    good: 3,
  };
  switch (key) {
    case "name":
      return (a, b) =>
        a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
    case "address":
      return (a, b) =>
        ipValue(a.addresses[0]) - ipValue(b.addresses[0]) ||
        (a.addresses[0] ?? "").localeCompare(b.addresses[0] ?? "") ||
        a.name.localeCompare(b.name);
    case "status":
      return (a, b) =>
        toneRank[hostState(a, now).tone] - toneRank[hostState(b, now).tone] ||
        a.name.localeCompare(b.name);
    case "cpu":
      return (a, b) => number(cpuPercent(b)) - number(cpuPercent(a));
    case "memory":
      return (a, b) => number(memoryUsage(b)) - number(memoryUsage(a));
    case "disk":
      return (a, b) =>
        number(worstDisk(b)?.percent ?? null) -
        number(worstDisk(a)?.percent ?? null);
    case "seen":
      return (a, b) =>
        Math.max(b.last_report ?? 0, b.last_seen) -
        Math.max(a.last_report ?? 0, a.last_seen);
  }
}

/** Matches name, addresses, MACs, OS and observed service names. */
export function matchesSearch(host: Host, query: string) {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return [
    host.name,
    host.name_override ?? "",
    ...host.addresses,
    ...host.macs,
    host.inventory?.os ?? "",
    host.inventory?.cpu_model ?? "",
    ...host.services.map((s) => `${s.port} ${s.name}`),
  ]
    .join(" ")
    .toLowerCase()
    .includes(needle);
}

/** Numeric value of a dotted IPv4 address for sorting; anything else sorts last. */
export function ipValue(address: string | undefined): number {
  if (!address) return Number.MAX_SAFE_INTEGER;
  const parts = address.split(".");
  if (parts.length !== 4) return Number.MAX_SAFE_INTEGER - 1;
  let value = 0;
  for (const part of parts) {
    const octet = Number(part);
    if (!Number.isInteger(octet) || octet < 0 || octet > 255)
      return Number.MAX_SAFE_INTEGER - 1;
    value = value * 256 + octet;
  }
  return value;
}

/** How long a request to update stays pending on the server. */
const UPDATE_REQUEST_SECONDS = 86400;

/** Where an agent that has a newer release on offer stands. */
export type UpdatePlan = {
  /** The release on offer. */
  target: string;
  /**
   * ready: the agent has an updater, so it can be asked from here.
   * pending: it was asked and starts the update when it next reports.
   * manual: it has no updater yet; the upgrade command must be run on the host once.
   */
  state: "ready" | "pending" | "manual";
  /** What the updater last said, when the last update failed. */
  failure: string | null;
};

export function updatePlan(
  host: Host,
  release: string | null | undefined,
  now = Date.now() / 1000,
): UpdatePlan | null {
  const target = agentUpdate(host, release);
  if (!target) return null;
  const updater = host.collectors.find((c) => c.name === "agent_update");
  const pending =
    host.update_requested_at != null &&
    host.update_target != null &&
    now - host.update_requested_at < UPDATE_REQUEST_SECONDS;
  const failed =
    updater?.available && updater.detail?.startsWith("The last update failed");
  return {
    target,
    state: pending ? "pending" : updater?.available ? "ready" : "manual",
    failure: failed ? (updater.detail ?? null) : null,
  };
}

/** How many agents have a newer release on offer, by what can be done about it. */
export function fleetUpdates(
  hosts: Host[],
  release: string | null | undefined,
  now = Date.now() / 1000,
) {
  const counts = { ready: 0, pending: 0, manual: 0, total: 0 };
  for (const host of hosts) {
    const plan = updatePlan(host, release, now);
    if (!plan) continue;
    counts[plan.state]++;
    counts.total++;
  }
  return counts;
}

/** Numeric parts of a version (`0.1.57` → [0, 1, 57]); anything after the numbers is ignored. */
function versionParts(version: string): number[] {
  return version
    .split(/[-+]/)[0]
    .split(".")
    .map((part) => Number.parseInt(part, 10) || 0);
}

/**
 * The release an agent can update to: the server's signed release when it is newer than the
 * version the agent last reported. Agents too old to report a version are not flagged.
 */
export function agentUpdate(
  host: Host,
  release: string | null | undefined,
): string | null {
  if (
    !host.agent_id ||
    host.agent_revoked ||
    host.agent_uninstalled ||
    !host.agent_version ||
    !release
  )
    return null;
  const have = versionParts(host.agent_version);
  const offer = versionParts(release);
  for (let i = 0; i < Math.max(have.length, offer.length); i++) {
    const difference = (offer[i] ?? 0) - (have[i] ?? 0);
    if (difference !== 0) return difference > 0 ? release : null;
  }
  return null;
}
