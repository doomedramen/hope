"use client";
import { useEffect, useMemo, useState } from "react";
import {
  Area,
  CartesianGrid,
  ComposedChart,
  Line,
  XAxis,
  YAxis,
  type DotItemDotProps,
} from "recharts";
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
import { api, bytes } from "@/lib/api";
import { appendLiveHistory, historySeries, memoryPercent } from "@/lib/history";
import { useNow } from "@/lib/clock";
import type { MetricPoint, Metrics, SourceMetricPoint } from "@/lib/protocol";

export type HistoryWindow = "1" | "6" | "24" | "168";

export function SourceMetricHistory({
  hostId,
  primarySourceId,
  sourceLabel,
}: {
  hostId: string;
  primarySourceId: string | null;
  sourceLabel: string;
}) {
  const [hours, setHours] = useState<HistoryWindow>("24");
  const now = useNow(30);
  const [result, setResult] = useState<{
    hostId: string;
    points: SourceMetricPoint[] | null;
    error: string;
  } | null>(null);
  const [selectedSource, setSelectedSource] = useState(
    primarySourceId ?? hostId,
  );
  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async () => {
      try {
        const points = await api<SourceMetricPoint[]>(
          `/hosts/${hostId}/source-metrics?hours=${hours}`,
          { signal: controller.signal },
        );
        if (!controller.signal.aborted)
          setResult({ hostId, points, error: "" });
      } catch (cause) {
        if (!controller.signal.aborted)
          setResult((previous) => ({
            hostId,
            points: previous?.hostId === hostId ? previous.points : null,
            error: (cause as Error).message,
          }));
      } finally {
        if (!controller.signal.aborted) timer = setTimeout(refresh, 30000);
      }
    };
    void refresh();
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, [hostId, hours]);
  const points = result?.hostId === hostId ? result.points : null;
  const error = result?.hostId === hostId ? result.error : "";
  const sourceIds = [
    ...new Set((points ?? []).map((point) => point.source_device_id ?? hostId)),
  ];
  const effectiveSource = sourceIds.includes(selectedSource)
    ? selectedSource
    : primarySourceId && sourceIds.includes(primarySourceId)
      ? primarySourceId
      : (sourceIds[0] ?? hostId);
  const start = now - Number(hours) * 3600;
  const selectedPoints =
    points
      ?.filter(
        (point) => (point.source_device_id ?? hostId) === effectiveSource,
      )
      .filter(
        (point) =>
          point.collected_at + point.resolution_seconds > start &&
          point.collected_at <= now,
      )
      .sort((a, b) => a.collected_at - b.collected_at) ?? [];
  const sourceSeries = (
    select: (
      metrics: SourceMetricPoint["metrics"],
    ) => Record<string, number | null>,
  ) => sourceHistorySeries(selectedPoints, start, now, select);
  const timeLabel = (value: number) =>
    new Date(value).toLocaleString([], {
      ...(Number(hours) > 24 ? { month: "short", day: "numeric" } : {}),
      hour: "2-digit",
      minute: "2-digit",
    });
  return (
    <section
      className="container-info-section source-metric-history"
      aria-label="Source metrics history"
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2>{sourceLabel} history</h2>
        <HistoryRange value={hours} onChange={setHours} />
      </div>
      {sourceIds.length > 1 && (
        <div className="source-history-controls mb-3 flex flex-wrap items-center gap-2">
          <label className="text-sm" htmlFor="source-history-selector">
            Historical source
          </label>
          <Select
            value={effectiveSource}
            onValueChange={(value) => value && setSelectedSource(value)}
          >
            <SelectTrigger
              className="source-history-select"
              id="source-history-selector"
              aria-label="Historical source"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                {sourceIds.map((id) => (
                  <SelectItem key={id} value={id}>
                    {id === primarySourceId
                      ? `Current source · ${id.slice(0, 8)}`
                      : `Merged source · ${id.slice(0, 8)}`}
                  </SelectItem>
                ))}
              </SelectGroup>
            </SelectContent>
          </Select>
        </div>
      )}
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : points == null ? (
        <Skeleton className="h-24 w-full" />
      ) : selectedPoints.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          No source samples in this window.
        </p>
      ) : (
        <>
          <div className="charts flex flex-col gap-5">
            <ChartBlock
              title={`${sourceLabel} CPU`}
              unit="%"
              series={[{ key: "cpu", label: "CPU", color: SLOTS[0] }]}
              data={sourceSeries((metric) => ({ cpu: metric.cpu_percent }))}
              format={percent}
              height="h-32"
              start={start}
              end={now}
              timeLabel={timeLabel}
            />
            <ChartBlock
              title={`${sourceLabel} memory`}
              unit="GiB"
              kind="line"
              series={[
                { key: "used", label: "Used", color: SLOTS[0] },
                { key: "total", label: "Total", color: SLOTS[1] },
              ]}
              data={sourceSeries((metric) => ({
                used:
                  metric.memory_used_bytes == null
                    ? null
                    : metric.memory_used_bytes / GIB,
                total:
                  metric.memory_total_bytes == null
                    ? null
                    : metric.memory_total_bytes / GIB,
              }))}
              format={gib}
              height="h-32"
              start={start}
              end={now}
              timeLabel={timeLabel}
            />
            <ChartBlock
              title={`${sourceLabel} network`}
              unit="bytes per second"
              series={[
                { key: "rx", label: "Receive", color: SLOTS[0] },
                { key: "tx", label: "Transmit", color: SLOTS[1] },
              ]}
              data={sourceSeries((metric) => ({
                rx: metric.network_rx_bytes_per_second,
                tx: metric.network_tx_bytes_per_second,
              }))}
              format={ratePerSecond}
              height="h-32"
              start={start}
              end={now}
              timeLabel={timeLabel}
            />
            <ChartBlock
              title={`${sourceLabel} disk I/O`}
              unit="bytes per second"
              series={[
                { key: "read", label: "Read", color: SLOTS[0] },
                { key: "write", label: "Write", color: SLOTS[1] },
              ]}
              data={sourceSeries((metric) => ({
                read: metric.disk_read_bytes_per_second,
                write: metric.disk_write_bytes_per_second,
              }))}
              format={ratePerSecond}
              height="h-32"
              start={start}
              end={now}
              timeLabel={timeLabel}
            />
          </div>
          <p className="text-xs text-muted-foreground">
            Values come from {sourceLabel}; unavailable readings remain gaps.
            CPU and memory are not inferred from network or disk activity.
          </p>
        </>
      )}
    </section>
  );
}

