import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DiscoveryScopeSetup, ScanLaunch } from "./DiscoveryScopeSetup";
import type { Network, ScanRun, ScanRunStatus } from "@/lib/api";

const network: Network = {
  id: "network-1",
  site_id: null,
  cidr: "192.168.1.0/24",
  vlan: null,
  gateway: "192.168.1.1",
  scan_policy: {},
  name: "Lab network",
  version: 1,
  created_at: "2026-09-19T10:00:00Z",
  updated_at: "2026-09-19T10:00:00Z",
};

function makeRun(
  status: ScanRunStatus,
  overrides: Partial<ScanRun> = {},
): ScanRun {
  return {
    id: "run-1",
    network_id: "network-1",
    job_id: "job-1",
    kind: "initial_discovery",
    status,
    scope_version: 1,
    targets_planned: 2,
    targets_completed: status === "succeeded" ? 2 : 0,
    ports_planned: 131_070,
    ports_completed: status === "succeeded" ? 131_070 : 0,
    complete: status === "succeeded",
    authoritative: status === "succeeded",
    cancellation_requested: status === "cancelled",
    requested_by: "user-1",
    source: "operator",
    error: status === "failed" ? "scanner stopped" : null,
    started_at: null,
    finished_at: null,
    created_at: "2026-09-19T10:00:00Z",
    updated_at: "2026-09-19T10:00:00Z",
    ...overrides,
  };
}

function jsonResponse(value: unknown): Response {
  return new Response(JSON.stringify(value), {
    headers: { "content-type": "application/json" },
  });
}

function renderScanLaunch(run: ScanRun) {
  const queryClient = new QueryClient({
    defaultOptions: {
      mutations: { retry: false },
      queries: { retry: false },
    },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <ScanLaunch
        error={null}
        network={network}
        onLaunch={() => undefined}
        pending={false}
        run={run}
        targetCount="2"
      />
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("ScanLaunch", () => {
  it("polls an active run until server reports completion", async () => {
    const running = makeRun("running", {
      targets_completed: 1,
      ports_completed: 65_535,
      complete: false,
      authoritative: false,
    });
    const completed = makeRun("succeeded");
    let statusRequests = 0;
    const fetchMock = vi
      .fn()
      .mockImplementation((_input: RequestInfo | URL, init?: RequestInit) => {
        if (init?.method === "POST")
          return Promise.resolve(jsonResponse(completed));
        statusRequests += 1;
        return Promise.resolve(
          jsonResponse(statusRequests === 1 ? running : completed),
        );
      });
    vi.stubGlobal("fetch", fetchMock);

    renderScanLaunch(makeRun("pending"));
    await waitFor(() =>
      expect(screen.getByText("Initial discovery running")).toBeInTheDocument(),
    );

    await waitFor(
      () =>
        expect(
          screen.getByText("Initial discovery completed"),
        ).toBeInTheDocument(),
      { timeout: 5_000 },
    );
    expect(statusRequests).toBe(2);
  });

  it("offers cancellation for active runs and removes it after cancellation", async () => {
    const pending = makeRun("pending");
    const cancelled = makeRun("cancelled", {
      complete: false,
      authoritative: false,
      cancellation_requested: true,
    });
    const fetchMock = vi
      .fn()
      .mockImplementation((_input: RequestInfo | URL, init?: RequestInit) =>
        Promise.resolve(
          jsonResponse(init?.method === "POST" ? cancelled : pending),
        ),
      );
    vi.stubGlobal("fetch", fetchMock);

    renderScanLaunch(pending);
    const cancelButton = await screen.findByRole("button", {
      name: "Cancel scan",
    });
    fireEvent.click(cancelButton);

    await waitFor(() =>
      expect(
        screen.getByText("Initial discovery cancelled"),
      ).toBeInTheDocument(),
    );
    expect(
      screen.queryByRole("button", { name: "Cancel scan" }),
    ).not.toBeInTheDocument();
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/scans/run-1/cancel",
      expect.objectContaining({ method: "POST" }),
    );
  });
});

describe("DiscoveryScopeSetup", () => {
  it("gives an empty inventory a path to add its first network", () => {
    const queryClient = new QueryClient({
      defaultOptions: {
        mutations: { retry: false },
        queries: { retry: false },
      },
    });
    const onAddNetwork = vi.fn();

    render(
      <QueryClientProvider client={queryClient}>
        <DiscoveryScopeSetup
          error={null}
          loading={false}
          networks={[]}
          onAddNetwork={onAddNetwork}
        />
      </QueryClientProvider>,
    );

    const addButtons = screen.getAllByRole("button", { name: "Add network" });
    expect(addButtons).toHaveLength(2);
    fireEvent.click(addButtons[0]!);

    expect(onAddNetwork).toHaveBeenCalledOnce();
  });
});
