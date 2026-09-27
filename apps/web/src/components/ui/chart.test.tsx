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

it("formats values and ranges without losing tooltip labels or indicators", () => {
  render(
    <ChartContainer
      config={{
        value: { label: "Average", color: "var(--chart-1)" },
        range: { label: "Min–max", color: "var(--chart-1)" },
      }}
    >
      <ChartTooltipContent
        active
        label="12:30"
        payload={[
          {
            graphicalItemId: "value",
            dataKey: "value",
            name: "value",
            value: 0,
          },
          {
            graphicalItemId: "range",
            dataKey: "range",
            name: "range",
            value: [0, 12.5],
          },
        ]}
        valueFormatter={(value) =>
          Array.isArray(value)
            ? value.map((entry) => `${entry}%`).join(" – ")
            : `${value}%`
        }
      />
    </ChartContainer>,
  );
  expect(screen.getByText("12:30")).toBeVisible();
  expect(screen.getByText("Average")).toBeVisible();
  expect(screen.getByText("Min–max")).toBeVisible();
  expect(screen.getByText("0%")).toBeVisible();
  expect(screen.getByText("0% – 12.5%")).toBeVisible();
});
