import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MonitorList } from "./MonitorList";
import { MonitorProposalQueue } from "./MonitorProposalQueue";
import type {
  Incident,
  Monitor,
  MonitorResult,
  MonitorProposal,
} from "@/lib/api";

const navigateMock = vi.fn();
let monitorSearch: { monitor?: string } = {};

vi.mock("@tanstack/react-router", () => ({
  useNavigate: () => navigateMock,
  useSearch: () => monitorSearch,
}));

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    headers: { "content-type": "application/json" },
    status,
  });
}

function monitor(overrides: Partial<Monitor> = {}): Monitor {
  return {
    id: "monitor-1",
    proposal_id: null,
    service_id: "service-1",
    endpoint_id: "endpoint-1",
    monitor_type: "http",
    config: { path: "/health" },
    interval_seconds: 60,
    timeout_ms: 10_000,
    failure_threshold: 3,
    recovery_threshold: 2,
    enabled: true,
    state: "up",
    underlying_state: "up",
    consecutive_failures: 0,
    consecutive_successes: 4,
    last_result_at: "2026-09-21T11:59:00Z",
    last_success_at: "2026-09-21T11:59:00Z",
    last_failure_at: null,
    next_run_at: "2026-09-21T12:00:00Z",
    lease_owner: null,
    lease_expires_at: null,
    version: 1,
    created_by: null,
    created_at: "2026-09-20T10:00:00Z",
    updated_at: "2026-09-21T11:59:00Z",
    endpoint_address: "192.168.1.20",
    endpoint_port: 8096,
    endpoint_url: "http://192.168.1.20:8096",
    endpoint_dns_name: null,
    service_name: "Jellyfin",
    service_product: "Jellyfin",
    service_product_version: null,
    ...overrides,
  };
}

function result(overrides: Partial<MonitorResult> = {}): MonitorResult {
  return {
    id: "result-1",
    monitor_id: "monitor-1",
    status: "failure",
    observed_at: "2026-09-21T11:59:00Z",
    latency_ms: null,
    error: "Connection timed out",
    details: {},
    created_at: "2026-09-21T11:59:00Z",
    ...overrides,
  };
}

function incident(overrides: Partial<Incident> = {}): Incident {
  return {
    id: "incident-1",
    monitor_id: "monitor-1",
    state: "open",
    severity: "critical",
    opened_at: "2026-09-21T11:58:00Z",
    recovered_at: null,
    last_event_at: "2026-09-21T11:59:00Z",
    failure_count: 3,
    last_result_id: "result-1",
    summary: "Jellyfin is not responding",
    created_at: "2026-09-21T11:58:00Z",
    updated_at: "2026-09-21T11:59:00Z",
    service_id: "service-1",
    endpoint_id: "endpoint-1",
    monitor_type: "http",
    monitor_state: "down",
    endpoint_address: "192.168.1.20",
    endpoint_port: 8096,
    endpoint_url: "http://192.168.1.20:8096",
    endpoint_dns_name: null,
    service_name: "Jellyfin",
    service_product: "Jellyfin",
    ...overrides,
  };
}

function proposal(id: string, target: string): MonitorProposal {
  return {
    id,
    service_id: `service-${id}`,
    endpoint_id: `endpoint-${id}`,
    rule_id: "rule-http",
    rule_version: 1,
    target_identity: target,
    target: { url: `http://${target}/health` },
    protocol: "http",
    product: target,
    product_version: null,
    check_type: "http",
    check_config: { path: "/health" },
    confidence: 0.99,
    auto_create_allowed: false,
    resolved_from: {},
    source_evidence_id: null,
    source_rule_id: "rule-http",
    status: "pending",
    decision_source: "manual",
    decided_by: null,
    decided_at: null,
    user_overrides: {},
    override_by: null,
    override_at: null,
    version: 1,
    created_at: "2026-09-21T10:00:00Z",
    updated_at: "2026-09-21T10:00:00Z",
  };
}

function renderWithClient(children: React.ReactNode) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
  navigateMock.mockReset();
  monitorSearch = {};
});

