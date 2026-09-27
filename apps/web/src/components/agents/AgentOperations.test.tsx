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
import { AgentLogs } from "./AgentOperations";
import { fetchAgentLogs } from "@/lib/api";
vi.mock("@/lib/api", () => ({
  fetchAgentLogs: vi.fn(),
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
