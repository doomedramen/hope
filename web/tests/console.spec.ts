import {
  test as base,
  expect,
  type APIRequestContext,
  type BrowserContext,
  type Page,
} from "@playwright/test";
import { randomUUID, randomBytes } from "node:crypto";
import type { Bootstrap } from "@/lib/protocol";
let password = "hope-browser-test-password";
let adminState: Awaited<ReturnType<BrowserContext["storageState"]>> | undefined;
const test = base.extend({
  storageState: async ({ browserName }, provide) => {
    void browserName;
    await provide(adminState);
  },
});

async function expectMapFits(page: import("@playwright/test").Page) {
  await page.getByRole("button", { name: "Fit all", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() => {
        const canvas = document
          .querySelector(".react-flow")
          ?.getBoundingClientRect();
        const nodes = [...document.querySelectorAll(".map-device")];
        return (
          !!canvas &&
          nodes.length > 0 &&
          nodes.every((node) => {
            const box = node.getBoundingClientRect();
            return (
              box.left >= canvas.left - 2 &&
              box.right <= canvas.right + 2 &&
              box.top >= canvas.top - 2 &&
              box.bottom <= canvas.bottom + 2
            );
          })
        );
      }),
    )
    .toBeTruthy();
}

async function navigate(page: Page, name: string | RegExp) {
  const target = page
    .getByRole("navigation", { name: "Main navigation" })
    .getByRole("button", { name });
  if (!(await target.isVisible()))
    await page.getByRole("button", { name: "Navigation", exact: true }).click();
  await target.click();
}

async function globalAction(page: Page, name: string) {
  const target = page
    .getByRole("button", { name, exact: true })
    .filter({ visible: true })
    .first();
  const moreActions = page.getByRole("button", {
    name: "More actions",
    exact: true,
  });
  await expect
    .poll(
      async () => (await target.count()) > 0 || (await moreActions.isVisible()),
      { timeout: 10_000 },
    )
    .toBeTruthy();
  if (!(await target.count())) {
    await moreActions.click();
    await expect(target).toBeVisible();
  }
  await target.click();
}

/** A bounded, representative snapshot for visual and interaction acceptance. */
function refinementSnapshot(bootstrap: Bootstrap): Bootstrap {
  const template = bootstrap.hosts.find((host) => host.metrics)!;
  const now = Date.now() / 1000;
  const gib = 1024 ** 3;
  const names = [
    "gateway",
    "proxmox-01",
    "atlas-nas",
    "docker-01",
    "dns-01",
    "immich",
    "home-assistant",
    "studio-mac",
    "backup-01",
    "worker",
    "worker",
    "workshop-pi",
  ];
  const parents = [null, 0, 0, 0, 1, 1, 1, 0, 0, 3, 3, null];
  const hosts = names.map((name, index) => {
    const address = `192.168.1.${index + 1}`;
    const used = [0, 48, 88, 12, 3.8, 6, 2, 14, 4, 3, 2, 1][index];
    const total = [8, 128, 128, 32, 4, 16, 4, 32, 16, 4, 4, 4][index];
    const host: Bootstrap["hosts"][number] = {
      ...template,
      id: `refinement-${index}`,
      name,
      name_override: null,
      icon_override: null,
      addresses: [address],
      primary_ip: address,
      agent_id: index === 0 ? null : `refinement-agent-${index}`,
      agent_revoked: false,
      agent_uninstalled: false,
      retired: false,
      source: null,
      source_device_id: null,
      source_observed_at: null,
      child: null,
      lifecycle: null,
      associations: [],
      relationships: [],
      last_report: index === 11 ? now - 300 : now,
      last_seen: now,
      last_collected_at: now,
      metrics_received_at: now,
      agent_version: bootstrap.agent_release,
      update_requested_at: null,
      update_target: null,
      uptime: { enabled: false, device: false, ports: [] },
      network_ids: ["refinement-home"],
      parent_id: parents[index] == null ? null : `refinement-${parents[index]}`,
      parent_reason: "Observed routing gateway",
      device: {
        kind:
          index === 0
            ? "router"
            : index === 1
              ? "hypervisor"
              : index === 2
                ? "nas"
                : "server",
        label: index === 0 ? "Router" : "Server",
        confidence: 100,
        reasons: [],
        manual: false,
      },
      inventory: template.inventory
        ? { ...template.inventory, hostname: name, interfaces: [], disks: [] }
        : null,
      metrics:
        index === 0
          ? null
          : {
              ...template.metrics!,
              cpu_percent: [0, 38, 17, 42, 8, 24, 6, 12, 4, 36, 18, 7][index],
              memory_used_bytes: used * gib,
              memory_total_bytes: total * gib,
              disks: [
                {
                  name: "root",
                  mount: "/",
                  filesystem: "ext4",
                  total_bytes: 1000 * gib,
                  available_bytes: (index === 2 ? 40 : 650) * gib,
                },
              ],
              pools:
                index === 2
                  ? [
                      {
                        name: "tank",
                        kind: "zfs",
                        size_bytes: 8e12,
                        allocated_bytes: 6e12,
                        health: "DEGRADED",
                        read_bytes_per_second: null,
                        write_bytes_per_second: null,
                        root: false,
                      },
                    ]
                  : [],
              services: [],
              service_summary: null,
              monitors: [],
              temperatures: [],
              containers: [],
              gpus: [],
            },
    };
    if ([4, 5, 6, 9, 10].includes(index))
      host.relationships = [
        {
          from_device_id: host.id,
          to_device_id: host.parent_id!,
          kind: "runs_on",
          detail: null,
        },
      ];
    return host;
  });
  return {
    ...bootstrap,
    hosts,
    networks: [
      {
        id: "refinement-home",
        name: "Home network",
        cidr: "192.168.1.0/24",
        enabled: true,
        interval_minutes: 5,
      },
    ],
    scans: [
      {
        id: "refinement-scan",
        network_id: "refinement-home",
        cidr: "192.168.1.0/24",
        status: "completed",
        scanned: 254,
        total: 254,
        discovered: 12,
        started_at: now - 180,
        finished_at: now - 150,
        error: null,
      },
    ],
    map: { revision: bootstrap.map.revision, positions: [], links: [] },
  };
}

function extended(sample: number) {
  return {
    system_uptime: {
      seconds: 172800 + sample * 60,
      boot_time: 1700000000,
      boot_id: "28046bda-bbbf-4101-9455-8a7818f0969b",
      scope: "system",
    },
    memory_cached_bytes: 6e9,
    memory_zfs_arc_bytes: 4e9,
    swap_used_bytes: 1e8,
    swap_total_bytes: 2e9,
    load_average: { one: 1.2 + sample * 0.1, five: 0.9, fifteen: 0.7 },
    disk_io: [
      {
        device: "nvme0n1",
        read_bytes_per_second: 1e6 * (sample + 1),
        write_bytes_per_second: 5e5,
        busy_percent: 12,
        root: true,
      },
    ],
    pools: [
      {
        name: "rpool",
        kind: "zfs",
        size_bytes: 2e12,
        allocated_bytes: 1.9e12,
        health: "ONLINE",
        read_bytes_per_second: 2e6,
        write_bytes_per_second: 1e6,
        root: false,
      },
      {
        name: "tank",
        kind: "zfs",
        size_bytes: 8e12,
        allocated_bytes: 3e12,
        health: "DEGRADED",
        read_bytes_per_second: null,
        write_bytes_per_second: null,
        root: false,
      },
    ],
    temperatures: [
      { sensor: "coretemp:temp1", label: "Package id 0", celsius: 61 + sample },
      { sensor: "nvme:temp1", label: "nvme Composite", celsius: 44 },
    ],
    services: [
      {
        name: "ssh.service",
        state: "active",
        sub_state: "running",
        description: "OpenBSD Secure Shell server",
        watched: true,
      },
      {
        name: "nginx.service",
        state: "failed",
        sub_state: "failed",
        description: "A high performance web server",
        watched: false,
      },
      {
        name: "ghost.service",
        state: "not-found",
        sub_state: "dead",
        description: "Missing watched unit",
        watched: true,
      },
    ],
    service_summary: { active: 40, failed: 1, total: 42 },
    containers: [
      {
        id: "a1b2c3d4e5f6",
        name: "immich-server",
        image: "ghcr.io/immich-app/immich-server:release",
        state: "running",
        status: "Up 2 days",
        ports: "2283/tcp",
        cpu_percent: 33.4,
        memory_used_bytes: 321 * 1024 * 1024,
        memory_limit_bytes: 2 * 1024 * 1024 * 1024,
        memory_percent: 15.7,
        network_rx_bytes: 64 * 1024 * 1024,
        network_tx_bytes: 8 * 1024 * 1024,
        block_read_bytes: 1 * 1024 * 1024 * 1024,
        block_write_bytes: 2 * 1024 * 1024,
        pids: 18,
      },
    ],
    monitors: [
      {
        target: "tcp://192.168.1.1:22",
        kind: "tcp",
        up: true,
        latency_ms: 2.5 + sample,
        status_code: null,
        detail: null,
      },
      {
        target: "https://example.com/",
        kind: "https",
        up: false,
        latency_ms: null,
        status_code: null,
        detail: "timed out",
      },
    ],
  };
}

async function seed(request: APIRequestContext, includeProxmox = false) {
  // The request fixture does not inherit the browser's authenticated storage
  // state. Sign it in before using the protected inventory and enrollment APIs.
  const auth = (await (await request.get("/api/v1/auth/status")).json()) as {
    setup_required: boolean;
  };
  const session = auth.setup_required
    ? await request.post("/api/v1/auth/setup", {
        data: { setup_token: "hope-playwright-setup-token", password },
      })
    : await request.post("/api/v1/auth/login", { data: { password } });
  expect(session.ok(), await session.text()).toBeTruthy();
  await request.post("/api/v1/networks", {
    data: { name: "Home network", cidr: "192.168.1.0/24", enabled: false },
  });
  await request.post("/api/v1/networks", {
    data: { name: "Workshop", cidr: "192.168.30.0/24", enabled: false },
  });
  const names = includeProxmox
    ? ["proxmox-fixture-node"]
    : ["atlas-nas", "studio-mac", "workshop-pi", "compute-node"];
  let proxmoxAgent = "";
  let proxmoxCredential = "";
  for (let i = 0; i < names.length; i++) {
    const enrollment = await (
      await request.post("/api/v1/enrollment-tokens", { data: {} })
    ).json();
    const agent = randomUUID();
    const inventory = {
      hostname: names[i],
      os: i === 1 ? "macOS 15" : "Linux",
      architecture: i === 1 || i === 2 ? "aarch64" : "x86_64",
      cpu_model: i === 1 ? "Apple M4" : "AMD Ryzen",
      cpu_cores: 8,
      memory_total_bytes: 34359738368,
      interfaces: [
        {
          name: "eth0",
          mac: `02:00:00:00:00:0${i + 1}`,
          addresses: [i === 2 ? "192.168.30.12" : `192.168.1.${10 + i}`],
          gateway: i === 2 ? "192.168.30.1" : "192.168.1.1",
        },
      ],
      disks: [
        {
          name: "nvme0",
          mount: "/",
          filesystem: "ext4",
          total_bytes: 1e12,
          available_bytes: 6e11,
        },
      ],
      gpus: [],
      collectors: [],
    };
    const enrolled = await request.post("/api/v1/agent/enroll", {
      data: {
        schema_version: 1,
        token: enrollment.token,
        agent_id: agent,
        credential: randomBytes(32).toString("hex"),
        inventory,
      },
    });
    expect(enrolled.ok(), await enrolled.text()).toBeTruthy();
    const credential = (await enrolled.json()).credential;
    if (includeProxmox && i === 0) {
      proxmoxAgent = agent;
      proxmoxCredential = credential;
    }
    for (let sample = 0; sample < 5; sample++) {
      const response = await request.post("/api/v1/agent/report", {
        headers: { Authorization: `Bearer ${credential}` },
        data: {
          schema_version: 1,
          report_id: randomUUID(),
          agent_id: agent,
          collected_at: Date.now() / 1000 - (4 - sample) * 60,
          inventory: sample === 0 ? inventory : null,
          metrics: {
            cpu_percent: 12 + i * 8 + sample * 2,
            memory_used_bytes: i === 3 ? null : 8589934592,
            memory_total_bytes: 34359738368,
            network_rx_bytes_per_second: 12000,
            network_tx_bytes_per_second: 4000,
            disk_read_bytes_per_second: null,
            disk_write_bytes_per_second: null,
            disks: inventory.disks,
            gpus: [],
            // The first host reports everything the extended agent can collect.
            ...(i === 0 ? extended(sample) : {}),
          },
          collectors: [
            {
              name: "gpu_metrics",
              available: false,
              detail: "No supported GPU driver",
            },
            ...(includeProxmox
              ? [
                  {
                    name: "proxmox_children",
                    available: true,
                    detail: null,
                  },
                ]
              : []),
          ],
        },
      });
      expect(response.ok()).toBeTruthy();
    }
  }
  if (includeProxmox) {
    const proxmoxScope = randomUUID();
    const childBatch = await request.post("/api/v1/agent/children", {
      headers: { Authorization: `Bearer ${proxmoxCredential}` },
      data: {
        schema_version: 1,
        batch_id: randomUUID(),
        agent_id: proxmoxAgent,
        source: "proxmox",
        source_scope: proxmoxScope,
        sequence: 1,
        inventory_generation: 1,
        inventory_id: randomUUID(),
        chunk_index: 0,
        chunk_count: 1,
        complete: true,
        observed_at: Date.now() / 1000,
        devices: [
          {
            identity: {
              kind: "proxmox_lxc",
              scope: proxmoxScope,
              key: "202",
              incarnation: "lxc-incarnation",
            },
            name: "dns-container",
            guest_agent_status: "available",
            guest_os: {
              hostname: "dns-container",
              name: "Debian GNU/Linux 13",
              kernel_release: "6.14.0-fixture",
              observed_at: Date.now() / 1000,
            },
            guest_filesystems: [
              {
                mountpoint: "/",
                filesystem_type: "ext4",
                total_bytes: 16e9,
                used_bytes: 5e9,
                observed_at: Date.now() / 1000,
              },
            ],
            network_configuration: [
              {
                name: "net0",
                bridge: "vmbr0",
                vlan_tag: 42,
                model: "virtio",
                // Deterministic test-only MAC; the UI route below models the
                // canonical scan host created from this configured NIC.
                mac: "02:fe:00:00:00:02",
              },
            ],
            disk_configuration: [
              {
                name: "scsi0",
                storage: "local-lvm",
                volume: "vm-202-disk-0",
                size_bytes: 32e9,
              },
            ],
            lifecycle: "running",
            status: "running",
            health: null,
            restart_count: null,
            parent_key: null,
            node: "proxmox-fixture-node",
            tags: ["infra"],
            interfaces: [],
            cpu_cores: 2,
            memory_bytes: 2 * 1024 * 1024 * 1024,
            image: null,
            compose_project: null,
            compose_service: null,
            published_ports: [],
            metrics: {
              cpu_percent: 15,
              cpu_capacity_cores: 2,
              cpu_measurement: "hypervisor_guest",
              memory_used_bytes: 512 * 1024 * 1024,
              memory_total_bytes: 2 * 1024 * 1024 * 1024,
              memory_measurement: "hypervisor_guest",
              disk_used_bytes: 4.7 * 1024 ** 3,
              disk_total_bytes: 14.9 * 1024 ** 3,
              network_rx_bytes_per_second: null,
              network_tx_bytes_per_second: null,
              network_rx_bytes: null,
              network_tx_bytes: null,
              disk_read_bytes: 1024 * 1024,
              disk_write_bytes: 512 * 1024,
              disk_read_bytes_per_second: 2048,
              disk_write_bytes_per_second: 1024,
              uptime_seconds: 86400,
            },
          },
        ],
      },
    });
    expect(childBatch.ok(), await childBatch.text()).toBeTruthy();
  }
}

async function designAgent(
  request: APIRequestContext,
  name: string,
  total = 4000,
) {
  const enrollment = await (
    await request.post("/api/v1/enrollment-tokens", { data: {} })
  ).json();
  const agent = randomUUID();
  const inventory = {
    hostname: name,
    os: "Linux",
    architecture: "x86_64",
    cpu_model: "Fixture CPU",
    cpu_cores: 4,
    memory_total_bytes: total,
    interfaces: [],
    disks: [],
    gpus: [],
    collectors: [],
  };
  const response = await request.post("/api/v1/agent/enroll", {
    data: {
      schema_version: 1,
      token: enrollment.token,
      agent_id: agent,
      credential: randomBytes(32).toString("hex"),
      inventory,
    },
  });
  expect(response.ok(), await response.text()).toBeTruthy();
  const credential = (await response.json()).credential;
  let sequence = 0;
  return {
    agent,
    async report(cpu: number, memory: number, disk = 40) {
      const response = await request.post("/api/v1/agent/report", {
        headers: { Authorization: `Bearer ${credential}` },
        data: {
          schema_version: 1,
          report_id: randomUUID(),
          agent_id: agent,
          sequence: ++sequence,
          collected_at: Date.now() / 1000,
          inventory: null,
          collectors: [],
          metrics: {
            cpu_percent: cpu,
            memory_used_bytes: memory,
            memory_total_bytes: total,
            disks: [
              {
                name: "fixture",
                mount: "/data",
                filesystem: "ext4",
                total_bytes: 1000,
                available_bytes: (100 - disk) * 10,
              },
            ],
            gpus: [],
          },
        },
      });
      expect(response.ok(), await response.text()).toBeTruthy();
    },
  };
}

