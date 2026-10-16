"use client";
import { useState } from "react";
import {
  ArrowDownToLine,
  ArrowLeft,
  Maximize2,
  Bot,
  Cpu,
  GitMerge,
  MemoryStick,
  Network,
  Radio,
  ShieldOff,
  Waypoints,
  Wifi,
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
import { StatusPill } from "@/components/status-pill";
import { HostIcon } from "@/components/host-table";
import {
  HistoryRange,
  MetricHistory,
  useHistory,
  type HistoryWindow,
} from "@/components/metric-history";
import { age, bytes, post, rate } from "@/lib/api";
import { hostIssues, hostState, type Tone } from "@/lib/fleet";
import type { Host } from "@/lib/protocol";

export function HostDetails({
  host,
  hosts,
  close,
  expand,
  wide = false,
  install,
  refresh,
  notify,
}: {
  host: Host;
  hosts: Host[];
  close: () => void;
  /** Opens the full-page view; absent on the page itself. */
  expand?: () => void;
  /** Full-page layout: facts on the left, charts in a two-column grid. */
  wide?: boolean;
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
    <aside
      className={`host-details${wide ? " host-page" : ""}`}
      aria-label="Host details"
    >
      <header className="details-header">
        <div className="flex items-start gap-3">
          {wide && (
            <Button
              variant="outline"
              size="sm"
              aria-label="Close host details"
              onClick={close}
            >
              <ArrowLeft data-icon="inline-start" />
              Back
            </Button>
          )}
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
          {!wide && expand && (
            <Button
              variant="ghost"
              size="icon"
              aria-label="Open host page"
              onClick={expand}
            >
              <Maximize2 />
            </Button>
          )}
          {!wide && (
            <Button
              variant="ghost"
              size="icon"
              aria-label="Close host details"
              onClick={close}
            >
              <X />
            </Button>
          )}
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
              ? `Reported ${age(host.last_report)} · collected ${age(host.last_collected_at)}`
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
        {host.agent_id && (
          <section className="history">
            <h3 className="detail-heading">
              <Waypoints />
              History
              <span className="spacer" />
              <HistoryRange value={hours} onChange={setHours} />
            </h3>
            <MetricHistory
              points={history.points}
              error={history.error}
              hours={hours}
              windowStart={history.windowStart}
              windowEnd={history.windowEnd}
            />
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
        {metrics?.service_summary && (
          <section data-testid="service-units">
            <h3 className="detail-heading">
              <Zap />
              Service units
              <span className="spacer" />
              <span className="text-xs font-normal text-muted-foreground">
                {metrics.service_summary.active} active ·{" "}
                <span
                  className={
                    metrics.service_summary.failed ? "tone-critical" : ""
                  }
                >
                  {metrics.service_summary.failed} failed
                </span>{" "}
                of {metrics.service_summary.total}
              </span>
            </h3>
            {metrics.services.length ? (
              <div className="flex flex-col">
                {metrics.services.map((unit) => (
                  <div key={unit.name} className="interface-row row">
                    <span className="min-w-0">
                      <strong className="mono block truncate">
                        {unit.name}
                      </strong>
                      <span className="block truncate">{unit.description}</span>
                    </span>
                    <span
                      className="status-pill shrink-0"
                      data-tone={
                        unit.state === "active"
                          ? "good"
                          : unit.state === "failed"
                            ? "critical"
                            : "warning"
                      }
                    >
                      {unit.state}
                      {unit.sub_state !== unit.state && ` · ${unit.sub_state}`}
                    </span>
                  </div>
                ))}
              </div>
            ) : (
              <p className="text-xs text-muted-foreground">
                No failed units. Add names to HOPE_WATCH_SERVICES on the agent
                to always show them here.
              </p>
            )}
          </section>
        )}
        {(metrics?.monitors.length ?? 0) > 0 && (
          <section data-testid="monitors">
            <h3 className="detail-heading">
              <Wifi />
              Network monitors
            </h3>
            <div className="flex flex-col">
              {metrics?.monitors.map((monitor) => (
                <div key={monitor.target} className="interface-row row">
                  <span className="min-w-0">
                    <strong className="mono block truncate">
                      {monitor.target}
                    </strong>
                    <span className="block truncate">
                      {monitor.up
                        ? `${monitor.latency_ms != null ? `${monitor.latency_ms.toFixed(0)} ms` : "reachable"}${monitor.status_code ? ` · HTTP ${monitor.status_code}` : ""}`
                        : (monitor.detail ?? "unreachable")}
                    </span>
                  </span>
                  <span
                    className="status-pill shrink-0"
                    data-tone={monitor.up ? "good" : "critical"}
                  >
                    {monitor.up ? "Up" : "Down"}
                  </span>
                </div>
              ))}
            </div>
          </section>
        )}
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
            <div className="flex flex-wrap gap-1.5">
              {(metrics?.gpus ?? inventory?.gpus ?? []).map((g, i) => (
                <span key={g.id ?? `${g.name}-${i}`} className="service-chip">
                  {g.name}
                  {g.temperature_celsius != null && (
                    <b>{g.temperature_celsius.toFixed(0)}°C</b>
                  )}
                </span>
              ))}
            </div>
          </section>
        )}
        {host.agent_id && (
          <>
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
