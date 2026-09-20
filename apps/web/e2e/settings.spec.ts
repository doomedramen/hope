import { expect, test } from "./fixtures";

test("operator can create and toggle a notification channel", async ({
  page,
}) => {
  const channelName = `E2E webhook ${Date.now()}`;

  await page.goto("/settings");
  await expect(
    page.getByRole("heading", { name: "Settings", exact: true }),
  ).toBeVisible();
  await page.getByLabel("Name").fill(channelName);
  await page.getByLabel("URL").fill("https://example.test/hope-e2e-webhook");
  await page.getByRole("button", { name: "Save channel" }).click();

  await expect(
    page.getByText(channelName, { exact: true }).first(),
  ).toBeVisible();
  const disable = page.getByRole("button", { name: `Disable ${channelName}` });
  await disable.click();
  await expect(page.getByText("Disabled", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: `Enable ${channelName}` }).click();
  await expect(
    page.getByRole("button", { name: `Disable ${channelName}` }),
  ).toBeVisible();
});
