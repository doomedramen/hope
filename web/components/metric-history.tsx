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
import {
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectGroup,
  SelectItem,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { api } from "@/lib/api";
import { historySeries, memoryPercent } from "@/lib/history";
import type { MetricPoint } from "@/lib/protocol";

export type HistoryWindow = "1" | "6" | "24" | "168";

/** Loads one host's history for a window; refetches when a new report arrives. */
export function useHistory(
  hostId: string,
  hours: HistoryWindow,
  lastReport: number | null,
) {
  const [result, setResult] = useState<{
    hostId: string;
    hours: string;
    points: MetricPoint[];
  } | null>(null);
  const [error, setError] = useState("");
  const [windowEnd, setWindowEnd] = useState(() => Date.now() / 1000);
  useEffect(() => {
    let alive = true;
    api<MetricPoint[]>(`/hosts/${hostId}/metrics?hours=${hours}`)
      .then((p) => {
        if (alive) {
          setResult({ hostId, hours, points: p });
          setWindowEnd(Date.now() / 1000);
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
  const points =
    result?.hostId === hostId && result.hours === hours ? result.points : null;
  return {
    points,
    error,
    windowStart: windowEnd - Number(hours) * 3600,
    windowEnd,
  };
}

const SERIES = {
  cpu: { label: "CPU", color: "var(--chart-1)" },
  memory: { label: "Memory", color: "var(--chart-2)" },
  rx: { label: "Receive", color: "var(--chart-1)" },
  tx: { label: "Transmit", color: "var(--chart-2)" },
  utilization: { label: "GPU", color: "var(--chart-1)" },
  used: { label: "Used", color: "var(--chart-1)" },
} as const;

export function HistoryRange({
  value,
  onChange,
}: {
  value: HistoryWindow;
  onChange: (value: HistoryWindow) => void;
}) {
  return (
    <ToggleGroup
      value={[value]}
      onValueChange={(v) => {
        if (v[0] === "1" || v[0] === "6" || v[0] === "24" || v[0] === "168")
          onChange(v[0]);
      }}
      size="sm"
      variant="outline"
      aria-label="History window"
    >
      <ToggleGroupItem value="1">1h</ToggleGroupItem>
      <ToggleGroupItem value="6">6h</ToggleGroupItem>
      <ToggleGroupItem value="24">24h</ToggleGroupItem>
      <ToggleGroupItem value="168">7d</ToggleGroupItem>
    </ToggleGroup>
  );
}

export function MetricHistory({
  points,
  error,
  hours,
  windowStart,
  windowEnd,
}: {
  points: MetricPoint[] | null;
  error: string;
  hours: HistoryWindow;
  windowStart: number;
  windowEnd: number;
}) {
  const data = historySeries(
    points ?? [],
    windowStart,
    windowEnd,
    (metrics) => ({
      cpu: metrics.cpu_percent,
      memory: memoryPercent(metrics),
      rx:
        metrics.network_rx_bytes_per_second == null
          ? null
          : metrics.network_rx_bytes_per_second / 1024,
      tx:
        metrics.network_tx_bytes_per_second == null
          ? null
          : metrics.network_tx_bytes_per_second / 1024,
    }),
  );
  const timeLabel = (value: number) =>
    new Date(value).toLocaleString([], {
      ...(Number(hours) > 24 ? { month: "short", day: "numeric" } : {}),
      hour: "2-digit",
      minute: "2-digit",
    });
  if (error)
    return (
      <Alert variant="destructive">
        <AlertDescription>{error}</AlertDescription>
      </Alert>
    );
  if (!points)
    return (
      <div className="flex flex-col gap-3">
        <Skeleton className="h-40 w-full" />
        <Skeleton className="h-32 w-full" />
      </div>
    );
  if (points.length === 0)
    return (
      <p className="text-sm text-muted-foreground">
        History appears after the agent sends its first report.
      </p>
    );
  return (
    <div className="flex flex-col gap-5">
      <ChartBlock
        title="CPU and memory"
        unit="%"
        keys={["cpu", "memory"]}
        data={data}
        domain={[0, 100]}
        start={windowStart}
        end={windowEnd}
        timeLabel={timeLabel}
        height="h-40"
      />
      <ChartBlock
        title="Network throughput"
        unit="KiB/s"
        keys={["rx", "tx"]}
        data={data}
        start={windowStart}
        end={windowEnd}
        timeLabel={timeLabel}
        height="h-32"
      />
      <ResourceHistory
        points={points}
        start={windowStart}
        end={windowEnd}
        timeLabel={timeLabel}
      />
      <p className="text-xs text-muted-foreground">
        Gaps are missing samples or unavailable metrics, never zero. Windows
        beyond 24 hours use five-minute averages, grouped further for display.
      </p>
    </div>
  );
}

function ChartBlock({
  title,
  unit,
  keys,
  data,
  domain,
  start,
  end,
  timeLabel,
  height,
  action,
}: {
  title: string;
  unit: string;
  keys: (keyof typeof SERIES)[];
  data: Record<string, number | null>[];
  domain?: [number, number];
  start: number;
  end: number;
  timeLabel: (time: number) => string;
  height: string;
  action?: React.ReactNode;
}) {
  const config = Object.fromEntries(keys.map((key) => [key, SERIES[key]]));
  return (
    <div className="chart-block">
      <div className="chart-title">
        <span>
          {title}{" "}
          <span className="font-normal text-muted-foreground">({unit})</span>
        </span>
        {action}
      </div>
      {keys.length > 1 && (
        <div className="chart-legend" aria-label="Legend">
          {keys.map((key) => (
            <span key={key}>
              <i style={{ background: SERIES[key].color }} />
              {SERIES[key].label}
            </span>
          ))}
        </div>
      )}
      <ChartContainer config={config} className={`${height} w-full`}>
        <AreaChart
          accessibilityLayer
          data={data}
          margin={{ top: 6, right: 6, bottom: 0, left: 0 }}
        >
          <CartesianGrid vertical={false} strokeDasharray="0" />
          <XAxis
            dataKey="time"
            type="number"
            domain={[start * 1000, end * 1000]}
            tickFormatter={timeLabel}
            minTickGap={48}
            tickLine={false}
            axisLine={false}
          />
          <YAxis
            domain={domain}
            width={34}
            tickLine={false}
            axisLine={false}
            tickFormatter={(value: number) =>
              value >= 1000
                ? `${(value / 1000).toFixed(value >= 10000 ? 0 : 1)}K`
                : String(Math.round(value))
            }
          />
          <ChartTooltip
            content={
              <ChartTooltipContent
                labelFormatter={(v) => new Date(v).toLocaleString()}
              />
            }
          />
          {keys.map((key) => (
            <Area
              key={key}
              dataKey={key}
              type="monotone"
              stroke={`var(--color-${key})`}
              strokeWidth={2}
              fill={`var(--color-${key})`}
              fillOpacity={0.1}
              connectNulls={false}
              isAnimationActive={false}
              dot={false}
              activeDot={{ r: 4, strokeWidth: 2, stroke: "var(--card)" }}
            />
          ))}
        </AreaChart>
      </ChartContainer>
    </div>
  );
}

function ResourceHistory({
  points,
  start,
  end,
  timeLabel,
}: {
  points: MetricPoint[];
  start: number;
  end: number;
  timeLabel: (time: number) => string;
}) {
  const [gpuChoice, setGpuChoice] = useState<string | null>(null);
  const [diskChoice, setDiskChoice] = useState<string | null>(null);
  const gpuOptions = new Map(
    points.flatMap((p) =>
      p.metrics.gpus
        .filter((g) => g.id != null)
        .map((g) => [g.id!, g.name] as const),
    ),
  );
  const diskKey = (disk: MetricPoint["metrics"]["disks"][number]) =>
    JSON.stringify([disk.name, disk.mount, disk.filesystem]);
  const diskOptions = new Map(
    points.flatMap((p) =>
      p.metrics.disks.map(
        (d) => [diskKey(d), `${d.mount} · ${d.name}`] as const,
      ),
    ),
  );
  const gpuId =
    gpuChoice && gpuOptions.has(gpuChoice)
      ? gpuChoice
      : [...gpuOptions.keys()][0];
  const diskId =
    diskChoice && diskOptions.has(diskChoice)
      ? diskChoice
      : [...diskOptions.keys()][0];
  const gpuData = historySeries(points, start, end, (metrics) => {
    const gpu = metrics.gpus.find((g) => g.id === gpuId);
    return {
      utilization: gpu?.utilization_percent ?? null,
      memory:
        gpu?.memory_used_bytes != null &&
        gpu.memory_total_bytes != null &&
        gpu.memory_total_bytes > 0
          ? (gpu.memory_used_bytes / gpu.memory_total_bytes) * 100
          : null,
    };
  });
  const diskData = historySeries(points, start, end, (metrics) => {
    const disk = metrics.disks.find((d) => diskKey(d) === diskId);
    return {
      used:
        disk && disk.total_bytes > 0
          ? (1 - disk.available_bytes / disk.total_bytes) * 100
          : null,
    };
  });
  return (
    <>
      {diskOptions.size > 0 && (
        <ChartBlock
          title="Filesystem usage"
          unit="%"
          keys={["used"]}
          data={diskData}
          domain={[0, 100]}
          start={start}
          end={end}
          timeLabel={timeLabel}
          height="h-28"
          action={
            diskOptions.size > 1 ? (
              <Select value={diskId} onValueChange={setDiskChoice}>
                <SelectTrigger size="sm" aria-label="Filesystem history device">
                  <SelectValue>
                    {diskId ? diskOptions.get(diskId) : "Choose filesystem"}
                  </SelectValue>
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    {[...diskOptions].map(([id, name]) => (
                      <SelectItem key={id} value={id}>
                        {name}
                      </SelectItem>
                    ))}
                  </SelectGroup>
                </SelectContent>
              </Select>
            ) : (
              <span className="text-xs font-normal text-muted-foreground">
                {diskId ? diskOptions.get(diskId) : ""}
              </span>
            )
          }
        />
      )}
      {gpuOptions.size > 0 &&
        (gpuData.some(
          (row) => row.utilization != null || row.memory != null,
        ) ? (
          <ChartBlock
            title="GPU utilization and memory"
            unit="%"
            keys={["utilization", "memory"]}
            data={gpuData}
            domain={[0, 100]}
            start={start}
            end={end}
            timeLabel={timeLabel}
            height="h-28"
            action={
              <Select value={gpuId} onValueChange={setGpuChoice}>
                <SelectTrigger size="sm" aria-label="GPU history device">
                  <SelectValue>
                    {gpuId
                      ? `${gpuOptions.get(gpuId)} · ${gpuId.slice(-8)}`
                      : "Choose GPU"}
                  </SelectValue>
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    {[...gpuOptions].map(([id, name]) => (
                      <SelectItem key={id} value={id}>
                        {name} · {id}
                      </SelectItem>
                    ))}
                  </SelectGroup>
                </SelectContent>
              </Select>
            }
          />
        ) : (
          <p className="text-xs text-muted-foreground">
            GPU metrics are unavailable for this device in this window.
          </p>
        ))}
    </>
  );
}
