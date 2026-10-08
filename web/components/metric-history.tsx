"use client";
import { useEffect, useState } from "react";
import { Area, AreaChart, CartesianGrid, XAxis, YAxis } from "recharts";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
} from "@/components/ui/chart";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Skeleton } from "@/components/ui/skeleton";
import { api } from "@/lib/api";
import type { MetricPoint } from "@/lib/protocol";

export function MetricHistory({
  hostId,
  lastReport,
}: {
  hostId: string;
  lastReport: number | null;
}) {
  const [hours, setHours] = useState("24");
  const [points, setPoints] = useState<MetricPoint[] | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let alive = true;
    api<MetricPoint[]>(`/hosts/${hostId}/metrics?hours=${hours}`)
      .then((p) => {
        if (alive) {
          setPoints(p);
          setError("");
        }
      })
      .catch((e) => {
        if (alive) setError(e.message);
      });
    return () => {
      alive = false;
    };
  }, [hostId, hours, lastReport]);
  const data: {
    time: number;
    cpu: number | null;
    memory: number | null;
    rx: number | null;
    tx: number | null;
  }[] = [];
  points?.forEach((point, index) => {
    const previous = points[index - 1];
    if (
      previous &&
      point.collected_at - previous.collected_at >
        Math.max(point.resolution_seconds, previous.resolution_seconds) * 2.5
    )
      data.push({
        time: (previous.collected_at + previous.resolution_seconds) * 1000,
        cpu: null,
        memory: null,
        rx: null,
        tx: null,
      });
    data.push({
      time: point.collected_at * 1000,
      cpu: point.metrics.cpu_percent,
      memory: point.metrics.memory_total_bytes
        ? ((point.metrics.memory_used_bytes ?? 0) /
            point.metrics.memory_total_bytes) *
          100
        : null,
      rx:
        point.metrics.network_rx_bytes_per_second == null
          ? null
          : point.metrics.network_rx_bytes_per_second / 1024,
      tx:
        point.metrics.network_tx_bytes_per_second == null
          ? null
          : point.metrics.network_tx_bytes_per_second / 1024,
    });
  });
  return (
    <section className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="font-semibold">History</h3>
        <ToggleGroup
          value={[hours]}
          onValueChange={(v) => {
            if (v[0]) setHours(v[0]);
          }}
          size="sm"
          variant="outline"
        >
          <ToggleGroupItem value="1">1h</ToggleGroupItem>
          <ToggleGroupItem value="24">24h</ToggleGroupItem>
          <ToggleGroupItem value="168">7d</ToggleGroupItem>
        </ToggleGroup>
      </div>
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : !points ? (
        <Skeleton className="h-48 w-full" />
      ) : points.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          History appears after the agent sends its first report.
        </p>
      ) : (
        <>
          <p className="text-xs text-muted-foreground">CPU and memory (%)</p>
          <ChartContainer
            config={{
              cpu: { label: "CPU", color: "var(--chart-1)" },
              memory: { label: "Memory", color: "var(--chart-2)" },
            }}
            className="h-44 w-full"
          >
            <AreaChart accessibilityLayer data={data}>
              <CartesianGrid vertical={false} />
              <XAxis
                dataKey="time"
                type="number"
                domain={["dataMin", "dataMax"]}
                tickFormatter={(v) =>
                  new Date(v).toLocaleTimeString([], {
                    hour: "2-digit",
                    minute: "2-digit",
                  })
                }
                minTickGap={40}
              />
              <YAxis domain={[0, 100]} width={30} />
              <ChartTooltip
                content={
                  <ChartTooltipContent
                    labelFormatter={(v) => new Date(v).toLocaleString()}
                  />
                }
              />
              <Area
                dataKey="cpu"
                stroke="var(--color-cpu)"
                fill="var(--color-cpu)"
                fillOpacity={0.1}
                connectNulls={false}
                isAnimationActive={false}
              />
              <Area
                dataKey="memory"
                stroke="var(--color-memory)"
                fill="var(--color-memory)"
                fillOpacity={0.04}
                connectNulls={false}
                isAnimationActive={false}
              />
            </AreaChart>
          </ChartContainer>
          <p className="text-xs text-muted-foreground">
            Network throughput (KiB/s)
          </p>
          <ChartContainer
            config={{
              rx: { label: "Receive", color: "var(--chart-1)" },
              tx: { label: "Transmit", color: "var(--chart-3)" },
            }}
            className="h-36 w-full"
          >
            <AreaChart accessibilityLayer data={data}>
              <XAxis
                dataKey="time"
                type="number"
                domain={["dataMin", "dataMax"]}
                tickFormatter={(v) =>
                  new Date(v).toLocaleTimeString([], {
                    hour: "2-digit",
                    minute: "2-digit",
                  })
                }
                minTickGap={40}
              />
              <YAxis width={36} />
              <ChartTooltip
                content={
                  <ChartTooltipContent
                    labelFormatter={(v) => new Date(v).toLocaleString()}
                  />
                }
              />
              <Area
                dataKey="rx"
                stroke="var(--color-rx)"
                fill="var(--color-rx)"
                fillOpacity={0.1}
                connectNulls={false}
                isAnimationActive={false}
              />
              <Area
                dataKey="tx"
                stroke="var(--color-tx)"
                fill="var(--color-tx)"
                fillOpacity={0.1}
                connectNulls={false}
                isAnimationActive={false}
              />
            </AreaChart>
          </ChartContainer>
          <p className="text-xs text-muted-foreground">
            Gaps mean no samples. Older data uses five-minute averages; long
            views are further grouped for display.
          </p>
        </>
      )}
    </section>
  );
}
