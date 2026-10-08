import { test, expect, type APIRequestContext } from "@playwright/test";
import { randomUUID, randomBytes } from "node:crypto";
const password = "hope-browser-test-password";

async function seed(request: APIRequestContext) {
  await request.post("/api/v1/auth/login", { data: { password } });
  await request.post("/api/v1/networks", {
    data: { name: "Home network", cidr: "192.168.1.0/24", enabled: false },
  });
  await request.post("/api/v1/networks", {
    data: { name: "Workshop", cidr: "192.168.30.0/24", enabled: false },
  });
  const names = ["atlas-nas", "studio-mac", "workshop-pi", "compute-node"];
  for (let i = 0; i < names.length; i++) {
    const enrollment = await (
      await request.post("/api/v1/enrollment-tokens")
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
            memory_used_bytes: 8589934592,
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
    await page
      .getByLabel("Setup token", { exact: true })
      .fill("hope-playwright-setup-token");
    await page.getByLabel("Administrator password").fill(password);
    await page.getByRole("button", { name: "Create account" }).click();
    await expect(page.getByText("Your network starts here")).toBeVisible();
    await page.getByRole("button", { name: "Add agent", exact: true }).click();
    await page
      .getByRole("button", { name: "Generate installation command" })
      .click();
    await expect(page.locator(".install-command")).toContainText(
      "HOPE_ENROLLMENT_TOKEN=",
    );
    await page.keyboard.press("Escape");
  });
  test("maps, hosts, selection, history, and manual relationships", async ({
    page,
    request,
  }) => {
    await seed(request);
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.goto("/");
    await page.getByLabel("Administrator password").fill(password);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(page.getByTestId("network-map")).toBeVisible();
    await expect(
      page.locator(".map-device").filter({ hasText: "atlas-nas" }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Health", exact: true }).click();
    await expect(page.locator(".node-health").first()).toBeVisible();
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
    await page.screenshot({
      path: "test-results/hope-subnets.png",
      fullPage: true,
    });
    await page.getByRole("button", { name: "Health", exact: true }).click();
    await page.screenshot({
      path: "test-results/hope-health.png",
      fullPage: true,
    });
    expect(errors).toEqual([]);
  });
  test("mobile layout and reload of selected host", async ({
    page,
    request,
  }) => {
    await request.post("/api/v1/auth/login", { data: { password } });
    const data = await (await request.get("/api/v1/bootstrap")).json();
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(`/?host=${data.hosts[0].id}`);
    await page.getByLabel("Administrator password").fill(password);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(
      page.getByRole("complementary", { name: "Host details" }),
    ).toBeVisible();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBeTruthy();
    await page.screenshot({
      path: "test-results/hope-mobile.png",
      fullPage: true,
    });
  });
});
