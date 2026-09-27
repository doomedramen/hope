import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRouter,
} from "@tanstack/react-router";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { routeTree } from "@/routeTree.gen";
import {
  AddNetworkDialog,
  CreateDeviceDialog,
  EditDeviceDialog,
  InfrastructurePage,
  MergeDialog,
} from "./infrastructure";
import type { Device, DeviceDetail, Monitor, Service } from "@/lib/api";

function detail(id: string, name: string): DeviceDetail {
  return {
    id,
    device_type: "physical_host",
    name,
    status: "active",
    identity_confidence: 1,
    canonical_of: null,
    version: 1,
    created_at: "2026-09-20T10:00:00Z",
    updated_at: "2026-09-20T10:00:00Z",
    interfaces: [],
    evidence: [],
    merged_member_ids: [],
  };
}

function service(id: string, ownerId: string, name = "web"): Service {
  return {
    id,
    name,
    protocol: "https",
    product: "nginx",
    product_version: "1.25",
    owner_kind: "device",
    owner_id: ownerId,
    version: 1,
    created_at: "2026-09-20T10:00:00Z",
    updated_at: "2026-09-20T10:00:00Z",
  };
}

function monitor(
  id: string,
  serviceId: string,
  state: Monitor["state"],
): Monitor {
  return {
    id,
    proposal_id: null,
    service_id: serviceId,
    endpoint_id: `endpoint-${id}`,
    monitor_type: "https",
    config: {},
    interval_seconds: 60,
    timeout_ms: 5000,
    failure_threshold: 3,
    recovery_threshold: 2,
    enabled: true,
    state,
    underlying_state: state === "stale" ? "unknown" : state,
    consecutive_failures: state === "down" ? 3 : 0,
    consecutive_successes: state === "up" ? 2 : 0,
    last_result_at: "2026-09-21T10:00:00Z",
    last_success_at: state === "up" ? "2026-09-21T10:00:00Z" : null,
    last_failure_at: state === "down" ? "2026-09-21T10:00:00Z" : null,
    next_run_at: "2026-09-21T10:01:00Z",
    lease_owner: null,
    lease_expires_at: null,
    version: 1,
    created_by: null,
    created_at: "2026-09-20T10:00:00Z",
    updated_at: "2026-09-21T10:00:00Z",
    endpoint_address: "192.168.1.10",
    endpoint_port: 443,
    endpoint_url: "https://demo.example.test",
    endpoint_dns_name: null,
    service_name: "web",
    service_product: "nginx",
    service_product_version: "1.25",
  };
}

function jsonResponse(value: unknown): Response {
  return new Response(JSON.stringify(value), {
    headers: { "content-type": "application/json" },
  });
}

function renderInfrastructurePage() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <InfrastructurePage />
    </QueryClientProvider>,
  );
}

