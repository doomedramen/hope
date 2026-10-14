import { test, expect } from "@playwright/test";

test("console is served with hardening headers and no CSP violations", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const w = window as unknown as { __csp: string[] };
    w.__csp = [];
    document.addEventListener("securitypolicyviolation", (event) => {
      w.__csp.push(`${event.violatedDirective} ${event.blockedURI}`);
    });
  });
  const response = await page.goto("/");
  const headers = response!.headers();
  expect(headers["x-frame-options"]).toBe("DENY");
  expect(headers["x-content-type-options"]).toBe("nosniff");
  expect(headers["referrer-policy"]).toBe("no-referrer");
  expect(headers["content-security-policy"]).toContain(
    "frame-ancestors 'none'",
  );
  await page.waitForLoadState("networkidle");
  // Hydration requires the inline Next.js bootstrap scripts the policy allows.
  await expect(page.getByRole("textbox").first()).toBeVisible();
  const violations = await page.evaluate(
    () => (window as unknown as { __csp: string[] }).__csp,
  );
  expect(violations).toEqual([]);
  const auth = await page.request.get("/api/v1/auth/status");
  expect(auth.headers()["cache-control"]).toBe("no-store");
});
