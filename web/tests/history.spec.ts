import { test, expect } from "@playwright/test";
import { historySeries, memoryPercent } from "../lib/history";
import type { Metrics, MetricPoint } from "../lib/protocol";
const metrics: Metrics = {
  cpu_percent: 20,
  memory_used_bytes: null,
  memory_total_bytes: 100,
  network_rx_bytes_per_second: null,
  network_tx_bytes_per_second: null,
  network_interfaces: [],
  memory_cached_bytes: null,
  memory_zfs_arc_bytes: null,
  swap_used_bytes: null,
  swap_total_bytes: null,
  load_average: null,
  disk_read_bytes_per_second: null,
  disk_write_bytes_per_second: null,
  disk_io: [],
  pools: [],
  temperatures: [],
  services: [],
  service_summary: null,
  monitors: [],
  containers: [],
  disks: [],
  gpus: [],
};
test("missing memory stays unavailable and history has leading, internal and trailing gaps", () => {
  expect(memoryPercent(metrics)).toBeNull();
  expect(memoryPercent({ ...metrics, memory_used_bytes: 0 })).toBe(0);
  const points: MetricPoint[] = [100, 500].map((collected_at) => ({
    collected_at,
    metrics,
    resolution_seconds: 30,
  }));
  const rows = historySeries(points, 0, 1000, (m) => ({
    cpu: m.cpu_percent,
    memory: memoryPercent(m),
  }));
  expect(rows.map((row) => row.time)).toEqual([
    0, 100000, 130000, 499000, 500000, 530000, 1000000,
  ]);
  expect(rows.every((row) => row.memory == null)).toBeTruthy();
  expect(rows[0].cpu).toBeNull();
  expect(rows.at(-1)?.cpu).toBeNull();
});
test("overlapping aggregate remains visible at window boundary", () => {
  const rows = historySeries(
    [{ collected_at: 0, resolution_seconds: 300, metrics }],
    100,
    400,
    (m) => ({ cpu: m.cpu_percent }),
  );
  expect(
    rows.some((row) => row.time === 100000 && row.cpu === 20),
  ).toBeTruthy();
});
