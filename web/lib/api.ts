import { hostState } from "@/lib/fleet";

export async function api<T>(
  path: string,
  options: RequestInit = {},
): Promise<T> {
  const response = await fetch(`/api/v1${path}`, {
    ...options,
    credentials: "same-origin",
    cache: "no-store",
    headers: { "Content-Type": "application/json", ...options.headers },
  });
  if (!response.ok) {
    if (response.status === 401 && !path.startsWith("/auth/"))
      window.dispatchEvent(new Event("hope:session-expired"));
    const body = await response
      .json()
      .catch(() => ({ error: `Request failed (${response.status})` }));
    throw new Error(body.error || `Request failed (${response.status})`);
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
  last_report: number | null;
  last_seen: number;
}) {
  return hostState({ agent_revoked: false, ...host }).label;
}
