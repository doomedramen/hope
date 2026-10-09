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

export function MetricHistory({
  hostId,
  lastReport,
}: {
  hostId: string;
  lastReport: number | null;
}) {
  const [hours, setHours] = useState("24");
  const [result, setResult] = useState<{
    hostId: string;
    hours: string;
    points: MetricPoint[];
  } | null>(null);
  const points =
    result?.hostId === hostId && result.hours === hours ? result.points : null;
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
  const windowStart = windowEnd - Number(hours) * 3600;
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
                domain={[windowStart * 1000, windowEnd * 1000]}
                tickFormatter={timeLabel}
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
                domain={[windowStart * 1000, windowEnd * 1000]}
                tickFormatter={timeLabel}
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
          <ResourceHistory
            points={points}
            start={windowStart}
            end={windowEnd}
            timeLabel={timeLabel}
          />
          <p className="text-xs text-muted-foreground">
            Gaps mean missing samples or unavailable metrics. Older data uses
            five-minute averages; long views are further grouped for display.
          </p>
        </>
      )}
    </section>
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
      <p className="text-xs text-muted-foreground">
        GPU utilization and memory (%)
      </p>
      {gpuOptions.size ? (
        <>
          <Select value={gpuId} onValueChange={setGpuChoice}>
            <SelectTrigger aria-label="GPU history device">
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
          {gpuData.some(
            (row) => row.utilization != null || row.memory != null,
          ) ? (
            <ResourceChart
              data={gpuData}
              keys={["utilization", "memory"]}
              labels={["GPU", "Memory"]}
              start={start}
              end={end}
              timeLabel={timeLabel}
            />
          ) : (
            <p className="text-sm text-muted-foreground">
              GPU metrics unavailable for this device in this window.
            </p>
          )}
        </>
      ) : (
        <p className="text-sm text-muted-foreground">
          GPU history unavailable: no identified adapter reports. Inventory
          remains in hardware details.
        </p>
      )}
      <p className="text-xs text-muted-foreground">Filesystem usage (%)</p>
      {diskOptions.size ? (
        <>
          <Select value={diskId} onValueChange={setDiskChoice}>
            <SelectTrigger aria-label="Filesystem history device">
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
          <ResourceChart
            data={diskData}
            keys={["used"]}
            labels={["Used"]}
            start={start}
            end={end}
            timeLabel={timeLabel}
          />
        </>
      ) : (
        <p className="text-sm text-muted-foreground">
          Filesystem history unavailable in this window.
        </p>
      )}
    </>
  );
}
function ResourceChart({
  data,
  keys,
  labels,
  start,
  end,
  timeLabel,
}: {
  data: Record<string, number | null>[];
  keys: string[];
  labels: string[];
  start: number;
  end: number;
  timeLabel: (time: number) => string;
}) {
  return (
    <ChartContainer
      config={Object.fromEntries(
        keys.map((key, index) => [
          key,
          { label: labels[index], color: `var(--chart-${index + 1})` },
        ]),
      )}
      className="h-36 w-full"
    >
      <AreaChart accessibilityLayer data={data}>
        <XAxis
          dataKey="time"
          type="number"
          domain={[start * 1000, end * 1000]}
          tickFormatter={timeLabel}
          minTickGap={40}
        />
        <YAxis domain={[0, 100]} width={30} />
        <ChartTooltip
          content={
            <ChartTooltipContent
              labelFormatter={(value) => new Date(value).toLocaleString()}
            />
          }
        />
        {keys.map((key) => (
          <Area
            key={key}
            dataKey={key}
            stroke={`var(--color-${key})`}
            fill={`var(--color-${key})`}
            fillOpacity={0.08}
            connectNulls={false}
            isAnimationActive={false}
          />
        ))}
      </AreaChart>
    </ChartContainer>
  );
}
