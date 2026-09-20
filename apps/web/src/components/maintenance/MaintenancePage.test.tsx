import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MaintenancePage } from "./MaintenancePage";
import type { MaintenanceEvent } from "@/lib/api";

const event: MaintenanceEvent = {
  id: "event-1",
  name: "Network core maintenance",
  description: "Replace switch firmware.",
  timezone: "Europe/London",
  start_at: "2026-09-21T22:00:00Z",
  end_at: "2026-09-21T22:30:00Z",
  recurrence_rule: null,
  lead_in_seconds: 300,
  cooldown_seconds: 600,
  disruptive: true,
  notification_policy: {},
  owner: "operator",
  source: "manual",
  notes: null,
  links: [],
  state: "scheduled",
  version: 1,
  created_at: "2026-09-20T10:00:00Z",
  updated_at: "2026-09-20T10:00:00Z",
  resources: [
    {
      role: "affected",
      kind: null,
      id: null,
      key: "network-core",
      expected_failure: true,
    },
  ],
  occurrences: [
    {
      id: "occurrence-1",
      occurrence_key: "2026-09-21T22:00:00Z",
      occurrence_index: 0,
      start_at: "2026-09-21T22:00:00Z",
      end_at: "2026-09-21T22:30:00Z",
      reservation_start: "2026-09-21T21:55:00Z",
      reservation_end: "2026-09-21T22:40:00Z",
      timezone: "Europe/London",
    },
  ],
};

function response(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    headers: { "content-type": "application/json" },
    status,
  });
}

function renderPage() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <MaintenancePage />
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("MaintenancePage", () => {
  it("shows timeline, calendar, list, event details, and create dialog", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((path: string) =>
        Promise.resolve(
          path.includes("/conflicts")
            ? response({ items: [], next_cursor: null })
            : response({ items: [event], next_cursor: null }),
        ),
      ),
    );

    renderPage();

    expect(
      (await screen.findAllByText("Network core maintenance")).length,
    ).toBeGreaterThan(0);
    expect(
      await screen.findByText("No overlapping reservation found."),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("tab", { name: "Calendar" }));
    expect(screen.getByText("Maintenance calendar")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "List" }));
    expect(screen.getByRole("table")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "New event" }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "Create maintenance event" }),
    ).toBeInTheDocument();
  });

  it("shows conflict reasoning for selected event", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((path: string) =>
        Promise.resolve(
          path.includes("/conflicts")
            ? response({
                items: [
                  {
                    event_id: "event-1",
                    occurrence_id: "occurrence-1",
                    conflicting_event_id: "event-2",
                    conflicting_occurrence_id: "occurrence-2",
                    resource_role: "affected",
                    resource_kind: null,
                    resource_id: null,
                    resource_key: "network-core",
                    related_resource_role: "required",
                    related_resource_kind: null,
                    related_resource_id: null,
                    related_resource_key: "network-core",
                    relationship: "resource_overlap",
                    overlap_start: "2026-09-21T22:00:00Z",
                    overlap_end: "2026-09-21T22:10:00Z",
                    reason: "Both events reserve network-core.",
                    suggestion: "Move later event after cooldown.",
                    suggested_move: {
                      event_id: "event-2",
                      occurrence_id: "occurrence-2",
                      start_after: "2026-09-21T22:40:00Z",
                    },
                  },
                ],
                next_cursor: null,
              })
            : response({ items: [event], next_cursor: null }),
        ),
      ),
    );

    renderPage();

    expect(
      await screen.findByText("Both events reserve network-core."),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Move later event after cooldown."),
    ).toBeInTheDocument();
    expect(screen.getByText("1 conflict")).toBeInTheDocument();
  });

  it("shows empty and error states", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(response({ items: [], next_cursor: null }));
    vi.stubGlobal("fetch", fetchMock);
    renderPage();
    expect(
      await screen.findByText("No maintenance events"),
    ).toBeInTheDocument();

    vi.unstubAllGlobals();
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValue(
          response({ error: "maintenance store unavailable" }, 503),
        ),
    );
    renderPage();
    expect(
      await screen.findByText("Maintenance unavailable"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });
});