function sourceHistorySeries(
  points: SourceMetricPoint[],
  start: number,
  end: number,
  select: (
    metrics: SourceMetricPoint["metrics"],
  ) => Record<string, number | null>,
) {
  const samples = points
    .filter(
      (point) =>
        point.collected_at + point.resolution_seconds > start &&
        point.collected_at <= end,
    )
    .map((point) => ({
      ...point,
      collected_at: Math.max(start, point.collected_at),
    }))
    .sort((a, b) => a.collected_at - b.collected_at);
  const keys = new Set(
    samples.flatMap((point) => Object.keys(select(point.metrics))),
  );
  const empty = (time: number) => ({
    time: time * 1000,
    ...Object.fromEntries([...keys].map((key) => [key, null])),
  });
  const result: Record<string, number | null>[] = [empty(start)];
  samples.forEach((point, index) => {
    const previous = samples[index - 1];
    if (
      previous &&
      point.collected_at - previous.collected_at >
        Math.max(point.resolution_seconds, previous.resolution_seconds) * 2.5
    ) {
      result.push(
        empty(previous.collected_at + previous.resolution_seconds),
        empty(point.collected_at - 1),
      );
    }
    result.push({ time: point.collected_at * 1000, ...select(point.metrics) });
  });
  const last = samples.at(-1);
  if (last && end - last.collected_at > last.resolution_seconds * 2.5)
    result.push(empty(last.collected_at + last.resolution_seconds));
  result.push(empty(end));
  return result;
}

/** Seconds per point the server averages each window into; 0 keeps raw samples. */
const BUCKET_SECONDS: Record<HistoryWindow, number> = {
  "1": 0,
  "6": 0,
  "24": 120,
  "168": 900,
};
/** Raw windows still reload now and then so the server's stored samples replace live ones. */
const RAW_RELOAD_SECONDS = 600;

/**
 * Loads one host's history for a window and keeps it live: each new report the
 * host carries is appended as it arrives, and the window slides with it. The
 * stored history is reloaded when an averaging bucket closes (or every ten
 * minutes for raw windows), which folds the live points back into it.
 */
