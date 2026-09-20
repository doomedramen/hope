import fs from "node:fs/promises";
import path from "node:path";
import { request, type FullConfig } from "@playwright/test";

const DEFAULT_EMAIL = "e2e@example.test";
const DEFAULT_PASSWORD = "e2e-password-123";

export default async function globalSetup(config: FullConfig) {
  const baseURL = config.projects[0]?.use.baseURL;
  if (typeof baseURL !== "string") {
    throw new Error("Playwright E2E_BASE_URL must be configured");
  }

  const email = process.env.E2E_EMAIL ?? DEFAULT_EMAIL;
  const password = process.env.E2E_PASSWORD ?? DEFAULT_PASSWORD;
  const context = await request.newContext({ baseURL });

  try {
    const setupResponse = await context.get("/api/v1/setup");
    if (!setupResponse.ok()) {
      throw new Error(
        `Could not read Hope setup status (${setupResponse.status()})`,
      );
    }
    const setup = (await setupResponse.json()) as { setup_required?: boolean };
    if (setup.setup_required) {
      const createResponse = await context.post("/api/v1/setup", {
        data: { email, password },
        headers: { "x-requested-with": "hope" },
      });
      if (!createResponse.ok()) {
        throw new Error(
          `Could not create the E2E operator (${createResponse.status()})`,
        );
      }
    }

    const loginResponse = await context.post("/api/v1/login", {
      data: { email, password },
      headers: { "x-requested-with": "hope" },
    });
    if (!loginResponse.ok()) {
      throw new Error(
        `Could not authenticate the E2E operator (${loginResponse.status()}); set E2E_EMAIL/E2E_PASSWORD for the existing account`,
      );
    }

    const authPath = path.resolve("test-results/.auth/operator.json");
    await fs.mkdir(path.dirname(authPath), { recursive: true });
    await context.storageState({ path: authPath });
  } finally {
    await context.dispose();
  }
}
