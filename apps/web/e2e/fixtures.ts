import { test as base, expect } from "@playwright/test";
import { execFile } from "node:child_process";
import net from "node:net";
import path from "node:path";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

export interface AgentTarget {
  host: string;
  port: number;
  username: string;
  password: string;
  containerName: string;
}

type WorkerFixtures = {
  agentTarget: AgentTarget;
};

export const test = base.extend<WorkerFixtures>({
  agentTarget: [
    // The fixture does not depend on the base test fixtures.
    // eslint-disable-next-line no-empty-pattern
    async ({}, use, testInfo) => {
      const image =
        process.env.E2E_AGENT_TARGET_IMAGE ?? "hope-e2e-agent-target:local";
      const targetDir = path.resolve("e2e/agent-target");
      const containerName = `hope-e2e-agent-${process.pid}-${Date.now()}-${testInfo.workerIndex}`;
      const password = "hope-e2e-root";
      let containerId: string | undefined;

      try {
        if (process.env.E2E_AGENT_TARGET_SKIP_BUILD !== "1") {
          await runDocker(["build", "--tag", image, targetDir]);
        }
        const port = await reserveTcpPort();

        const dockerArgs = [
          "run",
          "--detach",
          "--name",
          containerName,
          "--privileged",
          "--tmpfs",
          "/run",
          "--tmpfs",
          "/run/lock",
          "--volume",
          "/sys/fs/cgroup:/sys/fs/cgroup:rw",
          "--publish",
          `127.0.0.1:${port}:22/tcp`,
          image,
        ];
        // Docker Desktop already provides a reachable IPv4
        // host.docker.internal entry. Adding host-gateway on macOS can add an
        // IPv6-only alias that the minimal target cannot route to. Linux CI
        // needs the explicit mapping because the alias is not built in.
        if (process.platform !== "darwin") {
          dockerArgs.splice(
            5,
            0,
            "--add-host",
            "host.docker.internal:host-gateway",
          );
        }
        const result = await runDocker(dockerArgs);
        containerId = result.stdout.trim();
        await waitForPort("127.0.0.1", port, 30_000);

        // This is Playwright's fixture callback, not a React hook.
        // eslint-disable-next-line react-hooks/rules-of-hooks
        await use({
          host: "127.0.0.1",
          port,
          username: "root",
          password,
          containerName,
        });
      } finally {
        if (
          process.env.E2E_AGENT_KEEP_TARGET !== "1" &&
          (containerId || containerName)
        ) {
          await runDocker(["rm", "--force", containerName], {
            allowFailure: true,
          });
        }
      }
    },
    { scope: "worker" },
  ],
});

export { expect };

async function runDocker(
  args: string[],
  options: { allowFailure?: boolean } = {},
): Promise<{ stdout: string; stderr: string }> {
  try {
    return await execFileAsync("docker", args, {
      cwd: path.resolve("."),
      maxBuffer: 16 * 1024 * 1024,
    });
  } catch (error) {
    if (options.allowFailure) return { stdout: "", stderr: "" };
    const detail = error as {
      stdout?: string;
      stderr?: string;
      message?: string;
    };
    throw new Error(
      `docker ${args.join(" ")} failed: ${detail.stderr || detail.stdout || detail.message || error}`,
    );
  }
}

async function reserveTcpPort(): Promise<number> {
  const server = net.createServer();
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => resolve());
  });
  const address = server.address();
  await new Promise<void>((resolve, reject) => {
    server.close((error) => (error ? reject(error) : resolve()));
  });
  if (!address || typeof address === "string") {
    throw new Error("Could not reserve a localhost TCP port");
  }
  return address.port;
}

async function waitForPort(host: string, port: number, timeoutMs: number) {
  const deadline = Date.now() + timeoutMs;
  let lastError: unknown;
  while (Date.now() < deadline) {
    try {
      await new Promise<void>((resolve, reject) => {
        const socket = net.createConnection({ host, port });
        socket.once("connect", () => {
          socket.destroy();
          resolve();
        });
        socket.once("error", (error) => {
          socket.destroy();
          reject(error);
        });
      });
      return;
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }
  throw new Error(
    `Timed out waiting for ${host}:${port}: ${String(lastError)}`,
  );
}
