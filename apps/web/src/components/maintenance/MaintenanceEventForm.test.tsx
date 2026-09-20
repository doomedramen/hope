import "@testing-library/jest-dom/vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { MaintenanceEventForm } from "./MaintenanceEventForm";

describe("MaintenanceEventForm", () => {
  it("rejects an unsupported timezone before submit", () => {
    const onSubmit = vi.fn();
    render(
      <MaintenanceEventForm
        error={null}
        event={null}
        onOpenChange={vi.fn()}
        onSubmit={onSubmit}
        open
        pending={false}
      />,
    );

    fireEvent.change(screen.getByLabelText("Event name"), {
      target: { value: "QA invalid timezone" },
    });
    fireEvent.change(screen.getByLabelText("Timezone"), {
      target: { value: "Not/AZone" },
    });
    fireEvent.submit(screen.getByLabelText("Event name").closest("form")!);

    expect(
      screen.getByText("Timezone must be a supported IANA timezone."),
    ).toBeInTheDocument();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("rejects malformed recurrence rules before submit", () => {
    const onSubmit = vi.fn();
    render(
      <MaintenanceEventForm
        error={null}
        event={null}
        onOpenChange={vi.fn()}
        onSubmit={onSubmit}
        open
        pending={false}
      />,
    );

    fireEvent.change(screen.getByLabelText("Event name"), {
      target: { value: "QA invalid recurrence" },
    });
    fireEvent.change(screen.getByLabelText("Recurrence rule"), {
      target: { value: "not-an-rrule" },
    });
    fireEvent.submit(screen.getByLabelText("Event name").closest("form")!);

    expect(
      screen.getByText("Recurrence rule must include a supported FREQ value."),
    ).toBeInTheDocument();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("accepts the documented weekly recurrence format", () => {
    const onSubmit = vi.fn();
    render(
      <MaintenanceEventForm
        error={null}
        event={null}
        onOpenChange={vi.fn()}
        onSubmit={onSubmit}
        open
        pending={false}
      />,
    );

    fireEvent.change(screen.getByLabelText("Event name"), {
      target: { value: "QA weekly maintenance" },
    });
    fireEvent.change(screen.getByLabelText("Recurrence rule"), {
      target: { value: "FREQ=WEEKLY;BYDAY=SA" },
    });
    fireEvent.submit(screen.getByLabelText("Event name").closest("form")!);

    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({ recurrence_rule: "FREQ=WEEKLY;BYDAY=SA" }),
    );
  });
});
