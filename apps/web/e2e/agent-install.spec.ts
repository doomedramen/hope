import { expect, test } from "./fixtures";

test.describe("agent deployment", () => {
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
    await page.getByRole("button", { name: "Install agent" }).click();
    const installDialog = page.getByRole("dialog");

    await installDialog.getByLabel("SSH host").fill(agentTarget.host);
    await installDialog.getByLabel("Port").fill(String(agentTarget.port));
    await installDialog
      .getByRole("button", { name: "Create credential" })
      .click();
    await installDialog
      .getByLabel("Credential name")
      .fill("E2E disposable root");
    await installDialog.getByLabel("Username").fill(agentTarget.username);
    await installDialog.getByLabel("Password").fill(agentTarget.password);
    await installDialog
      .getByRole("button", { name: "Save credential" })
      .click();
    await expect(installDialog.getByLabel("SSH credential")).not.toHaveValue(
      "",
    );

    await installDialog.getByRole("button", { name: "Install agent" }).click();
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
