import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AgentsPage } from "./AgentsPage";
import type { Agent, AgentDetail } from "@/lib/api";

const agent: Agent = {
  id: "agent-1",
  hostname: "edge-01",
  status: "online",
  agent_version: "0.5.0",
  os: "linux",
  arch: "amd64",
  capabilities: ["filesystem", "socket"],
  last_seen: "2026-09-19T10:00:00Z",
  last_heartbeat_at: "2026-09-19T10:00:00Z",
  inventory_summary: {
    interfaces: 2,
    filesystems: 3,
    processes: 12,
    sockets: 1,
    containers: 1,
  },
  device_id: "device-1",
  protocol_version: 1,
  revoked_at: null,
  created_at: "2026-09-19T09:00:00Z",
};

const metrics = {
  range: "1h" as const,
  from: "2026-09-19T09:00:00Z",
  to: "2026-09-19T10:00:00Z",
  latest: {
    sample_id: "sample-1",
    collected_at: "2026-09-19T10:00:00Z",
    received_at: "2026-09-19T10:00:01Z",
    metrics: {
      memory: { used_percent: 25 },
      network: { interfaces: [{ name: "eth0", rx_bytes_per_sec: 1024 }] },
      disk: { devices: [{ name: "sda", utilization_percent: 10 }] },
      pressure: { io: { some_avg10: 1 } },
      gpu: { devices: [] },
    },
  },
  freshness: { state: "fresh", age_seconds: 1 },
  availability: {
    status: "partial",
    collectors: { cpu: "available", gpu: "unavailable" },
  },
  dimensions: {
    network_interfaces: ["eth0"],
    disk_devices: ["sda"],
    gpu_devices: [],
  },
  series: [
    {
      timestamp: "2026-09-19T10:00:00Z",
      sample_count: 1,
      values: {
        "cpu.usage_percent": {
          average: 42,
          minimum: 42,
          maximum: 42,
          latest: 42,
        },
        "memory.used_percent": {
          average: 25,
          minimum: 25,
          maximum: 25,
          latest: 25,
        },
        "network.interfaces.eth0.rx_bytes_per_sec": {
          average: 1024,
          minimum: 1024,
          maximum: 1024,
          latest: 1024,
        },
        "disk.devices.sda.utilization_percent": {
          average: 10,
          minimum: 10,
          maximum: 10,
          latest: 10,
        },
        "pressure.io.some_avg10": {
          average: 1,
          minimum: 1,
          maximum: 1,
          latest: 1,
        },
      },
    },
  ],
};

const detail: AgentDetail = {
  ...agent,
  host: {
    hostname: "edge-01",
    os: "linux",
    distribution: "debian",
    kernel: "6.1.0",
    arch: "amd64",
    boot_id: "boot-1",
    uptime_seconds: 3600,
    cpu_model: "CPU",
    cpu_count: 4,
    load_1m: 0.2,
    memory_total_bytes: 8_000_000_000,
    memory_used_bytes: 2_000_000_000,
    machine_id_hash: "machine-hash",
  },
  network: {
    interfaces: [],
    routes: [],
  },
  filesystems: [],
  processes: [],
  sockets: [
    {
      protocol: "tcp",
      local_address: "0.0.0.0",
      local_port: 8080,
      state: "listen",
      listening: true,
      process_name: "web",
      process_id: 42,
      reachability: {
        state: "reachable",
        checked_at: "2026-09-19T10:00:00Z",
        worker_id: "worker-1",
        endpoint: { address: "192.0.2.10", port: 8080, protocol: "tcp" },
      },
    },
  ],
  containers: [],
  evidence: [
    {
      id: "evidence-1",
      source: "agent",
      source_instance: "agent-1",
      attribute: "hostname",
      value: "edge-01",
      confidence: 1,
      observed_at: "2026-09-19T10:00:00Z",
      expires_at: null,
      absent: false,
      confirmed: true,
    },
  ],
  reconciliation: {
    status: "matched",
    device_id: "device-1",
    confidence: 1,
    matched_identifiers: ["agent_id"],
    conflicts: [],
    last_reconciled_at: "2026-09-19T10:00:00Z",
    explanation: "Agent identity matches the linked device.",
  },
};

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    headers: { "content-type": "application/json" },
    status,
  });
}