test.describe.serial("H.O.P.E console", () => {
  test("protected first-run setup and empty state", async ({ page }) => {
    await page.goto("/");
    await expect(
      page.getByRole("heading", { name: "Set up H.O.P.E" }),
    ).toBeVisible();
    await page.screenshot({ path: "test-results/hope-setup.png" });
    await page
      .getByLabel("Setup token", { exact: true })
      .fill("hope-playwright-setup-token");
    await page.getByLabel("Administrator password").fill(password);
    await page.getByRole("button", { name: "Create account" }).click();
    await expect(page.getByText("No devices yet")).toBeVisible();
    await page.screenshot({
      path: "test-results/hope-empty.png",
      fullPage: true,
    });
    adminState = await page.context().storageState();
    await page.getByRole("button", { name: "Add agent", exact: true }).click();
    await expect(
      page.getByText("No signed agent packages are available.", {
        exact: false,
      }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Generate installation command" }),
    ).toBeDisabled();
    await page.keyboard.press("Escape");
  });
  test("a stalled snapshot times out and a queued refresh recovers the console", async ({
    page,
  }) => {
    await page.clock.install();
    let attempts = 0;
    let release!: () => void;
    const stalled = new Promise<void>((resolve) => {
      release = resolve;
    });
    await page.route("**/api/v1/bootstrap", async (route) => {
      if (++attempts === 1) {
        await stalled;
        await route.abort().catch(() => {});
      } else await route.continue();
    });
    await page.goto("/?view=hosts");
    await expect.poll(() => attempts).toBe(1);
    await page.clock.runFor(21000);
    await expect.poll(() => attempts).toBeGreaterThanOrEqual(2);
    await expect(page.getByText("No devices yet")).toBeVisible();
    await expect(
      page.getByText("The server took too long to respond. Please try again."),
    ).toHaveCount(0);
    release();
  });
  test("a stalled authentication request presents a working retry", async ({
    page,
  }) => {
    await page.clock.install();
    let attempts = 0;
    let release!: () => void;
    const stalled = new Promise<void>((resolve) => {
      release = resolve;
    });
    await page.route("**/api/v1/auth/status", async (route) => {
      if (++attempts === 1) {
        await stalled;
        await route.abort().catch(() => {});
      } else await route.continue();
    });
    await page.goto("/");
    await expect.poll(() => attempts).toBe(1);
    await page.clock.runFor(21000);
    await expect(
      page.getByText("Cannot reach H.O.P.E", { exact: true }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Retry connection" }).click();
    await expect(page.getByText("No devices yet")).toBeVisible();
    expect(attempts).toBe(2);
    release();
  });
  test("maps, hosts, selection, history, and manual relationships", async ({
    page,
    request,
  }) => {
    await seed(request);
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.goto("/?view=map");
    await expect(page.getByTestId("network-map")).toBeVisible();
    await expect(
      page.locator(".map-device").filter({ hasText: "atlas-nas" }),
    ).toBeVisible();
    await page
      .getByRole("button", { name: "Show metrics", exact: true })
      .click();
    await expect(page.locator(".node-health").first()).toBeVisible();
    await expectMapFits(page);
    await expect(
      page
        .locator(".map-device")
        .filter({ hasText: "compute-node" })
        .locator(".node-health"),
    ).toContainText("RAM —");
    await page.locator(".map-device").filter({ hasText: "atlas-nas" }).click();
    await page.getByRole("tab", { name: "Info" }).click();
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toContainText("AMD Ryzen");
    await expect(page).toHaveURL(/view=host&host=/);
    await expect(page.getByTestId("network-map")).toHaveCount(0);
    await page.getByRole("button", { name: "Close host details" }).click();
    await navigate(page, /^Hosts/);
    await expect(page.getByRole("table")).toContainText("atlas-nas");
    await expect(
      page.getByRole("row").filter({ hasText: "atlas-nas" }),
    ).toContainText("Reporting");
    await expect(
      page.getByRole("row").filter({ hasText: "atlas-nas" }),
    ).toContainText(/\d+ issues/);
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toHaveCount(0);
    await page.getByRole("button", { name: "atlas-nas", exact: true }).click();
    await expect(page.getByRole("table")).toHaveCount(0);
    await page.getByRole("tab", { name: "Info" }).click();
    const hostDetails = page.getByRole("complementary", {
      name: "Host details",
    });
    await expect(hostDetails).toContainText("System uptime");
    await expect(hostDetails).toContainText("2d 0h · at last sample");
    await expect(hostDetails).toContainText(
      "28046bda-bbbf-4101-9455-8a7818f0969b",
    );
    await expect(
      hostDetails.locator('time[datetime="2023-11-14T22:13:20.000Z"]'),
    ).toBeVisible();
    await hostDetails
      .getByRole("heading", { name: "Hardware", exact: true })
      .locator("..")
      .screenshot({ path: "test-results/host-uptime.png" });
    await hostDetails.getByRole("tab", { name: "Containers" }).click();
    await expect(
      page.getByRole("heading", { name: "Docker containers" }),
    ).toBeVisible();
    await expect(
      page.getByAltText("immich-server application icon"),
    ).toBeVisible();
    await expect(
      hostDetails.getByRole("columnheader", { name: "CPU" }),
    ).toBeVisible();
    await expect(hostDetails.getByText("33.4%", { exact: true })).toBeVisible();
    await hostDetails.getByRole("button", { name: "immich-server" }).click();
    await expect(page).toHaveURL(/view=container/);
    await expect(
      page.getByRole("heading", { name: "immich-server" }),
    ).toBeVisible();
    await expect(page.getByText("321.0 MiB / 2.0 GiB")).toBeVisible();
    await expect(page.getByTestId("chart-Container CPU")).toBeVisible();
    await expect(page.getByTestId("chart-Container memory")).toBeVisible();
    await page.screenshot({
      path: "test-results/container-history.png",
      fullPage: true,
    });
    await page.getByRole("button", { name: "Back to atlas-nas" }).click();
    await expect(
      hostDetails.getByRole("tab", { name: "Containers" }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Close host details" }).click();
    await page
      .getByRole("row")
      .filter({ hasText: "studio-mac" })
      .first()
      .click();
    await expect(
      hostDetails.getByRole("tab", { name: "Containers" }),
    ).toHaveCount(0);
    await page.getByRole("button", { name: "Close host details" }).click();
    await page.getByLabel("Search hosts").fill("workshop");
    await expect(page.getByRole("table").getByRole("row")).toHaveCount(2);
    await page.getByLabel("Search hosts").fill("");
    await navigate(page, "Network map");
    await page.getByRole("button", { name: "Subnets", exact: true }).click();
    await expect(page.getByTestId("subnet-group").first()).toBeVisible();
    await expect(page.locator(".react-flow__edge")).toHaveCount(0);
    await expectMapFits(page);
    await page.screenshot({
      path: "test-results/hope-subnets.png",
      fullPage: true,
    });
    await page.getByRole("button", { name: "Topology", exact: true }).click();
    await expectMapFits(page);
    await page.screenshot({
      path: "test-results/hope-health.png",
      fullPage: true,
    });
    const bootstrap = await (await request.get("/api/v1/bootstrap")).json();
    const source = bootstrap.hosts.find(
      (host: { name: string }) => host.name === "atlas-nas",
    );
    const target = bootstrap.hosts.find(
      (host: { name: string }) => host.name === "studio-mac",
    );
    expect(
      (
        await request.patch("/api/v1/map", {
          data: {
            operation: "add_link",
            link: {
              id: "browser-manual",
              source: source.id,
              target: target.id,
              label: "Fixture",
            },
          },
        })
      ).ok(),
    ).toBeTruthy();
    await expect(
      page.getByText("Fixture · Manual", { exact: true }),
    ).toBeVisible();
    await page.locator(".manual-edge .react-flow__edge-textbg").dblclick();
    await expect(
      page.getByText("Fixture · Manual", { exact: true }),
    ).toHaveCount(0);
    expect(errors).toEqual([]);
  });
  test("mobile layout and reload of selected host", async ({
    page,
    request,
  }) => {
    const data = await (await request.get("/api/v1/bootstrap")).json();
    const host = data.hosts.find(
      (h: { name: string }) => h.name === "atlas-nas",
    );
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto("/?view=map");
    await expectMapFits(page);
    await page.locator(".map-device").filter({ hasText: "atlas-nas" }).click();
    await expect(page).toHaveURL(new RegExp(`view=host&host=${host.id}`));
    const details = page.getByRole("complementary", { name: "Host details" });
    await expect(
      details.getByRole("heading", { name: "atlas-nas", exact: true }),
    ).toBeFocused();
    await expect(page.getByTestId("network-map")).toHaveCount(0);
    await expect(
      details.getByRole("tab", { name: "Metrics", exact: true }),
    ).toHaveAttribute("aria-selected", "true");
    await expect(details.getByTestId("chart-CPU")).toBeVisible();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBeTruthy();
    await page.screenshot({
      path: "test-results/hope-mobile.png",
      fullPage: true,
    });
    await page.reload();
    await expect(details).toBeVisible();
    await page.goBack();
    await expect(page).toHaveURL(/view=map/);
    await expectMapFits(page);
  });
  test("positions survive mode switching and reload; selection follows back navigation", async ({
    page,
    request,
  }) => {
    const data = await (await request.get("/api/v1/bootstrap")).json();
    const host = data.hosts.find(
      (h: { name: string }) => h.name === "atlas-nas",
    );
    const other = data.hosts.find(
      (h: { name: string }) => h.name === "studio-mac",
    );
    // Positions are relative to the host's subnet container; both land on empty grid cells.
    for (const [mode, x, y] of [
      ["topology", 276, 310],
      ["subnets", 20, 446],
    ] as const) {
      expect(
        (
          await request.patch("/api/v1/map", {
            data: {
              operation: "set_position",
              position: {
                id: `${mode === "topology" ? "topology:v2" : mode}:${host.id}`,
                x,
                y,
              },
            },
          })
        ).ok(),
      ).toBeTruthy();
    }
    await page.goto("/?view=map");
    const node = page.locator(`[data-id="${host.id}"]`);
    await expect(node).toHaveAttribute("style", /translate\(276px, 310px\)/);
    await expectMapFits(page);
    let rejectFirstSave = true;
    await page.route("**/api/v1/map", (route) => {
      if (rejectFirstSave) {
        rejectFirstSave = false;
        return route.fulfill({
          status: 503,
          contentType: "application/json",
          body: JSON.stringify({ error: "Fixture save failure" }),
        });
      }
      return route.continue();
    });
    // Hover waits for the measured/fitted node to be stable and receive events.
    // Raw mouse coordinates captured during first layout can hit another node.
    await node.hover();
    const bounds = await node.boundingBox();
    expect(bounds).not.toBeNull();
    await page.mouse.down();
    await page.mouse.move(
      bounds!.x + bounds!.width / 2 + 40,
      bounds!.y + bounds!.height / 2 + 30,
      {
        steps: 5,
      },
    );
    await page.mouse.up();
    await expect(
      page.getByRole("button", { name: "Retry 1 unsaved map change" }),
    ).toBeVisible();
    await page
      .getByRole("button", { name: "Retry 1 unsaved map change" })
      .click();
    await expect
      .poll(async () => {
        const state = await (await request.get("/api/v1/bootstrap")).json();
        const x = state.map.positions.find(
          (p: { id: string }) => p.id === `topology:v2:${host.id}`,
        )?.x;
        return typeof x === "number" && x !== 276;
      })
      .toBeTruthy();
    const saved = (
      await (await request.get("/api/v1/bootstrap")).json()
    ).map.positions.find(
      (p: { id: string }) => p.id === `topology:v2:${host.id}`,
    );
    await page
      .getByRole("button", { name: "Show metrics", exact: true })
      .click();
    await expect(node).toHaveAttribute(
      "style",
      new RegExp(`translate\\(${saved.x}px, ${saved.y}px\\)`),
    );
    await page.getByRole("button", { name: "Subnets", exact: true }).click();
    await expect(node).toHaveAttribute("style", /translate\(20px, 446px\)/);
    await page.getByRole("button", { name: "Topology", exact: true }).click();
    await expect
      .poll(async () => {
        const position = await node.evaluate((element) => {
          const matrix = new DOMMatrix(
            (element as HTMLElement).style.transform,
          );
          return { x: matrix.m41, y: matrix.m42 };
        });
        return Math.hypot(position.x - saved.x, position.y - saved.y);
      })
      .toBeLessThan(0.01);
    await page.reload();
    await expect
      .poll(async () => {
        const position = await node.evaluate((element) => {
          const matrix = new DOMMatrix(
            (element as HTMLElement).style.transform,
          );
          return { x: matrix.m41, y: matrix.m42 };
        });
        return Math.hypot(position.x - saved.x, position.y - saved.y);
      })
      .toBeLessThan(0.01);
    await navigate(page, /^Hosts/);
    await page.getByRole("button", { name: "atlas-nas", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`host=${host.id}`));
    await page.getByRole("button", { name: "Close host details" }).click();
    await page.getByRole("button", { name: "studio-mac", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`host=${other.id}`));
    await page.goBack();
    await expect(page.getByRole("table")).toBeVisible();
    await page.goBack();
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toContainText("atlas-nas");
  });
  test("a container omitted from the latest report keeps its history on mobile", async ({
    page,
    request,
  }) => {
    const bootstrap = await (await request.get("/api/v1/bootstrap")).json();
    const host = bootstrap.hosts.find(
      (item: { name: string }) => item.name === "atlas-nas",
    );
    expect(host).toBeTruthy();
    await page.route("**/api/v1/bootstrap", async (route) => {
      const response = await route.fetch();
      const body = await response.json();
      const item = body.hosts.find(
        (item: { id: string }) => item.id === host.id,
      );
      item.metrics.containers = [];
      await route.fulfill({ response, json: body });
    });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(`/?view=container&host=${host.id}&container=a1b2c3d4e5f6`);
    await expect(
      page.getByRole("heading", { name: "Container not in the latest report" }),
    ).toBeVisible();
    await expect(page.getByTestId("chart-Container CPU")).toBeVisible();
    await expect(page.getByTestId("chart-Container memory")).toBeVisible();
    await expect
      .poll(() =>
        page.evaluate(
          () => document.documentElement.scrollWidth <= window.innerWidth,
        ),
      )
      .toBe(true);
    await page.screenshot({
      path: "test-results/container-history-mobile.png",
      fullPage: true,
    });
    await page.getByRole("button", { name: "7d", exact: true }).click();
    await expect(page.getByTestId("chart-Container CPU")).toBeVisible();
  });

  test("boot information retains observed uptime during outages and labels container scope", async ({
    page,
    request,
  }) => {
    const auth = (await (await request.get("/api/v1/auth/status")).json()) as {
      setup_required: boolean;
    };
    if (auth.setup_required) await seed(request);
    else {
      const session = await request.post("/api/v1/auth/login", {
        data: { password },
      });
      expect(session.ok(), await session.text()).toBeTruthy();
    }
    await page.context().addCookies((await request.storageState()).cookies);
    const bootstrap = await (await request.get("/api/v1/bootstrap")).json();
    const host = bootstrap.hosts.find(
      (item: { name: string }) => item.name === "atlas-nas",
    );
    let legacy = false;
    await page.route("**/api/v1/bootstrap", (route) => {
      const metrics = { ...host.metrics };
      if (legacy) delete metrics.system_uptime;
      else
        metrics.system_uptime = {
          ...metrics.system_uptime,
          seconds: 86460,
          scope: "container_kernel",
        };
      return route.fulfill({
        json: {
          ...bootstrap,
          hosts: bootstrap.hosts.map((item: { id: string }) =>
            item.id === host.id
              ? { ...host, metrics, last_report: Date.now() / 1000 - 3600 }
              : item,
          ),
        },
      });
    });
    await page.clock.install();
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(`/?view=host&host=${host.id}`);
    const details = page.getByRole("complementary", { name: "Host details" });
    await details.getByRole("tab", { name: "Info" }).click();
    await expect(details).toContainText("Kernel uptime");
    await expect(details).toContainText("1d 0h · at last sample");
    await expect(details).toContainText(
      "Kernel visible to this container; may describe the host.",
    );
    await page.clock.runFor(120000);
    await expect(details).toContainText("1d 0h · at last sample");
    await expect
      .poll(() =>
        page.evaluate(
          () => document.documentElement.scrollWidth <= window.innerWidth,
        ),
      )
      .toBe(true);
    await details
      .getByRole("heading", { name: "Hardware", exact: true })
      .locator("..")
      .screenshot({ path: "test-results/host-uptime-mobile.png" });
    legacy = true;
    await page.reload();
    await details.getByRole("tab", { name: "Info" }).click();
    await expect(details).toContainText("System uptime");
    await expect(
      details
        .locator("dt")
        .filter({ hasText: /^System uptime$/ })
        .locator("+ dd"),
    ).toHaveText("Unavailable");
    await expect(details.getByText("Boot ID", { exact: true })).toHaveCount(0);
  });

  test("history advances during a reporting outage and unavailable GPUs remain selectable", async ({
    page,
    request,
  }) => {
    const bootstrap = await (await request.get("/api/v1/bootstrap")).json();
    const host = bootstrap.hosts.find(
      (item: { name: string }) => item.name === "atlas-nas",
    );
    const gpu = (
      id: string,
      name: string,
      utilization_percent: number | null,
    ) => ({
      id,
      name,
      vendor: "fixture",
      utilization_percent,
      memory_used_bytes: null,
      memory_total_bytes: null,
      temperature_celsius: null,
    });
    await page.clock.install();
    const collected_at = await page.evaluate(() => Date.now() / 1000);
    await page.route(
      `**/api/v1/hosts/${host.id}/metrics?hours=*`,
      async (route) => {
        await route.fulfill({
          json: [
            ...(new URL(route.request().url()).searchParams.get("hours") === "6"
              ? [
                  {
                    collected_at: collected_at - 600,
                    resolution_seconds: 30,
                    metrics: host.metrics,
                  },
                ]
              : []),
            {
              collected_at,
              resolution_seconds: 30,
              metrics: {
                ...host.metrics,
                gpus: [
                  gpu("first", "Unavailable GPU", null),
                  gpu("second", "Working GPU", 40),
                ],
              },
            },
          ],
        });
      },
    );
    await page.goto(`/?view=host&host=${host.id}`);
    await page.getByRole("tab", { name: "Metrics", exact: true }).click();
    const selector = page.getByRole("combobox", { name: "GPU history device" });
    await expect(selector).toBeVisible();
    await expect(
      page.getByTestId("chart-CPU").locator("circle.history-sample"),
    ).toHaveCount(1);
    await page.screenshot({
      path: "test-results/history-gpu-unavailable.png",
      fullPage: true,
    });
    await selector.click();
    await page.getByRole("option", { name: /Working GPU/ }).click();
    await expect(
      page.getByTestId("chart-GPU utilization and memory"),
    ).toBeVisible();
    await expect(
      page
        .getByTestId("chart-GPU utilization and memory")
        .locator('circle.history-sample[data-series="utilization"]'),
    ).toHaveCount(1);
    await page.screenshot({
      path: "test-results/history-isolated-sample.png",
      fullPage: true,
    });
    await page.getByRole("button", { name: "6h", exact: true }).click();
    await expect(
      page.getByTestId("chart-CPU").locator("circle.history-sample"),
    ).toHaveCount(2);
    await page.clock.runFor(180000);
    const through = await page.evaluate(() =>
      new Date(Date.now()).toLocaleString([], {
        hour: "2-digit",
        minute: "2-digit",
      }),
    );
    await expect(
      page.getByTestId("chart-CPU").getByText(through, { exact: true }),
    ).toBeVisible();
  });

  test("expired session returns to login and mobile logout is reachable", async ({
    page,
  }) => {
    await page.goto("/?view=map");
    await expect(page.getByTestId("network-map")).toBeVisible();
    await page.setViewportSize({ width: 390, height: 844 });
    await globalAction(page, "Sign out");
    await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
    await page.getByLabel("Administrator password").fill(password);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(page.getByTestId("network-map")).toBeVisible();
    adminState = await page.context().storageState();
    await page.route("**/api/v1/bootstrap", (route) =>
      route.fulfill({
        status: 401,
        contentType: "application/json",
        body: JSON.stringify({
          error: "Session expired",
          code: "authentication_required",
        }),
      }),
    );
    await page.reload();
    await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
  });
  test("overlapping ranges keep filtered hosts in every map mode", async ({
    page,
    request,
  }) => {
    expect(
      (
        await request.post("/api/v1/networks", {
          data: {
            name: "A overlapping range",
            cidr: "192.168.1.0/25",
            enabled: false,
          },
        })
      ).ok(),
    ).toBeTruthy();
    await page.goto("/?view=map");
    await page.getByRole("combobox", { name: "Filter by network" }).click();
    await page
      .getByRole("option", { name: "Home network", exact: true })
      .click();
    for (const mode of ["Topology", "Subnets"]) {
      await page.getByRole("button", { name: mode, exact: true }).click();
      await expect(
        page.locator(".map-device").filter({ hasText: "atlas-nas" }),
      ).toBeVisible();
      await expect(
        page.locator(".map-device").filter({ hasText: "workshop-pi" }),
      ).toHaveCount(0);
    }
    await page.getByLabel("Search hosts").fill("no matching fixture");
    await expect(
      page.getByRole("button", { name: "Clear filters" }),
    ).toBeVisible();
  });
  test("failed initial scan retries the saved range without another create", async ({
    page,
  }) => {
    let creates = 0;
    let scans = 0;
    await page.route("**/api/v1/networks", (route) => {
      creates++;
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          id: "fixture-range",
          name: "Retry fixture",
          cidr: "198.18.99.0/30",
          enabled: true,
        }),
      });
    });
    await page.route("**/api/v1/networks/fixture-range/scan", (route) => {
      scans++;
      return route.fulfill({
        status: 503,
        contentType: "application/json",
        body: JSON.stringify({ error: "Fixture scanner unavailable" }),
      });
    });
    await page.goto("/");
    await page.getByRole("button", { name: "Networks", exact: true }).click();
    await page
      .getByRole("button", { name: "Add network", exact: true })
      .click();
    await page.getByLabel("Network name").fill("Retry fixture");
    await page.getByLabel("IPv4 CIDR").fill("198.18.99.0/30");
    await page.getByRole("button", { name: "Add and discover" }).click();
    await expect(
      page.getByRole("button", { name: "Retry discovery" }),
    ).toBeVisible();
    await expect(page.getByLabel("IPv4 CIDR")).toBeDisabled();
    await page.getByRole("button", { name: "Retry discovery" }).click();
    await expect.poll(() => scans).toBe(2);
    expect(creates).toBe(1);
  });
  test("overview surfaces fleet health and leads to the host that needs attention", async ({
    page,
    request,
  }) => {
    const enrollment = await (
      await request.post("/api/v1/enrollment-tokens", { data: {} })
    ).json();
    const enrolled = await request.post("/api/v1/agent/enroll", {
      data: {
        schema_version: 1,
        token: enrollment.token,
        agent_id: randomUUID(),
        credential: randomBytes(32).toString("hex"),
        inventory: {
          hostname: "quiet-node",
          os: "Linux",
          architecture: "x86_64",
          cpu_model: "Intel N100",
          cpu_cores: 4,
          memory_total_bytes: 8589934592,
          interfaces: [
            {
              name: "eth0",
              mac: "02:00:00:00:00:99",
              addresses: ["192.168.1.99"],
              gateway: "192.168.1.1",
            },
          ],
          disks: [],
          gpus: [],
          collectors: [],
        },
      },
    });
    expect(enrolled.ok()).toBeTruthy();
    await page.goto("/");
    await expect(
      page.getByRole("heading", { name: "Overview", exact: true }),
    ).toBeVisible();
    await expect(page.getByTestId("tile-hosts")).toContainText(
      "Hosts recently seen",
    );
    await expect(page.getByTestId("tile-attention")).toContainText(
      "3 critical",
    );
    const attention = page.getByRole("region", { name: "Needs attention" });
    await expect(attention).toContainText("quiet-node");
    await expect(attention).toContainText("Agent has never reported");
    await expect(attention).toContainText("Pool tank is DEGRADED");
    await expect(attention).toContainText("https://example.com/ is down");
    await expect(attention).toContainText("nginx.service failed");
    await expect(
      page.getByRole("region", { name: "Busiest CPUs" }),
    ).toContainText("compute-node");
    await page.screenshot({
      path: "test-results/hope-overview.png",
      fullPage: true,
    });
    await attention.getByRole("button", { name: /quiet-node/ }).click();
    await expect(page).toHaveURL(/view=host&/);
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toContainText("Intel N100");
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toContainText("Agent has never reported");
    await page.getByRole("button", { name: "Close host details" }).click();
    await expect(page).toHaveURL(/view=hosts/);
    await page.getByRole("button", { name: /^Needs attention/ }).click();
    await expect(page.getByRole("table").getByRole("row")).toHaveCount(3);
    await expect(page.getByRole("table")).toContainText("quiet-node");
    await expect(page.getByRole("table")).toContainText("atlas-nas");
    await page.getByRole("button", { name: /^Reporting/ }).click();
    await expect(page.getByRole("table")).not.toContainText("quiet-node");
    await page
      .getByRole("columnheader", { name: "CPU" })
      .getByRole("button")
      .click();
    await expect(page.getByRole("table").getByRole("row").nth(1)).toContainText(
      "compute-node",
    );
    await page.screenshot({
      path: "test-results/hope-hosts.png",
      fullPage: true,
    });
  });
  test("host details show pools, sensors, service units, monitors and their history", async ({
    page,
  }) => {
    await page.goto("/?view=hosts");
    await page.getByRole("button", { name: "atlas-nas", exact: true }).click();
    const details = page.getByRole("complementary", { name: "Host details" });
    await expect(
      details.getByRole("tab", { name: "Metrics", exact: true }),
    ).toHaveAttribute("aria-selected", "true");
    await expect(details.getByTestId("chart-CPU")).toContainText("20%");
    await expect(details.getByTestId("chart-Load average")).toContainText(
      "1 min1.60",
    );
    await expect(details.getByTestId("chart-Memory")).toContainText(
      "ZFS ARC3.73 GiB",
    );
    await expect(details.getByTestId("chart-Memory")).toContainText(
      "Cache and buffers5.59 GiB",
    );
    await expect(details.getByTestId("chart-Memory")).toContainText("Swap");
    await expect(details.getByTestId("chart-Memory")).toContainText("0.09 GiB");
    await expect(details.getByTestId("chart-Disk I/O")).toContainText(
      "nvme0n1 (root) · 12% busy",
    );
    await expect(details.getByTestId("chart-Pool usage")).toContainText(
      "ONLINE",
    );
    await expect(details.getByTestId("chart-Pool usage")).toContainText(
      "rpool · 1.7 TiB of 1.8 TiB",
    );
    await expect(details.getByTestId("chart-Temperatures")).toContainText(
      "Package id 065.0 °C",
    );
    await expect(details.getByTestId("chart-Monitor latency")).toContainText(
      "7 ms",
    );
    await details.getByRole("tab", { name: "Services", exact: true }).click();
    await expect(details.getByTestId("service-units")).toContainText(
      "40 active",
    );
    await expect(details.getByTestId("service-units")).toContainText(
      "nginx.service",
    );
    await expect(details.getByTestId("service-units")).toContainText(
      "ghost.service",
    );
    await expect(details.getByTestId("service-units")).toContainText(
      "not-found",
    );
    await expect(details.getByTestId("monitors")).toContainText("timed out");
    await expect(details).not.toContainText("Live usage");
    await details.getByRole("tab", { name: "Metrics", exact: true }).click();
    await expect(details.getByTestId("chart-Filesystem usage")).toContainText(
      "/ · nvme0 · 372.5 GiB of 931.3 GiB",
    );
    await expect(page).toHaveURL(/view=host&host=/);
    await expect(
      page.getByRole("heading", { name: "Host", exact: true }),
    ).toBeVisible();
    await page.setViewportSize({ width: 320, height: 800 });
    await expect
      .poll(() =>
        page.locator(".host-page .details-header").evaluate((header) => {
          const parent = header.parentElement!;
          const row = header.querySelector(".flex");
          return (
            header.scrollWidth <= header.clientWidth &&
            header.getBoundingClientRect().right <=
              parent.getBoundingClientRect().right + 1 &&
            (!row || row.scrollWidth <= row.clientWidth)
          );
        }),
      )
      .toBeTruthy();
    await details.getByRole("tab", { name: "Metrics", exact: true }).click();
    await expect(page.locator(".host-page .charts")).toBeVisible();
    await expect(details.getByTestId("chart-Temperatures")).toBeVisible();
    await expect(details.getByTestId("chart-Temperatures")).toContainText(
      "nvme Composite",
    );
    await details.getByLabel("Filter sensors").fill("package");
    await expect(details.getByTestId("chart-Temperatures")).not.toContainText(
      "nvme Composite",
    );
    await details.getByLabel("Filter sensors").fill("");
    await page.screenshot({
      path: "test-results/hope-host-page.png",
      fullPage: true,
    });
    await page.getByRole("button", { name: "Close host details" }).click();
    await expect(page).toHaveURL(/view=hosts/);
    await expect(page.getByRole("table")).toBeVisible();
  });
  test("service failures remain visible when bounded reports omit unit details", async ({
    page,
    request,
  }) => {
    const bootstrap = await (await request.get("/api/v1/bootstrap")).json();
    const host = bootstrap.hosts.find(
      (item: { name: string }) => item.name === "atlas-nas",
    );
    await page.route("**/api/v1/bootstrap", (route) =>
      route.fulfill({
        json: {
          ...bootstrap,
          hosts: bootstrap.hosts.map((item: { id: string }) =>
            item.id === host.id
              ? {
                  ...host,
                  metrics: {
                    ...host.metrics,
                    services: [],
                    service_summary: { active: 1, failed: 200, total: 201 },
                  },
                }
              : item,
          ),
        },
      }),
    );
    await page.goto(`/?view=host&host=${host.id}`);
    const details = page.getByRole("complementary", { name: "Host details" });
    await details.getByRole("tab", { name: "Info" }).click();
    await expect(details).toContainText("200 service units failed");
    await details.getByRole("tab", { name: "Services", exact: true }).click();
    const units = details.getByTestId("service-units");
    await expect(units).toContainText("200 failed");
    await expect(units).toContainText("Showing 0 of 200 failed units");
    await expect(units).toContainText("Failed unit details were not included");
    await expect(units).not.toContainText("No failed units");
    await units.screenshot({ path: "test-results/services-omitted.png" });
  });

  test("command palette jumps to a host and the theme persists", async ({
    page,
  }) => {
    await page.goto("/");
    await expect(
      page.getByRole("heading", { name: "Overview", exact: true }),
    ).toBeVisible();
    await page.keyboard.press("Control+k");
    const palette = page.getByRole("dialog", { name: "Search and commands" });
    await expect(palette).toBeVisible();
    await palette.getByRole("combobox").fill("studio");
    await expect(palette.getByRole("option")).toHaveCount(1);
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/view=host&host=/);
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toContainText("studio-mac");
    await expect(page.locator("html")).not.toHaveClass(/dark/);
    await page
      .getByRole("group", { name: "Theme" })
      .getByRole("button", { name: "Dark" })
      .click();
    await expect(page.locator("html")).toHaveClass(/dark/);
    await page.screenshot({
      path: "test-results/hope-dark.png",
      fullPage: true,
    });
    await page.reload();
    await expect(page.locator("html")).toHaveClass(/dark/);
    await expect(
      page.getByRole("heading", { name: "Host", exact: true }),
    ).toBeVisible();
    await page.keyboard.press("Control+k");
    await page
      .getByRole("dialog", { name: "Search and commands" })
      .getByRole("combobox")
      .fill("light theme");
    await page.keyboard.press("Enter");
    await expect(page.locator("html")).not.toHaveClass(/dark/);
  });
  test("theme choices still apply when browser storage is denied", async ({
    page,
  }) => {
    await page.addInitScript(() => {
      const get = Storage.prototype.getItem,
        set = Storage.prototype.setItem,
        remove = Storage.prototype.removeItem;
      const deny = (key: string) => {
        if (key === "hope-theme")
          throw new DOMException("Storage denied", "SecurityError");
      };
      Storage.prototype.getItem = function (key) {
        deny(key);
        return get.call(this, key);
      };
      Storage.prototype.setItem = function (key, value) {
        deny(key);
        set.call(this, key, value);
      };
      Storage.prototype.removeItem = function (key) {
        deny(key);
        remove.call(this, key);
      };
    });
    await page.emulateMedia({ colorScheme: "light" });
    await page.goto("/");
    const theme = page.getByRole("group", { name: "Theme" });
    await theme.getByRole("button", { name: "Dark", exact: true }).click();
    await expect(page.locator("html")).toHaveClass(/dark/);
    await theme.getByRole("button", { name: "Light", exact: true }).click();
    await expect(page.locator("html")).not.toHaveClass(/dark/);
    await theme.getByRole("button", { name: "Auto", exact: true }).click();
    await page.emulateMedia({ colorScheme: "dark" });
    await expect(page.locator("html")).toHaveClass(/dark/);
  });

  test("a range can be removed from settings without losing its hosts", async ({
    page,
    request,
  }) => {
    const before = await (await request.get("/api/v1/bootstrap")).json();
    await page.goto("/");
    await page.getByRole("button", { name: "Networks", exact: true }).click();
    const dialog = page.locator(".networks-page");
    await dialog.getByRole("button", { name: "Remove Workshop" }).click();
    await dialog.getByRole("button", { name: "Remove range" }).click();
    await expect(dialog).not.toContainText("192.168.30.0/24");
    const after = await (await request.get("/api/v1/bootstrap")).json();
    expect(after.networks.length).toBe(before.networks.length - 1);
    expect(after.hosts.length).toBe(before.hosts.length);
    await expect(page.getByText("Removed Workshop.")).toBeVisible();
  });

  test("devices show their type and evidence, and the type can be corrected", async ({
    page,
    request,
  }) => {
    await page.goto("/?view=hosts");
    const atlasRow = page.getByRole("row").filter({ hasText: "atlas-nas" });
    if ((await atlasRow.getAttribute("data-state")) !== "selected") {
      await atlasRow.click();
    }
    await page.getByRole("tab", { name: "Info" }).click();
    const device = page.getByTestId("device");
    // Its name says what it is.
    await expect(device).toContainText("NAS");
    await expect(device.getByLabel("Why this type")).toContainText(
      'Named "atlas-nas"',
    );
    await page.getByRole("combobox", { name: "Device type" }).click();
    await page.getByRole("option", { name: "Server", exact: true }).click();
    await expect(
      page.getByText("atlas-nas is now marked as server."),
    ).toBeVisible();
    await expect(device).toContainText("set by you");
    await page.getByRole("button", { name: "Close host details" }).click();
    await expect(
      page.getByRole("row").filter({ hasText: "atlas-nas" }),
    ).toContainText("Server");
    await atlasRow.click();
    await page.getByRole("tab", { name: "Info" }).click();
    await page.screenshot({ path: "test-results/hope-device.png" });
    await page.getByRole("combobox", { name: "Device type" }).click();
    await page
      .getByRole("option", { name: "Automatic detection", exact: true })
      .click();
    await expect(
      page.getByText("atlas-nas is back to automatic detection."),
    ).toBeVisible();
    await expect(device).not.toContainText("set by you");

    await page.getByRole("button", { name: "Change icon" }).click();
    const iconDialog = page.getByRole("dialog", { name: "Change icon" });
    await iconDialog.getByRole("textbox", { name: "Search icons" }).fill("kin");
    await expect(
      iconDialog.getByRole("option", { name: /Amazon Kindle/ }).first(),
    ).toBeVisible();
    await iconDialog.getByRole("textbox", { name: "Search icons" }).fill("un");
    await expect(
      iconDialog.getByRole("option", { name: /UniFi/ }).first(),
    ).toBeVisible();
    await iconDialog
      .getByRole("textbox", { name: "Search icons" })
      .fill("Xbox");
    await iconDialog.getByRole("option", { name: /Xbox/ }).first().click();
    await iconDialog
      .getByRole("button", { name: "Change", exact: true })
      .click();
    await expect(iconDialog).toHaveCount(0);

    await page.getByRole("tab", { name: "Settings", exact: true }).click();
    const uptime = page.getByRole("region", { name: "Uptime monitoring" });
    await expect(
      uptime.getByLabel("Enable uptime monitoring"),
    ).not.toBeChecked();
    await uptime.getByLabel("Enable uptime monitoring").check();
    await uptime.getByLabel("Monitor device downtime").check();
    await uptime.getByLabel("TCP ports to monitor").fill("443, 8080");
    await uptime
      .getByRole("button", { name: "Save uptime monitoring" })
      .click();
    await expect(page.getByText("Uptime monitoring saved.")).toBeVisible();
    const uptimeBootstrap = await (
      await request.get("/api/v1/bootstrap")
    ).json();
    expect(
      uptimeBootstrap.hosts.find(
        (host: { name: string }) => host.name === "atlas-nas",
      ).uptime,
    ).toEqual({ enabled: true, device: true, ports: [443, 8080] });
    await page.getByRole("button", { name: "Close host details" }).click();
    await page.getByRole("button", { name: "1 uptime monitored" }).click();
    await expect(
      page.getByRole("button", { name: "1 uptime monitored" }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(page.locator(".host-table tbody tr")).toHaveCount(1);
    await page
      .getByRole("button", { name: "Clear filters", exact: true })
      .click();
    await atlasRow.click();
    await page.getByRole("tab", { name: "Settings", exact: true }).click();
    await uptime.getByLabel("Enable uptime monitoring").uncheck();
    await uptime
      .getByRole("button", { name: "Save uptime monitoring" })
      .click();
    await expect(uptime.getByLabel("TCP ports to monitor")).toBeDisabled();
    await page
      .getByRole("textbox", { name: "Name (Alias)" })
      .fill("Cinema Xbox");
    await page.getByRole("button", { name: "Save name" }).click();
    await expect(
      page.getByRole("heading", { name: "Cinema Xbox", exact: true }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Close host details" }).click();
    await expect(
      page.getByRole("row").filter({ hasText: "Cinema Xbox" }),
    ).toBeVisible();
    const bootstrap = await (await request.get("/api/v1/bootstrap")).json();
    const renamed = bootstrap.hosts.find(
      (host: { name: string }) => host.name === "atlas-nas",
    );
    expect(renamed.name_override).toBe("Cinema Xbox");
    expect(renamed.icon_override).toMatch(/^(?:selfhst:[a-z0-9-]+|unifi:\d+)$/);
    const reset = await request.put(`/api/v1/hosts/${renamed.id}/identity`, {
      data: { name: null, icon: null },
    });
    expect(reset.ok()).toBeTruthy();
  });
  test("console stays readable across sizes and supports keyboard navigation", async ({
    page,
    request,
  }) => {
    const auth = (await (await request.get("/api/v1/auth/status")).json()) as {
      setup_required: boolean;
    };
    if (auth.setup_required) await seed(request);
    else {
      const session = await request.post("/api/v1/auth/login", {
        data: { password },
      });
      expect(session.ok(), await session.text()).toBeTruthy();
      const bootstrap = await (await request.get("/api/v1/bootstrap")).json();
      if (
        !bootstrap.hosts.some(
          (host: { name: string }) => host.name === "atlas-nas",
        )
      )
        await seed(request);
    }
    await page.context().addCookies((await request.storageState()).cookies);
    await page.goto("/?view=hosts");
    await expect(
      page.getByRole("navigation", { name: "Main navigation" }),
    ).toBeVisible();
    await page.keyboard.press("Tab");
    await expect(
      page.getByRole("link", { name: "Skip to main content" }),
    ).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page.locator("#main-content")).toBeFocused();
    await page.getByRole("button", { name: "atlas-nas", exact: true }).click();
    await page.emulateMedia({ reducedMotion: "reduce" });
    for (const width of [375, 768, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      await expect
        .poll(() =>
          page.evaluate(
            () => document.documentElement.scrollWidth <= window.innerWidth,
          ),
        )
        .toBeTruthy();
      if (width === 375) {
        await page.getByRole("button", { name: "Close host details" }).click();
        const table = page.locator(".host-table");
        await expect(
          table.getByRole("columnheader", { name: "Status" }),
        ).toBeVisible();
        expect(
          await table.evaluate(
            (element) =>
              element.scrollWidth <= element.parentElement!.clientWidth,
          ),
        ).toBeTruthy();
        await page
          .getByRole("button", { name: "atlas-nas", exact: true })
          .click();
      }
      await expect(
        page.getByRole("tab", { name: "Metrics", exact: true }),
      ).toHaveAttribute("aria-selected", "true");
      await expect(page.locator(".reading-meter-card")).toHaveCount(0);
      await page.screenshot({
        path: `test-results/hope-polish-${width}.png`,
        fullPage: true,
      });
    }
    await page.getByRole("button", { name: "Dark", exact: true }).click();
    await expect(page.locator("html")).toHaveClass(/dark/);
    await page.screenshot({
      path: "test-results/hope-polish-dark.png",
      fullPage: true,
    });
    await page.getByRole("button", { name: "Auto", exact: true }).click();
  });

  test("each range chooses how often it is scanned in the background", async ({
    page,
    request,
  }) => {
    await page.goto("/");
    await page.getByRole("button", { name: "Networks", exact: true }).click();
    const dialog = page.locator(".networks-page");
    await expect(dialog).toContainText("Every 5 minutes");
    await dialog
      .getByRole("combobox", { name: "Scan interval for Home network" })
      .click();
    await page.getByRole("option", { name: "Every hour", exact: true }).click();
    await expect(
      page.getByText("Home network is now scanned every hour."),
    ).toBeVisible();
    const bootstrap = await (await request.get("/api/v1/bootstrap")).json();
    expect(
      bootstrap.networks.find(
        (n: { name: string }) => n.name === "Home network",
      ).interval_minutes,
    ).toBe(60);
  });
  test("agent reports appear live and the report interval is configurable", async ({
    page,
    request,
  }) => {
    const enrollment = await (
      await request.post("/api/v1/enrollment-tokens", { data: {} })
    ).json();
    const agent = randomUUID();
    const enrolled = await request.post("/api/v1/agent/enroll", {
      data: {
        schema_version: 1,
        token: enrollment.token,
        agent_id: agent,
        credential: randomBytes(32).toString("hex"),
        inventory: {
          hostname: "live-node",
          os: "Linux",
          architecture: "x86_64",
          cpu_model: "Test CPU",
          cpu_cores: 4,
          memory_total_bytes: 8e9,
          interfaces: [
            {
              name: "eth0",
              mac: "02:00:00:00:00:77",
              addresses: ["192.168.1.77"],
              gateway: "192.168.1.1",
            },
          ],
          disks: [],
          gpus: [],
          collectors: [],
        },
      },
    });
    expect(enrolled.ok()).toBeTruthy();
    const credential = (await enrolled.json()).credential;
    await page.goto("/?view=hosts");
    const row = page.getByRole("row", { name: /live-node/ });
    await expect(row).toBeVisible();
    const report = async (cpu: number) => {
      const response = await request.post("/api/v1/agent/report", {
        headers: { authorization: `Bearer ${credential}` },
        data: {
          schema_version: 1,
          report_id: randomUUID(),
          agent_id: agent,
          collected_at: Date.now() / 1000,
          inventory: null,
          metrics: {
            cpu_percent: cpu,
            memory_used_bytes: null,
            memory_total_bytes: null,
            network_rx_bytes_per_second: null,
            network_tx_bytes_per_second: null,
            disk_read_bytes_per_second: null,
            disk_write_bytes_per_second: null,
            disks: [],
            gpus: [],
          },
          collectors: [],
        },
      });
      expect(response.ok()).toBeTruthy();
    };
    for (const cpu of [37, 64]) {
      await report(cpu);
      // Well inside the 15-second fallback reload: the event itself carried the figures.
      await expect(row).toContainText(`${cpu}%`, { timeout: 5000 });
    }
    // Join source inventory to this same device using the real ingestion endpoint. A later
    // heartbeat must preserve this projection in the live event, without waiting for polling.
    const scope = `browser-engine-${agent}`;
    const child = await request.post("/api/v1/agent/children", {
      headers: { authorization: `Bearer ${credential}` },
      data: {
        schema_version: 1,
        batch_id: randomUUID(),
        agent_id: agent,
        source: "docker",
        source_scope: scope,
        sequence: 1,
        inventory_generation: 1,
        complete: true,
        observed_at: Date.now() / 1000,
        devices: [
          {
            identity: {
              kind: "docker_container",
              scope,
              key: "live-source",
              incarnation: "first",
            },
            name: "live-source",
            lifecycle: "running",
            interfaces: [
              {
                name: "eth0",
                mac: "02:00:00:00:00:77",
                addresses: ["192.168.1.77"],
                gateway: null,
              },
            ],
          },
        ],
      },
    });
    expect(child.ok(), await child.text()).toBeTruthy();
    await row.getByText("live-node", { exact: true }).click();
    await page.getByRole("tab", { name: "Info" }).click();
    const sourceInventory = page.getByRole("region", {
      name: "Source inventory",
      exact: true,
    });
    await expect(sourceInventory).toBeVisible();
    await report(65);
    await expect(sourceInventory).toBeVisible();
    await report(64);
    // History stores one sample per 30 seconds, so these reports seconds apart
    // reach the chart only as live points.
    await page.getByRole("tab", { name: "Metrics", exact: true }).click();
    const cpu = page
      .getByRole("complementary", { name: "Host details" })
      .getByTestId("chart-CPU");
    await expect(cpu).toContainText("64%");
    await report(81);
    await expect(cpu).toContainText("81%", { timeout: 5000 });
    // Averaged windows show the live points after their last bucket too.
    await page
      .getByRole("region", { name: "Source metrics history" })
      .getByRole("group", { name: "History window" })
      .getByText("24h", { exact: true })
      .click();
    await expect(cpu).toContainText("81%");
    await page.getByRole("button", { name: "Close host details" }).click();
    await globalAction(page, "Settings");
    const dialog = page.getByRole("dialog", { name: "Settings" });
    await dialog
      .getByRole("combobox", { name: "Agent report interval" })
      .click();
    await page
      .getByRole("option", { name: "Every 5 seconds", exact: true })
      .click();
    await expect(
      page.getByText(
        "Agents will report every 5 seconds from their next report.",
      ),
    ).toBeVisible();
    const bootstrap = await (await request.get("/api/v1/bootstrap")).json();
    expect(bootstrap.report_interval_seconds).toBe(5);
  });
  test("notifications are configured once and reach an ntfy server", async ({
    page,
  }) => {
    const { createServer } = await import("node:http");
    const received: { auth?: string; body: Record<string, unknown> }[] = [];
    const ntfy = createServer((req, res) => {
      let body = "";
      req.on("data", (chunk) => (body += chunk));
      req.on("end", () => {
        received.push({
          auth: req.headers.authorization,
          body: JSON.parse(body),
        });
        res.writeHead(200, { "content-type": "application/json" });
        res.end("{}");
      });
    });
    await new Promise<void>((resolve) => ntfy.listen(0, "127.0.0.1", resolve));
    const port = (ntfy.address() as { port: number }).port;
    try {
      // The shell can finish loading before bootstrap has rendered its actions.
      // Keep that window open so the shared action helper proves it waits for UI.
      await page.route("**/api/v1/bootstrap", async (route) => {
        await new Promise((resolve) => setTimeout(resolve, 500));
        await route.continue();
      });
      await page.goto("/");
      await globalAction(page, "Notification settings");
      const form = page.getByRole("form", { name: "Notification settings" });
      await form.getByLabel("Send notifications").check();
      await form.getByLabel("ntfy server").fill(`http://127.0.0.1:${port}`);
      await form.getByLabel("Topic").fill("hope-browser");
      await form.getByLabel("Access token (optional)").fill("tk_browser");
      await expect(form.getByLabel("New devices")).toBeChecked();
      await page.screenshot({ path: "test-results/hope-notifications.png" });
      await form.getByRole("button", { name: "Send test" }).click();
      await expect(
        page.getByText("Test notification sent. Check your ntfy app."),
      ).toBeVisible();
      expect(received).toHaveLength(1);
      expect(received[0].auth).toBe("Bearer tk_browser");
      expect(received[0].body.topic).toBe("hope-browser");
      expect(received[0].body.title).toBe("H.O.P.E test notification");
      await expect(form.getByTestId("ntfy-status")).toContainText(
        "Last notification sent",
      );
      await page.keyboard.press("Escape");
      await globalAction(page, "Notification settings");
      await expect(form.getByLabel("Access token (optional)")).toHaveAttribute(
        "placeholder",
        "Stored; type to replace",
      );
      await expect(form.getByLabel("Topic")).toHaveValue("hope-browser");
      // Turn them off again so later tests never post to a closed server.
      await form.getByLabel("Send notifications").uncheck();
      await form.getByRole("button", { name: "Save", exact: true }).click();
      await expect(
        page.getByText("Notification settings saved."),
      ).toBeVisible();
    } finally {
      ntfy.close();
    }
  });
  test("an agent older than the server's release can be updated from the console", async ({
    page,
  }) => {
    let asked = false;
    let allAsked = false;
    const posts: string[] = [];
    await page.route("**/api/v1/bootstrap", async (route) => {
      const response = await route.fetch();
      const body = await response.json();
      body.agent_release = "0.1.60";
      for (const host of body.hosts) {
        if (host.name === "atlas-nas") host.agent_version = "0.1.57";
        else if (host.name === "studio-mac") {
          host.agent_version = "0.1.57";
          host.collectors = [
            { name: "agent_update", available: true, detail: "Updater ready" },
          ];
          if (asked) {
            host.update_requested_at = Date.now() / 1000;
            host.update_target = "0.1.60";
          }
        } else if (host.agent_id) host.agent_version = "0.1.60";
      }
      await route.fulfill({ response, json: body });
    });
    await page.route("**/api/v1/hosts/*/update-agent", async (route) => {
      posts.push(new URL(route.request().url()).pathname);
      asked = true;
      await route.fulfill({ json: { ok: true } });
    });
    await page.route("**/api/v1/agents/update", async (route) => {
      allAsked = true;
      await route.fulfill({ json: { requested: 1 } });
    });

    // The overview counts what can be done about each outdated agent.
    await page.goto("/");
    const fleet = page.getByTestId("fleet-updates");
    await expect(fleet).toContainText(
      "2 agents run an older release than 0.1.60",
    );
    await expect(fleet).toContainText("1 can update from here");
    await expect(fleet).toContainText("1 needs the upgrade command");
    await fleet.getByRole("button", { name: "Update 1 agent" }).click();
    await expect(page.getByText("Asked 1 agent to update.")).toBeVisible();
    expect(allAsked).toBe(true);

    // No updater yet: the host shows the token-less command to run once, and no button to ask.
    await page.goto("/?view=hosts");
    await page.getByRole("row").filter({ hasText: "atlas-nas" }).click();
    const details = page.getByRole("complementary", { name: "Host details" });
    await expect(details.getByText("Agent 0.1.57")).toBeVisible();
    await details.getByRole("tab", { name: "Settings", exact: true }).click();
    const card = details.getByTestId("agent-update");
    await expect(card).toContainText("Agent 0.1.60 is available");
    await expect(card).toContainText("has no working updater");
    await expect(card.locator("code")).toContainText("/agent/install");
    await expect(card.locator("code")).toContainText("HOPE_ONLY_NEWER=1");
    await expect(card.locator("code")).not.toContainText(
      "HOPE_ENROLLMENT_TOKEN",
    );
    await expect(card.getByRole("button", { name: "Update now" })).toHaveCount(
      0,
    );

    // An agent with an updater is asked with one click, then shows as waiting.
    await page.getByRole("button", { name: "Close host details" }).click();
    await page.getByRole("row").filter({ hasText: "studio-mac" }).click();
    await details.getByRole("tab", { name: "Settings", exact: true }).click();
    await expect(card).toContainText("after checking the release's signature");
    await card.getByRole("button", { name: "Update now" }).click();
    await expect(
      page.getByText(/Asked studio-mac to update to 0\.1\.60/),
    ).toBeVisible();
    expect(posts).toHaveLength(1);
    await expect(card).toContainText("Update requested");
    await expect(card.getByRole("button", { name: "Ask again" })).toBeVisible();

    // An up-to-date enrolled agent still exposes the automatic-update exclusion control.
    await page.getByRole("button", { name: "Close host details" }).click();
    await page.getByRole("row").filter({ hasText: "compute-node" }).click();
    await expect(details.getByText("Agent 0.1.60")).toBeVisible();
    await details.getByRole("tab", { name: "Settings", exact: true }).click();
    await expect(details.getByTestId("agent-update")).toContainText(
      "Exclude from automatic updates",
    );
  });
  test("the map draws inferred routing above its observed hosts", async ({
    page,
  }) => {
    let router = "";
    await page.route("**/api/v1/bootstrap", async (route) => {
      const response = await route.fetch();
      const body = await response.json();
      const atlas = body.hosts.find(
        (h: { name: string }) => h.name === "atlas-nas",
      );
      router = atlas.id;
      // This assertion covers automatic arrangement, independent of saved drag positions.
      body.map.positions = [
        { id: `topology:${atlas.id}`, x: 100000, y: -100000 },
      ];
      const home = body.networks.find(
        (n: { name: string }) => n.name === "Home network",
      );
      for (const host of body.hosts)
        if (host.id !== atlas.id && host.network_ids.includes(home.id))
          host.parent_id = atlas.id;
      await route.fulfill({ response, json: body });
    });
    await page.goto("/?view=map");
    await expect(page.getByTestId("network-map")).toBeVisible();
    await expect(
      page.locator(`.react-flow__edge[data-id^="topology-${router}-"]`),
    ).not.toHaveCount(0);
    const node = await page
      .locator(".react-flow__node")
      .filter({ hasText: "atlas-nas" })
      .boundingBox();
    const child = await page
      .locator(".map-device")
      .filter({ hasText: "studio-mac" })
      .boundingBox();
    expect(node && child && node.y + node.height <= child.y).toBeTruthy();
    await expect(page.getByTestId("subnet-group")).toHaveCount(0);
    await page.screenshot({
      path: "test-results/hope-hierarchy.png",
      fullPage: true,
    });
    await page.getByRole("button", { name: "Subnets", exact: true }).click();
    await expect(page.locator(".react-flow__edge")).toHaveCount(0);
  });
  test("device views keep visible filters and remember the chosen layout", async ({
    page,
  }) => {
    await page.goto("/?view=hosts");
    const navigation = page.getByRole("navigation", {
      name: "Main navigation",
    });
    await expect(navigation.getByRole("button")).toHaveCount(5);
    await page.getByRole("button", { name: /^Needs attention/ }).click();
    await navigate(page, "Network map");
    await expect(
      page.getByRole("button", { name: /^Needs attention/ }),
    ).toHaveAttribute("aria-pressed", "true");
    await page
      .getByRole("button", { name: "Clear filters", exact: true })
      .click();
    await expect(page.getByRole("button", { name: /^All / })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await navigation.getByRole("button", { name: "Overview" }).click();
    await navigation.getByRole("button", { name: "Network map" }).click();
    await expect(page).toHaveURL(/view=map/);
    await page.reload();
    await navigation.getByRole("button", { name: "Overview" }).click();
    await navigation.getByRole("button", { name: "Network map" }).click();
    await expect(page).toHaveURL(/view=map/);
    await page.getByLabel("Search hosts").fill("atlas-nas");
    await page.locator(".map-device").filter({ hasText: "atlas-nas" }).click();
    await expect(page).toHaveURL(/view=host&host=/);
    await expect(page.locator(".host-page")).toBeVisible();
    await expect(page.locator(".inventory-layout")).toHaveCount(0);
    await page.getByRole("button", { name: "Close host details" }).click();
    await expect(page).toHaveURL(/view=map/);
    await expect(page.getByLabel("Search hosts")).toHaveValue("atlas-nas");
    await navigate(page, /^Hosts/);
    await page.getByRole("button", { name: "atlas-nas", exact: true }).click();
    await expect(page).toHaveURL(/view=host&host=/);
    await expect(page.locator(".inventory-layout")).toHaveCount(0);
    await page.goBack();
    await expect(page).toHaveURL(/view=hosts/);
    await expect(page.getByLabel("Search hosts")).toHaveValue("atlas-nas");
    await expect(page.getByRole("table").getByRole("row")).toHaveCount(2);
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toHaveCount(0);
  });
  test("discovery contains progress, exposes failures, and scans all enabled ranges regardless of filters", async ({
    page,
    request,
  }) => {
    const data = await (await request.get("/api/v1/bootstrap")).json();
    const networks = Array.from({ length: 4 }, (_, i) => ({
      ...data.networks[0],
      id: `ux-network-${i}`,
      name: `UX network ${i}`,
      cidr: `10.${i}.0.0/24`,
      enabled: i !== 3,
    }));
    const scans = networks.slice(0, 3).map((n, i) => ({
      id: `ux-scan-${i}`,
      network_id: n.id,
      cidr: n.cidr,
      status: i === 2 ? "failed" : "running",
      scanned: 12,
      total: 254,
      discovered: 2,
      started_at: Date.now() / 1000,
      finished_at: null,
      error: i === 2 ? "Probe failed" : null,
    }));
    await page.route("**/api/v1/bootstrap", (route) =>
      route.fulfill({ json: { ...data, networks, scans } }),
    );
    const started: string[] = [];
    const cancelled: string[] = [];
    await page.route("**/api/v1/networks/*/scan", (route) => {
      started.push(route.request().url().split("/").at(-2)!);
      return route.fulfill({ json: {} });
    });
    await page.route("**/api/v1/scans/*/cancel", (route) => {
      cancelled.push(route.request().url().split("/").at(-2)!);
      return route.fulfill({ json: {} });
    });
    await page.goto("/?view=hosts");
    await expect(
      page.getByRole("region", { name: "Scan progress" }),
    ).toHaveCount(0);
    const trigger = page.getByRole("button", {
      name: "Scanning 2 networks, 1 failed",
    });
    await page.getByRole("combobox", { name: "Filter by network" }).click();
    await page
      .getByRole("option", { name: "UX network 0", exact: true })
      .click();
    await trigger.click();
    await expect(
      page.getByRole("region", { name: "Scan progress" }),
    ).toBeVisible();
    await expect(page.getByText("Probe failed", { exact: true })).toBeVisible();
    await page
      .getByRole("button", { name: "Scan all enabled networks" })
      .click();
    await expect.poll(() => started).toEqual(["ux-network-2"]);
    await page.getByRole("button", { name: "Cancel 10.0.0.0/24 scan" }).click();
    await expect.poll(() => cancelled).toEqual(["ux-scan-0"]);
    await page.keyboard.press("Escape");
    await expect(trigger).toBeFocused();
    await expect(
      page.getByRole("region", { name: "Scan progress" }),
    ).toHaveCount(0);
    await page.setViewportSize({ width: 390, height: 844 });
    await page.getByRole("button", { name: "Navigation", exact: true }).click();
    await expect(
      page.getByRole("button", { name: "Network map", exact: true }),
    ).toBeVisible();
    await page.keyboard.press("Escape");
    await page
      .getByRole("button", { name: "More actions", exact: true })
      .click();
    await expect(
      page.getByRole("button", { name: "Settings", exact: true }),
    ).toBeVisible();
    await page.keyboard.press("Escape");
    await trigger.click();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBeTruthy();
    await page.screenshot({ path: "test-results/hope-discovery-mobile.png" });
    await page.keyboard.press("Escape");
    await page.setViewportSize({ width: 1512, height: 982 });
    await trigger.click();
    await page.screenshot({ path: "test-results/hope-discovery.png" });
  });
  test("Networks is a reloadable page and Settings no longer contains network controls", async ({
    page,
  }) => {
    await page.goto("/");
    await page
      .getByRole("navigation", { name: "Main navigation" })
      .getByRole("button", { name: "Networks", exact: true })
      .click();
    await expect(page).toHaveURL(/view=networks/);
    await expect(
      page.getByRole("heading", { name: "Networks", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Scan Home network now" }),
    ).toBeVisible();
    await page.reload();
    await expect(
      page.getByRole("combobox", { name: "Scan interval for Home network" }),
    ).toBeVisible();
    await globalAction(page, "Settings");
    await expect(
      page
        .getByRole("dialog", { name: "Settings" })
        .getByText("Monitored ranges"),
    ).toHaveCount(0);
    await expect(
      page
        .getByRole("dialog", { name: "Settings" })
        .getByRole("button", { name: "Add network" }),
    ).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog", { name: "Settings" })).toHaveCount(0);
    await page.setViewportSize({ width: 390, height: 844 });
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBeTruthy();
    await page.screenshot({
      path: "test-results/hope-networks-mobile.png",
      fullPage: true,
    });
  });
  test("Agents excludes revoked records, includes never-reported and stale agents, and opens host details", async ({
    page,
    request,
  }) => {
    const data = await (await request.get("/api/v1/bootstrap")).json();
    const template = data.hosts.find(
      (host: { name: string }) => host.name === "atlas-nas",
    );
    const revoked = {
      ...template,
      id: "ux-revoked-host",
      agent_id: "ux-revoked-agent",
      name: "revoked-agent",
      agent_revoked: true,
    };
    const stale = {
      ...template,
      id: "ux-stale-host",
      agent_id: "ux-stale-agent",
      name: "stale-agent",
      last_report: 1,
      agent_revoked: false,
    };
    await page.route("**/api/v1/bootstrap", (route) =>
      route.fulfill({
        json: { ...data, hosts: [...data.hosts, revoked, stale] },
      }),
    );
    await page.goto("/?view=agents");
    await expect(
      page.getByRole("heading", { name: "Agents", exact: true }),
    ).toBeVisible();
    const enrolledAgentCount =
      data.hosts.filter(
        (host: {
          agent_id: string | null;
          agent_revoked: boolean;
          agent_uninstalled: boolean;
        }) => host.agent_id && !host.agent_revoked && !host.agent_uninstalled,
      ).length + 1;
    await expect(
      page
        .getByRole("button", { name: "Agents", exact: true })
        .locator(".count"),
    ).toHaveText(String(enrolledAgentCount));
    await expect(
      page.getByRole("article", { name: "quiet-node" }),
    ).toContainText("Never reported");
    await expect(
      page.getByRole("article", { name: "revoked-agent" }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("article", { name: "stale-agent" }),
    ).toContainText("Not reporting");
    const filters = page.getByRole("group", { name: "Agent status" });
    await filters.getByRole("button", { name: /^Never reported/ }).click();
    await expect(
      page.getByRole("article", { name: "quiet-node" }),
    ).toBeVisible();
    await expect(page.getByRole("article", { name: "atlas-nas" })).toHaveCount(
      0,
    );
    await expect(filters.getByRole("button", { name: /Revoked/ })).toHaveCount(
      0,
    );
    await filters.getByRole("button", { name: /^All agents/ }).click();
    await page
      .getByRole("textbox", { name: "Search agents" })
      .fill("ux-stale-agent");
    await expect(page.getByRole("article")).toHaveCount(1);
    await page.getByRole("textbox", { name: "Search agents" }).fill("");
    await page.reload();
    await expect(
      page.getByRole("article", { name: "quiet-node" }),
    ).toBeVisible();
    await page.screenshot({
      path: "test-results/hope-agents.png",
      fullPage: true,
    });
    await page.setViewportSize({ width: 390, height: 844 });
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBeTruthy();
    await page.screenshot({
      path: "test-results/hope-agents-mobile.png",
      fullPage: true,
    });
    await page.getByRole("button", { name: "quiet-node", exact: true }).click();
    await expect(page).toHaveURL(/view=host/);
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toContainText("Agent has never reported");
    await page.getByRole("button", { name: "Close host details" }).click();
    await expect(page).toHaveURL(/view=agents/);
  });
  test("top-level navigation clears host deep links and keeps Overview canonical", async ({
    page,
  }) => {
    await page.route("**/api/v1/bootstrap", async (route) => {
      const response = await route.fetch();
      const data = await response.json();
      const template = data.hosts.find(
        (host: { name: string }) => host.name === "atlas-nas",
      );
      const agent = {
        ...template,
        id: "navigation-agent-host",
        agent_id: "navigation-agent-id",
        name: "navigation-agent",
        last_report: Date.now() / 1000,
        agent_revoked: false,
        agent_uninstalled: false,
      };
      await route.fulfill({
        response,
        json: { ...data, hosts: [...data.hosts, agent] },
      });
    });

    await page.goto("/?view=agents");
    await page
      .getByRole("button", { name: "navigation-agent", exact: true })
      .click();
    await expect(page).toHaveURL(/view=host&host=navigation-agent-host/);
    await expect(page.locator(".host-page")).toBeVisible();

    await navigate(page, "Overview");
    await expect(
      page.getByRole("heading", { name: "Overview", exact: true }),
    ).toBeVisible();
    await expect(page).toHaveURL((url) => url.search === "");
    await page.reload();
    await expect(
      page.getByRole("heading", { name: "Overview", exact: true }),
    ).toBeVisible();
    await expect(page).toHaveURL((url) => url.search === "");

    await page.goBack();
    await expect(page).toHaveURL(/view=host&host=navigation-agent-host/);
    await expect(page.locator(".host-page")).toBeVisible();
    await page.goForward();
    await expect(page).toHaveURL((url) => url.search === "");
    await expect(
      page.getByRole("heading", { name: "Overview", exact: true }),
    ).toBeVisible();
  });
  test("agent uninstall keeps history and makes device removal explicit", async ({
    page,
    request,
  }) => {
    const data = await (await request.get("/api/v1/bootstrap")).json();
    const host = data.hosts.find(
      (entry: { agent_id: string | null; agent_revoked: boolean }) =>
        entry.agent_id && !entry.agent_revoked,
    );
    await page.goto(`/?view=host&host=${host.id}`);
    await page.getByRole("tab", { name: "Settings", exact: true }).click();
    await page
      .getByRole("button", { name: "Uninstall agent", exact: true })
      .click();
    const dialog = page.getByRole("dialog", { name: "Uninstall agent" });
    await dialog.getByRole("tab", { name: "Uninstall manually" }).click();
    await expect(dialog.locator("code")).toContainText("/agent/uninstall");
    await expect(dialog.locator("code")).toContainText(
      'sh "$installer" uninstall',
    );
    await expect(dialog).toContainText("Device history stays");
    await expect(dialog).toContainText("cannot confirm it remotely");
    const before = await (await request.get("/api/v1/bootstrap")).json();
    expect(
      before.hosts.find((entry: { id: string }) => entry.id === host.id)
        .agent_revoked,
    ).toBe(false);
    await dialog
      .getByRole("button", { name: "Disable agent", exact: true })
      .click();
    await expect(dialog).toContainText("Access revoked. Reports are blocked.");
    await expect(
      dialog.getByRole("button", { name: "Disable agent" }),
    ).toHaveCount(0);
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(
      dialog.getByRole("button", { name: "Copy uninstall command" }),
    ).toBeVisible();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBeTruthy();
    await page.screenshot({
      path: "test-results/hope-uninstall-mobile.png",
      fullPage: true,
    });
    await page.keyboard.press("Escape");
    await page.reload();
    await page.getByRole("tab", { name: "Settings", exact: true }).click();
    await expect(
      page.getByRole("button", { name: "Re-enroll agent", exact: true }),
    ).toBeVisible();
    await page
      .getByRole("button", { name: "Uninstall agent", exact: true })
      .click();
    await dialog.getByRole("tab", { name: "Uninstall manually" }).click();
    await expect(dialog).toContainText("Access revoked. Reports are blocked.");
    const after = await (await request.get("/api/v1/bootstrap")).json();
    expect(
      after.hosts.find((entry: { id: string }) => entry.id === host.id)
        .last_report,
    ).toBe(host.last_report);
  });
  test("SSH uninstall verifies the fingerprint and submits a removal job", async ({
    page,
    request,
  }) => {
    const data = await (await request.get("/api/v1/bootstrap")).json();
    const host = data.hosts.find(
      (entry: { agent_id: string | null }) => entry.agent_id,
    );
    const primaryAddress = "192.168.1.5";
    await page.route("**/api/v1/bootstrap", async (route) => {
      const response = await route.fetch();
      const bootstrap = await response.json();
      bootstrap.hosts = bootstrap.hosts.map(
        (entry: {
          id: string;
          addresses: string[];
          primary_ip?: string | null;
        }) =>
          entry.id === host.id
            ? {
                ...entry,
                addresses: ["127.0.0.1", "203.0.113.9", primaryAddress],
                primary_ip: primaryAddress,
              }
            : entry,
      );
      await route.fulfill({ response, json: bootstrap });
    });
    let submitted: Record<string, unknown> | undefined;
    await page.route("**/api/v1/ssh/jobs", (route) =>
      route.fulfill({ json: [] }),
    );
    await page.route("**/api/v1/ssh/fingerprint", (route) =>
      route.fulfill({
        json: {
          fingerprint: "SHA256:verified-test-key",
          instruction: "Verify the host key",
        },
      }),
    );
    await page.route("**/api/v1/ssh/uninstall", async (route) => {
      submitted = route.request().postDataJSON();
      await route.fulfill({
        json: {
          id: "uninstall-job",
          host_id: host.id,
          host: primaryAddress,
          port: 22,
          fingerprint: "SHA256:verified-test-key",
          username: "root",
          mode: "uninstall",
          status: "completed",
          stage: "complete",
          stages: [
            "connecting",
            "authenticating",
            "preflight",
            "uninstall",
            "complete",
          ],
          error_code: null,
          message:
            "Agent service, updater and binary removed. Access revoked; identity and device history retained.",
          guidance: null,
          notice: null,
          agent_id: host.agent_id,
          created_at: Date.now() / 1000,
          finished_at: Date.now() / 1000,
        },
      });
    });
    await page.goto(`/?view=host&host=${host.id}`);
    await page.getByRole("tab", { name: "Settings", exact: true }).click();
    await page
      .getByRole("button", { name: "Uninstall agent", exact: true })
      .click();
    const dialog = page.getByRole("dialog", { name: "Uninstall agent" });
    await expect(
      dialog.getByRole("tab", { name: "Uninstall over SSH" }),
    ).toHaveAttribute("aria-selected", "true");
    const addressSelect = dialog.getByRole("combobox").first();
    await expect(addressSelect).toContainText(primaryAddress);
    await addressSelect.click();
    await expect(page.getByRole("option", { name: "127.0.0.1" })).toBeVisible();
    await expect(
      page.getByRole("option", { name: "203.0.113.9" }),
    ).toBeVisible();
    await page.keyboard.press("Escape");
    await dialog.getByLabel("Username", { exact: true }).fill("root");
    await dialog
      .getByLabel("Password", { exact: true })
      .fill("ssh-test-password");
    const remove = dialog.getByRole("button", { name: /^Uninstall from/ });
    await expect(remove).toBeDisabled();
    await dialog
      .getByRole("button", { name: "Inspect host fingerprint" })
      .click();
    await expect(remove).toBeDisabled();
    await dialog
      .getByRole("button", { name: "I verified this fingerprint" })
      .click();
    await remove.click();
    await expect(dialog.getByTestId("ssh-job")).toContainText("Uninstall on");
    await expect(dialog).toContainText(
      "Agent service, updater and binary removed.",
    );
    expect(submitted).toMatchObject({
      host_id: host.id,
      address: primaryAddress,
      username: "root",
      password: "ssh-test-password",
      fingerprint: "SHA256:verified-test-key",
    });
    await page.setViewportSize({ width: 390, height: 844 });
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBeTruthy();
    await page.screenshot({
      path: "test-results/hope-ssh-uninstall-mobile.png",
      fullPage: true,
    });
  });
  test("SSH installation leaves an address blank when no configured network matches", async ({
    page,
    request,
  }) => {
    const auth = (await (await request.get("/api/v1/auth/status")).json()) as {
      setup_required: boolean;
    };
    if (auth.setup_required) await seed(request);
    else {
      const session = await request.post("/api/v1/auth/login", {
        data: { password },
      });
      expect(session.ok(), await session.text()).toBeTruthy();
      const bootstrap = await (await request.get("/api/v1/bootstrap")).json();
      if (!Array.isArray(bootstrap.hosts) || bootstrap.hosts.length === 0)
        await seed(request);
    }
    await page.context().addCookies((await request.storageState()).cookies);
    const data = await (await request.get("/api/v1/bootstrap")).json();
    const template = data.hosts[0];
    const host = {
      ...template,
      id: "no-network-address-host",
      name: "no-network-address-host",
      name_override: null,
      agent_id: null,
      agent_revoked: false,
      agent_uninstalled: false,
      source: null,
      source_device_id: null,
      child: null,
      addresses: ["127.0.0.1", "203.0.113.9"],
      primary_ip: null,
      network_ids: [],
    };
    await page.route("**/api/v1/bootstrap", async (route) => {
      const response = await route.fetch();
      const bootstrap = await response.json();
      bootstrap.hosts = [...bootstrap.hosts, host];
      await route.fulfill({ response, json: bootstrap });
    });
    await page.route("**/api/v1/releases", (route) =>
      route.fulfill({
        json: { packaged_platforms: ["linux-x86_64"], version: "0.1.0" },
      }),
    );
    await page.route("**/api/v1/ssh/jobs", (route) =>
      route.fulfill({ json: [] }),
    );
    await page.goto(`/?view=host&host=${host.id}`);
    await page.getByRole("tab", { name: "Info", exact: true }).click();
    await page.getByRole("button", { name: "Install reporting agent" }).click();
    const dialog = page.getByRole("dialog", { name: "Add a reporting agent" });
    const sshTab = dialog.getByRole("tab", { name: "Install over SSH" });
    await expect(sshTab).toBeEnabled();
    await sshTab.click();
    const addressSelect = dialog.getByRole("combobox").first();
    await expect(addressSelect).toContainText("Choose an address");
    await expect(
      dialog.getByRole("button", { name: "Inspect host fingerprint" }),
    ).toBeDisabled();
    await addressSelect.click();
    await expect(page.getByRole("option", { name: "127.0.0.1" })).toBeVisible();
    await expect(
      page.getByRole("option", { name: "203.0.113.9" }),
    ).toBeVisible();
  });
  test("the administrator changes the password and other browsers are signed out", async ({
    page,
    playwright,
  }) => {
    const changed = "hope-browser-new-password";
    // A second browser, signed in with the current password.
    const other = await playwright.request.newContext({
      baseURL: "http://127.0.0.1:8087",
    });
    expect(
      (await other.post("/api/v1/auth/login", { data: { password } })).status(),
    ).toBe(200);
    expect((await other.get("/api/v1/bootstrap")).status()).toBe(200);

    await page.goto("/");
    // Networks holds ranges and server facts only; the theme lives in the sidebar.
    await globalAction(page, "Settings");
    await expect(page.getByRole("dialog", { name: "Settings" })).toBeVisible();
    await expect(
      page.getByRole("dialog").getByRole("group", { name: "Theme" }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("dialog").getByRole("form", { name: "Change password" }),
    ).toHaveCount(0);
    await page.keyboard.press("Escape");
    await globalAction(page, "Account");
    await expect(page.getByRole("dialog", { name: "Account" })).toBeVisible();
    const form = page.getByRole("form", { name: "Change password" });
    const submit = form.getByRole("button", { name: "Change password" });
    await form.getByLabel("Current password").fill("not-the-password");
    await form.getByLabel("New password", { exact: true }).fill(changed);
    await form.getByLabel("Confirm new password").fill("something-else");
    await expect(
      form.getByText("The new passwords do not match."),
    ).toBeVisible();
    await expect(submit).toBeDisabled();
    await form.getByLabel("Confirm new password").fill(changed);
    await submit.click();
    await expect(
      form.getByText("The current password is incorrect"),
    ).toBeVisible();
    expect((await other.get("/api/v1/bootstrap")).status()).toBe(200);

    await form.getByLabel("Current password").fill(password);
    await page.screenshot({ path: "test-results/hope-change-password.png" });
    await submit.click();
    await expect(
      page.getByText("Password changed. Other browsers were signed out."),
    ).toBeVisible();
    await expect(page.getByRole("dialog", { name: "Account" })).toHaveCount(0);

    // The other browser is signed out; this one keeps working with its fresh session.
    expect((await other.get("/api/v1/bootstrap")).status()).toBe(401);
    await page.reload();
    await expect(page.getByRole("heading", { name: "Sign in" })).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: "Settings", exact: true }),
    ).toBeVisible();
    adminState = await page.context().storageState();
    for (const [candidate, status] of [
      [password, 401],
      [changed, 200],
    ] as const)
      expect(
        (
          await other.post("/api/v1/auth/login", {
            data: { password: candidate },
          })
        ).status(),
      ).toBe(status);
    await other.dispose();
    password = changed;
  });
  test("network inventory is compact by default and keyboard-toggleable on mobile", async ({
    page,
    request,
  }) => {
    await seed(request);
    await page.context().addCookies((await request.storageState()).cookies);
    const hostName = "atlas-nas";
    const interfaces = [
      {
        name: "eth0",
        mac: "02:00:00:00:10:01",
        addresses: ["192.168.1.81"],
        gateway: "192.168.1.1",
      },
      {
        name: "eth1",
        mac: "02:00:00:00:10:02",
        addresses: ["192.168.30.81", "fd00::81"],
        gateway: "192.168.30.1",
      },
      {
        name: "br-guest",
        mac: "02:00:00:00:10:03",
        addresses: [],
        gateway: null,
      },
    ];
    await page.route("**/api/v1/bootstrap", async (route) => {
      const response = await route.fetch();
      const bootstrap = await response.json();
      const hosts = bootstrap.hosts.filter(
        (item: { name: string }) => item.name === hostName,
      );
      expect(hosts.length).toBeGreaterThan(0);
      for (const host of hosts) {
        host.inventory = { ...host.inventory, interfaces };
        host.macs = interfaces.map((item) => item.mac);
      }
      await route.fulfill({ response, json: bootstrap });
    });

    await page.setViewportSize({ width: 360, height: 800 });
    await page.goto("/?view=hosts");
    await page
      .getByRole("button", { name: hostName, exact: true })
      .first()
      .click();
    const details = page.getByRole("complementary", { name: "Host details" });
    await details.getByRole("tab", { name: "Info", exact: true }).click();
    const networkInventory = details.locator("details.network-inventory");
    const summary = networkInventory.locator("summary");
    await expect(summary).toContainText("Primary MAC 02:00:00:00:10:01");
    await expect(summary).toContainText("3 MAC addresses · 3 interfaces");
    await expect(networkInventory).toHaveJSProperty("open", false);
    const interfaceRows = networkInventory.locator(
      ".network-inventory-content .interface-row",
    );
    await expect(interfaceRows).toHaveCount(3);
    await expect(interfaceRows.nth(0)).toBeHidden();

    await summary.focus();
    await expect(summary).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(networkInventory).toHaveJSProperty("open", true);
    await expect(interfaceRows.nth(0)).toContainText("eth0");
    await expect(interfaceRows.nth(1)).toContainText("eth1");
    await expect(interfaceRows.nth(2)).toContainText("br-guest");
    await expect(networkInventory).toContainText("fd00::81");
    await expect(networkInventory).toContainText("Gateway 192.168.30.1");
    await expect(networkInventory).toContainText("No address reported");
    await expect
      .poll(() =>
        page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
      )
      .toBeTruthy();

    await page.keyboard.press("Space");
    await expect(networkInventory).toHaveJSProperty("open", false);
    await expect(interfaceRows.nth(2)).toBeHidden();
  });
  test("Proxmox host details list guests and link to guest pages", async ({
    page,
    request,
  }) => {
    await seed(request, true);
    await page.context().addCookies((await request.storageState()).cookies);
    await page.clock.install();
    const collectedAt = Date.now() / 1000;
    let historicalCpu = 44;
    await page.route(
      "**/api/v1/hosts/*/source-metrics?hours=24",
      async (route) => {
        const sourceId = decodeURIComponent(
          new URL(route.request().url()).pathname.split("/").at(-2)!,
        );
        await route.fulfill({
          json: [
            {
              source_device_id: sourceId,
              collected_at: collectedAt - 60,
              resolution_seconds: 30,
              metrics: {
                cpu_percent: 15,
                memory_used_bytes: 512 * 1024 * 1024,
                memory_total_bytes: 2 * 1024 * 1024 * 1024,
                memory_measurement: "hypervisor_guest",
                network_rx_bytes_per_second: 10,
                network_tx_bytes_per_second: 5,
              },
            },
            {
              source_device_id: "historical-source-1234",
              collected_at: collectedAt - 120,
              resolution_seconds: 30,
              metrics: {
                cpu_percent: historicalCpu,
                memory_used_bytes: 1024,
                memory_total_bytes: 2048,
                memory_measurement: "hypervisor_guest",
                network_rx_bytes_per_second: 2,
                network_tx_bytes_per_second: 1,
              },
            },
          ],
        });
      },
    );
    // The browser API has no scan-result ingestion endpoint. Shape the server's
    // source-only fixture as the canonical DTO returned after a scan host with
    // the configured NIC MAC has been associated with that Proxmox source.
    await page.route("**/api/v1/bootstrap", async (route) => {
      const response = await route.fetch();
      const bootstrap = (await response.json()) as Bootstrap;
      const sourceGuest = bootstrap.hosts.find(
        (host) => host.name === "dns-container",
      );
      expect(
        sourceGuest,
        `Bootstrap hosts: ${bootstrap.hosts.map((host) => `${host.id}:${host.name}:${host.source?.kind ?? "core"}`).join(", ")}`,
      ).toBeTruthy();
      const proxmoxParent = bootstrap.hosts.find(
        (host) => host.name === "proxmox-fixture-node",
      );
      expect(proxmoxParent).toBeTruthy();
      const matchedGuests = Array.from({ length: 13 }, (_, index) => {
        const sourceDeviceId = `matched-source-${index}`;
        const key = index === 0 ? "202" : `matched-${index}`;
        return {
          ...sourceGuest!,
          id: `scanned-guest-${index}`,
          name: index === 0 ? "dns-container" : `matched-guest-${index}`,
          discovery_method: "arp",
          addresses: [`192.168.1.${41 + index}`],
          macs: [`02:fe:00:00:00:${(2 + index).toString(16).padStart(2, "0")}`],
          source_device_id: sourceDeviceId,
          source: { ...sourceGuest!.source!, key },
          child: {
            ...sourceGuest!.child!,
            identity: { ...sourceGuest!.child!.identity, key },
          },
          // The scan-derived host can retain an inferred network parent. The
          // authenticated source runs_on edge still identifies its hypervisor.
          parent_id: "inferred-network-gateway",
          relationships: [
            {
              from_device_id: sourceDeviceId,
              to_device_id: proxmoxParent!.id,
              kind: "runs_on" as const,
              detail: null,
            },
          ],
        };
      });
      const unmatchedGuests = Array.from({ length: 5 }, (_, index) => {
        const sourceDeviceId = `unmatched-source-${index}`;
        const key = `unmatched-${index}`;
        return {
          ...sourceGuest!,
          id: sourceDeviceId,
          name: `unmatched-guest-${index}`,
          source_device_id: sourceDeviceId,
          source: { ...sourceGuest!.source!, key },
          child: {
            ...sourceGuest!.child!,
            identity: { ...sourceGuest!.child!.identity, key },
          },
          parent_id: proxmoxParent!.id,
        };
      });
      const archivedRemoteGuest = {
        ...sourceGuest!,
        id: "archived-remote-dns2",
        name: "dns2",
        lifecycle: "archived" as const,
        child: { ...sourceGuest!.child!, node: "catacomb" },
      };
      bootstrap.hosts = [
        ...bootstrap.hosts.filter((host) => host.id !== sourceGuest!.id),
        ...matchedGuests,
        ...unmatchedGuests,
        archivedRemoteGuest,
      ];
      await route.fulfill({ response, json: bootstrap });
    });
    const bootstrapResponse = page.waitForResponse((response) =>
      response.url().endsWith("/api/v1/bootstrap"),
    );
    await page.goto("/?view=hosts");
    const bootstrap = (await (await bootstrapResponse).json()) as Bootstrap;
    const guestHosts = bootstrap.hosts.filter(
      (host) =>
        host.source?.kind === "proxmox_lxc" &&
        host.lifecycle !== "archived" &&
        (host.parent_id ===
          bootstrap.hosts.find((item) => item.name === "proxmox-fixture-node")
            ?.id ||
          host.relationships.some(
            (relationship) =>
              relationship.kind === "runs_on" &&
              relationship.from_device_id === host.source_device_id &&
              relationship.to_device_id ===
                bootstrap.hosts.find(
                  (item) => item.name === "proxmox-fixture-node",
                )?.id,
          )),
    );
    expect(guestHosts).toHaveLength(18);
    expect(new Set(guestHosts.map((host) => host.id)).size).toBe(18);
    expect(new Set(guestHosts.map((host) => host.source_device_id)).size).toBe(
      18,
    );
    const mergedGuest = guestHosts.find(
      (host) => host.name === "dns-container",
    );
    expect(mergedGuest?.id).toBe("scanned-guest-0");
    expect(mergedGuest?.source?.kind).toBe("proxmox_lxc");
    expect(mergedGuest?.source_device_id).toBe("matched-source-0");
    expect(mergedGuest?.child?.identity.key).toBe("202");
    expect(mergedGuest?.child?.metrics.memory_measurement).toBe(
      "hypervisor_guest",
    );
    expect(mergedGuest?.child?.metrics.disk_read_bytes_per_second).toBe(2048);
    expect(mergedGuest?.macs).toContain("02:fe:00:00:00:02");
    await page.getByLabel("Search hosts").fill("dns-container");
    await expect(
      page.getByRole("row").filter({ hasText: "dns-container" }),
    ).toHaveCount(1);
    const deviceColumns = await page
      .getByRole("columnheader")
      .allTextContents();
    const deviceCells = await page
      .getByRole("row")
      .filter({ hasText: "dns-container" })
      .getByRole("cell")
      .allTextContents();
    await page.getByLabel("Search hosts").fill("");
    await page.getByLabel("Search hosts").fill("proxmox-fixture-node");
    await page
      .getByRole("row")
      .filter({ hasText: "proxmox-fixture-node" })
      .click();
    const proxmoxDetails = page.getByRole("complementary", {
      name: "Host details",
    });
    await proxmoxDetails.getByRole("tab", { name: "Guests" }).click();
    await expect(
      proxmoxDetails.getByRole("heading", { name: "Proxmox guests" }),
    ).toBeVisible();
    await expect(
      proxmoxDetails.locator('section[aria-label="Proxmox guests"] tbody tr'),
    ).toHaveCount(18);
    await expect(proxmoxDetails.getByRole("table")).toContainText(
      "dns-container",
    );
    await expect(proxmoxDetails.getByRole("table")).toContainText("LXC");
    await expect(proxmoxDetails.getByRole("table")).toContainText(
      "Debian GNU/Linux 13",
    );
    await expect(proxmoxDetails.getByRole("table")).not.toContainText("dns2");
    const guestsTable = proxmoxDetails.getByRole("table");
    await expect(guestsTable.getByRole("columnheader")).toHaveText(
      deviceColumns,
    );
    const guestRow = guestsTable
      .getByRole("row")
      .filter({ hasText: "dns-container" });
    await expect(guestRow.getByRole("cell")).toHaveCount(deviceCells.length);
    // The relative last-seen time can advance between views.
    expect(
      (await guestRow.getByRole("cell").allTextContents()).slice(0, -1),
    ).toEqual(deviceCells.slice(0, -1));
    await expect(guestRow).toContainText("Running");
    await expect(guestRow.getByRole("cell").nth(3)).toContainText("15%");
    await expect(guestRow.getByRole("cell").nth(4)).toContainText("25%");
    await expect(guestRow.getByRole("cell").nth(5)).toContainText("32%");
    await expect(guestRow.getByRole("cell").nth(5)).toContainText("source");
    await guestsTable.getByRole("button", { name: "CPU", exact: true }).click();
    await expect(
      guestsTable.getByRole("columnheader", { name: "CPU", exact: true }),
    ).toHaveAttribute("aria-sort", "ascending");
    await page.screenshot({
      path: "test-results/hope-guests.png",
      fullPage: true,
    });
    await proxmoxDetails.getByRole("button", { name: "dns-container" }).click();
    await expect(page).toHaveURL(/host=/);
    await expect(
      page.getByRole("heading", { name: "dns-container" }),
    ).toBeVisible();
    const guestTabs = await page
      .getByRole("complementary", { name: "Host details" })
      .getByRole("tab")
      .allTextContents();
    expect(guestTabs.slice(0, 2)).toEqual(["Metrics", "Info"]);
    await expect(page.getByRole("tab", { name: "Metrics" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await page.getByRole("tab", { name: "Metrics" }).click();
    await expect(
      page.getByTestId("chart-Proxmox hypervisor CPU"),
    ).toBeVisible();
    await expect(
      page.getByTestId("chart-Proxmox hypervisor memory"),
    ).toBeVisible();
    await expect(
      page.getByTestId("chart-Proxmox hypervisor network"),
    ).toBeVisible();
    await expect(
      page.getByTestId("chart-Proxmox hypervisor disk I/O"),
    ).toBeVisible();
    await page.getByRole("tab", { name: "Info" }).click();
    await expect(page.locator(".reading-meter-card")).toHaveCount(0);
    await expect(
      page.getByRole("region", { name: "Current readings" }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("region", { name: "Source inventory" }),
    ).toContainText("Guest OS data observed");
    await expect(
      page.getByRole("region", { name: "Source inventory" }),
    ).toContainText("Hypervisor memory");
    await expect(
      page.getByRole("region", { name: "Source inventory" }),
    ).toContainText("Read 2.0 KiB/s");
    await expect(
      page.getByRole("region", { name: "Source inventory" }),
    ).toContainText("4.7 GiB used of 14.9 GiB");
    await expect(
      page.getByRole("region", { name: "Source inventory" }),
    ).toContainText("VLAN 42");
    await expect(
      page.getByRole("region", { name: "Source inventory" }),
    ).toContainText("local-lvm");
    await expect(
      page.getByRole("region", { name: "Source inventory" }),
    ).toContainText("Configured disk scsi0");
    await page.getByRole("tab", { name: "Metrics" }).click();
    const sourceHistory = page.getByRole("region", {
      name: "Source metrics history",
    });
    await expect(sourceHistory.getByRole("table").getByRole("row")).toHaveCount(
      2,
    );
    await sourceHistory.getByLabel("Historical source").click();
    await page
      .getByRole("option", { name: /^Merged source · historic/ })
      .click();
    await expect(sourceHistory.getByRole("table")).toContainText("44.0%");
    historicalCpu = 55;
    await page.clock.runFor(31000);
    await expect(sourceHistory.getByRole("table")).toContainText("55.0%");
    await expect(sourceHistory.getByRole("table").getByRole("row")).toHaveCount(
      2,
    );
    await page.getByRole("tab", { name: "Info" }).click();
    await expect(proxmoxDetails).toContainText("Runs on");
    await proxmoxDetails
      .getByRole("button", { name: "proxmox-fixture-node" })
      .click();
    await expect(
      page.getByRole("heading", { name: "proxmox-fixture-node" }),
    ).toBeVisible();
    await page.setViewportSize({ width: 375, height: 812 });
    await page.goto("/?view=hosts");
    await page.getByLabel("Search hosts").fill("proxmox-fixture-node");
    await page
      .getByRole("row")
      .filter({ hasText: "proxmox-fixture-node" })
      .click();
    const mobileDetails = page.getByRole("complementary", {
      name: "Host details",
    });
    await mobileDetails.getByRole("tab", { name: "Guests" }).click();
    await expect(
      mobileDetails.getByRole("columnheader", { name: "Status" }),
    ).toBeVisible();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBeTruthy();
    await page.screenshot({
      path: "test-results/hope-guests-mobile.png",
      fullPage: true,
    });
    await mobileDetails.getByRole("button", { name: "dns-container" }).click();
    await expect(
      page.getByRole("heading", { name: "dns-container" }),
    ).toBeVisible();
    const overflow = await page.evaluate(() => ({
      width: window.innerWidth,
      document: document.documentElement.scrollWidth,
      nodes: [...document.querySelectorAll("body *")]
        .map((element) => ({
          tag: element.tagName,
          className:
            typeof element.className === "string" ? element.className : "",
          right: Math.round(element.getBoundingClientRect().right),
          width: Math.round(element.getBoundingClientRect().width),
        }))
        .filter((item) => item.right > window.innerWidth + 1)
        .slice(0, 8),
    }));
    expect(
      overflow.document <= overflow.width,
      JSON.stringify(overflow),
    ).toBeTruthy();
  });
  test("topology uses collected guest relationships and keeps missing parents unresolved", async ({
    page,
    request,
  }) => {
    const bootstrap: Bootstrap = await (
      await request.get("/api/v1/bootstrap")
    ).json();
    const guest = bootstrap.hosts.find((h) => h.name === "dns-container")!;
    const relation = guest.relationships.find((r) => r.kind === "runs_on")!;
    expect(relation).toBeTruthy();
    const parent = bootstrap.hosts.find((h) => h.id === relation.to_device_id)!;
    expect(parent).toBeTruthy();
    let canonical = false;
    await page.route("**/api/v1/bootstrap", async (route) => {
      const response = await route.fetch();
      const body = await response.json();
      body.map.positions = [];
      if (canonical) {
        const projected = body.hosts.find(
          (host: { id: string }) => host.id === guest.id,
        );
        projected.id = `canonical-${guest.id}`;
        projected.child.metrics.memory_used_bytes = Math.floor(
          projected.child.metrics.memory_total_bytes * 0.95,
        );
      }
      await route.fulfill({ response, json: body });
    });
    await page.goto("/?view=map");
    await expectMapFits(page);
    const edge = page.locator(
      `.react-flow__edge[data-id="topology-${parent.id}-${guest.id}"]`,
    );
    await expect(edge).toHaveClass(/reported-edge/);
    await expect(edge).toHaveAttribute("aria-label", /Runs on.*Reported/);
    const childNode = page.locator(`[data-host-id="${guest.id}"]`);
    const parentNode = page.locator(`[data-host-id="${parent.id}"]`);
    const childBox = await childNode.boundingBox(),
      parentBox = await parentNode.boundingBox();
    expect(
      childBox && parentBox && childBox.y > parentBox.y + parentBox.height,
    ).toBeTruthy();
    await page.getByLabel("Search hosts").fill("dns-container");
    await expect(childNode).toBeVisible();
    await expect(page.locator(".react-flow__edge")).toHaveCount(0);
    await page
      .getByRole("button", { name: "Show metrics", exact: true })
      .click();
    await expect(childNode.locator(".node-health")).toContainText("RAM 25%");
    canonical = true;
    await page.reload();
    await expect(
      page.locator(
        `.react-flow__edge[data-id="topology-${parent.id}-canonical-${guest.id}"]`,
      ),
    ).toHaveClass(/reported-edge/);
    await expect(
      page.locator(`[data-host-id="canonical-${guest.id}"]`),
    ).toBeVisible();
    await expect(
      page.locator(`[data-host-id="canonical-${guest.id}"]`),
    ).toHaveAttribute("data-tone", "warning");
    await expect(
      page.locator(`[data-host-id="canonical-${guest.id}"] .node-status`),
    ).toHaveText("Memory at 95%");
    await expect(childNode).toHaveCount(0);
    await page.getByRole("button", { name: "Dark", exact: true }).click();
    await page.screenshot({
      path: "test-results/redesign-reported-topology.png",
      fullPage: true,
      animations: "disabled",
    });
  });

  test("archived source guests stay out of counts, maps and the found-device list", async ({
    page,
    request,
  }) => {
    await seed(request, true);
    const seededBootstrap = await (
      await request.get("/api/v1/bootstrap")
    ).json();
    const expectedActiveCount = seededBootstrap.hosts.filter(
      (host: { lifecycle: string | null }) => host.lifecycle !== "archived",
    ).length;
    await page.route("**/api/v1/bootstrap", async (route) => {
      const response = await route.fetch();
      const body = await response.json();
      const guest = body.hosts.find(
        (host: { name: string }) => host.name === "dns-container",
      );
      body.hosts.push({
        ...guest,
        id: `archived-${guest.id}`,
        name: "archived-fixture-only",
        lifecycle: "archived",
      });
      await route.fulfill({ response, json: body });
    });
    await page.context().addCookies((await request.storageState()).cookies);
    await page.goto("/");
    await expect(page.getByTestId("tile-hosts")).toContainText(
      `/ ${expectedActiveCount}`,
    );
    await expect(
      page.getByRole("region", { name: "Busiest CPUs" }),
    ).not.toContainText("archived-fixture-only");
    await page.goto("/?view=map");
    await expect(
      page.locator(".map-device").filter({ hasText: "archived-fixture-only" }),
    ).toHaveCount(0);
    await page.goto("/?view=hosts");
    await expect(page.locator(".host-table")).not.toContainText(
      "archived-fixture-only",
    );
    await expect(page.getByRole("button", { name: /^Archived/ })).toHaveCount(
      0,
    );
  });
  test("retired devices leave active views and can be restored with history intact", async ({
    page,
    request,
  }) => {
    await seed(request);
    const bootstrap = await (await request.get("/api/v1/bootstrap")).json();
    const candidate = bootstrap.hosts.find(
      (host: { name: string; source: unknown }) => !host.source,
    );
    expect(candidate).toBeTruthy();
    const deviceName = `retirement-${randomUUID().slice(0, 8)}`;
    const previousIdentity = {
      name: candidate.name_override,
      icon: candidate.icon_override,
    };
    try {
      expect(
        (
          await request.put(`/api/v1/hosts/${candidate.id}/identity`, {
            data: { name: deviceName, icon: null },
          })
        ).ok(),
      ).toBeTruthy();
      expect(
        (
          await request.post(`/api/v1/hosts/${candidate.id}/retire`, {
            data: {},
          })
        ).ok(),
      ).toBeTruthy();

      await page.goto("/?view=map");
      await expect(
        page.locator(".map-device").filter({ hasText: deviceName }),
      ).toHaveCount(0);
      await page.goto("/?view=hosts");
      await expect(page.locator(".host-table")).not.toContainText(deviceName);
      await expect(
        page.getByRole("button", { name: /^Retired/ }),
      ).toContainText("1");
      await page.getByRole("button", { name: /^Retired/ }).click();
      const row = page.getByRole("row").filter({ hasText: deviceName });
      await expect(row).toBeVisible();
      await row.click();
      await page.getByRole("tab", { name: "Settings" }).click();
      await page.getByRole("button", { name: "Restore device" }).click();
      await page.getByRole("button", { name: "Close host details" }).click();
      await page.getByRole("button", { name: /^All/ }).click();
      await expect(page.locator(".host-table")).toContainText(deviceName);
    } finally {
      await request.post(`/api/v1/hosts/${candidate.id}/restore`, {
        data: {},
      });
      await request.put(`/api/v1/hosts/${candidate.id}/identity`, {
        data: previousIdentity,
      });
    }
  });
  test("duplicate review reconciles selected groups in one bulk request", async ({
    page,
  }) => {
    let reconciled: unknown;
    let groups = [
      {
        suggested_survivor_id: "canonical-device",
        candidates: [
          {
            id: "canonical-device",
            name: "Known server",
            primary_ip: "192.0.2.10",
            device_type: "server",
            platform: "Linux",
            source_count: 2,
            last_seen: Date.now() / 1000,
            sources: ["agent", "scan"],
          },
          {
            id: "duplicate-device",
            name: "Known server copy",
            primary_ip: "192.0.2.11",
            device_type: "server",
            platform: "Linux",
            source_count: 1,
            last_seen: Date.now() / 1000,
            sources: ["scan"],
          },
        ],
        evidence: [
          { label: "Hardware identity", value: "same agent identity" },
        ],
      },
      {
        suggested_survivor_id: "guest-source",
        review_only: true,
        candidates: [
          {
            id: "guest-source",
            name: "dns-container",
            primary_ip: null,
            device_type: "container",
            platform: "Debian",
            source_count: 1,
            last_seen: Date.now() / 1000,
            sources: ["proxmox_lxc"],
          },
          {
            id: "scan-one",
            name: "dns-container scan one",
            primary_ip: "192.168.1.41",
            device_type: "server",
            platform: "Linux",
            source_count: 1,
            last_seen: Date.now() / 1000,
            sources: ["scan"],
          },
          {
            id: "scan-two",
            name: "dns-container scan two",
            primary_ip: "192.168.1.42",
            device_type: "server",
            platform: "Linux",
            source_count: 1,
            last_seen: Date.now() / 1000,
            sources: ["scan"],
          },
        ],
        evidence: [
          {
            label: "Configured NIC MAC",
            value: "appears on two scanned hosts; identity is ambiguous",
          },
        ],
      },
    ];
    await page.route("**/api/v1/devices/duplicate-groups", async (route) =>
      route.fulfill({ json: { groups } }),
    );
    await page.route(
      "**/api/v1/devices/duplicate-groups/reconcile",
      async (route) => {
        reconciled = route.request().postDataJSON();
        groups = [];
        await route.fulfill({
          json: {
            merged: [
              {
                survivor_id: "canonical-device",
                merged_ids: ["duplicate-device"],
              },
            ],
          },
        });
      },
    );
    await page.goto("/?view=hosts");
    const dialog = page.getByRole("dialog", {
      name: "Review duplicate devices",
    });
    await page.getByRole("button", { name: "Review duplicates" }).click();
    await expect(dialog).toContainText("same agent identity");
    await expect(dialog).toContainText("identity is ambiguous");
    await dialog.getByRole("button", { name: "Reconcile 1 groups" }).click();
    await expect
      .poll(() => reconciled)
      .toEqual({
        merges: [
          {
            survivor_id: "canonical-device",
            duplicate_ids: ["duplicate-device"],
          },
        ],
      });
    await expect(dialog).toContainText("No duplicate groups need review.");
  });
  test("an old merged-device URL opens its canonical survivor", async ({
    page,
    request,
  }) => {
    await seed(request);
    const bootstrap = await (await request.get("/api/v1/bootstrap")).json();
    const survivor = bootstrap.hosts.find(
      (host: { name: string }) => host.name === "atlas-nas",
    );
    await page.route("**/api/v1/bootstrap", async (route) => {
      const response = await route.fetch();
      const body = await response.json();
      body.hosts = body.hosts.filter(
        (host: { id: string }) => host.id !== survivor.id,
      );
      await route.fulfill({ response, json: body });
    });
    await page.route("**/api/v1/hosts/legacy-alias", async (route) =>
      route.fulfill({ json: survivor }),
    );
    await page.goto("/?view=host&host=legacy-alias");
    await expect(page).toHaveURL(new RegExp(`host=${survivor.id}`));
    await expect(
      page.getByRole("heading", { name: "atlas-nas" }),
    ).toBeVisible();
  });
  test("live readings recover after a clock correction and expose timestamp skew", async ({
    page,
    request,
  }) => {
    const auth = (await (await request.get("/api/v1/auth/status")).json()) as {
      setup_required: boolean;
    };
    if (auth.setup_required) await seed(request);
    else {
      const session = await request.post("/api/v1/auth/login", {
        data: { password },
      });
      expect(session.ok(), await session.text()).toBeTruthy();
    }
    const token = await (
      await request.post("/api/v1/enrollment-tokens", { data: {} })
    ).json();
    const agent = randomUUID();
    const inventory = {
      hostname: "clock-order-probe",
      os: "Linux",
      architecture: "x86_64",
      cpu_model: "Test CPU",
      cpu_cores: 2,
      memory_total_bytes: 1000,
      interfaces: [],
      disks: [],
      gpus: [],
      collectors: [],
    };
    const enrolled = await request.post("/api/v1/agent/enroll", {
      data: {
        schema_version: 1,
        token: token.token,
        agent_id: agent,
        credential: randomBytes(32).toString("hex"),
        inventory,
      },
    });
    expect(enrolled.ok(), await enrolled.text()).toBeTruthy();
    const credential = (await enrolled.json()).credential;
    const start = Date.now() / 1000;
    const report = async (
      sequence: number,
      collected_at: number,
      cpu_percent: number,
    ) => {
      const response = await request.post("/api/v1/agent/report", {
        headers: { Authorization: `Bearer ${credential}` },
        data: {
          schema_version: 1,
          report_id: randomUUID(),
          agent_id: agent,
          sequence,
          collected_at,
          inventory: null,
          metrics: { cpu_percent, disks: [], gpus: [] },
          collectors: [],
        },
      });
      expect(response.ok(), await response.text()).toBeTruthy();
      return response.json();
    };
    await report(1, start + 240, 10);
    const bootstrap: Bootstrap = await (
      await request.get("/api/v1/bootstrap")
    ).json();
    const host = bootstrap.hosts.find((host) => host.agent_id === agent)!;
    await page.context().addCookies((await request.storageState()).cookies);
    await page.goto(`/?view=host&host=${host.id}`);
    const details = page.getByRole("complementary", { name: "Host details" });
    const cpu = details.getByTestId("chart-CPU");
    await expect(cpu).toContainText("10%");
    await page.getByRole("tab", { name: "Info", exact: true }).click();
    await expect(details).toContainText("ahead of server receipt");
    expect((await report(2, start, 99)).live_applied).toBe(true);
    const historyResponse = await request.get(
      `/api/v1/hosts/${host.id}/metrics?hours=1`,
    );
    expect(historyResponse.ok(), await historyResponse.text()).toBeTruthy();
    const history = (await historyResponse.json()) as {
      metrics: { cpu_percent: number | null };
    }[];
    expect(history.some((point) => point.metrics.cpu_percent === 99)).toBe(
      true,
    );
    await page.getByRole("tab", { name: "Metrics", exact: true }).click();
    await page.reload();
    await expect(cpu).toContainText("99%");
    await expect(details).not.toContainText("ahead of server receipt");
    await expect(details).toContainText("measurements received");
    expect((await report(1, start + 240, 10)).live_applied).toBe(false);
    await expect(cpu).toContainText("99%");
    await page.reload();
    await expect(cpu).toContainText("99%");
  });
  test("topology handles a full inventory, cycles and unknown parents without inventing edges", async ({
    page,
  }) => {
    await page.route("**/api/v1/bootstrap", async (route) => {
      const response = await route.fetch();
      const body: Bootstrap = await response.json();
      const template = body.hosts[0];
      body.hosts = Array.from({ length: 250 }, (_, index) => ({
        ...template,
        id: `layout-${index}`,
        name: `layout-host-${index}`,
        name_override: null,
        agent_id: null,
        agent_revoked: false,
        agent_uninstalled: false,
        retired: false,
        source: null,
        source_device_id: null,
        child: null,
        relationships: [],
        associations: [],
        inventory: null,
        metrics: null,
        addresses: [`10.77.0.${index + 1}`],
        network_ids: [],
        last_seen: Date.now() / 1000,
        last_report: null,
        parent_id:
          index === 0
            ? "layout-1"
            : index === 1
              ? "layout-0"
              : index === 249
                ? "missing-parent"
                : "layout-0",
        parent_reason: "Fixture inferred routing",
      }));
      // A reported relationship wins over a contradictory inferred cycle.
      body.hosts[1].relationships = [
        {
          from_device_id: "layout-1",
          to_device_id: "layout-0",
          kind: "runs_on",
          detail: null,
        },
      ];
      body.map = { revision: body.map.revision, positions: [], links: [] };
      await route.fulfill({ response, json: body });
    });
    await page.goto("/?view=map");
    await expect(page.locator(".map-device")).toHaveCount(250);
    await expectMapFits(page);
    await expect(
      page.locator('.react-flow__edge[data-id="topology-layout-0-layout-1"]'),
    ).toHaveClass(/reported-edge/);
    await expect(
      page.locator('.react-flow__edge[data-id="topology-layout-1-layout-0"]'),
    ).toHaveCount(0);
    await expect(
      page.locator('.react-flow__edge[data-id$="-layout-249"]'),
    ).toHaveCount(0);
    expect(
      await page.locator(".map-device").evaluateAll((nodes) => {
        const boxes = nodes.map((node) => node.getBoundingClientRect());
        return boxes.every((a, i) =>
          boxes
            .slice(i + 1)
            .every(
              (b) =>
                a.right <= b.left + 0.1 ||
                b.right <= a.left + 0.1 ||
                a.bottom <= b.top + 0.1 ||
                b.bottom <= a.top + 0.1,
            ),
        );
      }),
    ).toBeTruthy();
    await page.getByLabel("Search hosts").fill("layout-host-249");
    await expect(page.locator(".map-device")).toHaveCount(1);
    await expect(page.locator(".react-flow__edge")).toHaveCount(0);
    await page.locator(".map-device").click();
    await expect(page).toHaveURL(/view=host&host=layout-249/);
  });

  test("topology health updates from real reports and expires without another report", async ({
    page,
    request,
  }) => {
    const agent = await designAgent(request, "map-health-live");
    await agent.report(98, 3800, 91);
    await page.clock.install();
    await page.goto("/?view=map");
    const node = page
      .locator(".map-device")
      .filter({ hasText: "map-health-live" });
    await expect(node).toHaveAttribute("data-tone", "critical");
    await expect(node).toContainText("CPU at 98%");
    await expect(
      page.getByRole("button", { name: "Health", exact: true }),
    ).toHaveCount(0);
    await expect(node.locator(".node-health")).toHaveCount(0);
    const toggle = page.getByRole("button", {
      name: "Show metrics",
      exact: true,
    });
    await toggle.focus();
    await page.keyboard.press("Space");
    await expect(toggle).toBeFocused();
    await expect(toggle).toHaveAttribute("aria-pressed", "true");
    await expect(node.locator(".node-health")).toContainText("98%");
    const viewportBeforeReport = await page
      .locator(".react-flow__viewport")
      .getAttribute("style");
    await agent.report(10, 3800, 91);
    await expect(node).toHaveAttribute("data-tone", "warning");
    await expect(page.locator(".react-flow__viewport")).toHaveAttribute(
      "style",
      viewportBeforeReport!,
    );
    await expect(node).toContainText("Memory at 95%");
    await agent.report(10, 2000, 40);
    await expect(node).toHaveAttribute("data-tone", "good");
    await expect(node.locator(".node-status")).toHaveText("Reporting");
    await expect(node.locator(".node-health")).toContainText("50%");
    const before = await node.locator("..").getAttribute("style");
    await toggle.click();
    await expect(node.locator("..")).toHaveAttribute("style", before!);
    await toggle.click();
    await page.clock.fastForward(130_000);
    await expect(node).toHaveAttribute("data-tone", "critical");
    await expect(node.locator(".node-status")).toHaveText("Agent stale");
    await expect(node.locator(".node-health strong")).toHaveText([
      "—",
      "—",
      "—",
    ]);
    await page.setViewportSize({ width: 390, height: 844 });
    await page.clock.runFor(1000);
    await page.getByLabel("Search hosts").fill("map-health-live");
    await expect(page.locator(".map-device")).toHaveCount(1);
    await expectMapFits(page);
    await expect(node).toBeInViewport();
    await expect(page.locator(".react-flow__minimap")).toBeHidden();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBeTruthy();
    await page.screenshot({
      path: "test-results/redesign-map-mobile.png",
      fullPage: true,
    });
    await node.click();
    await expect(page).toHaveURL(/view=host/);
  });

  test("overview ranks memory by bytes and excludes retained stale measurements", async ({
    page,
    request,
  }) => {
    const gib = 1024 ** 3;
    const large = await designAgent(request, "memory-large", 4096 * gib);
    const small = await designAgent(request, "memory-small", 256 * gib);
    const stale = await designAgent(request, "memory-stale", 8192 * gib);
    const heartbeat = await designAgent(
      request,
      "memory-heartbeat",
      8192 * gib,
    );
    await heartbeat.report(99, 4096 * gib);
    await large.report(10, 512 * gib);
    await small.report(10, 192 * gib);
    await stale.report(99, 4096 * gib);
    // Prepare the response before interception. A live `route.fetch()` response
    // can be disposed while the browser's bootstrap request is still in flight.
    const bootstrap: Bootstrap = await (
      await request.get("/api/v1/bootstrap")
    ).json();
    const host = bootstrap.hosts.find((h) => h.agent_id === stale.agent)!;
    host.last_report = Date.now() / 1000 - 300;
    host.metrics_received_at = host.last_report;
    const heartbeating = bootstrap.hosts.find(
      (host) => host.agent_id === heartbeat.agent,
    )!;
    heartbeating.metrics_received_at = Date.now() / 1000 - 300;
    await page.route("**/api/v1/bootstrap", (route) =>
      route.fulfill({ json: bootstrap }),
    );
    await page.goto("/");
    await expect(page.getByTestId("network-map")).toHaveCount(0);
    const memory = page.getByRole("region", { name: "Most memory used by" });
    await expect(memory.locator(".leader-row").nth(0)).toContainText(
      "memory-large",
    );
    await expect(memory.locator(".leader-row").nth(1)).toContainText(
      "memory-small",
    );
    await expect(memory).toContainText("512.0 GiB");
    await expect(memory).not.toContainText("memory-stale");
    await expect(memory).not.toContainText("memory-heartbeat");
    await expect(
      page.getByRole("region", { name: "Busiest CPUs" }),
    ).not.toContainText("memory-stale");
    await page.getByRole("button", { name: "Dark", exact: true }).click();
    await expect
      .poll(() =>
        page.evaluate(() => {
          const style = getComputedStyle(
            document.querySelector('.nav-item[aria-current="page"]')!,
          );
          const luminance = (color: string) => {
            const channels = (color.match(/[\d.]+/g) ?? [])
              .slice(0, 3)
              .map(Number)
              .map((v) => {
                const s = v / 255;
                return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
              });
            return (
              channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722
            );
          };
          const text = luminance(style.color),
            background = luminance(style.backgroundColor);
          return (
            (Math.max(text, background) + 0.05) /
            (Math.min(text, background) + 0.05)
          );
        }),
      )
      .toBeGreaterThan(4.5);
    await page.screenshot({
      path: "test-results/redesign-overview-dark.png",
      fullPage: true,
      animations: "disabled",
    });
    await page.setViewportSize({ width: 390, height: 844 });
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBeTruthy();
    await page.screenshot({
      path: "test-results/redesign-overview-mobile.png",
      fullPage: true,
      animations: "disabled",
    });
    await memory.getByRole("button", { name: /memory-large/ }).click();
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toContainText("memory-large");
  });
  test("refined overview groups host problems and uses byte-scaled memory comparisons", async ({
    page,
    request,
  }) => {
    const snapshot = refinementSnapshot(
      await (await request.get("/api/v1/bootstrap")).json(),
    );
    await page.route("**/api/v1/bootstrap", (route) =>
      route.fulfill({ json: snapshot }),
    );
    await page.goto("/");
    await page.getByRole("button", { name: "Dark", exact: true }).click();
    const attention = page.getByRole("region", { name: "Needs attention" });
    await expect(
      attention.locator('.attention-host[data-host-id="refinement-2"]'),
    ).toHaveCount(1);
    await expect(
      attention.locator('.attention-host[data-host-id="refinement-2"]'),
    ).toContainText("2 issues");
    await expect(attention).toContainText("Pool tank is DEGRADED");
    const memory = page.getByRole("region", { name: "Most memory used by" });
    await expect(memory.locator(".leader-value").first()).toHaveText(
      "88.0 GiB",
    );
    const bars = await memory
      .locator(".leader-byte-comparison > span")
      .evaluateAll((nodes) =>
        nodes.map((node) => parseFloat((node as HTMLElement).style.width)),
      );
    expect(bars[0]).toBe(100);
    expect(bars[1]).toBeCloseTo((48 / 88) * 100);
    await expect(memory).toContainText("69% of 128.0 GiB");
    const cpus = page.getByRole("region", { name: "Busiest CPUs" });
    await expect(cpus.getByRole("button", { name: /worker/ })).toContainText(
      "192.168.1.10",
    );
    await page.screenshot({
      path: "test-results/refined-overview-desktop.png",
      fullPage: true,
      animations: "disabled",
    });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.screenshot({
      path: "test-results/refined-overview-mobile.png",
      fullPage: true,
      animations: "disabled",
    });
    const attentionWidth = await attention.evaluate(
      (node) => node.getBoundingClientRect().width,
    );
    const rankingsWidth = await memory.evaluate(
      (node) => node.getBoundingClientRect().width,
    );
    expect(Math.abs(attentionWidth - rankingsWidth)).toBeLessThan(1);
    // Exercise five affected hosts, repeated names, and issue disclosure together.
    const worker = snapshot.hosts[9].metrics!;
    worker.cpu_percent = 99;
    worker.memory_used_bytes = worker.memory_total_bytes! * 0.98;
    worker.temperatures = [
      { sensor: "cpu", label: "CPU package", celsius: 99 },
    ];
    worker.pools = snapshot.hosts[2].metrics!.pools;
    snapshot.hosts[10].metrics!.cpu_percent = 94;
    snapshot.hosts[10].metrics!.memory_used_bytes =
      snapshot.hosts[10].metrics!.memory_total_bytes! * 0.93;
    await page.setViewportSize({ width: 1512, height: 982 });
    await page.reload();
    await expect(attention.locator(".attention-host")).toHaveCount(5);
    const busyWorker = attention.locator('[data-host-id="refinement-9"]');
    await expect(busyWorker).toContainText("192.168.1.10");
    await busyWorker.locator("summary").focus();
    await page.keyboard.press("Enter");
    await expect(busyWorker.locator("details")).toHaveAttribute("open", "");
    await page.screenshot({
      path: "test-results/refined-overview-busy-desktop.png",
      fullPage: true,
      animations: "disabled",
    });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.screenshot({
      path: "test-results/refined-overview-busy-mobile.png",
      fullPage: true,
      animations: "disabled",
    });
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBeTruthy();
    await attention.getByRole("button", { name: /atlas-nas/ }).click();
    await expect(page).toHaveURL(/host=refinement-2/);
  });

  test("refined map defaults to readable cards with compact reachable mobile controls", async ({
    page,
    request,
  }) => {
    const snapshot = refinementSnapshot(
      await (await request.get("/api/v1/bootstrap")).json(),
    );
    await page.route("**/api/v1/bootstrap", (route) =>
      route.fulfill({ json: snapshot }),
    );
    await page.goto("/?view=map");
    await page.getByRole("button", { name: "Dark", exact: true }).click();
    await expect(page.locator(".map-device")).toHaveCount(12);
    await expect
      .poll(() =>
        page
          .locator('.map-device[data-host-id="refinement-2"]')
          .evaluate((node) => node.getBoundingClientRect().width),
      )
      .toBeGreaterThan(168);
    await page.screenshot({
      path: "test-results/refined-map-desktop.png",
      fullPage: true,
      animations: "disabled",
    });
    await page.setViewportSize({ width: 768, height: 1024 });
    await expect(
      page.getByRole("combobox", { name: "Filter by parent" }),
    ).toBeHidden();
    await expect(page.locator(".react-flow__minimap")).toBeHidden();
    await page.getByRole("button", { name: /^Filters/ }).click();
    await expect(
      page.getByRole("combobox", { name: "Filter by parent" }),
    ).toBeVisible();
    await page.getByRole("button", { name: /^Filters/ }).click();
    await page.screenshot({
      path: "test-results/refined-map-tablet.png",
      fullPage: true,
      animations: "disabled",
    });
    await page.setViewportSize({ width: 390, height: 844 });
    await expect
      .poll(() =>
        page
          .locator(".react-flow")
          .evaluate((node) => node.getBoundingClientRect().top),
      )
      .toBeLessThan(430);
    const focal = page.locator('.map-device[data-host-id="refinement-2"]');
    await expect
      .poll(() => focal.evaluate((node) => node.getBoundingClientRect().width))
      .toBeGreaterThan(168);
    await expect
      .poll(() =>
        focal.evaluate((node) => {
          const box = node.getBoundingClientRect(),
            canvas = document
              .querySelector(".react-flow")!
              .getBoundingClientRect();
          return (
            box.left >= canvas.left &&
            box.right <= canvas.right &&
            box.top >= canvas.top &&
            box.bottom <= canvas.bottom
          );
        }),
      )
      .toBeTruthy();
    await page.screenshot({
      path: "test-results/refined-map-mobile-dark.png",
      fullPage: true,
      animations: "disabled",
    });
    await expectMapFits(page);
    await page
      .getByRole("button", { name: "Readable view", exact: true })
      .click();
    await expect
      .poll(() => focal.evaluate((node) => node.getBoundingClientRect().width))
      .toBeGreaterThan(168);
    await page.getByLabel("Search hosts").fill("dns-01");
    await expect(page.locator(".map-device")).toHaveCount(1);
    await expect(page.locator(".map-device")).toBeInViewport();
    await page
      .getByRole("button", { name: "Clear search", exact: true })
      .click();
    await page.getByRole("button", { name: /^Filters/ }).click();
    await page.getByRole("combobox", { name: "Filter by parent" }).click();
    await page.getByRole("option", { name: "proxmox-01", exact: true }).click();
    await expect(page.locator(".map-device")).toHaveCount(3);
    await page
      .getByRole("button", { name: "Clear filters", exact: true })
      .click();
    await expect(page.locator(".map-device")).toHaveCount(12);
    await page.getByRole("button", { name: /^Filters/ }).click();
    await globalAction(page, "Settings");
    await expect(page.getByRole("dialog", { name: "Settings" })).toBeVisible();
    await page.keyboard.press("Escape");
    await page
      .getByRole("button", { name: "More actions", exact: true })
      .click();
    await page.getByRole("button", { name: "Light", exact: true }).click();
    await page.keyboard.press("Escape");
    await page.screenshot({
      path: "test-results/refined-map-mobile-light.png",
      fullPage: true,
      animations: "disabled",
    });
    await navigate(page, "Networks");
    await expect(page).toHaveURL(/view=networks/);
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBeTruthy();
  });
  test("mobile navigation, filters and host inspection work with touch input", async ({
    browser,
    request,
  }) => {
    const snapshot = refinementSnapshot(
      await (await request.get("/api/v1/bootstrap")).json(),
    );
    const context = await browser.newContext({
      storageState: adminState,
      baseURL: "http://127.0.0.1:8087",
      viewport: { width: 390, height: 844 },
      hasTouch: true,
      isMobile: true,
    });
    try {
      const page = await context.newPage();
      await page.route("**/api/v1/bootstrap", (route) =>
        route.fulfill({ json: snapshot }),
      );
      await page.goto("/?view=map");
      await page.getByRole("button", { name: "Navigation", exact: true }).tap();
      await expect(
        page
          .getByRole("navigation", { name: "Main navigation" })
          .getByRole("button"),
      ).toHaveCount(5);
      await page.getByRole("button", { name: "Networks", exact: true }).tap();
      await expect(page).toHaveURL(/view=networks/);
      await page.getByRole("button", { name: "Navigation", exact: true }).tap();
      await page
        .getByRole("button", { name: "Network map", exact: true })
        .tap();
      await page.getByRole("button", { name: /^Filters/ }).tap();
      await expect(
        page.getByRole("combobox", { name: "Filter by network" }),
      ).toBeVisible();
      await page.getByRole("button", { name: /^Filters/ }).tap();
      await page.getByLabel("Search hosts").fill("dns-01");
      await expect(page.locator(".map-device")).toHaveCount(1);
      await page.locator(".map-device").tap();
      await expect(page).toHaveURL(/host=refinement-4/);
    } finally {
      await context.close();
    }
  });
});
