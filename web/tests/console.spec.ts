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
    await expect(page.locator(".membership-edge").first()).toBeAttached();
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
    await page.goto(`/?host=${data.hosts[0].id}`);
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
    for (const [mode, x] of [
      ["topology", 125],
      ["health", 450],
    ] as const) {
      expect(
        (
          await request.patch("/api/v1/map", {
            data: {
              operation: "set_position",
              position: { id: `${mode}:${host.id}`, x, y: 200 },
            },
          })
        ).ok(),
      ).toBeTruthy();
    }
    await page.goto("/?view=map");
    const node = page.locator(`[data-id="${host.id}"]`);
    await expect(node).toHaveAttribute("style", /translate\(125px, 200px\)/);
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
        return typeof x === "number" && x !== 125;
      })
      .toBeTruthy();
    const saved = (
      await (await request.get("/api/v1/bootstrap")).json()
    ).map.positions.find((p: { id: string }) => p.id === `topology:${host.id}`);
    await page.getByRole("button", { name: "Health", exact: true }).click();
    await expect(node).toHaveAttribute("style", /translate\(450px, 200px\)/);
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
    await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
    await expect(page.getByTestId("tile-hosts")).toContainText(
      "Hosts responding",
    );
    await expect(page.getByTestId("tile-attention")).toContainText("1");
    const attention = page.getByRole("region", { name: "Needs attention" });
    await expect(attention).toContainText("quiet-node");
    await expect(attention).toContainText("Agent has never reported");
    await expect(
      page.getByRole("region", { name: "Busiest CPUs" }),
    ).toContainText("compute-node");
    await page.screenshot({
      path: "test-results/hope-overview.png",
      fullPage: true,
    });
    await attention.getByRole("button", { name: /quiet-node/ }).click();
    await expect(page).toHaveURL(/view=hosts/);
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toContainText("Intel N100");
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toContainText("Agent has never reported");
    await page.getByRole("button", { name: "Close host details" }).click();
    await page.getByRole("button", { name: /^Needs attention/ }).click();
    await expect(page.getByRole("table").getByRole("row")).toHaveCount(2);
    await expect(page.getByRole("table")).toContainText("quiet-node");
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
  test("command palette jumps to a host and the theme persists", async ({
    page,
  }) => {
    await page.goto("/");
    await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
    await page.keyboard.press("Control+k");
    const palette = page.getByRole("dialog", { name: "Search and commands" });
    await expect(palette).toBeVisible();
    await palette.getByRole("combobox").fill("studio");
    await expect(palette.getByRole("option")).toHaveCount(1);
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(
      /view=hosts&host=|host=.*view=hosts|view=hosts/,
    );
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
    await expect(page.getByRole("heading", { name: "Hosts" })).toBeVisible();
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
    await page.getByRole("button", { name: "Networks & settings" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("button", { name: "Remove Workshop" }).click();
    await dialog.getByRole("button", { name: "Remove range" }).click();
    await expect(dialog).not.toContainText("192.168.30.0/24");
    const after = await (await request.get("/api/v1/bootstrap")).json();
    expect(after.networks.length).toBe(before.networks.length - 1);
    expect(after.hosts.length).toBe(before.hosts.length);
    await expect(page.getByText("Removed Workshop.")).toBeVisible();
  });
});