export function useHistory(
  hostId: string,
  hours: HistoryWindow,
  collectedAt: number | null,
  latest: Metrics | null,
) {
  const now = useNow(30);
  const [result, setResult] = useState<{
    hostId: string;
    hours: string;
    points: MetricPoint[];
  } | null>(null);
  const [failure, setFailure] = useState<{
    hostId: string;
    hours: HistoryWindow;
    message: string;
  } | null>(null);
  const [fetchedAt, setFetchedAt] = useState(() => Date.now() / 1000);
  const [live, setLive] = useState<{
    hostId: string;
    points: MetricPoint[];
  }>({ hostId, points: [] });
  // Append the host's newest sample while rendering, React's pattern for state
  // derived from a changing prop; an effect would paint the stale chart first.
  const newest = live.points.at(-1)?.collected_at ?? 0;
  if (live.hostId !== hostId) setLive({ hostId, points: [] });
  else if (collectedAt && latest && collectedAt > newest)
    setLive({
      hostId,
      points: appendLiveHistory(
        live.points,
        { collected_at: collectedAt, resolution_seconds: 30, metrics: latest },
        Math.max(BUCKET_SECONDS[hours], RAW_RELOAD_SECONDS),
      ),
    });
  const bucket = BUCKET_SECONDS[hours] || RAW_RELOAD_SECONDS;
  const reload = Math.floor(Math.max(now, collectedAt ?? 0) / bucket);
  useEffect(() => {
    const controller = new AbortController();
    api<MetricPoint[]>(`/hosts/${hostId}/metrics?hours=${hours}`, {
      signal: controller.signal,
    })
      .then((p) => {
        if (!controller.signal.aborted) {
          setResult({ hostId, hours, points: p });
          setFetchedAt(Date.now() / 1000);
          setFailure(null);
        }
      })
      .catch((e) => {
        if (!controller.signal.aborted)
          setFailure({ hostId, hours, message: e.message });
      });
    return () => {
      controller.abort();
    };
  }, [hostId, hours, reload]);
  const stored =
    result?.hostId === hostId && result.hours === hours ? result.points : null;
  const points = useMemo(() => {
    if (!stored) return null;
    const through = stored.at(-1)?.collected_at ?? 0;
    const fresh =
      live.hostId === hostId
        ? live.points.filter((point) => point.collected_at > through)
        : [];
    return fresh.length ? [...stored, ...fresh] : stored;
  }, [stored, live, hostId]);
  const windowEnd = Math.max(now, fetchedAt, collectedAt ?? 0);
  return {
    points,
    error:
      failure?.hostId === hostId && failure.hours === hours
        ? failure.message
        : "",
    windowStart: windowEnd - Number(hours) * 3600,
    windowEnd,
  };
}

/** Categorical slots in validated order; never cycled past eight. */
export const SLOTS = [
  "var(--chart-1)",
  "var(--chart-2)",
  "var(--chart-3)",
  "var(--chart-4)",
  "var(--chart-5)",
  "var(--chart-6)",
  "var(--chart-7)",
  "var(--chart-8)",
];
type Series = { key: string; label: string; color: string };
const GIB = 1024 ** 3;
const ratePerSecond = (value: number) => `${bytes(value)}/s`;
const gib = (value: number) => `${value.toFixed(value < 10 ? 2 : 1)} GiB`;
const percent = (value: number) => `${value.toFixed(0)}%`;
const celsius = (value: number) => `${value.toFixed(1)} °C`;
const plain = (value: number) => value.toFixed(2);
const millis = (value: number) => `${value.toFixed(0)} ms`;

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

