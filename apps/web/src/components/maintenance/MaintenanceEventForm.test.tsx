import "@testing-library/jest-dom/vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { MaintenanceEventForm } from "./MaintenanceEventForm";

describe("MaintenanceEventForm", () => {
  it("keeps the full event form usable at desktop width", () => {
    render(
      <MaintenanceEventForm
        error={null}
        event={null}
        onOpenChange={vi.fn()}
        onSubmit={vi.fn()}
        open
        pending={false}
      />,
    );

    expect(screen.getByRole("dialog")).toHaveClass("max-w-5xl");
  });

  it("uses an app-owned date picker instead of the native datetime picker", () => {
    render(
      <MaintenanceEventForm
        error={null}
        event={null}
        onOpenChange={vi.fn()}
        onSubmit={vi.fn()}
        open
        pending={false}
      />,
    );

    expect(screen.getByLabelText("Start")).toHaveAttribute("type", "text");
    expect(
      screen.getAllByRole("button", {
        name: "Show local date and time picker",
      }),
    ).toHaveLength(2);
  });

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

  it("rejects malformed advanced recurrence rules before submit", async () => {
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
    fireEvent.click(
      screen.getByRole("combobox", { name: "Recurrence input mode" }),
    );
    const advancedOption = await screen.findByRole("option", {
      name: "Advanced RRULE",
    });
    fireEvent.pointerDown(advancedOption, { pointerType: "mouse" });
    fireEvent.click(advancedOption, { detail: 1 });
    fireEvent.change(screen.getByLabelText("Advanced recurrence rule"), {
      target: { value: "not-an-rrule" },
    });
    fireEvent.submit(screen.getByLabelText("Event name").closest("form")!);

    expect(
      screen.getByText("Recurrence rule must include a supported FREQ value."),
    ).toBeInTheDocument();
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("accepts the documented weekly recurrence format in advanced mode", async () => {
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
    fireEvent.click(
      screen.getByRole("combobox", { name: "Recurrence input mode" }),
    );
    const advancedOption = await screen.findByRole("option", {
      name: "Advanced RRULE",
    });
    fireEvent.pointerDown(advancedOption, { pointerType: "mouse" });
    fireEvent.click(advancedOption, { detail: 1 });
    fireEvent.change(screen.getByLabelText("Advanced recurrence rule"), {
      target: { value: "FREQ=WEEKLY;BYDAY=SA" },
    });
    fireEvent.submit(screen.getByLabelText("Event name").closest("form")!);

    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({ recurrence_rule: "FREQ=WEEKLY;BYDAY=SA" }),
    );
  });

  it("builds a recurrence from guided frequency, weekdays, and end controls", async () => {
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
      target: { value: "QA guided recurrence" },
    });
    fireEvent.click(
      screen.getByRole("combobox", { name: "Recurrence frequency" }),
    );
    const weeklyOption = await screen.findByRole("option", { name: "Weekly" });
    fireEvent.pointerDown(weeklyOption, { pointerType: "mouse" });
    fireEvent.click(weeklyOption, { detail: 1 });
    fireEvent.change(screen.getByLabelText("Repeat every"), {
      target: { value: "2" },
    });
    fireEvent.click(
      screen
        .getAllByRole("checkbox")
        .find(
          (checkbox) => checkbox.getAttribute("aria-label") === "Saturday",
        )!,
    );
    fireEvent.click(screen.getByRole("combobox", { name: "Recurrence end" }));
    const countOption = await screen.findByRole("option", {
      name: "After a fixed number of occurrences",
    });
    fireEvent.pointerDown(countOption, { pointerType: "mouse" });
    fireEvent.click(countOption, { detail: 1 });
    fireEvent.change(screen.getByLabelText("Occurrences"), {
      target: { value: "3" },
    });
    fireEvent.submit(screen.getByLabelText("Event name").closest("form")!);

    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({
        recurrence_rule: "FREQ=WEEKLY;INTERVAL=2;BYDAY=SA;COUNT=3",
      }),
    );
  });
});
