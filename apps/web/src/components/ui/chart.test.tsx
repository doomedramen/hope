import "@testing-library/jest-dom/vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { ChartContainer, ChartTooltipContent } from "./chart";

afterEach(cleanup);

const payload = [
  {
    graphicalItemId: "minimum",
    dataKey: "minimum",
    name: "Minimum",
    value: 20,
  },
  {
    graphicalItemId: "maximum",
    dataKey: "maximum",
    name: "Maximum",
    value: 50,
  },
  { graphicalItemId: "value", dataKey: "value", name: "CPU", value: 35 },
];

it("passes numeric timestamps to tooltip date formatters", () => {
  const timestamp = Date.parse("2026-09-27T12:30:00Z");
  const labelFormatter = vi.fn((value) =>
    new Date(Number(value)).toLocaleString(),
  );
  render(
    <ChartContainer config={{ value: { label: "CPU" } }}>
      <ChartTooltipContent
        active
        label={timestamp}
        payload={payload}
        labelFormatter={labelFormatter}
      />
    </ChartContainer>,
  );
  expect(labelFormatter).toHaveBeenCalledWith(timestamp, payload);
  expect(screen.getByText(new Date(timestamp).toLocaleString())).toBeVisible();
  expect(screen.queryByText(/invalid date/i)).not.toBeInTheDocument();
});

it("shows a zero numeric label without replacing it with the series name", () => {
  render(
    <ChartContainer config={{ value: { label: "CPU" } }}>
      <ChartTooltipContent active label={0} payload={[payload[2]]} />
    </ChartContainer>,
  );
  expect(screen.getByText("0")).toBeVisible();
});

it("preserves configured labels for categorical charts", () => {
  render(
    <ChartContainer
      config={{ value: { label: "CPU" }, online: { label: "Online hosts" } }}
    >
      <ChartTooltipContent active label="online" payload={payload} />
    </ChartContainer>,
  );
  expect(screen.getByText("Online hosts")).toBeVisible();
});
