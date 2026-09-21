import { expect, test } from "./fixtures";

test("keeps device and network pages focused", async ({ page }) => {
  await page.goto("/devices");
  await expect(
    page.getByRole("heading", { name: "Devices", level: 1 }),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: "Manage networks" })).toHaveCount(
    0,
  );
  await expect(page.getByRole("link", { name: "View networks" })).toHaveCount(
    0,
  );

  await page.goto("/networks");
  await expect(
    page.getByRole("heading", { name: "Networks", level: 1 }),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: "View devices" })).toHaveCount(0);
});

test("operator can add a network before configuring discovery", async ({
  page,
}) => {
  await page.goto("/networks");
  await expect(
    page.getByRole("heading", { name: "Networks", level: 1 }),
  ).toBeVisible();

  await page.getByRole("button", { name: "Add network" }).first().click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("heading", { name: "Add network" }),
  ).toBeVisible();

  const networkName = `E2E lab network ${Date.now()}`;
  const thirdOctet = (Math.floor(Date.now() / 1_000) + 1) % 256;
  const cidr = `10.18.${thirdOctet}.0/24`;
  await dialog.getByLabel("Name").fill(networkName);
  await dialog.getByLabel("CIDR").fill(cidr);
  await dialog.getByLabel("Gateway").fill(`10.18.${thirdOctet}.1`);
  await dialog.getByLabel("VLAN").fill("10");
  await dialog.getByRole("button", { name: "Add network" }).click();

  await expect(
    page.getByRole("button", {
      name: new RegExp(`${networkName}.*${cidr.replaceAll(".", "\\.")}`),
    }),
  ).toBeVisible();
});

test("operator can see and cancel the active discovery job", async ({
  page,
}) => {
  await page.goto("/networks");
  await expect(
    page.getByRole("heading", { name: "Networks", level: 1 }),
  ).toBeVisible();

  const networkName = `E2E scan network ${Date.now()}`;
  const thirdOctet = Math.floor(Date.now() / 1_000) % 256;
  const cidr = `10.19.${thirdOctet}.0/24`;
  await page.getByRole("button", { name: "Add network" }).first().click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Name").fill(networkName);
  await dialog.getByLabel("CIDR").fill(cidr);
  await dialog.getByRole("button", { name: "Add network" }).click();

  const networkButton = page.getByRole("button", {
    name: new RegExp(`${networkName}.*${cidr.replaceAll(".", "\\.")}`),
  });
  await expect(networkButton).toBeVisible();
  await networkButton.click();

  await page.getByRole("button", { name: "Calculate targets" }).click();
  await expect(page.getByText(/254 scan targets/)).toBeVisible();
  await page.getByRole("checkbox", { name: /reviewed the target/i }).check();
  await page
    .getByRole("button", { name: "Confirm and launch initial discovery" })
    .click();
  const reviewDialog = page.getByRole("dialog", {
    name: "Review initial discovery",
  });
  await expect(reviewDialog).toBeVisible();
  await expect(
    reviewDialog.getByText(/65,535 TCP ports per target/),
  ).toBeVisible();
  await reviewDialog
    .getByRole("checkbox", {
      name: /understand this will scan the confirmed target set/i,
    })
    .check();
  await reviewDialog
    .getByRole("button", { name: "Launch initial discovery" })
    .click();

  await expect(
    page.getByText(/Initial discovery (queued|running)/),
  ).toBeVisible({
    timeout: 30_000,
  });
  await expect(page.getByText("Scan job already running")).toBeVisible();
  await expect(
    page
      .locator("#main-content")
      .getByRole("button", { name: "Launch initial discovery" }),
  ).toBeDisabled();
  await expect(page.getByLabel("Scan job details")).toBeVisible();

  await page.getByRole("button", { name: "Cancel scan" }).click();
  await expect(page.getByText("Cancellation requested.").first()).toBeVisible({
    timeout: 30_000,
  });
  await expect(page.getByText(/Activity log/)).toBeVisible();
});
