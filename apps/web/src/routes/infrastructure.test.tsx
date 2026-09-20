import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  AddNetworkDialog,
  CreateDeviceDialog,
  EditDeviceDialog,
  InfrastructurePage,
  MergeDialog,
} from "./infrastructure";
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
  window.history.replaceState({}, "", "/");
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
