import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createMaintenanceEvent,
  fetchMaintenanceConflicts,
  fetchMaintenanceEvents,
  startMaintenanceEvent,
} from "./api";

function jsonResponse(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), {
    headers: { "content-type": "application/json" },
    status,
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("maintenance API", () => {
  it("uses bounded list and conflict endpoints", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ items: [], next_cursor: null }))
      .mockResolvedValueOnce(jsonResponse({ items: [], next_cursor: null }));
    vi.stubGlobal("fetch", fetchMock);

    await fetchMaintenanceEvents();
    await fetchMaintenanceConflicts("event-1");

    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      "/api/v1/maintenance-events?limit=200",
      expect.objectContaining({ credentials: "same-origin" }),
    );
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      "/api/v1/maintenance-events/event-1/conflicts",
      expect.objectContaining({ credentials: "same-origin" }),
    );
  });

  it("sends create and lifecycle payloads with CSRF headers", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ id: "event-1" }, 201))
      .mockResolvedValueOnce(jsonResponse({ id: "event-1", state: "active" }));
    vi.stubGlobal("fetch", fetchMock);

    await createMaintenanceEvent({
      name: "Switch maintenance",
      timezone: "UTC",
      start: "2026-09-21T22:00",
      end: "2026-09-21T22:30",
      resources: [{ role: "affected", key: "network-core" }],
    });
    await startMaintenanceEvent("event-1", 3);

    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      "/api/v1/maintenance-events",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({
          name: "Switch maintenance",
          timezone: "UTC",
          start: "2026-09-21T22:00",
          end: "2026-09-21T22:30",
          resources: [{ role: "affected", key: "network-core" }],
        }),
      }),
    );
    const headers = new Headers(fetchMock.mock.calls[0][1]?.headers);
    expect(headers.get("x-requested-with")).toBe("hope");
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      "/api/v1/maintenance-events/event-1/start",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ version: 3 }),
      }),
    );
  });

  it("preserves structured conflict details on failed mutations", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        jsonResponse(
          {
            error: "maintenance event conflicts with existing reservations",
            conflicts: [
              {
                relationship: "resource_overlap",
                reason: "Both events reserve network-core.",
              },
            ],
          },
          409,
        ),
      ),
    );

    await expect(
      createMaintenanceEvent({
        name: "Conflicting event",
        timezone: "UTC",
        start: "2026-09-21T22:00",
        end: "2026-09-21T22:30",
        resources: [{ role: "affected", key: "network-core" }],
      }),
    ).rejects.toMatchObject({
      status: 409,
      conflicts: [{ reason: "Both events reserve network-core." }],
    });
  });
});
