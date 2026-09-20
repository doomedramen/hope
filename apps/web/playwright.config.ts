import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  globalSetup: "./e2e/global-setup.ts",
  outputDir: "./test-results",
  retries: 0,
  timeout: 180_000,
  workers: 1,
  reporter: process.env.CI ? "github" : "list",
  use: {
    baseURL: process.env.E2E_BASE_URL ?? "http://127.0.0.1:8080",
    screenshot: "only-on-failure",
    storageState: "./test-results/.auth/operator.json",
    trace: "retain-on-failure",
  },
});