function renderRouterApp(initialEntries = ["/devices"]) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const router = createRouter({
    history: createMemoryHistory({ initialEntries }),
    routeTree,
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

afterEach(() => {
  vi.unstubAllGlobals();
  window.history.replaceState({}, "", "/");
});

describe("Device primary address", () => {
  it.each([
    { lan: "192.168.1.20/24", expected: "192.168.1.20", name: "vmbr0" },
    { lan: "fd12:3456::20/64", expected: "fd12:3456::20", name: "eth0" },
    { lan: null, expected: "No address reported", name: "eth0" },
  ])(
    "uses $expected consistently in list and detail",
    async ({ lan, expected, name }) => {
      const device = detail(
        "device-1",
        "Proxmox host with a long descriptive name",
      );
      device.interfaces = ["lo", "docker0", name].map((description, index) => ({
        id: `interface-${index}`,
        device_id: device.id,
        description,
        mac: null,
        first_seen: device.created_at,
        last_seen: device.updated_at,
        version: 1,
        created_at: device.created_at,
        updated_at: device.updated_at,
      }));
      const addresses = [
        { interface_id: "interface-0", ip: "::1/128", is_current: true },
        {
          interface_id: "interface-0",
          ip: "0:0:0:0:0:0:0:1/128",
          is_current: true,
        },
        {
          interface_id: "interface-0",
          ip: "::ffff:127.0.0.1",
          is_current: true,
        },
        { interface_id: "interface-0", ip: "::/128", is_current: true },
        { interface_id: "interface-0", ip: "0.0.0.0", is_current: true },
        { interface_id: "interface-0", ip: "127.0.0.1/8", is_current: true },
        {
          interface_id: "interface-2",
          ip: "192.168.1.10/24",
          is_current: false,
        },
        ...(lan
          ? [
              {
                interface_id: "interface-1",
                ip: "172.17.0.1/16",
                is_current: true,
              },
              {
                interface_id: "interface-2",
                ip: "fe80::20/64",
                is_current: true,
              },
              { interface_id: "interface-2", ip: lan, is_current: true },
            ]
          : []),
      ];
      vi.stubGlobal(
        "fetch",
        vi.fn((input: RequestInfo | URL) => {
          const path = String(input);
          if (path.endsWith("/full-scan"))
            return Promise.resolve(jsonResponse(null));
          if (path === "/api/v1/devices/device-1")
            return Promise.resolve(jsonResponse(device));
          const items = path.startsWith("/api/v1/devices?")
            ? [device]
            : path.startsWith("/api/v1/interfaces?")
              ? device.interfaces
              : path.startsWith("/api/v1/addresses?")
                ? addresses
                : [];
          return Promise.resolve(jsonResponse({ items, next_cursor: null }));
        }),
      );
      renderInfrastructurePage();
      await waitFor(() =>
        expect(screen.getAllByText(expected)).toHaveLength(2),
      );
      fireEvent.click(
        screen.getByRole("button", { name: `Open ${device.name}` }),
      );
      const heading = await screen.findByRole("heading", {
        name: device.name!,
      });
      const header = heading.closest('[data-slot="card-header"]');
      await waitFor(() => expect(header).toHaveTextContent(expected));
      expect(header).not.toHaveTextContent("::1");
      expect(header).not.toHaveTextContent("127.0.0.1");
      expect(header).not.toHaveTextContent("172.17.0.1");
    },
  );
});

describe("EditDeviceDialog", () => {
  it("refreshes fields when selected device changes", () => {
    const first = detail("device-1", "QA VM device");
    const second = detail("device-2", "QA test device");
    const { rerender } = render(
      <EditDeviceDialog
        detail={first}
        error={null}
        onOpenChange={vi.fn()}
        open
        pending={false}
        submit={vi.fn()}
      />,
    );

    expect(screen.getByLabelText("Device name")).toHaveValue("QA VM device");

    rerender(
      <EditDeviceDialog
        detail={second}
        error={null}
        onOpenChange={vi.fn()}
        open
        pending={false}
        submit={vi.fn()}
      />,
    );

    expect(screen.getByLabelText("Device name")).toHaveValue("QA test device");
  });
});

describe("InfrastructurePage", () => {
  it("shows Proxmox guest counts and host inventory with honest stale states", async () => {
    const device = detail("pve-host", "Proxmox host");
    device.proxmox = {
      node: "pve-01",
      status: "stale",
      collected_at: "2026-09-20T10:00:00Z",
      vm_count: 1,
      lxc_count: 1,
      guests: [
        {
          id: "vm-101",
          host_device_id: device.id,
          vmid: "101",
          kind: "vm",
          name: "Application VM",
          status: "running",
          last_seen: "2026-09-20T10:00:00Z",
          is_current: true,
          template: false,
        },
        {
          id: "ct-102",
          host_device_id: device.id,
          vmid: "102",
          kind: "lxc",
          name: "DNS container",
          status: "stopped",
          last_seen: "2026-09-20T10:00:00Z",
          is_current: true,
          template: false,
        },
      ],
    };
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input);
        if (path === "/api/v1/devices?limit=100")
          return Promise.resolve(
            jsonResponse({ items: [device], next_cursor: null }),
          );
        if (path === "/api/v1/devices/pve-host")
          return Promise.resolve(jsonResponse(device));
        if (path.endsWith("/full-scan"))
          return Promise.resolve(jsonResponse(null));
        return Promise.resolve(jsonResponse({ items: [], next_cursor: null }));
      }),
    );
    renderInfrastructurePage();
    expect(
      await screen.findAllByText(
        "Proxmox · pve-01 · 1 VM · 1 LXC · last known",
      ),
    ).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "Open Proxmox host" }));
    expect(await screen.findByText("Application VM")).toBeInTheDocument();
    expect(screen.getByText("DNS container")).toBeInTheDocument();
    expect(
      screen.getByRole("region", { name: "Proxmox guests" }),
    ).not.toHaveTextContent("Running");
    expect(
      screen.getByRole("region", { name: "Proxmox guests" }),
    ).toHaveTextContent("current power state is unknown");
  });

  it("shows linked agent hostnames and metric links in the list and detail", async () => {
    const device = detail("device-1", "");
    device.agents = [
      {
        id: "agent-1",
        hostname: "worker-london-01",
        status: "online",
        os: "linux",
        arch: "amd64",
        agent_version: "0.0.74",
        last_seen: new Date().toISOString(),
      },
    ];
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input);
        if (path === "/api/v1/devices?limit=100")
          return Promise.resolve(
            jsonResponse({ items: [device], next_cursor: null }),
          );
        if (path === "/api/v1/devices/device-1")
          return Promise.resolve(jsonResponse(device));
        if (path === "/api/v1/interfaces?limit=200")
          return Promise.resolve(
            jsonResponse({
              items: [
                { id: "eth0", device_id: device.id, description: "eth0" },
                { id: "lo", device_id: device.id, description: "lo" },
                { id: "bridge", device_id: device.id, description: "docker0" },
              ],
              next_cursor: null,
            }),
          );
        if (path === "/api/v1/addresses?limit=200")
          return Promise.resolve(
            jsonResponse({
              items: [
                { interface_id: "lo", ip: "127.0.0.1/8", is_current: true },
                {
                  interface_id: "bridge",
                  ip: "172.17.0.1/16",
                  is_current: true,
                },
                {
                  interface_id: "eth0",
                  ip: "192.168.1.132/24",
                  is_current: true,
                },
                { interface_id: "eth0", ip: "fe80::1234/64", is_current: true },
                {
                  interface_id: "eth0",
                  ip: "192.168.1.120",
                  is_current: false,
                },
              ],
              next_cursor: null,
            }),
          );
        return Promise.resolve(jsonResponse({ items: [], next_cursor: null }));
      }),
    );
    renderInfrastructurePage();
    const links = await screen.findAllByRole("link", {
      name: "Agent metrics for worker-london-01",
    });
    expect(links).toHaveLength(2); // Desktop row and mobile card.
    for (const link of links) {
      expect(link).toHaveAttribute("href", "/agents?agent=agent-1&tab=metrics");
      expect(link.closest("button")).toBeNull();
    }
    expect(screen.getAllByText("Agent online")).toHaveLength(2);
    expect(await screen.findAllByText("192.168.1.132")).toHaveLength(2);
    expect(screen.queryByText(/Record updated/)).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Search devices" }), {
      target: { value: "worker-london" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Open worker-london-01" }),
    );
    await waitFor(() =>
      expect(
        screen.getAllByRole("link", {
          name: "Agent metrics for worker-london-01",
        }),
      ).toHaveLength(2),
    );
  });

  it("shows address failures instead of record timestamps", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input);
        if (path === "/api/v1/devices?limit=100")
          return Promise.resolve(
            jsonResponse({
              items: [detail("host", "Media server")],
              next_cursor: null,
            }),
          );
        if (path === "/api/v1/addresses?limit=200")
          return Promise.resolve(new Response("Unavailable", { status: 503 }));
        return Promise.resolve(jsonResponse({ items: [], next_cursor: null }));
      }),
    );
    renderInfrastructurePage();
    expect(await screen.findAllByText("Address unavailable")).toHaveLength(2);
    expect(screen.queryByText(/Record updated/)).not.toBeInTheDocument();
  });

  it("keeps custom names searchable by agent hostname and shows offline state", async () => {
    const device = detail("device-1", "Media server");
    device.agents = [
      {
        id: "agent-1",
        hostname: "worker-london-01",
        status: "offline",
        os: "linux",
        arch: "amd64",
        agent_version: "0.0.74",
        last_seen: null,
      },
    ];
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) =>
        Promise.resolve(
          jsonResponse(
            String(input) === "/api/v1/devices?limit=100"
              ? { items: [device], next_cursor: null }
              : { items: [], next_cursor: null },
          ),
        ),
      ),
    );
    renderInfrastructurePage();
    await screen.findByRole("button", { name: "Open Media server" });
    fireEvent.change(screen.getByRole("textbox", { name: "Search devices" }), {
      target: { value: "worker-london" },
    });
    expect(
      screen.getByRole("button", { name: "Open Media server" }),
    ).toBeInTheDocument();
    expect(
      screen.getAllByText(/worker-london-01 · Agent offline/),
    ).toHaveLength(2);
    expect(
      screen.getAllByRole("link", {
        name: "Agent metrics for worker-london-01",
      }),
    ).toHaveLength(2);
  });

  it("queues a full scan from one device", async () => {
    const device = detail("device-1", "QA VM device");
    const fetchMock = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const path = String(input);
      if (path === "/api/v1/devices?limit=100") {
        return Promise.resolve(
          jsonResponse({ items: [device], next_cursor: null }),
        );
      }
      if (path === "/api/v1/devices/device-1") {
        return Promise.resolve(jsonResponse(device));
      }
      if (path === "/api/v1/devices/device-1/full-scan") {
        return Promise.resolve(
          jsonResponse(
            init?.method === "POST"
              ? {
                  id: "scan-1",
                  status: "pending",
                  target_address: "192.168.1.10",
                  ports_completed: 0,
                  ports_planned: 65_535,
                }
              : null,
          ),
        );
      }
      return Promise.resolve(jsonResponse({ items: [], next_cursor: null }));
    });
    vi.stubGlobal("fetch", fetchMock);
    renderInfrastructurePage();

    fireEvent.click(
      await screen.findByRole("button", { name: "Open QA VM device" }),
    );
    await screen.findByRole("heading", { name: "QA VM device" });
    fireEvent.click(await screen.findByText("More actions"));
    fireEvent.click(
      await screen.findByRole("button", { name: "Full port scan" }),
    );
    expect(
      screen.getByRole("dialog", { name: "Full port scan for QA VM device" }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Start full scan" }));
    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        "/api/v1/devices/device-1/full-scan",
        expect.objectContaining({ method: "POST" }),
      ),
    );
    expect(
      await screen.findByText(/Full scan of 192\.168\.1\.10: pending/),
    ).toBeInTheDocument();
  });

  it("restores and focuses search when entered from the header search", async () => {
    window.history.replaceState({}, "", "/infrastructure?focus=search&q=QA");
    const first: Device = detail("device-1", "QA VM device");
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        if (String(input) === "/api/v1/devices?limit=100") {
          return Promise.resolve(
            jsonResponse({ items: [first], next_cursor: null }),
          );
        }
        return Promise.resolve(jsonResponse({ items: [] }));
      }),
    );

    renderInfrastructurePage();

    const search = await screen.findByRole("textbox", {
      name: "Search devices",
    });
    expect(search).toHaveValue("QA");
    expect(search).toHaveFocus();
  });

  it("selects the device supplied by an inventory link", async () => {
    window.history.replaceState({}, "", "/infrastructure?device=device-2");
    const first: Device = detail("device-1", "QA VM device");
    const second: Device = detail("device-2", "QA test device");
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input);
        if (path === "/api/v1/devices?limit=100") {
          return Promise.resolve(
            jsonResponse({ items: [first, second], next_cursor: null }),
          );
        }
        if (path === "/api/v1/devices/device-2") {
          return Promise.resolve(jsonResponse(second));
        }
        return Promise.resolve(jsonResponse({ items: [], next_cursor: null }));
      }),
    );

    renderInfrastructurePage();

    expect(
      await screen.findByRole("heading", { name: "QA test device" }),
    ).toBeInTheDocument();
    window.history.replaceState({}, "", "/");
  });

  it("does not select a device until the operator opens one", async () => {
    const first: Device = detail("device-1", "QA VM device");
    const second: Device = detail("device-2", "QA test device");
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const path = String(input);
      if (path === "/api/v1/devices?limit=100") {
        return Promise.resolve(
          jsonResponse({ items: [first, second], next_cursor: null }),
        );
      }
      if (path === "/api/v1/networks?limit=100") {
        return Promise.resolve(jsonResponse({ items: [], next_cursor: null }));
      }
      if (path === "/api/v1/identity-suggestions?limit=100") {
        return Promise.resolve(jsonResponse({ items: [], next_cursor: null }));
      }
      if (path === "/api/v1/addresses?limit=200") {
        return Promise.resolve(jsonResponse({ items: [], next_cursor: null }));
      }
      if (path === "/api/v1/devices/device-1") {
        return Promise.resolve(jsonResponse(first));
      }
      if (path === "/api/v1/devices/device-2") {
        return Promise.resolve(jsonResponse(second));
      }
      return Promise.resolve(jsonResponse({ items: [] }));
    });
    vi.stubGlobal("fetch", fetchMock);

    renderInfrastructurePage();
    expect(screen.queryByRole("heading", { name: "QA VM device" })).toBeNull();

    fireEvent.click(
      await screen.findByRole("button", { name: "Open QA VM device" }),
    );

    fireEvent.change(screen.getByRole("textbox", { name: "Search devices" }), {
      target: { value: "QA test" },
    });

    await waitFor(() => {
      expect(
        screen.queryByRole("heading", { name: "QA VM device" }),
      ).toBeNull();
      expect(screen.getByText("1 of 2 records")).toBeInTheDocument();
    });
  });

  it("distinguishes disabled checks from a healthy device record", async () => {
    const device = detail("device-1", "QA VM device");
    const web = service("service-web", device.id);
    const disabledCheck = {
      ...monitor("monitor-disabled", web.id, "up"),
      enabled: false,
    };
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input);
        if (path === "/api/v1/devices?limit=100") {
          return Promise.resolve(
            jsonResponse({ items: [device], next_cursor: null }),
          );
        }
        if (path === "/api/v1/services?limit=200") {
          return Promise.resolve(
            jsonResponse({ items: [web], next_cursor: null }),
          );
        }
        if (path === "/api/v1/monitors?limit=100") {
          return Promise.resolve(jsonResponse({ items: [disabledCheck] }));
        }
        if (path === "/api/v1/devices/device-1") {
          return Promise.resolve(jsonResponse(device));
        }
        if (path === "/api/v1/devices/device-1/full-scan") {
          return Promise.resolve(jsonResponse(null));
        }
        return Promise.resolve(jsonResponse({ items: [] }));
      }),
    );

    renderInfrastructurePage();
    fireEvent.click(
      await screen.findByRole("button", { name: "Open QA VM device" }),
    );

    expect(
      await screen.findByRole("link", { name: "Disabled" }),
    ).toBeInTheDocument();
    expect(screen.getAllByText("Unknown").length).toBeGreaterThanOrEqual(1);
  });

  it("separates an empty filter from an empty inventory", async () => {
    const device = detail("device-1", "QA VM device");
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        if (String(input) === "/api/v1/devices?limit=100") {
          return Promise.resolve(
            jsonResponse({ items: [device], next_cursor: null }),
          );
        }
        return Promise.resolve(jsonResponse({ items: [] }));
      }),
    );

    renderInfrastructurePage();
    fireEvent.change(
      await screen.findByRole("textbox", { name: "Search devices" }),
      {
        target: { value: "does-not-exist" },
      },
    );

    expect(await screen.findByText("No matching devices")).toBeInTheDocument();
    expect(screen.queryByText("No devices yet")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Clear search" }));
    expect(
      (await screen.findAllByText("QA VM device")).length,
    ).toBeGreaterThanOrEqual(1);
  });

  it("associates every service check by canonical owner ids", async () => {
    const device = detail("device-1", "QA VM device");
    const web = service("service-web", device.id);
    const healthyCheck = monitor("monitor-up", web.id, "up");
    const failingCheck = monitor("monitor-down", web.id, "down");
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input);
        if (path === "/api/v1/devices?limit=100") {
          return Promise.resolve(
            jsonResponse({ items: [device], next_cursor: null }),
          );
        }
        if (path === "/api/v1/services?limit=200") {
          return Promise.resolve(
            jsonResponse({ items: [web], next_cursor: null }),
          );
        }
        if (path === "/api/v1/monitors?limit=100") {
          return Promise.resolve(
            jsonResponse({ items: [healthyCheck, failingCheck] }),
          );
        }
        if (path === "/api/v1/devices/device-1") {
          return Promise.resolve(jsonResponse(device));
        }
        if (path === "/api/v1/devices/device-1/full-scan") {
          return Promise.resolve(jsonResponse(null));
        }
        return Promise.resolve(jsonResponse({ items: [] }));
      }),
    );

    renderInfrastructurePage();
    fireEvent.click(
      await screen.findByRole("button", { name: "Open QA VM device" }),
    );

    expect((await screen.findAllByText("web")).length).toBeGreaterThanOrEqual(
      1,
    );
    expect(screen.getByRole("link", { name: "Up" })).toHaveAttribute(
      "href",
      "/monitoring?monitor=monitor-up",
    );
    expect(screen.getByRole("link", { name: "Down" })).toHaveAttribute(
      "href",
      "/monitoring?monitor=monitor-down",
    );
  });

  it("shows unavailable check data instead of claiming there are no checks", async () => {
    const device = detail("device-1", "QA VM device");
    const web = service("service-web", device.id);
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input);
        if (path === "/api/v1/devices?limit=100") {
          return Promise.resolve(
            jsonResponse({ items: [device], next_cursor: null }),
          );
        }
        if (path === "/api/v1/services?limit=200") {
          return Promise.resolve(
            jsonResponse({ items: [web], next_cursor: null }),
          );
        }
        if (path === "/api/v1/monitors?limit=100") {
          return Promise.reject(new Error("monitor service unavailable"));
        }
        if (path === "/api/v1/devices/device-1") {
          return Promise.resolve(jsonResponse(device));
        }
        if (path === "/api/v1/devices/device-1/full-scan") {
          return Promise.resolve(jsonResponse(null));
        }
        return Promise.resolve(jsonResponse({ items: [] }));
      }),
    );

    renderInfrastructurePage();
    fireEvent.click(
      await screen.findByRole("button", { name: "Open QA VM device" }),
    );

    expect(
      await screen.findByText("Check data unavailable"),
    ).toBeInTheDocument();
    expect(screen.queryByText("No active check")).not.toBeInTheDocument();
  });

  it("uses validated URL search for selection, replace-search, and Back/Forward", async () => {
    const device = detail("device-1", "QA VM device");
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input);
        if (path === "/api/v1/devices?limit=100") {
          return Promise.resolve(
            jsonResponse({ items: [device], next_cursor: null }),
          );
        }
        if (path === "/api/v1/setup") {
          return Promise.resolve(jsonResponse({ setup_required: false }));
        }
        if (path === "/api/v1/devices/device-1") {
          return Promise.resolve(jsonResponse(device));
        }
        if (path === "/api/v1/devices/device-1/full-scan") {
          return Promise.resolve(jsonResponse(null));
        }
        return Promise.resolve(jsonResponse({ items: [] }));
      }),
    );

    vi.stubGlobal("scrollTo", vi.fn());
    const router = renderRouterApp();
    await screen.findByRole("heading", { name: "Devices" });
    const search = screen.getByRole("textbox", { name: "Search devices" });
    fireEvent.change(search, { target: { value: "QA" } });
    await waitFor(() => {
      expect(router.state.location.search.q).toBe("QA");
    });
    expect(router.history.length).toBe(1);

    fireEvent.click(
      await screen.findByRole("button", { name: "Open QA VM device" }),
    );
    await waitFor(() => {
      expect(router.state.location.search).toMatchObject({
        device: "device-1",
        q: "QA",
      });
    });
    expect(
      await screen.findByRole("heading", { name: "QA VM device" }),
    ).toBeInTheDocument();

    router.history.back();
    await waitFor(() => {
      expect(router.state.location.search).toMatchObject({ q: "QA" });
      expect(router.state.location.search.device).toBeUndefined();
    });
    expect(screen.queryByRole("heading", { name: "QA VM device" })).toBeNull();

    router.history.forward();
    await waitFor(() => {
      expect(router.state.location.search).toMatchObject({
        device: "device-1",
        q: "QA",
      });
    });
    expect(
      await screen.findByRole("heading", { name: "QA VM device" }),
    ).toBeInTheDocument();
  });
});

