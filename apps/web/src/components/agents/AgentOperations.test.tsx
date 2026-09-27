import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  cleanup,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { AgentLogs, AgentBulkUpdates, AgentSettings } from "./AgentOperations";
import {
  fetchAgentLogs,
  fetchAgentUpdates,
  updateAgent,
  fetchAgentCollection,
  saveAgentCollection,
  saveAgentUpdatePolicy,
  type AgentUpdatePolicy,
} from "@/lib/api";
vi.mock("@/lib/api", () => ({
  fetchAgentLogs: vi.fn(),
  fetchAgentCollection: vi.fn(),
  saveAgentCollection: vi.fn(),
  saveAgentUpdatePolicy: vi.fn(),
  fetchAgentUsage: vi.fn().mockResolvedValue({}),
  fetchAgentUpdates: vi.fn(),
  updateAgent: vi.fn(),
  getUserFacingError: () => "Unavailable",
}));
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});
const entry = (id: string, message: string) => ({
  id,
  event_id: id,
  source: "journal:app.service",
  severity: "info",
  message,
  observed_at: "2026-09-27T10:00:00Z",
  received_at: "2026-09-27T10:00:01Z",
  attributes: {},
});
it("keeps rows stable while paused and reveals buffered entries on resume", async () => {
  const old = entry("1", "First entry");
  vi.mocked(fetchAgentLogs).mockResolvedValue({
    items: [old],
    next_cursor: null,
  });
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={cache}>
      <AgentLogs agentId="a" location={{}} navigate={vi.fn()} />
    </QueryClientProvider>,
  );
  expect(await screen.findByText("First entry")).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: "Pause live" }));
  const next = entry("2", "<script>Never execute log text</script>");
  vi.mocked(fetchAgentLogs).mockResolvedValue({
    items: [next, old],
    next_cursor: null,
  });
  await act(async () => {
    await cache.invalidateQueries({ queryKey: ["agent-logs"] });
  });
  expect(screen.queryByText(next.message)).not.toBeInTheDocument();
  fireEvent.click(
    await screen.findByRole("button", { name: "Resume live (1+ new)" }),
  );
  expect(await screen.findByText(next.message)).toBeVisible();
  expect(document.querySelector("section script")).toBeNull();
  cache.clear();
});
it("carries filters to search and keeps source identity visible", async () => {
  vi.mocked(fetchAgentLogs).mockResolvedValue({
    items: [entry("1", "Marker")],
    next_cursor: null,
  });
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const navigate = vi.fn();
  render(
    <QueryClientProvider client={cache}>
      <AgentLogs
        agentId="a"
        location={{
          logq: "Marker",
          source: "journal:app.service",
          from: "100",
          to: "200",
        }}
        navigate={navigate}
      />
    </QueryClientProvider>,
  );
  await waitFor(() =>
    expect(fetchAgentLogs).toHaveBeenCalledWith(
      "a",
      expect.objectContaining({
        q: "Marker",
        source: "journal:app.service",
        from: "100",
        to: "200",
      }),
    ),
  );
  fireEvent.click(screen.getByRole("button", { name: "Agent diagnostics" }));
  expect(navigate).toHaveBeenCalledWith({ source: "agent" });
  cache.clear();
});
it("reports each selected agent independently when one update cannot proceed", async () => {
  vi.mocked(fetchAgentUpdates)
    .mockResolvedValueOnce({
      policy: {
        mode: "notify",
        channel: "stable",
        pinned_version: null,
        rollout_percent: 5,
        window_start_utc: 0,
        window_end_utc: 0,
      },
      current_version: "1.0.0",
      target_version: "1.1.0",
      blocked_reason: null,
      operations: [],
    })
    .mockRejectedValueOnce(new Error("unavailable"));
  vi.mocked(updateAgent).mockResolvedValue({ operation_id: "operation" });
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  render(
    <QueryClientProvider client={cache}>
      <AgentBulkUpdates selected={["a", "b"]} clear={vi.fn()} />
    </QueryClientProvider>,
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Update selected agents" }),
  );
  expect(await screen.findByText(/Queued 1.1.0/)).toBeVisible();
  expect(await screen.findByText(/Unavailable/)).toBeVisible();
  expect(updateAgent).toHaveBeenCalledExactlyOnceWith("a", "1.1.0");
  cache.clear();
});

