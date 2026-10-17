import { test, expect } from "@playwright/test";
import {
  agentUpdate,
  compareHosts,
  fleetSummary,
  fleetUpdates,
  hostIssues,
  hostState,
  matchesFilter,
  matchesSearch,
  updatePlan,
  worstDisk,
} from "../lib/fleet";
import { deviceAppIcon, deviceFingerprintId } from "../lib/devices";
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
  containers: [],
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
    agent_version: null,
    update_requested_at: null,
    update_target: null,
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
    vendor: null,
    device: null,
    evidence: {
      dns_name: null,
      mdns_name: null,
      mdns_services: [],
      mdns_model: null,
      mdns_instances: [],
      ssdp_server: null,
      upnp_device_type: null,
      upnp_friendly_name: null,
      upnp_manufacturer: null,
      upnp_model: null,
      http_title: null,
      http_server: null,
    },
    ports_scanned_at: null,
    parent_id: null,
    parent_reason: null,
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

test("consumer device labels map to UniFi fingerprint icons", () => {
  expect(deviceFingerprintId(host({ name: "iPhone" }))).toBe("91");
  expect(deviceFingerprintId(host({ name: "Kindle Paperwhite" }))).toBe("41");
  expect(deviceFingerprintId(host({ name: "Kindle Fire tablet" }))).toBe("67");
  expect(deviceFingerprintId(host({ name: "XBOX", vendor: "Microsoft" }))).toBe(
    "2271",
  );
  expect(deviceFingerprintId(host({ name: "unidentified device" }))).toBeNull();
});

test("recognizable self-hosted app hosts use app artwork", () => {
  expect(deviceAppIcon(host({ name: "Plex Media Server" }))).toBe("plex");
  expect(deviceAppIcon(host({ name: "ordinary server" }))).toBeNull();
});

