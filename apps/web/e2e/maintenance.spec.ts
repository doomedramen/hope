import { expect, test } from "./fixtures";

test("operator can create and run a maintenance lifecycle", async ({
  page,
}) => {
  const eventName = `E2E maintenance ${Date.now()}`;

  await page.goto("/maintenance");
  await expect(
    page.getByRole("heading", { name: "Maintenance", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "New event" }).click();

  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Event name").fill(eventName);
  await dialog.getByRole("button", { name: "Add resource" }).click();
  await dialog.getByLabel("Resource key").fill("e2e-disposable-resource");
  await expect(dialog.getByLabel("Resource key")).toHaveValue(
    "e2e-disposable-resource",
  );
  await dialog.getByRole("button", { name: "Create event" }).click();

  await expect(dialog).not.toBeVisible();
  await expect(
    page.locator('[id^="maintenance-event-"]').getByRole("heading", {
      name: eventName,
    }),
  ).toBeVisible();
  await expect(
    page
      .getByRole("region", { name: "Reserved resources" })
      .getByText("e2e-disposable-resource", { exact: true }),
  ).toBeVisible();

  await page.getByRole("button", { name: "Start now" }).click();
  await expect(
    page.getByRole("button", { name: "Mark complete" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Mark complete" }).click();
  await expect(
    page
      .locator('[id^="maintenance-event-"]')
      .getByText("Completed", { exact: true }),
  ).toBeVisible();
});
