import { spawn } from "node:child_process";
import { rmSync, mkdirSync } from "node:fs";
import { resolve } from "node:path";
const directory = resolve("../target/playwright-data");
rmSync(directory, { recursive: true, force: true });
mkdirSync(directory, { recursive: true });
const server = spawn(resolve("../target/debug/hope-server"), [], {
  env: {
    ...process.env,
    HOPE_BIND: "127.0.0.1:8087",
    HOPE_SERVER_URL: "http://127.0.0.1:8087",
    HOPE_SETUP_TOKEN: "hope-playwright-setup-token",
    HOPE_DATA_DIR: directory,
    HOPE_WEB_DIR: resolve("out"),
    HOPE_DISCOVERY_MODE: "limited",
  },
  stdio: ["ignore", "pipe", "pipe"],
});
server.stdout.on("data", () => {});
server.stderr.on("data", (chunk) => {
  if (!chunk.toString().includes("setup token:")) process.stderr.write(chunk);
});
for (const signal of ["SIGTERM", "SIGINT"])
  process.on(signal, () => server.kill("SIGINT"));
server.on("exit", (code) => process.exit(code ?? 0));
