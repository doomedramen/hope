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
    expect(enrolled.ok(), await enrolled.text()).toBeTruthy();
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
    await page.getByRole("button", { name: "List", exact: true }).click();
    await expect(page.getByRole("table")).toContainText("atlas-nas");
    await expect(
      page.getByRole("row").filter({ hasText: "atlas-nas" }),
    ).toContainText("Reporting");
    await expect(
      page.getByRole("row").filter({ hasText: "atlas-nas" }),
    ).toContainText(/\d+ issues/);
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toBeVisible();
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
      details.getByRole("tab", { name: "Summary", exact: true }),
    ).toHaveAttribute("aria-selected", "true");
    await expect(
      details.getByRole("region", { name: "Current readings" }),
    ).toContainText("20%");
    await expect(details.getByTestId("chart-CPU")).toHaveCount(0);
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
    await page.getByRole("button", { name: "List", exact: true }).click();
    await page.getByRole("button", { name: "atlas-nas", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`host=${host.id}`));
    await page.getByRole("button", { name: "studio-mac", exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`host=${other.id}`));
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
    await page
      .getByRole("button", { name: "Sign out", exact: true })
      .filter({ visible: true })
      .click();
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
    // Current values live on the charts, not in a separate facts list.
    await expect(
      details.getByRole("region", { name: "Current readings" }),
    ).toContainText("20%");
    await expect(details.getByTestId("chart-CPU")).toHaveCount(0);
    await details.getByRole("tab", { name: "Metrics", exact: true }).click();
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
    await expect(details.getByTestId("monitors")).toContainText("timed out");
    await expect(details).not.toContainText("Live usage");
    await details.getByRole("tab", { name: "Metrics", exact: true }).click();
    await expect(details.getByTestId("chart-Filesystem usage")).toContainText(
      "/ · nvme0 · 372.5 GiB of 931.3 GiB",
    );
    await page.getByRole("button", { name: "Open host page" }).click();
    await expect(page).toHaveURL(/view=host&host=/);
    await expect(
      page.getByRole("heading", { name: "Host", exact: true }),
    ).toBeVisible();
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
    await page.getByRole("button", { name: "1 uptime monitored" }).click();
    await expect(
      page.getByRole("button", { name: "1 uptime monitored" }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(page.locator(".host-table tbody tr")).toHaveCount(1);
    await page
      .getByRole("button", { name: "Clear filters", exact: true })
      .click();
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
  }) => {
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
      }
      const readings = page.locator(".reading-meter-card");
      for (const reading of await readings.all()) {
        expect(
          await reading.evaluate(
            (element) => element.scrollWidth <= element.clientWidth,
          ),
        ).toBeTruthy();
      }
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
    // History stores one sample per 30 seconds, so these reports seconds apart
    // reach the chart only as live points.
    await row.getByText("live-node", { exact: true }).click();
    await page.getByRole("tab", { name: "Metrics", exact: true }).click();
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
    await page.getByRole("button", { name: "Settings", exact: true }).click();
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
      await page.goto("/");
      await page
        .getByRole("button", { name: "Notification settings", exact: true })
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
        .getByRole("button", { name: "Notification settings", exact: true })
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
    await expect(card).toContainText("has no updater yet");
    await expect(card.locator("code")).toContainText("/agent/install");
    await expect(card.locator("code")).toContainText("HOPE_ONLY_NEWER=1");
    await expect(card.locator("code")).not.toContainText(
      "HOPE_ENROLLMENT_TOKEN",
    );
    await expect(card.getByRole("button", { name: "Update now" })).toHaveCount(
      0,
    );

    // An agent with an updater is asked with one click, then shows as waiting.
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

    // An agent that is up to date has no card at all.
    await page.getByRole("row").filter({ hasText: "compute-node" }).click();
    await expect(details.getByText("Agent 0.1.60")).toBeVisible();
    await details.getByRole("tab", { name: "Settings", exact: true }).click();
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
  test("device views keep visible filters and remember the chosen layout", async ({
    page,
  }) => {
    await page.goto("/?view=hosts");
    const navigation = page.getByRole("navigation", {
      name: "Main navigation",
    });
    await expect(navigation.getByRole("button")).toHaveCount(4);
    await page.getByRole("button", { name: /^Needs attention/ }).click();
    await page.getByRole("button", { name: "Map", exact: true }).click();
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
    await navigation.getByRole("button", { name: "Devices" }).click();
    await expect(page).toHaveURL(/view=map/);
    await page.reload();
    await navigation.getByRole("button", { name: "Overview" }).click();
    await navigation.getByRole("button", { name: "Devices" }).click();
    await expect(page).toHaveURL(/view=map/);
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
    await expect(
      page.getByRole("button", { name: "Devices", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Settings", exact: true }),
    ).toBeVisible();
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
    await page.getByRole("button", { name: "Settings", exact: true }).click();
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
  test("Agents includes never-reported, stale and revoked records and opens host details", async ({
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
    await expect(
      page.getByRole("article", { name: "quiet-node" }),
    ).toContainText("Never reported");
    await expect(
      page.getByRole("article", { name: "revoked-agent" }),
    ).toContainText("Revoked");
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
    await filters.getByRole("button", { name: /^Revoked/ }).click();
    await expect(page.getByRole("article")).toHaveCount(1);
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
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Settings" })).toBeVisible();
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
  });
});
