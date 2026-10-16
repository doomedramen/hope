import { test, expect } from "@playwright/test";
import {
  compareHosts,
  fleetSummary,
  hostIssues,
  hostState,
  matchesFilter,
  matchesSearch,
} from "../lib/fleet";
import type { Host, Metrics } from "../lib/protocol";

const now = 1_700_000_000;
const metrics: Metrics = {
  cpu_percent: 20,
  memory_used_bytes: 4e9,
  memory_total_bytes: 8e9,
  network_rx_bytes_per_second: 1000,
  network_tx_bytes_per_second: 500,
  network_interfaces: [],
  memory_cached_bytes: null,
  memory_zfs_arc_bytes: null,
  swap_used_bytes: null,
  swap_total_bytes: null,
  load_average: null,
  disk_read_bytes_per_second: null,
  disk_write_bytes_per_second: null,
  disk_io: [],
  pools: [],
  temperatures: [],
  services: [],
  service_summary: null,
  monitors: [],
  disks: [
    {
      name: "nvme0",
      mount: "/",
      filesystem: "ext4",
      total_bytes: 100,
      available_bytes: 2,
    },
  ],
  gpus: [],
};
function host(overrides: Partial<Host>): Host {
  return {
    id: "h",
    agent_id: null,
    name: "host",
    addresses: ["192.168.1.2"],
    macs: [],
    network_ids: [],
    last_seen: now,
    last_report: null,
    last_collected_at: null,
    agent_revoked: false,
    enrichment_status: "completed",
    discovery_method: "icmp",
    inventory: null,
    metrics: null,
    collectors: [],
    services: [{ port: 22, name: "SSH", observed_at: now }],
    ...overrides,
  };
}

test("host state distinguishes reporting, stale, revoked, observed and unobserved", () => {
  expect(hostState(host({}), now).key).toBe("observed");
  expect(hostState(host({ last_seen: now - 601 }), now).key).toBe("unobserved");
  expect(
    hostState(host({ agent_id: "a", last_report: now - 30 }), now).key,
  ).toBe("reporting");
  expect(
    hostState(host({ agent_id: "a", last_report: now - 121 }), now).key,
  ).toBe("stale");
  expect(
    hostState(host({ agent_id: "a", agent_revoked: true }), now).tone,
  ).toBe("neutral");
});

test("issues cover silent agents, unobserved hosts and full resources, most severe first", () => {
  const full = host({
    id: "full",
    name: "full",
    agent_id: "a",
    last_report: now - 10,
    metrics: { ...metrics, cpu_percent: 99, memory_used_bytes: 7.3e9 },
  });
  const issues = hostIssues(full, now);
  expect(issues.map((i) => [i.kind, i.severity])).toEqual([
    ["cpu", "critical"],
    ["disk", "critical"],
    ["memory", "warning"],
  ]);
  expect(issues[1].message).toBe("/ is 98% full");
  expect(
    hostIssues(host({ agent_id: "a", last_report: null }), now)[0].message,
  ).toBe("Agent has never reported");
  expect(hostIssues(host({ last_seen: now - 7200 }), now)[0].message).toBe(
    "Not observed for 2 h",
  );
  // Stale metrics never raise resource issues.
  expect(
    hostIssues(
      host({
        agent_id: "a",
        last_report: now - 1000,
        metrics: { ...metrics, cpu_percent: 100 },
      }),
      now,
    ).map((i) => i.kind),
  ).toEqual(["agent"]);
});