/** Container identity is its daemon ID, never its mutable/reusable name. */
export function ContainerMetricHistory({
  containerId,
  hostId,
  collectedAt,
  latest,
}: {
  containerId: string;
  hostId: string;
  collectedAt: number | null;
  latest: Metrics | null;
}) {
  const [hours, setHours] = useState<HistoryWindow>("1");
  const history = useHistory(hostId, hours, collectedAt, latest);
  const { points, error, windowStart: start, windowEnd: end } = history;
  const frame = {
    start,
    end,
    timeLabel: (value: number) =>
      new Date(value).toLocaleString([], {
        ...(Number(hours) > 24 ? { month: "short", day: "numeric" } : {}),
        hour: "2-digit",
        minute: "2-digit",
      }),
  };
  const select = (metrics: Metrics) =>
    metrics.containers.find((item) => item.id === containerId);
  return (
    <section className="container-info-section" aria-label="Container history">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2>Resource history</h2>
        <HistoryRange value={hours} onChange={setHours} />
      </div>
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : !points ? (
        <Skeleton className="h-40 w-full" />
      ) : !points.some((point) => select(point.metrics)) ? (
        <p className="text-sm text-muted-foreground">
          No samples for this container in this window.
        </p>
      ) : (
        <div className="charts flex flex-col gap-5">
          <ChartBlock
            title="Container CPU"
            unit="% of one core"
            series={[{ key: "cpu", label: "CPU", color: SLOTS[0] }]}
            data={historySeries(points, start, end, (metrics) => ({
              cpu: select(metrics)?.cpu_percent ?? null,
            }))}
            format={percent}
            height="h-40"
            {...frame}
          />
          <ChartBlock
            title="Container memory"
            unit="GiB"
            kind="line"
            series={[
              { key: "used", label: "Used", color: SLOTS[0] },
              { key: "limit", label: "Limit", color: SLOTS[1] },
            ]}
            data={historySeries(points, start, end, (metrics) => {
              const container = select(metrics);
              return {
                used:
                  container?.memory_used_bytes == null
                    ? null
                    : container.memory_used_bytes / GIB,
                limit:
                  container?.memory_limit_bytes == null
                    ? null
                    : container.memory_limit_bytes / GIB,
              };
            })}
            format={gib}
            height="h-40"
            {...frame}
          />
          <p className="text-sm text-muted-foreground">
            CPU uses 100% per core. Missing readings remain gaps. Longer windows
            average CPU and memory by container ID; cumulative I/O totals remain
            in the latest report.
          </p>
        </div>
      )}
    </section>
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
  const frame = { start: windowStart, end: windowEnd, timeLabel };
  const series = (select: (m: Metrics) => Record<string, number | null>) =>
    historySeries(points, windowStart, windowEnd, select);
  const hasBreakdown = points.some(
    (p) => p.metrics.memory_cached_bytes != null,
  );
  const hasArc = points.some((p) => p.metrics.memory_zfs_arc_bytes != null);
  const hasSwap = points.some(
    (p) =>
      p.metrics.swap_used_bytes != null || p.metrics.swap_total_bytes != null,
  );
  const hasLoad = points.some((p) => p.metrics.load_average);
  return (
    <div className="charts flex flex-col gap-5">
      <ChartBlock
        title="CPU"
        unit="%"
        series={[{ key: "cpu", label: "CPU", color: SLOTS[0] }]}
        data={series((m) => ({ cpu: m.cpu_percent }))}
        domain={[0, 100]}
        format={percent}
        height="h-32"
        {...frame}
      />
      {hasLoad && (
        <ChartBlock
          title="Load average"
          unit="runnable tasks"
          kind="line"
          series={[
            { key: "one", label: "1 min", color: SLOTS[0] },
            { key: "five", label: "5 min", color: SLOTS[1] },
            { key: "fifteen", label: "15 min", color: SLOTS[2] },
          ]}
          data={series((m) => ({
            one: m.load_average?.one ?? null,
            five: m.load_average?.five ?? null,
            fifteen: m.load_average?.fifteen ?? null,
          }))}
          format={plain}
          height="h-32"
          {...frame}
        />
      )}
      <ChartBlock
        title="Memory"
        unit="GiB"
        kind="stacked"
        series={[
          {
            key: "used",
            label: hasArc ? "Used (excluding ARC)" : "Used",
            color: SLOTS[0],
          },
          ...(hasArc
            ? [{ key: "arc", label: "ZFS ARC", color: SLOTS[1] }]
            : []),
          ...(hasBreakdown
            ? [{ key: "cache", label: "Cache and buffers", color: SLOTS[2] }]
            : []),
          ...(hasSwap ? [{ key: "swap", label: "Swap", color: SLOTS[3] }] : []),
        ]}
        total={{ key: "total", label: hasSwap ? "RAM + swap" : "Total" }}
        data={series((m) => {
          const arc = m.memory_zfs_arc_bytes;
          const used = m.memory_used_bytes;
          return {
            used: used == null ? null : Math.max(0, used - (arc ?? 0)) / GIB,
            arc: arc == null ? null : arc / GIB,
            cache:
              m.memory_cached_bytes == null
                ? null
                : m.memory_cached_bytes / GIB,
            swap: m.swap_used_bytes == null ? null : m.swap_used_bytes / GIB,
            total:
              m.memory_total_bytes == null ||
              (hasSwap && m.swap_total_bytes == null)
                ? null
                : (m.memory_total_bytes + (m.swap_total_bytes ?? 0)) / GIB,
          };
        })}
        format={gib}
        height="h-40"
        {...frame}
      />
      <ChartBlock
        title="Network throughput"
        unit="per second"
        series={[
          { key: "rx", label: "Receive", color: SLOTS[0] },
          { key: "tx", label: "Transmit", color: SLOTS[1] },
        ]}
        data={series((m) => ({
          rx: m.network_rx_bytes_per_second,
          tx: m.network_tx_bytes_per_second,
        }))}
        format={ratePerSecond}
        height="h-32"
        {...frame}
      />
      <DeviceHistory points={points} {...frame} />
      <PoolHistory points={points} {...frame} />
      <SensorHistory points={points} {...frame} />
      <MonitorHistory points={points} {...frame} />
      <ResourceHistory points={points} {...frame} />
      <p className="text-xs text-muted-foreground">
        Gaps are missing samples or unavailable metrics, never zero. Windows
        beyond 24 hours use five-minute averages, grouped further for display.
      </p>
    </div>
  );
}