describe("MonitorList", () => {
  it("selects a monitor through the validated URL search state", async () => {
    const selected = monitor();
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input);
        if (path === "/api/v1/monitors?limit=100") {
          return Promise.resolve(jsonResponse({ items: [selected] }));
        }
        if (path === "/api/v1/monitors/monitor-1/results?limit=100") {
          return Promise.resolve(
            jsonResponse({ items: [result({ status: "success" })] }),
          );
        }
        if (path === "/api/v1/incidents?limit=100") {
          return Promise.resolve(jsonResponse({ items: [] }));
        }
        return Promise.resolve(jsonResponse({ items: [] }));
      }),
    );

    renderWithClient(<MonitorList />);
    fireEvent.click(await screen.findByRole("button", { name: /Jellyfin/ }));

    expect(navigateMock).toHaveBeenCalledTimes(1);
    const navigation = navigateMock.mock.calls[0][0] as {
      search: (previous: { monitor?: string }) => { monitor?: string };
    };
    expect(navigation.search({})).toEqual({ monitor: "monitor-1" });

    monitorSearch = { monitor: "monitor-1" };
    renderWithClient(<MonitorList />);
    expect(
      await screen.findByRole("heading", { name: "Jellyfin" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Latest result")).toBeInTheDocument();
  });

  it("keeps unavailable result data distinct from an empty result history", async () => {
    monitorSearch = { monitor: "monitor-1" };
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input);
        if (path === "/api/v1/monitors?limit=100") {
          return Promise.resolve(jsonResponse({ items: [monitor()] }));
        }
        if (path === "/api/v1/monitors/monitor-1/results?limit=100") {
          return Promise.resolve(jsonResponse({ error: "results down" }, 503));
        }
        if (path === "/api/v1/incidents?limit=100") {
          return Promise.resolve(jsonResponse({ items: [] }));
        }
        return Promise.resolve(jsonResponse({ items: [] }));
      }),
    );

    renderWithClient(<MonitorList />);

    expect(
      await screen.findByText("Latest result unavailable"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("History is unavailable until results can be loaded."),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("No result recorded yet."),
    ).not.toBeInTheDocument();
  });

  it("shows only the selected monitor's contextual incident", async () => {
    monitorSearch = { monitor: "monitor-1" };
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input);
        if (path === "/api/v1/monitors?limit=100") {
          return Promise.resolve(
            jsonResponse({ items: [monitor({ state: "down" })] }),
          );
        }
        if (path === "/api/v1/monitors/monitor-1/results?limit=100") {
          return Promise.resolve(jsonResponse({ items: [result()] }));
        }
        if (path === "/api/v1/incidents?limit=100") {
          return Promise.resolve(
            jsonResponse({
              items: [
                incident(),
                incident({
                  id: "incident-other",
                  monitor_id: "monitor-other",
                  summary: "Other service is not responding",
                }),
              ],
            }),
          );
        }
        return Promise.resolve(jsonResponse({ items: [] }));
      }),
    );

    renderWithClient(<MonitorList />);

    expect(
      await screen.findByText("Jellyfin is not responding"),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("Other service is not responding"),
    ).not.toBeInTheDocument();
  });
});

describe("MonitorProposalQueue", () => {
  it("refreshes monitor data after approving a suggested check", async () => {
    const firstMonitor = monitor();
    const approvedMonitor = monitor({
      id: "monitor-2",
      service_id: "service-2",
      endpoint_id: "endpoint-2",
      service_name: "Plex",
      service_product: "Plex",
    });
    const pendingProposal = proposal("proposal-1", "Plex");
    let monitorReads = 0;
    let proposalReads = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const path = String(input);
        if (path === "/api/v1/monitors?limit=100") {
          monitorReads += 1;
          return Promise.resolve(
            jsonResponse({
              items: [monitorReads === 1 ? firstMonitor : approvedMonitor],
            }),
          );
        }
        if (path === "/api/v1/monitor-proposals?limit=100&status=pending") {
          proposalReads += 1;
          return Promise.resolve(
            jsonResponse({
              items: proposalReads === 1 ? [pendingProposal] : [],
            }),
          );
        }
        if (
          path === "/api/v1/monitor-proposals/proposal-1/approve" &&
          init?.method === "POST"
        ) {
          return Promise.resolve(
            jsonResponse({ ...pendingProposal, status: "approved" }),
          );
        }
        return Promise.resolve(jsonResponse({ items: [] }));
      }),
    );

    renderWithClient(
      <>
        <MonitorList />
        <MonitorProposalQueue />
      </>,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Approve" }));

    await waitFor(() => expect(screen.getByText("Plex")).toBeInTheDocument());
    expect(monitorReads).toBeGreaterThan(1);
    expect(proposalReads).toBeGreaterThan(1);
  });
});
