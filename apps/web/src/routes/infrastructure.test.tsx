import "@testing-library/jest-dom/vitest";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { EditDeviceDialog } from "./infrastructure";
import type { DeviceDetail } from "@/lib/api";

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
