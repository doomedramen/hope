import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { EditDeviceDialog, InfrastructurePage } from "./infrastructure";
import type { Device, DeviceDetail } from "@/lib/api";

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

afterEach(() => {
  vi.unstubAllGlobals();
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
  it("selects a visible device when search hides the current selection", async () => {
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
    expect(
      await screen.findByRole("heading", { name: "QA VM device" }),
    ).toBeInTheDocument();

    fireEvent.change(screen.getByRole("textbox", { name: "Search devices" }), {
      target: { value: "QA test" },
    });

    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "QA test device" }),
      ).toBeInTheDocument(),
    );
  });
});