test("issues cover silent agents and full resources, most severe first", () => {
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
  expect(hostIssues(host({ last_seen: now - 7200 }), now)).toEqual([]);
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

test("guest volumes on a hypervisor never stand in for the host's own disks", () => {
  const disk = (name: string, mount: string, available_bytes: number) => ({
    name,
    mount,
    filesystem: "zfs",
    total_bytes: 100,
    available_bytes,
  });
  const proxmox = host({
    agent_id: "pve",
    last_report: now,
    metrics: {
      ...metrics,
      disks: [
        disk("rpool/ROOT/pve-1", "/", 60),
        disk(
          "rpool/data/subvol-123-disk-0",
          "/rpool/data/subvol-123-disk-0",
          1,
        ),
        disk(
          "rpool/data/basevol-900-disk-0",
          "/rpool/data/basevol-900-disk-0",
          1,
        ),
        disk("/dev/sdb1", "/var/lib/lxc/101/rootfs", 1),
      ],
    },
  });
  expect(worstDisk(proxmox)?.disk.mount).toBe("/");
  expect(worstDisk(proxmox)?.percent).toBe(40);
  expect(hostIssues(proxmox, now).some((i) => i.kind === "disk")).toBe(false);
  expect(fleetSummary([proxmox], [], now).diskTotal).toBe(100);
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
  expect(matchesFilter(a, "unobserved", now)).toBeFalsy();
  expect(matchesFilter(b, "attention", now)).toBeFalsy();
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
  const ip = (id: string, addresses: string[]) => host({ id, addresses });
  expect(
    [
      ip("none", []),
      ip("v6", ["fe80::1"]),
      ip("hundred", ["10.0.0.100"]),
      ip("nine", ["10.0.0.9"]),
      ip("public", ["192.168.1.2"]),
    ]
      .sort(compareHosts("address", now))
      .map((h) => h.id),
  ).toEqual(["nine", "hundred", "public", "v6", "none"]);
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

test("agents are offered an update only when the server's release is newer", () => {
  const agent = host({ agent_id: "a", agent_version: "0.1.57" });
  expect(agentUpdate(agent, "0.1.60")).toBe("0.1.60");
  expect(agentUpdate(agent, "0.2.1")).toBe("0.2.1");
  expect(agentUpdate(agent, "0.1.57")).toBeNull();
  expect(agentUpdate(agent, "0.1.9")).toBeNull();
  expect(agentUpdate(agent, null)).toBeNull();
  // Agents too old to report a version, discovered hosts and revoked agents are never flagged.
  expect(agentUpdate(host({ agent_id: "a" }), "0.1.60")).toBeNull();
  expect(agentUpdate(host({ agent_version: "0.1.1" }), "0.1.60")).toBeNull();
  expect(agentUpdate({ ...agent, agent_revoked: true }, "0.1.60")).toBeNull();
});

test("an update plan says whether the agent can be asked, is waiting, or needs the command once", () => {
  const updater = (available: boolean, detail = "Updater ready") => [
    { name: "agent_update", available, detail },
  ];
  const old = (overrides: Partial<Host> = {}) =>
    host({
      agent_id: "a",
      agent_version: "0.1.57",
      collectors: updater(true),
      ...overrides,
    });
  expect(updatePlan(old(), "0.1.60", now)).toEqual({
    target: "0.1.60",
    state: "ready",
    failure: null,
  });
  // No updater reported, or none installed: the upgrade command is the way.
  expect(updatePlan(old({ collectors: [] }), "0.1.60", now)?.state).toBe(
    "manual",
  );
  expect(
    updatePlan(old({ collectors: updater(false) }), "0.1.60", now)?.state,
  ).toBe("manual");
  // Asked, and still inside the server's window.
  const asked = { update_requested_at: now - 600, update_target: "0.1.60" };
  expect(updatePlan(old(asked), "0.1.60", now)?.state).toBe("pending");
  expect(
    updatePlan(
      old({ ...asked, update_requested_at: now - 90000 }),
      "0.1.60",
      now,
    )?.state,
  ).toBe("ready");
  // A failed attempt is shown but does not stop another request.
  const failed = old({
    collectors: updater(true, "The last update failed (BAD_SIGNATURE)"),
  });
  expect(updatePlan(failed, "0.1.60", now)).toMatchObject({
    state: "ready",
    failure: "The last update failed (BAD_SIGNATURE)",
  });
  // Up to date, unknown version, revoked or discovered-only hosts have no plan.
  expect(
    updatePlan(old({ agent_version: "0.1.60" }), "0.1.60", now),
  ).toBeNull();
  expect(updatePlan(old({ agent_version: null }), "0.1.60", now)).toBeNull();
  expect(updatePlan(old({ agent_revoked: true }), "0.1.60", now)).toBeNull();
  expect(updatePlan(old(), null, now)).toBeNull();
});

test("the fleet counts agents by what can be done about their update", () => {
  const hosts = [
    host({
      id: "1",
      agent_id: "1",
      agent_version: "0.1.57",
      collectors: [{ name: "agent_update", available: true, detail: null }],
    }),
    host({
      id: "2",
      agent_id: "2",
      agent_version: "0.1.57",
      collectors: [{ name: "agent_update", available: true, detail: null }],
    }),
    host({
      id: "3",
      agent_id: "3",
      agent_version: "0.1.57",
      collectors: [{ name: "agent_update", available: true, detail: null }],
      update_requested_at: now - 60,
      update_target: "0.1.60",
    }),
    host({ id: "4", agent_id: "4", agent_version: "0.1.50" }),
    host({ id: "5", agent_id: "5", agent_version: "0.1.60" }),
    host({ id: "6" }),
  ];
  expect(fleetUpdates(hosts, "0.1.60", now)).toEqual({
    ready: 2,
    pending: 1,
    manual: 1,
    total: 4,
  });
  expect(fleetUpdates(hosts, null, now)).toEqual({
    ready: 0,
    pending: 0,
    manual: 0,
    total: 0,
  });
});
