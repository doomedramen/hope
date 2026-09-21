import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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

function renderScanLaunch(run: ScanRun, onLaunch = () => undefined) {
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
        onLaunch={onLaunch}
        pending={false}
        run={run}
      />
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

beforeEach(() => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue(jsonResponse({ scope: null, scan_run: null })),
  );
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
      expect(screen.getByText("Full TCP scan running")).toBeInTheDocument(),
    );

    await waitFor(
      () =>
        expect(screen.getByText("Full TCP scan completed")).toBeInTheDocument(),
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
      expect(screen.getByText("Full TCP scan cancelled")).toBeInTheDocument(),
    );
    expect(
      screen.queryByRole("button", { name: "Cancel scan" }),
    ).not.toBeInTheDocument();
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/scans/run-1/cancel",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("launches standard scans directly", () => {
    const onLaunch = vi.fn();
    renderScanLaunch(makeRun("succeeded"), onLaunch);

    fireEvent.click(
      screen.getByRole("button", { name: "Scan standard ports" }),
    );
    expect(onLaunch).toHaveBeenCalledOnce();
    expect(
      screen.queryByRole("button", { name: /full TCP scan/i }),
    ).not.toBeInTheDocument();
  });
});

describe("DiscoveryScopeSetup", () => {
  it("restores a confirmed scope and active scan from the server", async () => {
    const queryClient = new QueryClient({
      defaultOptions: {
        mutations: { retry: false },
        queries: { retry: false },
      },
    });
    const persistedScope = {
      network_id: network.id,
      excluded_cidrs: ["192.168.1.10/32"],
      scan_profile: "low_impact",
      target_count: 252,
      confirmed_target_count: 252,
      confirmed_at: "2026-09-19T10:00:00Z",
      enabled: true,
      version: 2,
    };
    const persistedRun = makeRun("running", {
      targets_planned: 252,
      targets_completed: 12,
      ports_planned: 16_514_820,
      ports_completed: 786_420,
      complete: false,
      authoritative: false,
    });
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValue(
          jsonResponse({ scope: persistedScope, scan_run: persistedRun }),
        ),
    );

    render(
      <QueryClientProvider client={queryClient}>
        <DiscoveryScopeSetup
          error={null}
          loading={false}
          networks={[network]}
        />
      </QueryClientProvider>,
    );

    expect(
      await screen.findByText("Full TCP scan running"),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Excluded addresses or ranges")).toHaveValue(
      "192.168.1.10/32",
    );
    const optionsSummary = screen.getByText("Scan options").parentElement;
    expect(optionsSummary).toHaveTextContent("Low impact");
    expect(optionsSummary).toHaveTextContent("1 exclusion");
    expect(screen.getByRole("combobox")).toHaveTextContent("low_impact");
    expect(
      screen.queryByRole("button", { name: "Scan standard ports" }),
    ).toBeDisabled();
  });

  it("exposes edit and delete actions for configured networks", () => {
    const queryClient = new QueryClient({
      defaultOptions: {
        mutations: { retry: false },
        queries: { retry: false },
      },
    });
    const onEditNetwork = vi.fn();
    const onDeleteNetwork = vi.fn();

    render(
      <QueryClientProvider client={queryClient}>
        <DiscoveryScopeSetup
          error={null}
          loading={false}
          networks={[network]}
          onDeleteNetwork={onDeleteNetwork}
          onEditNetwork={onEditNetwork}
        />
      </QueryClientProvider>,
    );

    fireEvent.click(screen.getByRole("button", { name: "Edit Lab network" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete Lab network" }));

    expect(onEditNetwork).toHaveBeenCalledWith(network);
    expect(onDeleteNetwork).toHaveBeenCalledWith(network);
  });

  it("identifies invalid exclusion CIDRs before calculating targets", () => {
    const queryClient = new QueryClient({
      defaultOptions: {
        mutations: { retry: false },
        queries: { retry: false },
      },
    });

    render(
      <QueryClientProvider client={queryClient}>
        <DiscoveryScopeSetup
          error={null}
          loading={false}
          networks={[network]}
        />
      </QueryClientProvider>,
    );

    const exclusions = screen.getByLabelText("Excluded addresses or ranges");
    fireEvent.change(exclusions, { target: { value: "not-a-cidr" } });

    expect(
      screen.getByText("CIDR must be a valid network range."),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Calculate targets" }),
    ).toBeDisabled();
  });

  it("summarizes scan cost and supports one-step confirmation and launch", async () => {
    const queryClient = new QueryClient({
      defaultOptions: {
        mutations: { retry: false },
        queries: { retry: false },
      },
    });
    const scope = {
      network_id: network.id,
      excluded_cidrs: [],
      scan_profile: "normal",
      target_count: 252,
      confirmed_target_count: null,
      confirmed_at: null,
      enabled: true,
    };
    const confirmedScope = {
      ...scope,
      confirmed_target_count: 252,
      confirmed_at: "2026-09-19T10:00:00Z",
    };
    const pendingRun = makeRun("pending", {
      targets_planned: 252,
      ports_planned: 9_828,
    });
    const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      if (path.endsWith("/discovery-state")) {
        return Promise.resolve(jsonResponse({ scope: null, scan_run: null }));
      }
      if (path.endsWith("/discovery-scope/confirm")) {
        return Promise.resolve(jsonResponse(confirmedScope));
      }
      if (path.endsWith("/discovery-scope")) {
        return Promise.resolve(jsonResponse(scope));
      }
      if (path.endsWith("/scans") && init?.method === "POST") {
        return Promise.resolve(jsonResponse(pendingRun));
      }
      if (path.endsWith("/scans/run-1")) {
        return Promise.resolve(jsonResponse(pendingRun));
      }
      return Promise.resolve(jsonResponse({ scope: null, scan_run: null }));
    });
    vi.stubGlobal("fetch", fetchMock);

    render(
      <QueryClientProvider client={queryClient}>
        <DiscoveryScopeSetup
          error={null}
          loading={false}
          networks={[network]}
        />
      </QueryClientProvider>,
    );

    const calculateButton = await screen.findByRole("button", {
      name: "Calculate targets",
    });
    await waitFor(() => expect(calculateButton).not.toBeDisabled());
    fireEvent.submit(calculateButton.closest("form")!);
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
    await waitFor(() =>
      expect(document.body.textContent).toContain("9,828 standard TCP probes"),
    );
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(
      screen.getByRole("button", {
        name: "Confirm and launch initial discovery",
      }),
    );
    expect(await screen.findByText("Standard scan queued")).toBeInTheDocument();
    expect(
      fetchMock.mock.calls.some(
        ([path, init]) =>
          String(path).endsWith("/discovery-scope/confirm") &&
          init?.method === "POST",
      ),
    ).toBe(true);
    expect(
      fetchMock.mock.calls.some(
        ([path, init]) =>
          String(path).endsWith("/scans") && init?.method === "POST",
      ),
    ).toBe(true);
  });

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
