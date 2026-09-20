import { expect, test } from "./fixtures";

test.describe("operator authentication", () => {
  test.use({ storageState: { cookies: [], origins: [] } });

  test("can sign in from a fresh browser context", async ({ page }) => {
    await page.goto("/");

    await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
    await page
      .getByLabel("Email")
      .fill(process.env.E2E_EMAIL ?? "e2e@example.test");
    await page
      .getByLabel("Password")
      .fill(process.env.E2E_PASSWORD ?? "e2e-password-123");
    await page.getByRole("button", { name: "Sign in" }).click();

    await expect(
      page.getByRole("heading", { name: "Infrastructure overview" }),
    ).toBeVisible();
    await expect(
      page.getByRole("navigation", { name: "Primary navigation" }),
    ).toBeVisible();
  });
});