type Frame = {
  start: number;
  end: number;
  timeLabel: (time: number) => string;
};

function ChartBlock({
  title,
  unit,
  series,
  data,
  domain,
  start,
  end,
  timeLabel,
  height,
  action,
  kind = "area",
  total,
  format = (value) => String(Math.round(value)),
}: Frame & {
  title: string;
  unit: string;
  series: Series[];
  data: Record<string, number | null>[];
  domain?: [number, number];
  height: string;
  action?: React.ReactNode;
  kind?: "area" | "line" | "stacked";
  /** A reference series drawn as a line above the areas (for example total memory). */
  total?: { key: string; label: string };
  format?: (value: number) => string;
}) {
  const config = Object.fromEntries([
    ...series.map((s) => [s.key, { label: s.label, color: s.color }]),
    ...(total
      ? [[total.key, { label: total.label, color: "var(--muted-foreground)" }]]
      : []),
  ]);
  const legend = [
    ...series,
    ...(total
      ? [
          {
            key: total.key,
            label: total.label,
            color: "var(--muted-foreground)",
          },
        ]
      : []),
  ];
  // The most recent known value per series, so the chart carries the current reading itself.
  const latest = (key: string) => {
    for (let i = data.length - 1; i >= 0; i--) {
      const value = data[i][key];
      if (value != null) return format(value);
    }
    return null;
  };
  // A line cannot show an isolated observation. Draw just those samples,
  // preserving the null gaps instead of inventing a connected interval.
  const sampleDot = (key: string, color: string) => {
    const isolated = new Set<number>();
    data.forEach((row, index) => {
      if (
        row[key] != null &&
        data[index - 1]?.[key] == null &&
        data[index + 1]?.[key] == null
      )
        isolated.add(index);
    });
    if (!isolated.size) return false;
    return function IsolatedSample({ cx, cy, index }: DotItemDotProps) {
      return isolated.has(index) &&
        typeof cx === "number" &&
        typeof cy === "number" ? (
        <circle
          className="history-sample"
          data-series={key}
          cx={cx}
          cy={cy}
          r={3}
          fill={color}
        />
      ) : null;
    };
  };
  return (
    <div className="chart-block" data-testid={`chart-${title}`}>
      <div className="chart-title">
        <span>
          {title}{" "}
          <span className="font-normal text-muted-foreground">({unit})</span>
          {legend.length === 1 && (
            <span className="chart-latest">{latest(series[0].key) ?? "—"}</span>
          )}
        </span>
        {action}
      </div>
      {legend.length > 1 && (
        <div className="chart-legend" aria-label="Legend">
          {legend.map((s) => (
            <span key={s.key}>
              <i style={{ background: s.color }} />
              {s.label}
              <b>{latest(s.key) ?? "—"}</b>
            </span>
          ))}
        </div>
      )}
      <ChartContainer config={config} className={`${height} w-full`}>
        <ComposedChart
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
            width={66}
            tickCount={4}
            tickLine={false}
            axisLine={false}
            tickFormatter={(value: number) => format(value)}
          />
          <ChartTooltip
            content={
              <ChartTooltipContent
                labelFormatter={(_, payload) => {
                  const time = payload?.[0]?.payload?.time;
                  return typeof time === "number"
                    ? new Date(time).toLocaleString()
                    : "";
                }}
                formatter={(value, name, item) => (
                  <>
                    <span
                      className="inline-block size-2.5 shrink-0 rounded-[2px]"
                      style={{ background: item.color }}
                    />
                    <span className="text-muted-foreground">
                      {config[name as string]?.label ?? name}
                    </span>
                    <span className="ml-auto font-mono font-medium tabular-nums text-foreground">
                      {typeof value === "number"
                        ? format(value)
                        : String(value)}
                    </span>
                  </>
                )}
              />
            }
          />
          {series.map((s) =>
            kind === "line" ? (
              <Line
                key={s.key}
                dataKey={s.key}
                type="monotone"
                stroke={`var(--color-${s.key})`}
                strokeWidth={2}
                connectNulls={false}
                isAnimationActive={false}
                dot={sampleDot(s.key, `var(--color-${s.key})`)}
                activeDot={{ r: 4, strokeWidth: 2, stroke: "var(--card)" }}
              />
            ) : (
              <Area
                key={s.key}
                dataKey={s.key}
                type="monotone"
                stackId={kind === "stacked" ? "stack" : undefined}
                stroke={`var(--color-${s.key})`}
                strokeWidth={kind === "stacked" ? 1 : 2}
                fill={`var(--color-${s.key})`}
                fillOpacity={kind === "stacked" ? 0.35 : 0.1}
                connectNulls={false}
                isAnimationActive={false}
                dot={sampleDot(s.key, `var(--color-${s.key})`)}
                activeDot={{ r: 4, strokeWidth: 2, stroke: "var(--card)" }}
              />
            ),
          )}
          {total && (
            <Line
              dataKey={total.key}
              type="stepAfter"
              stroke="var(--muted-foreground)"
              strokeWidth={1.5}
              connectNulls={false}
              isAnimationActive={false}
              dot={sampleDot(total.key, "var(--muted-foreground)")}
              activeDot={false}
            />
          )}
        </ComposedChart>
      </ChartContainer>
    </div>
  );
}

