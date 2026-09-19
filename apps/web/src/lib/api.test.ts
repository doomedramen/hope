import { afterEach, describe, expect, it, vi } from "vitest";
import {
  cancelScanRun,
  confirmDiscoveryScope,
  draftDiscoveryScope,
  fetchScanRun,
  fetchHealthReady,
  launchNetworkScan,
} from "./api";

describe("fetchHealthReady", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
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
});
