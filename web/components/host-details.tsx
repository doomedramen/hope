"use client";
import { useState } from "react";
import {
  Activity,
  ArrowDownToLine,
  Bot,
  Cpu,
  GitMerge,
  HardDrive,
  MemoryStick,
  Network,
  Radio,
  ShieldOff,
  Waypoints,
  X,
  Zap,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectGroup,
  SelectItem,
} from "@/components/ui/select";
import { Meter } from "@/components/ui/meter";
import { Sparkline } from "@/components/ui/sparkline";
import { StatusPill } from "@/components/status-pill";
import { HostIcon } from "@/components/host-table";
import {
  HistoryRange,
  MetricHistory,
  useHistory,
  type HistoryWindow,
} from "@/components/metric-history";
import { age, bytes, percent0, percentage, post, rate } from "@/lib/api";
import { historySeries, memoryPercent } from "@/lib/history";
import {
  cpuPercent,
  diskPercent,
  hostIssues,
  hostState,
  memoryUsage,
  usageTone,
  worstDisk,
  type Tone,
} from "@/lib/fleet";
import type { Host } from "@/lib/protocol";

export function HostDetails({
  host,
  hosts,
  close,
  install,
  refresh,
  notify,
}: {
  host: Host;
  hosts: Host[];
  close: () => void;
  install: () => void;
  refresh: () => void;
  notify: (message: string, tone?: Tone) => void;
}) {
  const [error, setError] = useState("");
  const [source, setSource] = useState<string | null>(null);
  const [confirmRevoke, setConfirmRevoke] = useState(false);
  const [hours, setHours] = useState<HistoryWindow>("1");
  const inventory = host.inventory;
  const metrics = host.metrics;
  const state = hostState(host);
  const issues = hostIssues(host);
  const history = useHistory(host.id, hours, host.last_report);
  const series = historySeries(
    history.points ?? [],
    history.windowStart,
    history.windowEnd,
    (m) => ({
      cpu: m.cpu_percent,
      memory: memoryPercent(m),
      disk: (() => {
        const worst = m.disks
          .map(diskPercent)
          .filter((value): value is number => value != null);
        return worst.length ? Math.max(...worst) : null;
      })(),
      net:
        m.network_rx_bytes_per_second == null &&
        m.network_tx_bytes_per_second == null
          ? null
          : (m.network_rx_bytes_per_second ?? 0) +
            (m.network_tx_bytes_per_second ?? 0),
    }),
  );
  const spark = (key: "cpu" | "memory" | "disk" | "net") =>
    series.map((row) => ({ time: row.time ?? 0, value: row[key] ?? null }));
  const cpu = cpuPercent(host);
  const memory = memoryUsage(host);
  const disk = worstDisk(host);
  async function revoke() {
    try {
      await post(`/agents/${host.agent_id}/revoke`);
      setConfirmRevoke(false);
      notify(`Revoked the agent credential for ${host.name}.`, "warning");
      refresh();
    } catch (e) {
      setError((e as Error).message);
    }
  }
  async function merge() {
    if (!source) return;
    const merged = hosts.find((h) => h.id === source);
    try {
      await post(`/hosts/${host.id}/merge`, { source_id: source });
      setSource(null);
      notify(`Merged ${merged?.name ?? "host"} into ${host.name}.`, "good");
      refresh();
    } catch (e) {
      setError((e as Error).message);
    }
  }
  return (
    <aside className="host-details" aria-label="Host details">
      <header className="details-header">
        <div className="flex items-start gap-3">
          <span className="host-avatar" data-tone={state.tone}>
            <HostIcon host={host} size={17} />
          </span>
          <div className="min-w-0 flex-1">
            <h2 className="truncate text-lg font-semibold tracking-tight">
              {host.name}
            </h2>
            <p className="mono mt-0.5 break-all text-muted-foreground">
              {host.addresses.join(" · ")}
            </p>
          </div>
          <Button
            variant="ghost"
            size="icon"
            aria-label="Close host details"
            onClick={close}
          >
            <X />
          </Button>
        </div>
        <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
          <StatusPill host={host} />
          {host.agent_id && !host.agent_revoked && (
            <Badge variant="outline">
              <Bot data-icon="inline-start" />
              Agent
            </Badge>
          )}
          <span>
            {host.agent_id && host.last_report
              ? `Reported ${age(host.last_report)}`
              : `Seen ${age(host.last_seen)}`}
          </span>
        </div>
      </header>
      <div className="details-body">
        {error && (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}
        {issues.length > 0 && (
          <div className="flex flex-col gap-1.5">
            {issues.map((issue) => (
              <div
                key={issue.kind}
                className={`flex items-center gap-2 text-xs font-medium tone-${issue.severity}`}
              >
                <span
                  className="status-dot"
                  data-tone={issue.severity}
                  aria-hidden="true"
                />
                {issue.message}
              </div>
            ))}
          </div>
        )}
        {!host.agent_id && (
          <div className="rounded-xl border border-dashed p-3.5">
            <p className="mb-2.5 text-xs leading-relaxed text-muted-foreground">
              Discovery can only see what answers on the network. Install the
              agent for hardware, storage, GPU and live usage with history.
            </p>
            <Button size="sm" onClick={install}>
              <ArrowDownToLine data-icon="inline-start" />
              Install reporting agent
            </Button>
          </div>
        )}
        {host.agent_revoked && (
          <Alert>
            <AlertDescription>
              This agent&rsquo;s credential was revoked. Generate a new
              installation command to enroll it again.
              <div className="mt-2">
                <Button size="sm" variant="outline" onClick={install}>
                  Re-enroll agent
                </Button>
              </div>
            </AlertDescription>
          </Alert>
        )}
        {metrics && (
          <section>
            <div className="detail-heading">
              <Activity />
              Live usage
              <span className="spacer" />
              <HistoryRange value={hours} onChange={setHours} />
            </div>
            <div className="tile-grid">
              <MiniTile
                label="CPU"
                value={percent0(cpu)}
                percent={cpu}
                spark={spark("cpu")}
                max={100}
                detail={inventory ? `${inventory.cpu_cores} cores` : undefined}
              />
              <MiniTile
                label="Memory"
                value={percent0(memory)}
                percent={memory}
                spark={spark("memory")}
                max={100}
                detail={
                  metrics.memory_used_bytes != null
                    ? `${bytes(metrics.memory_used_bytes)} of ${bytes(metrics.memory_total_bytes)}`
                    : "Unavailable"
                }
              />
              <MiniTile
                label="Disk"
                value={percent0(disk?.percent)}
                percent={disk?.percent ?? null}
                spark={spark("disk")}
                max={100}
                detail={disk ? `${disk.disk.mount} fullest` : "Unavailable"}
              />
              <MiniTile
                label="Network"
                value={rate(
                  metrics.network_rx_bytes_per_second == null &&
                    metrics.network_tx_bytes_per_second == null
                    ? null
                    : (metrics.network_rx_bytes_per_second ?? 0) +
                        (metrics.network_tx_bytes_per_second ?? 0),
                )}
                spark={spark("net")}
                color="var(--chart-3)"
                detail={`↓ ${rate(metrics.network_rx_bytes_per_second)} · ↑ ${rate(metrics.network_tx_bytes_per_second)}`}
              />
            </div>
            <p className="text-xs text-muted-foreground">
              Collected {age(host.last_collected_at)}; received{" "}
              {age(host.last_report)}.
            </p>
          </section>
        )}
        <section>
          <h3 className="detail-heading">
            <Cpu />
            Hardware
          </h3>
          {inventory ? (
            <dl className="detail-list">
              <dt>System</dt>
              <dd>{inventory.os}</dd>
              <dt>Hostname</dt>
              <dd className="mono">{inventory.hostname}</dd>
              <dt>Architecture</dt>
              <dd>{inventory.architecture}</dd>
              <dt>CPU</dt>
              <dd>
                {inventory.cpu_model} · {inventory.cpu_cores} cores
              </dd>
              <dt>Memory</dt>
              <dd>{bytes(inventory.memory_total_bytes)}</dd>
            </dl>
          ) : (
            <dl className="detail-list">
              <dt>Discovered by</dt>
              <dd>{host.discovery_method}</dd>
              <dt>Enrichment</dt>
              <dd>{host.enrichment_status}</dd>
              <dt>MAC</dt>
              <dd className="mono">{host.macs.join(", ") || "Unavailable"}</dd>
            </dl>
          )}
        </section>
        {(metrics?.disks ?? inventory?.disks ?? []).length > 0 && (
          <section>
            <h3 className="detail-heading">
              <HardDrive />
              Storage
              <span className="spacer" />
              {metrics &&
                (metrics.disk_read_bytes_per_second != null ||
                  metrics.disk_write_bytes_per_second != null) && (
                  <span className="text-xs font-normal text-muted-foreground">
                    R {rate(metrics.disk_read_bytes_per_second)} · W{" "}
                    {rate(metrics.disk_write_bytes_per_second)}
                  </span>
                )}
            </h3>
            {metrics && !metrics.disks.length && !!inventory?.disks.length && (
              <p className="text-xs text-muted-foreground">
                Filesystem metrics are unavailable; showing the last reported
                mounts.
              </p>
            )}
            {(metrics?.disks.length ? metrics.disks : (inventory?.disks ?? []))
              .map((d) => ({ disk: d, percent: diskPercent(d) }))
              .sort((a, b) => (b.percent ?? -1) - (a.percent ?? -1))
              .map(({ disk: d, percent }) => (
                <div key={`${d.name}-${d.mount}`} className="storage-row">
                  <div className="label">
                    <span className="truncate">
                      <span className="mono font-semibold">{d.mount}</span>
                      <span className="ml-2 text-muted-foreground">
                        {d.name} · {d.filesystem}
                      </span>
                    </span>
                    <span>
                      {percent != null
                        ? `${bytes(d.total_bytes - d.available_bytes)} of ${bytes(d.total_bytes)}`
                        : "Usage unavailable"}
                    </span>
                  </div>
                  <Meter
                    value={percent}
                    label={`${d.mount} storage used`}
                    tone={usageTone(percent)}
                  />
                </div>
              ))}
          </section>
        )}
        <section>
          <h3 className="detail-heading">
            <Network />
            Network
          </h3>
          {inventory && (
            <dl className="detail-list">
              <dt>MAC</dt>
              <dd className="mono">{host.macs.join(", ") || "Unavailable"}</dd>
              <dt>Discovered by</dt>
              <dd>{host.discovery_method}</dd>
            </dl>
          )}
          {inventory?.interfaces.map((i) => (
            <div key={i.name} className="interface-row">
              <strong className="mono">
                {i.name}
                {i.mac && (
                  <span className="ml-2 font-normal text-muted-foreground">
                    {i.mac}
                  </span>
                )}
              </strong>
              <span>{i.addresses.join(", ") || "No address"}</span>
              {i.gateway && <span>Gateway {i.gateway}</span>}
            </div>
          ))}
          {(metrics?.network_interfaces.length ?? 0) > 0 && (
            <details className="text-xs">
              <summary className="cursor-pointer text-muted-foreground">
                Interface counters (
                {metrics?.network_interfaces
                  .filter((i) => i.included_in_total)
                  .map((i) => i.name)
                  .join(", ") || "none"}{" "}
                in totals)
              </summary>
              {metrics?.network_interfaces.map((i) => (
                <div className="interface-row" key={`counter:${i.name}`}>
                  <strong className="mono">{i.name}</strong>
                  <span>
                    ↓ {rate(i.rx_bytes_per_second)} · ↑{" "}
                    {rate(i.tx_bytes_per_second)}
                  </span>
                  <span>
                    Total RX {bytes(i.received_bytes)} · TX{" "}
                    {bytes(i.transmitted_bytes)}
                  </span>
                </div>
              ))}
            </details>
          )}
        </section>
        <section>
          <h3 className="detail-heading">
            <Radio />
            Observed services
          </h3>
          {host.services.length ? (
            <div className="flex flex-wrap gap-1.5">
              {[...host.services]
                .sort((a, b) => a.port - b.port)
                .map((s) => (
                  <span key={s.port} className="service-chip">
                    <b>{s.port}</b>
                    {s.name}
                  </span>
                ))}
            </div>
          ) : (
            <p className="text-sm text-muted-foreground">
              No open services observed yet.
            </p>
          )}
        </section>
        {(metrics?.gpus ?? inventory?.gpus ?? []).length > 0 && (
          <section>
            <h3 className="detail-heading">
              <Zap />
              Graphics
            </h3>
            {(metrics?.gpus ?? inventory?.gpus ?? []).map((g, i) => (
              <div key={g.id ?? `${g.name}-${i}`} className="interface-row">
                <strong>{g.name}</strong>
                <span>
                  Utilization {percentage(g.utilization_percent)} · VRAM{" "}
                  {bytes(g.memory_used_bytes)}
                  {g.memory_total_bytes != null &&
                    ` of ${bytes(g.memory_total_bytes)}`}
                </span>
                <span>
                  {g.temperature_celsius == null
                    ? "Temperature unavailable"
                    : `${g.temperature_celsius.toFixed(0)}°C`}
                </span>
              </div>
            ))}
          </section>
        )}
        {host.agent_id && (
          <>
            <section>
              <h3 className="detail-heading">
                <Waypoints />
                History
                <span className="spacer" />
                {!metrics && <HistoryRange value={hours} onChange={setHours} />}
              </h3>
              <MetricHistory
                points={history.points}
                error={history.error}
                hours={hours}
                windowStart={history.windowStart}
                windowEnd={history.windowEnd}
              />
            </section>
            {host.collectors.some((c) => !c.available) && (
              <section>
                <h3 className="detail-heading">
                  <MemoryStick />
                  Unavailable collectors
                </h3>
                {host.collectors
                  .filter((c) => !c.available)
                  .map((c) => (
                    <p key={c.name} className="text-xs text-muted-foreground">
                      <strong className="text-foreground">
                        {c.name.replaceAll("_", " ")}
                      </strong>
                      : {c.detail}
                    </p>
                  ))}
              </section>
            )}
            <section>
              <h3 className="detail-heading">
                <GitMerge />
                Reconcile a discovered host
              </h3>
              <p className="text-xs text-muted-foreground">
                Merge a duplicate discovery into this agent after verifying it
                is the same machine. History and map links are preserved.
              </p>
              <div className="flex gap-2">
                <Select value={source} onValueChange={setSource}>
                  <SelectTrigger className="min-w-0 flex-1">
                    <SelectValue placeholder="Choose a discovered host" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      {hosts
                        .filter((h) => !h.agent_id && h.id !== host.id)
                        .map((h) => (
                          <SelectItem key={h.id} value={h.id}>
                            {h.name} ({h.addresses[0]})
                          </SelectItem>
                        ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
                <Button variant="outline" disabled={!source} onClick={merge}>
                  Merge
                </Button>
              </div>
            </section>
            {!host.agent_revoked && (
              <section className="danger-zone">
                <h3 className="detail-heading">
                  <ShieldOff />
                  Agent access
                </h3>
                {confirmRevoke ? (
                  <Alert>
                    <AlertDescription>
                      This agent will no longer be able to send reports. History
                      stays; a new installation command re-enrolls it.
                      <div className="mt-3 flex gap-2">
                        <Button variant="destructive" onClick={revoke}>
                          Revoke credential
                        </Button>
                        <Button
                          variant="outline"
                          onClick={() => setConfirmRevoke(false)}
                        >
                          Cancel
                        </Button>
                      </div>
                    </AlertDescription>
                  </Alert>
                ) : (
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <p className="text-xs text-muted-foreground">
                      Stop accepting reports from this agent.
                    </p>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => setConfirmRevoke(true)}
                    >
                      <ShieldOff data-icon="inline-start" />
                      Revoke agent access
                    </Button>
                  </div>
                )}
              </section>
            )}
          </>
        )}
      </div>
    </aside>
  );
}

function MiniTile({
  label,
  value,
  percent,
  spark,
  max,
  color,
  detail,
}: {
  label: string;
  value: string;
  percent?: number | null;
  spark: { time: number; value: number | null }[];
  max?: number;
  color?: string;
  detail?: string;
}) {
  return (
    <div
      className="mini-tile"
      data-tone={percent != null ? usageTone(percent) : undefined}
    >
      <header>
        <span>{label}</span>
      </header>
      <div className="value">{value}</div>
      <div className="spark">
        <Sparkline points={spark} max={max} color={color} />
      </div>
      {detail && (
        <div className="truncate text-[11px] text-muted-foreground">
          {detail}
        </div>
      )}
    </div>
  );
}