test("pools, sensors, failed units and down monitors raise issues only while reporting", () => {
  const rich = host({
    agent_id: "a",
    last_report: now - 10,
    metrics: {
      ...metrics,
      disks: [],
      pools: [
        {
          name: "tank",
          kind: "zfs",
          size_bytes: 100,
          allocated_bytes: 50,
          health: "DEGRADED",
          read_bytes_per_second: null,
          write_bytes_per_second: null,
          root: false,
        },
        {
          name: "rpool",
          kind: "zfs",
          size_bytes: 100,
          allocated_bytes: 98,
          health: "ONLINE",
          read_bytes_per_second: null,
          write_bytes_per_second: null,
          root: true,
        },
      ],
      temperatures: [
        { sensor: "a", label: "Package id 0", celsius: 96 },
        { sensor: "b", label: "nvme", celsius: 40 },
      ],
      services: [
        {
          name: "nginx.service",
          state: "failed",
          sub_state: "failed",
          description: "",
          watched: false,
        },
      ],
      monitors: [
        {
          target: "tcp://10.0.0.1:22",
          kind: "tcp",
          up: false,
          latency_ms: null,
          status_code: null,
          detail: "timed out",
        },
      ],
    },
  });
  expect(
    hostIssues(rich, now).map((i) => [i.kind, i.severity, i.message]),
  ).toEqual([
    ["pool", "critical", "Pool tank is DEGRADED"],
    ["pool", "critical", "Pool rpool is 98% allocated"],
    ["sensor", "critical", "Package id 0 at 96°C"],
    ["monitor", "critical", "tcp://10.0.0.1:22 is down"],
    ["service", "warning", "nginx.service failed"],
  ]);
  expect(
    hostIssues({ ...rich, last_report: now - 1000 }, now).map((i) => i.kind),
  ).toEqual(["agent"]);
});

test("fleet summary sums only reporting agents and never invents missing values", () => {
  const reporting = host({
    id: "a",
    name: "a",
    agent_id: "a",
    last_report: now - 5,
    metrics,
  });
  const partial = host({
    id: "b",
    name: "b",
    agent_id: "b",
    last_report: now - 5,
    metrics: {
      ...metrics,
      cpu_percent: null,
      memory_used_bytes: null,
      network_rx_bytes_per_second: null,
      disks: [],
    },
  });
  const stale = host({
    id: "c",
    name: "c",
    agent_id: "c",
    last_report: now - 5000,
    metrics: { ...metrics, cpu_percent: 100 },
  });
  const summary = fleetSummary(
    [reporting, partial, stale, host({ id: "d" })],
    [],
    now,
  );
  expect(summary.total).toBe(4);
  expect(summary.online).toBe(3);
  expect(summary.agents).toBe(3);
  expect(summary.reporting).toBe(2);
  expect(summary.cpuAverage).toBe(20);
  expect(summary.memoryUsed).toBe(4e9);
  expect(summary.memoryTotal).toBe(8e9);
  expect(summary.diskUsed).toBe(98);
  expect(summary.rx).toBe(1000);
  expect(summary.tx).toBe(1000);
  expect(summary.issues.map((i) => i.host.id)).toEqual(["a", "c"]);
  expect(fleetSummary([partial], [], now).cpuAverage).toBeNull();
  expect(fleetSummary([partial], [], now).rx).toBeNull();
});

test("filters, search and sorting", () => {
  const a = host({
    id: "a",
    name: "alpha",
    agent_id: "a",
    last_report: now,
    metrics,
  });
  const b = host({
    id: "b",
    name: "beta",
    inventory: { ...inventoryOf("macOS 15"), hostname: "beta" },
    last_seen: now - 10000,
  });
  expect(matchesFilter(a, "reporting", now)).toBeTruthy();
  expect(matchesFilter(b, "unobserved", now)).toBeTruthy();
  expect(matchesFilter(a, "attention", now)).toBeTruthy();
  expect(matchesFilter(b, "attention", now)).toBeTruthy();
  expect(matchesSearch(b, "macos")).toBeTruthy();
  expect(matchesSearch(a, "22 ssh")).toBeTruthy();
  expect(matchesSearch(a, "nothing")).toBeFalsy();
  expect([b, a].sort(compareHosts("name", now)).map((h) => h.id)).toEqual([
    "a",
    "b",
  ]);
  expect([a, b].sort(compareHosts("status", now)).map((h) => h.id)).toEqual([
    "b",
    "a",
  ]);
  expect([b, a].sort(compareHosts("cpu", now)).map((h) => h.id)).toEqual([
    "a",
    "b",
  ]);
});

function inventoryOf(os: string) {
  return {
    hostname: "x",
    os,
    architecture: "aarch64",
    cpu_model: "Apple M4",
    cpu_cores: 8,
    memory_total_bytes: 1,
    interfaces: [],
    disks: [],
    gpus: [],
    collectors: [],
  };
}
