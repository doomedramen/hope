import { expect, test } from "./fixtures";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

test.describe("agent deployment", () => {
  test.skip(
    process.arch !== "x64",
    "The published agent supports amd64 hosts only.",
  );

  test("serves a verified agent release from the web origin", async ({
    request,
  }) => {
    const response = await request.get("/agent/v1/releases/latest/linux/amd64");
    expect(response.ok()).toBeTruthy();
    expect(await response.text()).toMatch(
      /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?\n$/,
    );
  });

  test("manual command installs and checks in from a Linux target", async ({
    page,
    agentTarget,
  }) => {
    await page.goto("/agents");
    await page.getByRole("button", { name: "Enroll agent" }).click();
    const dialog = page.getByRole("dialog");
    await dialog
      .getByLabel("Agent connection address")
      .fill("http://host.docker.internal:18443");
    const command = await dialog.locator("pre").innerText();
    expect(command).toContain("HOPE_TLS_PIN=");
    try {
      await execFileAsync(
        "docker",
        ["exec", agentTarget.containerName, "bash", "-lc", command],
        { timeout: 120_000 },
      );
    } catch (error) {
      const stderr = String(
        (error as { stderr?: string }).stderr ?? "",
      ).replace(/[0-9a-f]{64}\.[0-9a-f]{64}/g, "[redacted enrollment code]");
      throw new Error(
        `the generated manual agent command failed on the disposable target: ${stderr}`,
      );
    }
    const hostname = (
      await execFileAsync("docker", [
        "exec",
        agentTarget.containerName,
        "hostname",
      ])
    ).stdout.trim();
    await dialog.getByRole("button", { name: "Done" }).click();
    await expect(
      page.locator("tbody tr").filter({ hasText: hostname }),
    ).toContainText("Online", { timeout: 45_000 });
    await page.getByRole("button", { name: `Select ${hostname}` }).click();
    await page.getByRole("tab", { name: "Metrics" }).click();
    await expect(page.getByText("Resource telemetry")).toBeVisible();
    await expect(page.getByRole("img", { name: "CPU chart" })).toBeVisible({
      timeout: 45_000,
    });
  });

  test("installs and enrolls a disposable Linux target from infrastructure", async ({
    page,
    agentTarget,
  }) => {
    const deviceName = `E2E agent target ${Date.now()}`;

    await page.goto("/devices");
    await page.getByRole("button", { name: "Add device" }).first().click();
    const createDialog = page.getByRole("dialog");
    await createDialog.getByLabel("Device name").fill(deviceName);
    await createDialog.getByRole("button", { name: "Create device" }).click();

    await expect(page.getByRole("heading", { name: deviceName })).toBeVisible();
    await page.getByText("More actions", { exact: true }).click();
    await page.getByRole("button", { name: "Deploy agent" }).click();
    const installDialog = page.getByRole("dialog");

    await installDialog
      .getByLabel("SSH host")
      .fill(process.env.E2E_AGENT_SSH_HOST ?? agentTarget.host);
    await installDialog.getByLabel("Port").fill(String(agentTarget.port));
    await installDialog
      .getByLabel("Agent connection address")
      .fill("http://host.docker.internal:18443");
    await installDialog.getByLabel("Username").fill(agentTarget.username);
    await installDialog.getByLabel("Password").fill(agentTarget.password);
    await installDialog.getByRole("button", { name: "Deploy agent" }).click();
    await expect(
      installDialog.getByText("Review the SSH host key"),
    ).toBeVisible({ timeout: 45_000 });

    await installDialog.getByRole("button", { name: "Trust host key" }).click();
    await expect(
      installDialog.getByText(/Agent install succeeded/),
    ).toBeVisible({ timeout: 90_000 });

    await page.goto("/agents");
    await expect(
      page.getByRole("heading", { name: "Agents", exact: true }),
    ).toBeVisible();
    await expect(page.getByText("No enrolled agents")).not.toBeVisible({
      timeout: 45_000,
    });
    await expect(page.locator("tbody")).toContainText("Online", {
      timeout: 45_000,
    });
  });
});
