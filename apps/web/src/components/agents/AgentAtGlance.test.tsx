import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, cleanup } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { AgentAtGlance } from "./AgentAtGlance";
import {
  fetchAgentCollection,
  fetchAgentUpdates,
  type AgentDetail,
  type AgentMetricsResponse,
} from "@/lib/api";
vi.mock("@/lib/api", () => ({
  fetchAgentCollection: vi.fn(),
  fetchAgentUpdates: vi.fn(),
  updateAgent: vi.fn(),
  getUserFacingError: () => "Unavailable",
}));
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});
beforeEach(() => {
  vi.mocked(fetchAgentCollection).mockResolvedValue({
    config: {
      revision: 1,
      journal_units: ["app.service"],
      docker_containers: [],
      redact: [],
    },
    applied_revision: 1,
    log_retention_days: 7,
  });
  vi.mocked(fetchAgentUpdates).mockResolvedValue({
    policy: {
      mode: "notify",
      channel: "stable",
      pinned_version: null,
      rollout_percent: 5,
      window_start_utc: 0,
      window_end_utc: 0,
    },
    current_version: "1.0.0",
    target_version: "1.0.0",
    blocked_reason: null,
    operations: [],
  });
});
function setup(stale = false, limited = false) {
  const at = new Date(Date.now() - (stale ? 300_000 : 1000));
  const agent = {
    id: "a",
    status: stale ? "offline" : "online",
    agent_version: "1.0.0",
    last_seen: at.toISOString(),
    collector_status: [],
  } as unknown as AgentDetail;
  const metrics = {
    latest: {
      collected_at: at.toISOString(),
      metrics: {
        cpu: { usage_percent: 18 },
        memory: { used_percent: 42 },
        delivery: {
          logs: {
            sources: { "journal:app.service": at.getTime() / 1000 },
            budgets: limited
              ? {
                  "journal:app.service": {
                    dropped_records: 12,
                    last_dropped_at: at.getTime() / 1000,
                  },
                }
              : {},
          },
        },
      },
    },
    freshness: { state: stale ? "stale" : "fresh" },
  } as unknown as AgentMetricsResponse;
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={cache}>
      <AgentAtGlance
        agent={agent}
        metrics={metrics}
        loading={false}
        error={null}
        navigate={vi.fn()}
      />
    </QueryClientProvider>,
  );
  return cache;
}
it("shows current readings and source health without any interaction", async () => {
  const cache = setup();
  expect(
    screen.getByRole("heading", { name: "Data arriving normally" }),
  ).toBeVisible();
  expect(screen.getByText("18.0%")).toBeVisible();
  expect(screen.getByText("42.0%")).toBeVisible();
  expect(await screen.findByText("1/1 collecting")).toBeVisible();
  expect(screen.queryByText("Protocol")).not.toBeInTheDocument();
  cache.clear();
});
it("labels old readings and log state as historical when disconnected", async () => {
  const cache = setup(true);
  expect(
    screen.getByRole("heading", { name: "Agent is not reporting" }),
  ).toBeVisible();
  expect(
    screen.getByRole("button", { name: "Inspect connection events" }),
  ).toBeVisible();
  expect(await screen.findByText("1/1 at last sample")).toBeVisible();
  expect(screen.getByText("CPU · last reading")).toBeVisible();
  expect(screen.queryByText("Data arriving normally")).not.toBeInTheDocument();
  cache.clear();
});

it("puts recent source budget loss and its action in the initial summary", async () => {
  const cache = setup(false, true);
  expect(
    await screen.findByRole("heading", { name: "Log source limit reached" }),
  ).toBeVisible();
  expect(screen.getByRole("button", { name: "Review log gaps" })).toBeVisible();
  cache.clear();
});
