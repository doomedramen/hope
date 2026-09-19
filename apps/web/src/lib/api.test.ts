import { afterEach, describe, expect, it, vi } from "vitest";
import {
  confirmDiscoveryScope,
  draftDiscoveryScope,
  fetchHealthReady,
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
});
