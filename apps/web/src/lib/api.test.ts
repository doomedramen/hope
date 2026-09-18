import { afterEach, describe, expect, it, vi } from "vitest";
import { fetchHealthReady } from "./api";

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
});
