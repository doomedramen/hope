import { afterEach, describe, expect, it, vi } from "vitest";
import {
  cancelScanRun,
  confirmDiscoveryScope,
  createNetwork,
  deleteNetwork,
  draftDiscoveryScope,
  fetchAgent,
  fetchAgents,
  fetchDiscoveryState,
  fetchScanRun,
  fetchHealthReady,
  fetchMonitors,
  launchNetworkScan,
  patchNetwork,
  ApiError,
} from "./api";

describe("fetchHealthReady", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("fetches enrolled agents with bounded pagination", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          items: [
            {
              id: "agent-1",
              hostname: "edge-01",
              status: "online",
              agent_version: "0.5.0",
              os: "linux",
              arch: "amd64",
              capabilities: ["filesystem", "socket"],
              last_seen: "2026-09-19T10:00:00Z",
              inventory_summary: {
                interfaces: 2,
                filesystems: 3,
                processes: 12,
                sockets: 4,
                containers: 1,
              },
            },
          ],
          next_cursor: null,
        }),
        { headers: { "content-type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const result = await fetchAgents();

    expect(result.items[0]?.hostname).toBe("edge-01");
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/agents?limit=100",
      expect.objectContaining({ credentials: "same-origin" }),
    );
  });

  it("fetches an agent detail record with inventory and reconciliation state", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          id: "agent-1",
          hostname: "edge-01",
          status: "online",
          agent_version: "0.5.0",
          sockets: [
            {
              protocol: "tcp",
              local_address: "0.0.0.0",
              local_port: 8080,
              listening: true,
              reachability: { state: "reachable" },
            },
          ],
          reconciliation: {
            status: "matched",
            confidence: 1,
            matched_identifiers: ["agent_id"],
            conflicts: [],
          },
        }),
        { headers: { "content-type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const result = await fetchAgent("agent-1");

    expect(result.reconciliation.status).toBe("matched");
    expect(result.sockets[0]?.reachability.state).toBe("reachable");
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/agents/agent-1",
      expect.objectContaining({ credentials: "same-origin" }),
    );
  });

  it("parses a ready response", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        json: async () => ({ status: "ready" }),
      }),
    );

    const result = await fetchHealthReady();
    expect(result.status).toBe("ready");
  });

  it("drafts a scope and confirms the server-calculated target count", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            network_id: "network-1",
            excluded_cidrs: ["192.168.1.10/32"],
            scan_profile: "low_impact",
            target_count: 252,
            confirmed_target_count: null,
            confirmed_at: null,
          }),
          { status: 201, headers: { "content-type": "application/json" } },
        ),
      )
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            network_id: "network-1",
            excluded_cidrs: ["192.168.1.10/32"],
            scan_profile: "low_impact",
            target_count: 252,
            confirmed_target_count: 252,
            confirmed_at: "2026-09-19T10:00:00Z",
          }),
          { headers: { "content-type": "application/json" } },
        ),
      );
    vi.stubGlobal("fetch", fetchMock);

    const draft = await draftDiscoveryScope("network-1", {
      excluded_cidrs: ["192.168.1.10/32"],
      scan_profile: "low_impact",
    });
    const confirmed = await confirmDiscoveryScope(
      "network-1",
      draft.target_count,
    );

    expect(draft.target_count).toBe(252);
    expect(confirmed.confirmed_target_count).toBe(252);
    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      "/api/v1/networks/network-1/discovery-scope",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({
          excluded_cidrs: ["192.168.1.10/32"],
          scan_profile: "low_impact",
        }),
      }),
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      "/api/v1/networks/network-1/discovery-scope/confirm",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ target_count: 252 }),
      }),
    );
    const headers = new Headers(fetchMock.mock.calls[1][1]?.headers);
    expect(headers.get("x-requested-with")).toBe("hope");
  });

  it("loads persisted discovery scope and latest scan state", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          scope: {
            network_id: "network-1",
            excluded_cidrs: ["192.168.1.10/32"],
            scan_profile: "low_impact",
            target_count: 252,
            confirmed_target_count: 252,
            confirmed_at: "2026-09-19T10:00:00Z",
            enabled: true,
            version: 2,
          },
          scan_run: {
            id: "run-1",
            network_id: "network-1",
            status: "running",
            targets_planned: 252,
            targets_completed: 12,
            ports_planned: 16_515_420,
            ports_completed: 786_420,
            complete: false,
            authoritative: false,
          },
        }),
        { headers: { "content-type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const result = await fetchDiscoveryState("network-1");

    expect(result.scope?.confirmed_target_count).toBe(252);
    expect(result.scan_run?.status).toBe("running");
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/networks/network-1/discovery-state",
      expect.objectContaining({ credentials: "same-origin" }),
    );
  });

  it("creates a network with the discovery boundary fields", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          id: "network-1",
          cidr: "192.168.1.0/24",
          gateway: "192.168.1.1",
          name: "Lab network",
          vlan: 10,
        }),
        { status: 201, headers: { "content-type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const result = await createNetwork({
      cidr: "192.168.1.0/24",
      gateway: "192.168.1.1",
      name: "Lab network",
      vlan: 10,
    });

    expect(result.cidr).toBe("192.168.1.0/24");
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/networks",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({
          cidr: "192.168.1.0/24",
          gateway: "192.168.1.1",
          name: "Lab network",
          vlan: 10,
        }),
      }),
    );
    const headers = new Headers(fetchMock.mock.calls[0][1]?.headers);
    expect(headers.get("x-requested-with")).toBe("hope");
  });

  it("edits and deletes a network through its lifecycle endpoints", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(
          JSON.stringify({
            id: "network-1",
            cidr: "192.168.2.0/24",
            version: 2,
          }),
          { headers: { "content-type": "application/json" } },
        ),
      )
      .mockResolvedValueOnce(new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetchMock);

    await patchNetwork("network-1", {
      version: 1,
      cidr: "192.168.2.0/24",
      gateway: null,
      name: "Updated LAN",
      vlan: null,
    });
    await deleteNetwork("network-1");

    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      "/api/v1/networks/network-1",
      expect.objectContaining({
        method: "PATCH",
        body: JSON.stringify({
          version: 1,
          cidr: "192.168.2.0/24",
          gateway: null,
          name: "Updated LAN",
          vlan: null,
        }),
      }),
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      "/api/v1/networks/network-1",
      expect.objectContaining({ method: "DELETE" }),
    );
  });

  it("preserves field-level network validation errors", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({
            error: "Please correct the highlighted network fields.",
            field_errors: { cidr: "CIDR must be a valid network range." },
          }),
          { status: 400, headers: { "content-type": "application/json" } },
        ),
      ),
    );

    await expect(createNetwork({ cidr: "not-a-cidr" })).rejects.toMatchObject({
      fieldErrors: { cidr: "CIDR must be a valid network range." },
    } satisfies Partial<ApiError>);
  });

  it("launches an initial discovery scan with an idempotency key", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          id: "run-1",
          network_id: "network-1",
          job_id: "job-1",
          kind: "initial_discovery",
          status: "pending",
          targets_planned: 252,
        }),
        { status: 202, headers: { "content-type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const run = await launchNetworkScan("network-1", {
      kind: "initial_discovery",
      idempotencyKey: "scan-network-1-unique-key",
    });

    expect(run.id).toBe("run-1");
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/networks/network-1/scans",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ kind: "initial_discovery" }),
      }),
    );
    const headers = new Headers(fetchMock.mock.calls[0][1]?.headers);
    expect(headers.get("idempotency-key")).toBe("scan-network-1-unique-key");
    expect(headers.get("x-requested-with")).toBe("hope");
  });

  it("fetches scan progress and requests cancellation with CSRF headers", async () => {
    const run = {
      id: "run-1",
      network_id: "network-1",
      job_id: "job-1",
      kind: "initial_discovery",
      status: "running",
      targets_planned: 252,
      targets_completed: 12,
      ports_planned: 16_515_420,
      ports_completed: 786_420,
      complete: false,
      authoritative: false,
      error: null,
    };
    const cancelled = {
      ...run,
      status: "cancelled",
      cancellation_requested: true,
    };
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify(run), {
          headers: { "content-type": "application/json" },
        }),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify(cancelled), {
          headers: { "content-type": "application/json" },
        }),
      );
    vi.stubGlobal("fetch", fetchMock);

    const current = await fetchScanRun("run-1");
    const result = await cancelScanRun("run-1");

    expect(current.ports_completed).toBe(786_420);
    expect(result.status).toBe("cancelled");
    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      "/api/v1/scans/run-1",
      expect.objectContaining({ credentials: "same-origin" }),
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      "/api/v1/scans/run-1/cancel",
      expect.objectContaining({ method: "POST" }),
    );
    const headers = new Headers(fetchMock.mock.calls[1][1]?.headers);
    expect(headers.get("x-requested-with")).toBe("hope");
  });

  it("fetches monitor rows with the bounded list size", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          items: [
            {
              id: "monitor-1",
              state: "up",
              service_name: "Router",
              endpoint_address: "192.0.2.10/32",
            },
          ],
        }),
        { headers: { "content-type": "application/json" } },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    const result = await fetchMonitors();

    expect(result.items[0]?.state).toBe("up");
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/monitors?limit=100",
      expect.objectContaining({ credentials: "same-origin" }),
    );
  });
});
