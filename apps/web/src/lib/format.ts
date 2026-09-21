export function shortId(id: string): string {
  return id.slice(0, 8);
}

export function labelize(value: unknown): string {
  const text = value === null || value === undefined ? "Unknown" : String(value);
  return text
    .replaceAll("_", " ")
    .replaceAll("-", " ")
    .replaceAll(".", " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

export function formatDate(value: string | null | undefined): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "—";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

export function formatRelative(value: string | null | undefined): string {
  if (!value) return "—";
  const seconds = Math.round((new Date(value).getTime() - Date.now()) / 1000);
  const absolute = Math.abs(seconds);
  const unit =
    absolute < 60
      ? "second"
      : absolute < 3600
        ? "minute"
        : absolute < 86400
          ? "hour"
          : "day";
  const divisor =
    unit === "second"
      ? 1
      : unit === "minute"
        ? 60
        : unit === "hour"
          ? 3600
          : 86400;
  return new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }).format(
    Math.round(seconds / divisor),
    unit,
  );
}

export function displayValue(value: unknown): string {
  if (typeof value === "string") return value;
  if (value === null || value === undefined) return "—";
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

export function formatEvidenceValue(value: unknown): string {
  return formatReadableValue(value);
}

function formatReadableValue(value: unknown, depth = 0): string {
  if (value === null || value === undefined || value === "") return "—";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") {
    return String(value);
  }
  if (depth >= 3) {
    return Array.isArray(value)
      ? `${value.length} items`
      : `${Object.keys(value as Record<string, unknown>).length} fields`;
  }
  if (Array.isArray(value)) {
    return value.map((item) => formatReadableValue(item, depth + 1)).join(", ");
  }
  if (typeof value === "object") {
    return Object.entries(value as Record<string, unknown>)
      .map(
        ([key, item]) =>
          `${labelize(key)}: ${formatReadableValue(item, depth + 1)}`,
      )
      .join(" · ");
  }
  return String(value);
}
