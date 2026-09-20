import { expect, test } from "@playwright/test";

test("operator can add a network before configuring discovery", async ({
  page,
}) => {
  await page.goto("/infrastructure");

  await expect(
    page.getByRole("heading", { name: "Create operator account" }),
  ).toBeVisible();
  await page.getByLabel("Email").fill("e2e@example.test");
  await page.getByLabel("Password").fill("e2e-password-123");
  await page.getByRole("button", { name: "Create account" }).click();

  await expect(
    page.getByRole("heading", { name: "Devices and network records" }),
  ).toBeVisible();
  await expect(page.getByText("No networks configured")).toBeVisible();

  await page.getByRole("button", { name: "Add network" }).first().click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("heading", { name: "Add network" }),
  ).toBeVisible();

  await dialog.getByLabel("Name").fill("Lab network");
  await dialog.getByLabel("CIDR").fill("192.168.1.0/24");
  await dialog.getByLabel("Gateway").fill("192.168.1.1");
  await dialog.getByLabel("VLAN").fill("10");
  await dialog.getByRole("button", { name: "Add network" }).click();

  await expect(
    page.getByRole("button", { name: /Lab network.*192\.168\.1\.0\/24/ }),
  ).toBeVisible();
});
