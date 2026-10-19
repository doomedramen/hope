import { hostState } from "@/lib/fleet";

export async function api<T>(
  path: string,
  options: RequestInit = {},
): Promise<T> {
  const headers = new Headers(options.headers);
  if (!headers.has("Content-Type"))
    headers.set("Content-Type", "application/json");
  const response = await fetch(`/api/v1${path}`, {
    ...options,
    credentials: "same-origin",
    cache: "no-store",
    headers,
  });
  if (!response.ok) {
    if (response.status === 401 && !path.startsWith("/auth/"))
      window.dispatchEvent(new Event("hope:session-expired"));
    const body: unknown = await response.json().catch(() => null);
    const message =
      body !== null &&
      typeof body === "object" &&
      "error" in body &&
      typeof body.error === "string" &&
      body.error.trim()
        ? body.error
        : `Request failed (${response.status})`;
    throw Object.assign(new Error(message), { status: response.status });
  }
  return response.json();
}
export const post = <T>(path: string, data: unknown = {}) =>
  api<T>(path, { method: "POST", body: JSON.stringify(data) });
export const put = <T>(path: string, data: unknown) =>
  api<T>(path, { method: "PUT", body: JSON.stringify(data) });
export const del = <T>(path: string) => api<T>(path, { method: "DELETE" });
export function bytes(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return "Unavailable";
  if (value === 0) return "0 B";
  const index = Math.max(
    0,
    Math.min(4, Math.floor(Math.log(value) / Math.log(1024))),
  );
  return `${(value / 1024 ** index).toFixed(index ? 1 : 0)} ${["B", "KiB", "MiB", "GiB", "TiB"][index]}`;
}
/** Throughput as "12.3 KiB/s" or "—" when the counter is unavailable. */
export function rate(value: number | null | undefined): string {
  return value == null || !Number.isFinite(value) ? "—" : `${bytes(value)}/s`;
}
export function age(
  value: number | null | undefined,
  now = Date.now() / 1000,
): string {
  if (!value) return "Never";
  const seconds = Math.max(0, Math.floor(now - value));
  return seconds < 5
    ? "Just now"
    : seconds < 60
      ? `${seconds}s ago`
      : seconds < 3600
        ? `${Math.floor(seconds / 60)}m ago`
        : seconds < 86400
          ? `${Math.floor(seconds / 3600)}h ago`
          : `${Math.floor(seconds / 86400)}d ago`;
}
/** Observed uptime, without advancing stale reports using the browser clock. */
export function uptimeDuration(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value) || value < 0)
    return "Unavailable";
  const seconds = Math.floor(value);
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  return hours >= 24
    ? `${Math.floor(hours / 24)}d ${hours % 24}h`
    : hours > 0
      ? `${hours}h ${minutes % 60}m`
      : minutes > 0
        ? `${minutes}m`
        : `${seconds}s`;
}
export function percentage(value: number | null | undefined) {
  return value == null || !Number.isFinite(value)
    ? "—"
    : `${value.toFixed(1)}%`;
}
/** Whole-number percentage for dense tables and tiles. */
export function percent0(value: number | null | undefined) {
  return value == null || !Number.isFinite(value)
    ? "—"
    : `${Math.round(value)}%`;
}
export function status(host: {
  agent_id: string | null;
  agent_revoked?: boolean;
  agent_uninstalled?: boolean;
  last_report: number | null;
  last_seen: number;
}) {
  return hostState({
    agent_revoked: false,
    agent_uninstalled: false,
    source: null,
    lifecycle: null,
    source_observed_at: null,
    ...host,
  }).label;
}