function Picker({
  value,
  options,
  onChange,
  label,
}: {
  value: string;
  options: [string, string][];
  onChange: (value: string) => void;
  label: string;
}) {
  const name = options.find(([id]) => id === value)?.[1] ?? value;
  if (options.length <= 1)
    return (
      <span className="text-xs font-normal text-muted-foreground">{name}</span>
    );
  return (
    <Select value={value} onValueChange={(v) => v && onChange(v)}>
      <SelectTrigger size="sm" aria-label={label}>
        <SelectValue>{name}</SelectValue>
      </SelectTrigger>
      <SelectContent>
        <SelectGroup>
          {options.map(([id, label]) => (
            <SelectItem key={id} value={id}>
              {label}
            </SelectItem>
          ))}
        </SelectGroup>
      </SelectContent>
    </Select>
  );
}

/** Keeps at most eight series: the latest-highest ones, so identity never cycles hues. */
function topKeys(
  points: MetricPoint[],
  extract: (m: Metrics) => [string, string, number | null][],
): [string, string][] {
  const latest = new Map<string, { label: string; value: number }>();
  for (const point of points)
    for (const [id, label, value] of extract(point.metrics))
      if (value != null) latest.set(id, { label, value });
  return [...latest]
    .sort((a, b) => b[1].value - a[1].value || a[0].localeCompare(b[0]))
    .slice(0, 8)
    .map(([id, entry]) => [id, entry.label]);
}