function renderPage() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
    },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <AgentsPage />
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("AgentsPage", () => {
  it("shows inventory and separates local listening from worker reachability", async () => {
    const fetchMock = vi
      .fn()
      .mockImplementation((path: string) =>
        Promise.resolve(
          path === "/api/v1/agents?limit=100"
            ? jsonResponse({ items: [agent], next_cursor: null })
            : path.startsWith("/api/v1/agents/agent-1/metrics")
              ? jsonResponse(metrics)
              : jsonResponse(detail),
        ),
      );
    vi.stubGlobal("fetch", fetchMock);

    renderPage();

    expect((await screen.findAllByText("edge-01")).length).toBeGreaterThan(0);
    fireEvent.click(
      await screen.findByRole("button", { name: "Select edge-01" }),
    );
    const overview = await screen.findByRole("tabpanel", { name: "Overview" });
    expect(
      within(overview).getByRole("region", { name: "Agent at a glance" }),
    ).toBeVisible();
    expect(
      screen
        .getByRole("tablist", { name: "Agent views" })
        .compareDocumentPosition(overview) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: "Settings" }));
    expect(
      screen.queryByRole("region", { name: "Agent at a glance" }),
    ).not.toBeInTheDocument();
    fireEvent.click(
      await screen.findByText("Agent identity and delivery details", {
        selector: "summary",
      }),
    );
    expect(
      await screen.findByText("Internal listener vs worker reachability"),
    ).toBeInTheDocument();
    expect(screen.getByText("Evidence reconciliation")).toBeInTheDocument();
    fireEvent.click(
      await screen.findByText("Sockets", { selector: "summary" }),
    );
    expect(await screen.findByText("Listening")).toBeInTheDocument();
    expect(await screen.findByText("Reachable")).toBeInTheDocument();
  });

  it("renders unknown inventory fields honestly and skips idle virtual chart defaults", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((path: string) =>
        Promise.resolve(
          path === "/api/v1/agents?limit=100"
            ? jsonResponse({ items: [agent], next_cursor: null })
            : path.includes("/metrics")
              ? jsonResponse({
                  ...metrics,
                  dimensions: {
                    ...metrics.dimensions,
                    network_interfaces: ["lo", "eth0"],
                    disk_devices: ["loop0", "sda"],
                  },
                })
              : jsonResponse({
                  ...detail,
                  sockets: [
                    {
                      ...detail.sockets[0],
                      process_id: undefined,
                      process_name: null,
                    },
                  ],
                  filesystems: [
                    {
                      mount_point: "/",
                      device: "/dev/sda",
                      total_bytes: null,
                      used_bytes: null,
                      available_bytes: null,
                      filesystem: null,
                      inode_used: null,
                      inode_total: null,
                      read_only: null,
                    },
                  ],
                }),
        ),
      ),
    );
    renderPage();
    fireEvent.click(
      await screen.findByRole("button", { name: "Select edge-01" }),
    );
    fireEvent.click(await screen.findByRole("tab", { name: "Metrics" }));
    expect(
      await screen.findByRole("combobox", { name: "Network interface" }),
    ).toHaveValue("eth0");
    expect(screen.getByRole("combobox", { name: "Disk device" })).toHaveValue(
      "sda",
    );
    fireEvent.click(screen.getByRole("tab", { name: "Settings" }));
    fireEvent.click(
      await screen.findByText("Agent identity and delivery details", {
        selector: "summary",
      }),
    );
    fireEvent.click(screen.getByText("Sockets", { selector: "summary" }));
    expect(screen.queryByText("pid undefined")).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("Filesystems", { selector: "summary" }));
    const row = screen.getByText("/dev/sda").closest("tr")!;
    expect(within(row).getAllByRole("cell")[3]).toHaveTextContent("— / —");
    expect(within(row).getByText("Unknown")).toBeVisible();
    expect(within(row).queryByText("Writable")).not.toBeInTheDocument();
  });

  it("loads Metrics and renders charts plus unavailable GPU state", async () => {
    const fetchMock = vi
      .fn()
      .mockImplementation((path: string) =>
        Promise.resolve(
          path === "/api/v1/agents?limit=100"
            ? jsonResponse({ items: [agent], next_cursor: null })
            : path.startsWith("/api/v1/agents/agent-1/metrics")
              ? jsonResponse(metrics)
              : jsonResponse(detail),
        ),
      );
    vi.stubGlobal("fetch", fetchMock);

    renderPage();

    fireEvent.click(
      await screen.findByRole("button", { name: "Select edge-01" }),
    );
    fireEvent.click(await screen.findByRole("tab", { name: "Metrics" }));
    expect(await screen.findByText("Resource telemetry")).toBeInTheDocument();
    expect(screen.queryByText("GPU utilization")).not.toBeInTheDocument();
    expect(
      screen.getByText("No supported GPU telemetry reported."),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "6 hours" }));
    expect(
      await screen.findByRole("button", { name: "6 hours" }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/agents/agent-1/metrics?range=6h",
      expect.anything(),
    );
  });

  it("keeps an isolated reading visible and identifies an empty final bucket", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((path: string) =>
        Promise.resolve(
          path === "/api/v1/agents?limit=100"
            ? jsonResponse({ items: [agent], next_cursor: null })
            : path.startsWith("/api/v1/agents/agent-1/metrics")
              ? jsonResponse({
                  ...metrics,
                  series: [
                    metrics.series[0],
                    {
                      timestamp: "2026-09-19T10:01:00Z",
                      sample_count: 0,
                      values: {},
                    },
                  ],
                })
              : jsonResponse(detail),
        ),
      ),
    );
    renderPage();
    fireEvent.click(
      await screen.findByRole("button", { name: "Select edge-01" }),
    );
    fireEvent.click(await screen.findByRole("tab", { name: "Metrics" }));
    const chart = await screen.findByRole("img", { name: "CPU chart" });
    const card = chart.closest('[data-slot="card"]')! as HTMLElement;
    expect(within(card).getByText(/No reading in last bucket/)).toBeVisible();
    expect(within(card).getByText("—")).toBeVisible();
    expect(within(card).getByText(/1 reading/)).toBeVisible();
    expect(chart.querySelector('circle[r="3"]')).toBeInTheDocument();
    expect(
      within(card).getByText(/last populated bucket average 42%/),
    ).toBeInTheDocument();
  });

  it("makes stale and empty metric history explicit", async () => {
    const staleMetrics = {
      ...metrics,
      series: [],
      freshness: { state: "stale", age_seconds: 600 },
    };
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockImplementation((path: string) =>
          Promise.resolve(
            path === "/api/v1/agents?limit=100"
              ? jsonResponse({ items: [agent], next_cursor: null })
              : path.startsWith("/api/v1/agents/agent-1/metrics")
                ? jsonResponse(staleMetrics)
                : jsonResponse(detail),
          ),
        ),
    );

    renderPage();
    fireEvent.click(
      await screen.findByRole("button", { name: "Select edge-01" }),
    );
    fireEvent.click(await screen.findByRole("tab", { name: "Metrics" }));

    expect(await screen.findByText(/Samples are stale/)).toBeInTheDocument();
    expect(screen.getAllByText("No chart data").length).toBeGreaterThan(0);
  });

  it("shows an empty state when no agents are enrolled", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(jsonResponse({ items: [], next_cursor: null })),
    );

    renderPage();

    expect(await screen.findByText("No enrolled agents")).toBeInTheDocument();
    expect(
      screen.queryByRole("textbox", { name: "Search agents" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Enrolled")).not.toBeInTheDocument();
  });

  it("offers a guided enrollment flow from the empty state", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockImplementation((path: string) =>
        Promise.resolve(
          path === "/api/v1/agent-enrollment"
            ? jsonResponse(
                {
                  code: "token.fingerprint",
                  tls_pin: "sha256//test-pin",
                  expires_in_minutes: 15,
                },
                201,
              )
            : jsonResponse({ items: [], next_cursor: null }),
        ),
      ),
    );

    renderPage();

    expect(await screen.findByText("No enrolled agents")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Enroll agent" }));

    expect(
      await screen.findByRole("heading", { name: "Enroll a Linux agent" }),
    ).toBeInTheDocument();
    expect(
      await screen.findByText(
        /curl -fsSL .*agent\/install\.sh.*HOPE_ENROLLMENT_CODE='token\.fingerprint'.*bash/,
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Copy agent install command" }),
    ).toBeInTheDocument();
  });

  it("verifies only the agent linked to the enrollment attempt", async () => {
    const fetchMock = vi.fn().mockImplementation((path: string) =>
      Promise.resolve(
        path === "/api/v1/agent-enrollment"
          ? jsonResponse(
              {
                code: "secret.pin",
                attempt_id: "attempt-42",
                tls_pin: "pin",
                expires_in_minutes: 15,
              },
              201,
            )
          : path === "/api/v1/agent-enrollment/attempt-42"
            ? jsonResponse({
                id: "attempt-42",
                agent_id: "new-agent",
                hostname: "new-host",
                authenticated_at: "2026-09-27",
                inventory_at: "2026-09-27",
                metrics_at: "2026-09-27",
                complete: true,
                expired: false,
                revoked_at: null,
                log_sources: [],
              })
            : jsonResponse({ items: [], next_cursor: null }),
      ),
    );
    vi.stubGlobal("fetch", fetchMock);
    renderPage();
    fireEvent.click(
      await screen.findByRole("button", { name: "Enroll agent" }),
    );
    expect(await screen.findByText("new-host is sending data")).toBeVisible();
    expect(
      screen.getByRole("link", { name: "Open this agent" }),
    ).toHaveAttribute("href", "/agents?agent=new-agent");
    expect(
      screen.queryByRole("button", { name: "Copy agent install command" }),
    ).not.toBeInTheDocument();
    expect(
      fetchMock.mock.calls.some(
        ([path]) => path === "/api/v1/agent-enrollment/attempt-42",
      ),
    ).toBe(true);
  });

  it("shows an actionable error state when the list cannot load", async () => {
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValue(jsonResponse({ error: "gateway unavailable" }, 503)),
    );

    renderPage();

    expect(await screen.findByText("Agents unavailable")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });
});
