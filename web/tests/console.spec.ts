import {
  test as base,
  expect,
  type APIRequestContext,
  type BrowserContext,
} from "@playwright/test";
import { randomUUID, randomBytes } from "node:crypto";
const password = "hope-browser-test-password";
let adminState: Awaited<ReturnType<BrowserContext["storageState"]>> | undefined;
const test = base.extend({
  storageState: async ({ browserName }, provide) => {
    void browserName;
    await provide(adminState);
  },
});

async function expectMapFits(page: import("@playwright/test").Page) {
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

function extended(sample: number) {
  return {
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
    ],
    service_summary: { active: 40, failed: 1, total: 42 },
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

async function seed(request: APIRequestContext) {
  await request.post("/api/v1/networks", {
    data: { name: "Home network", cidr: "192.168.1.0/24", enabled: false },
  });
  await request.post("/api/v1/networks", {
    data: { name: "Workshop", cidr: "192.168.30.0/24", enabled: false },
  });
  const names = ["atlas-nas", "studio-mac", "workshop-pi", "compute-node"];
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
    expect(enrolled.ok()).toBeTruthy();
    const credential = (await enrolled.json()).credential;
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
          ],
        },
      });
      expect(response.ok()).toBeTruthy();
    }
  }
}

test.describe.serial("H.O.P.E console", () => {
  test("protected first-run setup and empty state", async ({ page }) => {
    await page.goto("/");
    await expect(
      page.getByRole("heading", { name: "Meet your network." }),
    ).toBeVisible();
    await page.screenshot({ path: "test-results/hope-setup.png" });
    await page
      .getByLabel("Setup token", { exact: true })
      .fill("hope-playwright-setup-token");
    await page.getByLabel("Administrator password").fill(password);
    await page.getByRole("button", { name: "Create account" }).click();
    await expect(page.getByText("Your network starts here")).toBeVisible();
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
    await page.getByRole("button", { name: "Health", exact: true }).click();
    await expect(page.locator(".node-health").first()).toBeVisible();
    await expect
      .poll(() =>
        page.evaluate(() => {
          const canvas = document
            .querySelector(".react-flow")!
            .getBoundingClientRect();
          return [...document.querySelectorAll(".map-device")].every((node) => {
            const box = node.getBoundingClientRect();
            return (
              box.left >= canvas.left - 2 &&
              box.right <= canvas.right + 2 &&
              box.top >= canvas.top - 2 &&
              box.bottom <= canvas.bottom + 2
            );
          });
        }),
      )
      .toBeTruthy();
    await expect(
      page
        .locator(".map-device")
        .filter({ hasText: "compute-node" })
        .locator(".node-health"),
    ).toContainText("RAM —");
    await page.locator(".map-device").filter({ hasText: "atlas-nas" }).click();
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toContainText("AMD Ryzen");
    await expect(page).toHaveURL(/host=/);
    await page.getByRole("button", { name: "Hosts", exact: true }).click();
    await expect(page.getByRole("table")).toContainText("atlas-nas");
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toBeVisible();
    await page.getByLabel("Search hosts").fill("workshop");
    await expect(page.getByRole("table").getByRole("row")).toHaveCount(2);
    await page.getByLabel("Search hosts").fill("");
    await page.getByRole("button", { name: "Close host details" }).click();
    await page.getByRole("button", { name: "Map", exact: true }).click();
    await page.getByRole("button", { name: "Subnets", exact: true }).click();
    await expect(page.getByTestId("subnet-group").first()).toBeVisible();
    await expect(page.locator(".react-flow__edge")).toHaveCount(0);
    await expectMapFits(page);
    await page.screenshot({
      path: "test-results/hope-subnets.png",
      fullPage: true,
    });
    await page.getByRole("button", { name: "Health", exact: true }).click();
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
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(`/?view=map&host=${data.hosts[0].id}`);
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toBeVisible();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBeTruthy();
    await expectMapFits(page);
    await page.screenshot({
      path: "test-results/hope-mobile.png",
      fullPage: true,
    });
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
      ["health", 20, 446],
    ] as const) {
      expect(
        (
          await request.patch("/api/v1/map", {
            data: {
              operation: "set_position",
              position: { id: `${mode}:${host.id}`, x, y },
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
    const bounds = await node.boundingBox();
    expect(bounds).not.toBeNull();
    await page.mouse.move(bounds!.x + bounds!.width / 2, bounds!.y + 20);
    await page.mouse.down();
    await page.mouse.move(bounds!.x + bounds!.width / 2 + 40, bounds!.y + 50, {
      steps: 5,
    });
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
          (p: { id: string }) => p.id === `topology:${host.id}`,
        )?.x;
        return typeof x === "number" && x !== 276;
      })
      .toBeTruthy();
    const saved = (
      await (await request.get("/api/v1/bootstrap")).json()
    ).map.positions.find((p: { id: string }) => p.id === `topology:${host.id}`);
    await page.getByRole("button", { name: "Health", exact: true }).click();
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
    await page.getByRole("button", { name: "Hosts", exact: true }).click();
    await page.getByRole("button", { name: "atlas-nas", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`host=${host.id}`));
    await page.getByRole("button", { name: "studio-mac", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`host=${other.id}`));
    await page.goBack();
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toContainText("atlas-nas");
  });
  test("expired session returns to login and mobile logout is reachable", async ({
    page,
  }) => {
    await page.goto("/?view=map");
    await expect(page.getByTestId("network-map")).toBeVisible();
    await page.setViewportSize({ width: 390, height: 844 });
    await page
      .getByRole("button", { name: "Sign out", exact: true })
      .filter({ visible: true })
      .click();
    await expect(
      page.getByRole("heading", { name: "Welcome back." }),
    ).toBeVisible();
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
    await expect(
      page.getByRole("heading", { name: "Welcome back." }),
    ).toBeVisible();
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
    for (const mode of ["Topology", "Subnets", "Health"]) {
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
      "Hosts responding",
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
    // Current values live on the charts, not in a separate facts list.
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
    await expect(details.getByTestId("service-units")).toContainText(
      "40 active",
    );
    await expect(details.getByTestId("service-units")).toContainText(
      "nginx.service",
    );
    await expect(details.getByTestId("monitors")).toContainText("timed out");
    await expect(details).not.toContainText("Live usage");
    await expect(details.getByTestId("chart-Filesystem usage")).toContainText(
      "/ · nvme0 · 372.5 GiB of 931.3 GiB",
    );
    await page.getByRole("button", { name: "Open host page" }).click();
    await expect(page).toHaveURL(/view=host&host=/);
    await expect(
      page.getByRole("heading", { name: "Host", exact: true }),
    ).toBeVisible();
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
  test("a range can be removed from settings without losing its hosts", async ({
    page,
    request,
  }) => {
    const before = await (await request.get("/api/v1/bootstrap")).json();
    await page.goto("/");
    await page.getByRole("button", { name: "Networks", exact: true }).click();
    const dialog = page.getByRole("dialog");
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
  }) => {
    await page.goto("/?view=hosts");
    await page
      .getByRole("table")
      .getByText("atlas-nas", { exact: true })
      .click();
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
    await expect(
      page.getByRole("row").filter({ hasText: "atlas-nas" }),
    ).toContainText("Server");
    await page.screenshot({ path: "test-results/hope-device.png" });
    await page.getByRole("combobox", { name: "Device type" }).click();
    await page
      .getByRole("option", { name: "Automatic detection", exact: true })
      .click();
    await expect(
      page.getByText("atlas-nas is back to automatic detection."),
    ).toBeVisible();
    await expect(device).not.toContainText("set by you");
  });
  test("each range chooses how often it is scanned in the background", async ({
    page,
    request,
  }) => {
    await page.goto("/");
    await page.getByRole("button", { name: "Networks", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Networks" });
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
    // History stores one sample per 30 seconds, so these reports seconds apart
    // reach the chart only as live points.
    await row.getByText("live-node", { exact: true }).click();
    const cpu = page
      .getByRole("complementary", { name: "Host details" })
      .getByTestId("chart-CPU");
    await expect(cpu).toContainText("64%");
    await report(81);
    await expect(cpu).toContainText("81%", { timeout: 5000 });
    // Averaged windows show the live points after their last bucket too.
    await page
      .getByRole("group", { name: "History window" })
      .getByText("24h", { exact: true })
      .click();
    await expect(cpu).toContainText("81%");
    await page.getByRole("button", { name: "Close host details" }).click();
    await page.getByRole("button", { name: "Networks", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Networks" });
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
      await page.goto("/");
      await page
        .getByRole("button", { name: "Notifications", exact: true })
        .click();
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
      await page
        .getByRole("button", { name: "Notifications", exact: true })
        .click();
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
  test("an agent older than the server's release offers an update", async ({
    page,
  }) => {
    let atlas = "";
    await page.route("**/api/v1/bootstrap", async (route) => {
      const response = await route.fetch();
      const body = await response.json();
      body.agent_release = "0.1.60";
      for (const host of body.hosts) {
        if (host.name === "atlas-nas") {
          atlas = host.id;
          host.agent_version = "0.1.57";
        } else if (host.agent_id) host.agent_version = "0.1.60";
      }
      await route.fulfill({ response, json: body });
    });
    await page.goto("/?view=hosts");
    await page.getByRole("row").filter({ hasText: "atlas-nas" }).click();
    const details = page.getByRole("complementary", { name: "Host details" });
    await expect(details.getByText("Agent 0.1.57")).toBeVisible();
    await expect(details.getByTestId("agent-update")).toContainText(
      "Agent 0.1.60 is available",
    );
    await details.getByRole("button", { name: "Update agent" }).click();
    await expect(page.getByRole("dialog")).toBeVisible();
    await page.keyboard.press("Escape");
    expect(atlas).not.toBe("");
    await page.getByRole("row").filter({ hasText: "studio-mac" }).click();
    await expect(details.getByText("Agent 0.1.60")).toBeVisible();
    await expect(details.getByTestId("agent-update")).toHaveCount(0);
  });
  test("the map draws routers above the ranges they serve", async ({
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
      page.locator(`.react-flow__edge[data-id^="feeds-${router}-"]`),
    ).toHaveCount(1);
    const node = await page
      .locator(".react-flow__node")
      .filter({ hasText: "atlas-nas" })
      .boundingBox();
    const range = await page
      .getByTestId("subnet-group")
      .filter({ hasText: "Home network" })
      .boundingBox();
    expect(node && range && node.y + node.height <= range.y).toBeTruthy();
    await page.screenshot({
      path: "test-results/hope-hierarchy.png",
      fullPage: true,
    });
    await page.getByRole("button", { name: "Subnets", exact: true }).click();
    await expect(page.locator(".react-flow__edge")).toHaveCount(0);
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
    await page.getByRole("button", { name: "Networks", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Networks" })).toBeVisible();
    await expect(
      page.getByRole("dialog").getByRole("group", { name: "Theme" }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("dialog").getByRole("form", { name: "Change password" }),
    ).toHaveCount(0);
    await page.keyboard.press("Escape");
    await page.getByRole("button", { name: "Account", exact: true }).click();
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
    await expect(
      page.getByRole("heading", { name: "Welcome back." }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: "Networks", exact: true }),
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
  });
});