function DeviceHistory({
  points,
  ...frame
}: Frame & { points: MetricPoint[] }) {
  const [choice, setChoice] = useState<string | null>(null);
  const options = new Map<string, string>();
  let root: string | null = null;
  for (const point of points)
    for (const device of point.metrics.disk_io) {
      const busy =
        device.busy_percent == null
          ? ""
          : ` · ${device.busy_percent.toFixed(0)}% busy`;
      options.set(
        device.device,
        `${device.device}${device.root ? " (root)" : ""}${busy}`,
      );
      if (device.root) root = device.device;
    }
  if (!options.size) return null;
  const selected =
    choice && options.has(choice) ? choice : (root ?? [...options.keys()][0]);
  const data = historySeries(points, frame.start, frame.end, (m) => {
    const device = m.disk_io.find((d) => d.device === selected);
    return {
      read: device?.read_bytes_per_second ?? null,
      write: device?.write_bytes_per_second ?? null,
    };
  });
  return (
    <ChartBlock
      title="Disk I/O"
      unit="per second"
      series={[
        { key: "read", label: "Read", color: SLOTS[0] },
        { key: "write", label: "Write", color: SLOTS[1] },
      ]}
      data={data}
      format={ratePerSecond}
      height="h-32"
      action={
        <Picker
          value={selected}
          options={[...options]}
          onChange={setChoice}
          label="Disk I/O device"
        />
      }
      {...frame}
    />
  );
}

function PoolHistory({ points, ...frame }: Frame & { points: MetricPoint[] }) {
  const [choice, setChoice] = useState<string | null>(null);
  const options = new Map<string, string>();
  const health = new Map<string, string>();
  let root: string | null = null;
  for (const point of points)
    for (const pool of point.metrics.pools) {
      options.set(
        pool.name,
        `${pool.name}${pool.root ? " (root)" : ""} · ${bytes(pool.allocated_bytes)} of ${bytes(pool.size_bytes)}`,
      );
      health.set(pool.name, pool.health);
      if (pool.root) root = pool.name;
    }
  if (!options.size) return null;
  const selected =
    choice && options.has(choice) ? choice : (root ?? [...options.keys()][0]);
  const pick = (m: Metrics) => m.pools.find((p) => p.name === selected);
  const state = health.get(selected) ?? "";
  const picker = (label: string) => (
    <span className="flex items-center gap-2">
      <span
        className="status-pill"
        data-tone={state.toUpperCase() === "ONLINE" ? "good" : "critical"}
      >
        {state}
      </span>
      <Picker
        value={selected}
        options={[...options]}
        onChange={setChoice}
        label={label}
      />
    </span>
  );
  return (
    <>
      <ChartBlock
        title="Pool usage"
        unit="%"
        series={[{ key: "used", label: "Allocated", color: SLOTS[0] }]}
        data={historySeries(points, frame.start, frame.end, (m) => {
          const pool = pick(m);
          return {
            used:
              pool && pool.size_bytes > 0
                ? (pool.allocated_bytes / pool.size_bytes) * 100
                : null,
          };
        })}
        domain={[0, 100]}
        format={percent}
        height="h-28"
        action={picker("Pool usage device")}
        {...frame}
      />
      <ChartBlock
        title="Pool I/O"
        unit="per second"
        series={[
          { key: "read", label: "Read", color: SLOTS[0] },
          { key: "write", label: "Write", color: SLOTS[1] },
        ]}
        data={historySeries(points, frame.start, frame.end, (m) => {
          const pool = pick(m);
          return {
            read: pool?.read_bytes_per_second ?? null,
            write: pool?.write_bytes_per_second ?? null,
          };
        })}
        format={ratePerSecond}
        height="h-28"
        action={picker("Pool I/O device")}
        {...frame}
      />
    </>
  );
}