it("saves per-source bounds and removes policies for deselected sources", async () => {
  vi.mocked(fetchAgentUpdates).mockResolvedValue({} as never);
  vi.mocked(fetchAgentCollection).mockResolvedValue({
    supports_source_controls: true,
    applied_revision: 1,
    log_retention_days: 7,
    config: {
      revision: 1,
      journal_units: ["app.service"],
      docker_containers: ["old-container"],
      redact: [],
      source_policies: {
        "docker:old-container": {
          minimum_severity: "debug",
          max_events_per_minute: 10,
          max_bytes_per_minute: 1024,
        },
      },
    },
  });
  vi.mocked(saveAgentCollection).mockResolvedValue({ revision: 2 });
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={cache}>
      <AgentSettings agentId="a" />
    </QueryClientProvider>,
  );
  fireEvent.change(
    await screen.findByLabelText("journal:app.service minimum severity"),
    { target: { value: "warning" } },
  );
  fireEvent.change(
    screen.getByLabelText("journal:app.service records per minute"),
    { target: { value: "50" } },
  );
  fireEvent.change(screen.getByLabelText("Docker containers, one per line"), {
    target: { value: "" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save log sources" }));
  await waitFor(() => expect(saveAgentCollection).toHaveBeenCalled());
  const saved = vi.mocked(saveAgentCollection).mock.calls[0][1];
  expect(saved.config.source_policies).toEqual({
    "journal:app.service": {
      minimum_severity: "warning",
      max_events_per_minute: 50,
      max_bytes_per_minute: 1048576,
    },
  });
  cache.clear();
});

function renderUpdatePolicy() {
  let storedPolicy: AgentUpdatePolicy = {
    mode: "notify",
    channel: "stable",
    pinned_version: null,
    rollout_percent: 5,
    window_start_utc: 0,
    window_end_utc: 0,
  };
  vi.mocked(fetchAgentCollection).mockResolvedValue({} as never);
  vi.mocked(fetchAgentUpdates).mockImplementation(async () => ({
    policy: storedPolicy,
    target_version: null,
    blocked_reason:
      "Bootstrap required: install a signed agent with the updater service",
    operations: [],
  }));
  vi.mocked(saveAgentUpdatePolicy).mockImplementation(async (_id, policy) => {
    storedPolicy = { ...policy };
    return { policy: storedPolicy };
  });
  const cache = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const view = render(
    <QueryClientProvider client={cache}>
      <AgentSettings agentId="a" />
    </QueryClientProvider>,
  );
  return { cache, view };
}

it("saves automatic update policy on click and reloads the saved choice", async () => {
  const { cache, view } = renderUpdatePolicy();
  fireEvent.change(await screen.findByLabelText("Updates"), {
    target: { value: "automatic" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save update policy" }));
  await waitFor(() =>
    expect(saveAgentUpdatePolicy).toHaveBeenCalledExactlyOnceWith("a", {
      mode: "automatic",
      channel: "stable",
      pinned_version: null,
      rollout_percent: 5,
      window_start_utc: 0,
      window_end_utc: 0,
    }),
  );
  expect(await screen.findByText("Update policy saved.")).toBeVisible();
  await waitFor(() => expect(fetchAgentUpdates).toHaveBeenCalledTimes(2));
  view.unmount();
  cache.clear();
  render(
    <QueryClientProvider client={cache}>
      <AgentSettings agentId="a" />
    </QueryClientProvider>,
  );
  expect(await screen.findByLabelText("Updates")).toHaveValue("automatic");
  cache.clear();
});

it("shows policy save failures and allows retrying the selected choice", async () => {
  const { cache } = renderUpdatePolicy();
  vi.mocked(saveAgentUpdatePolicy).mockRejectedValueOnce(
    new Error("Save failed"),
  );
  fireEvent.change(await screen.findByLabelText("Updates"), {
    target: { value: "automatic" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save update policy" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Unavailable");
  expect(screen.queryByText("Update policy saved.")).not.toBeInTheDocument();
  expect(screen.getByLabelText("Updates")).toHaveValue("automatic");
  fireEvent.click(screen.getByRole("button", { name: "Save update policy" }));
  expect(await screen.findByText("Update policy saved.")).toBeVisible();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  expect(saveAgentUpdatePolicy).toHaveBeenCalledTimes(2);
  cache.clear();
});
