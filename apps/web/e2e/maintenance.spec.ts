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

test("maintenance event dialog stays usable across viewport sizes", async ({
  page,
}) => {
  const viewports = [
    { width: 1440, height: 900 },
    { width: 1024, height: 768 },
    { width: 390, height: 844 },
  ];

  for (const viewport of viewports) {
    await page.setViewportSize(viewport);
    await page.goto("/maintenance");
    await page.getByRole("button", { name: "New event" }).click();

    const dialog = page.getByRole("dialog", {
      name: "Create maintenance event",
    });
    await expect(dialog).toBeVisible();
    await expect(
      dialog.getByRole("button", { name: "Create event" }),
    ).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeVisible();

    const box = await dialog.boundingBox();
    expect(box).not.toBeNull();
    expect(box?.width).toBeLessThanOrEqual(viewport.width - 32);
    expect(box?.width).toBeGreaterThan(0);

    const hasHorizontalOverflow = await page.evaluate(
      () =>
        document.documentElement.scrollWidth >
        document.documentElement.clientWidth,
    );
    expect(hasHorizontalOverflow).toBe(false);

    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).not.toBeVisible();
  }
});