function SensorHistory({
  points,
  ...frame
}: Frame & { points: MetricPoint[] }) {
  const [filter, setFilter] = useState("");
  const needle = filter.trim().toLowerCase();
  const total = new Set(
    points.flatMap((p) => p.metrics.temperatures.map((t) => t.sensor)),
  ).size;
  // The hottest eight matching sensors keep the palette within its validated slots.
  const keys = topKeys(points, (m) =>
    m.temperatures
      .filter(
        (t) =>
          !needle ||
          t.label.toLowerCase().includes(needle) ||
          t.sensor.toLowerCase().includes(needle),
      )
      .map((t) => [t.sensor, t.label, t.celsius]),
  );
  if (!total) return null;
  const data = historySeries(points, frame.start, frame.end, (m) =>
    Object.fromEntries(
      keys.map(([id], index) => [
        `s${index}`,
        m.temperatures.find((t) => t.sensor === id)?.celsius ?? null,
      ]),
    ),
  );
  return (
    <ChartBlock
      title="Temperatures"
      unit="°C"
      kind="line"
      series={keys.map(([, label], index) => ({
        key: `s${index}`,
        label,
        color: SLOTS[index],
      }))}
      data={data}
      format={celsius}
      height="h-36"
      action={
        <span className="flex items-center gap-2 text-xs font-normal text-muted-foreground">
          {total > keys.length &&
            !needle &&
            `Hottest ${keys.length} of ${total}`}
          {total > 1 && (
            <input
              className="chart-filter"
              aria-label="Filter sensors"
              placeholder="Filter sensors…"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
          )}
        </span>
      }
      {...frame}
    />
  );
}

function MonitorHistory({
  points,
  ...frame
}: Frame & { points: MetricPoint[] }) {
  const keys = topKeys(points, (m) =>
    m.monitors.map((r) => [r.target, r.target, r.latency_ms]),
  );
  if (!keys.length) return null;
  const data = historySeries(points, frame.start, frame.end, (m) =>
    Object.fromEntries(
      keys.map(([id], index) => [
        `m${index}`,
        m.monitors.find((r) => r.target === id)?.latency_ms ?? null,
      ]),
    ),
  );
  return (
    <ChartBlock
      title="Monitor latency"
      unit="ms"
      kind="line"
      series={keys.map(([, label], index) => ({
        key: `m${index}`,
        label: label.replace(/^[a-z]+:\/\//, ""),
        color: SLOTS[index],
      }))}
      data={data}
      format={millis}
      height="h-32"
      {...frame}
    />
  );
}

function ResourceHistory({
  points,
  ...frame
}: Frame & { points: MetricPoint[] }) {
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
        (d) =>
          [
            diskKey(d),
            `${d.mount} · ${d.name} · ${
              d.total_bytes > 0
                ? `${bytes(d.total_bytes - d.available_bytes)} of ${bytes(d.total_bytes)}`
                : "size unknown"
            }`,
          ] as const,
      ),
    ),
  );
  const rootKey = [...diskOptions.keys()].find(
    (key) => JSON.parse(key)[1] === "/",
  );
  const gpuId =
    gpuChoice && gpuOptions.has(gpuChoice)
      ? gpuChoice
      : [...gpuOptions.keys()][0];
  const diskId =
    diskChoice && diskOptions.has(diskChoice)
      ? diskChoice
      : (rootKey ?? [...diskOptions.keys()][0]);
  const gpuData = historySeries(points, frame.start, frame.end, (metrics) => {
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
  const diskData = historySeries(points, frame.start, frame.end, (metrics) => {
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
          series={[{ key: "used", label: "Used", color: SLOTS[0] }]}
          data={diskData}
          domain={[0, 100]}
          format={percent}
          height="h-28"
          action={
            <Picker
              value={diskId}
              options={[...diskOptions]}
              onChange={setDiskChoice}
              label="Filesystem history device"
            />
          }
          {...frame}
        />
      )}
      {gpuOptions.size > 0 &&
        (gpuData.some(
          (row) => row.utilization != null || row.memory != null,
        ) ? (
          <ChartBlock
            title="GPU utilization and memory"
            unit="%"
            series={[
              { key: "utilization", label: "GPU", color: SLOTS[0] },
              { key: "memory", label: "Memory", color: SLOTS[1] },
            ]}
            data={gpuData}
            domain={[0, 100]}
            format={percent}
            height="h-28"
            action={
              <Picker
                value={gpuId}
                options={[...gpuOptions].map(([id, name]) => [
                  id,
                  `${name} · ${id.slice(-8)}`,
                ])}
                onChange={setGpuChoice}
                label="GPU history device"
              />
            }
            {...frame}
          />
        ) : (
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-xs text-muted-foreground">
              GPU metrics are unavailable for this device in this window.
            </p>
            <Picker
              value={gpuId}
              options={[...gpuOptions].map(([id, name]) => [
                id,
                `${name} · ${id.slice(-8)}`,
              ])}
              onChange={setGpuChoice}
              label="GPU history device"
            />
          </div>
        ))}
    </>
  );
}

export { memoryPercent };
