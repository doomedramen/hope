import type { MetricPoint, Metrics } from "@/lib/protocol";

export function memoryPercent(metrics: Metrics): number | null {
  return metrics.memory_used_bytes != null &&
    metrics.memory_total_bytes != null &&
    metrics.memory_total_bytes > 0
    ? (metrics.memory_used_bytes / metrics.memory_total_bytes) * 100
    : null;
}

/** Null boundaries preserve outages at both ends as well as between samples. */
export function historySeries(
  points: MetricPoint[],
  start: number,
  end: number,
  select: (metrics: Metrics) => Record<string, number | null>,
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
