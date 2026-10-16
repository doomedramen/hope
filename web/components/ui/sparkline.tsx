"use client";

export type SparkPoint = { time: number; value: number | null };

/**
 * A tiny trend line. Gaps in the data stay gaps: the path breaks wherever a
 * value is missing, so an outage is never drawn as a flat line.
 */
export function Sparkline({
  points,
  max,
  color,
  className,
  label,
}: {
  points: SparkPoint[];
  /** Fixed upper bound (for percentages); otherwise the series maximum. */
  max?: number;
  color?: string;
  className?: string;
  label?: string;
}) {
  const width = 100;
  const height = 32;
  const times = points.map((p) => p.time);
  const start = Math.min(...times);
  const end = Math.max(...times);
  const span = end - start || 1;
  const values = points.flatMap((p) => (p.value == null ? [] : [p.value]));
  const top = max ?? Math.max(1e-9, ...values);
  const x = (time: number) => ((time - start) / span) * width;
  const y = (value: number) =>
    height - 2 - (Math.min(Math.max(value, 0), top) / top) * (height - 4);
  const segments: string[] = [];
  const areas: string[] = [];
  let current: string[] = [];
  let currentStart: number | null = null;
  let last: SparkPoint | null = null;
  const flush = () => {
    if (current.length > 1 && currentStart != null && last?.value != null) {
      segments.push(`M${current.join("L")}`);
      areas.push(
        `M${x(currentStart)},${height}L${current.join("L")}L${x(last.time)},${height}Z`,
      );
    }
    current = [];
    currentStart = null;
  };
  for (const point of points) {
    if (point.value == null) {
      flush();
      continue;
    }
    if (currentStart == null) currentStart = point.time;
    current.push(`${x(point.time).toFixed(2)},${y(point.value).toFixed(2)}`);
    last = point;
  }
  flush();
  const end_point = [...points].reverse().find((p) => p.value != null);
  if (!values.length)
    return (
      <svg
        className={`sparkline ${className ?? ""}`}
        viewBox={`0 0 ${width} ${height}`}
        preserveAspectRatio="none"
        aria-hidden="true"
      />
    );
  return (
    <svg
      className={`sparkline ${className ?? ""}`}
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      style={color ? ({ "--spark": color } as React.CSSProperties) : undefined}
    >
      {areas.map((d, i) => (
        <path key={`a${i}`} className="area" d={d} />
      ))}
      {segments.map((d, i) => (
        <path key={`l${i}`} className="line" d={d} />
      ))}
      {end_point && end_point.value != null && (
        <circle
          cx={x(end_point.time)}
          cy={y(end_point.value)}
          r={2.6}
          vectorEffect="non-scaling-stroke"
        />
      )}
    </svg>
  );
}