describe("create dialogs", () => {
  it("resets device fields and blocks unnamed creation", () => {
    const { rerender } = render(
      <CreateDeviceDialog
        error={null}
        onOpenChange={vi.fn()}
        open
        pending={false}
        submit={vi.fn()}
      />,
    );
    const name = screen.getByLabelText("Device name");
    fireEvent.change(name, { target: { value: "stale device" } });
    expect(name).toHaveValue("stale device");

    rerender(
      <CreateDeviceDialog
        error={null}
        onOpenChange={vi.fn()}
        open={false}
        pending={false}
        submit={vi.fn()}
      />,
    );
    rerender(
      <CreateDeviceDialog
        error={null}
        onOpenChange={vi.fn()}
        open
        pending={false}
        submit={vi.fn()}
      />,
    );

    expect(screen.getByLabelText("Device name")).toHaveValue("");
    expect(
      screen.getByRole("button", { name: "Create device" }),
    ).toBeDisabled();
  });

  it("resets network fields after reopening", () => {
    const { rerender } = render(
      <AddNetworkDialog
        error={null}
        onOpenChange={vi.fn()}
        open
        pending={false}
        submit={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByLabelText("Name"), {
      target: { value: "stale network" },
    });
    fireEvent.change(screen.getByLabelText("CIDR"), {
      target: { value: "192.168.1.0/24" },
    });

    rerender(
      <AddNetworkDialog
        error={null}
        onOpenChange={vi.fn()}
        open={false}
        pending={false}
        submit={vi.fn()}
      />,
    );
    rerender(
      <AddNetworkDialog
        error={null}
        onOpenChange={vi.fn()}
        open
        pending={false}
        submit={vi.fn()}
      />,
    );

    expect(screen.getByLabelText("Name")).toHaveValue("");
    expect(screen.getByLabelText("CIDR")).toHaveValue("");
  });

  it("loads network fields when editing an existing network", () => {
    render(
      <AddNetworkDialog
        error={null}
        network={{
          cidr: "192.168.1.0/24",
          created_at: "2026-09-20T10:00:00Z",
          gateway: "192.168.1.1",
          id: "network-1",
          name: "Lab network",
          scan_policy: {},
          site_id: null,
          updated_at: "2026-09-20T10:00:00Z",
          version: 3,
          vlan: 20,
        }}
        onOpenChange={vi.fn()}
        open
        pending={false}
        submit={vi.fn()}
      />,
    );

    expect(screen.getByLabelText("Name")).toHaveValue("Lab network");
    expect(screen.getByLabelText("CIDR")).toHaveValue("192.168.1.0/24");
    expect(screen.getByLabelText("VLAN")).toHaveValue(20);
    expect(
      screen.getByRole("button", { name: "Save network" }),
    ).toBeInTheDocument();
  });

  it("validates CIDR, gateway, and VLAN before submitting", () => {
    const submit = vi.fn();
    render(
      <AddNetworkDialog
        error={null}
        onOpenChange={vi.fn()}
        open
        pending={false}
        submit={submit}
      />,
    );

    fireEvent.change(screen.getByLabelText("CIDR"), {
      target: { value: "not-a-cidr" },
    });
    fireEvent.change(screen.getByLabelText("Gateway"), {
      target: { value: "not-an-ip" },
    });
    fireEvent.change(screen.getByLabelText("VLAN"), {
      target: { value: "4096" },
    });
    fireEvent.submit(screen.getByLabelText("CIDR").closest("form")!);

    expect(
      screen.getByText("CIDR must be a valid network range."),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Gateway must be a valid IP address."),
    ).toBeInTheDocument();
    expect(
      screen.getByText("VLAN must be between 1 and 4094."),
    ).toBeInTheDocument();
    expect(submit).not.toHaveBeenCalled();
  });
});

describe("MergeDialog", () => {
  it("shows the selected survivor label in the collapsed control", () => {
    const source = detail("device-source", "Source device");
    const survivor = detail("device-survivor", "QA test device");

    render(
      <MergeDialog
        devices={[source, survivor]}
        error={null}
        onOpenChange={vi.fn()}
        open
        pending={false}
        source={source}
        submit={vi.fn()}
      />,
    );

    expect(
      screen.getByRole("combobox", { name: "Into survivor" }),
    ).toHaveTextContent("QA test device");
    expect(
      screen.getByRole("combobox", { name: "Into survivor" }),
    ).toHaveTextContent("device-s");
  });
});
