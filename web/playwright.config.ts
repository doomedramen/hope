import { defineConfig, devices } from "@playwright/test";
export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  workers: 1,
  timeout: 30000,
  use: {
    baseURL: "http://127.0.0.1:8087",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        viewport: { width: 1512, height: 982 },
      },
    },
  ],
  webServer: {
    command: "node tests/server.mjs",
    url: "http://127.0.0.1:8087/healthz",
    reuseExistingServer: false,
    timeout: 30000,
  },
});
