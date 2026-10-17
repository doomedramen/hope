"use client";
import { Age } from "@/components/age";
import { useEffect, useRef, useState } from "react";
import {
  ArrowDownToLine,
  ArrowLeft,
  Maximize2,
  Bot,
  Check,
  Copy,
  Boxes,
  Cpu,
  GitMerge,
  MemoryStick,
  Network,
  Radio,
  ShieldOff,
  Tag,
  Waypoints,
  Wifi,
  X,
  Zap,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
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
import { bytes, percent0, post, put, rate } from "@/lib/api";
import {
  DEVICE_KINDS,
  confidenceLabel,
  deviceLabel,
  kindLabel,
} from "@/lib/devices";
import {
  cpuPercent,
  memoryUsage,
  worstDisk,
  hostIssues,
  hostState,
  updatePlan,
  type Tone,
} from "@/lib/fleet";
import type { Host } from "@/lib/protocol";

export function HostDetails({
  host,
  hosts,
  close,
  expand,
  wide = false,
  agentRelease = null,
  upgradeCommand = "",
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
  /** Version of the signed agent release the server offers. */
  agentRelease?: string | null;
  /** Token-less command that updates an installed agent from the host itself. */
  upgradeCommand?: string;
  install: () => void;
  refresh: () => void;
  notify: (message: string, tone?: Tone) => void;
}) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (wide) {
      heading.current?.focus({ preventScroll: true });
      window.scrollTo({ top: 0 });
    }
  }, [wide, host.id]);
  const [error, setError] = useState("");
  const [source, setSource] = useState<string | null>(null);
  const [confirmRevoke, setConfirmRevoke] = useState(false);
  const [hours, setHours] = useState<HistoryWindow>("1");
  const inventory = host.inventory;
  const metrics = host.metrics;
  const update = updatePlan(host, agentRelease);
  const [copied, setCopied] = useState(false);
  const state = hostState(host);
  const issues = hostIssues(host);
  const history = useHistory(
    host.id,
    hours,
    host.last_collected_at,
    host.metrics,
  );
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
  async function requestUpdate() {
    try {
      await post(`/hosts/${host.id}/update-agent`);
      notify(
        `Asked ${host.name} to update to ${update?.target ?? "the latest release"}.`,
        "good",
      );
      refresh();
    } catch (e) {
      setError((e as Error).message);
    }
  }
  async function setType(value: string) {
    const kind = value === "auto" ? null : value;
    try {
      await put(`/hosts/${host.id}/type`, { kind });
      notify(
        kind
          ? `${host.name} is now marked as ${kindLabel(kind).toLowerCase()}.`
          : `${host.name} is back to automatic detection.`,
        "good",
      );
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
            <h2
              ref={heading}
              tabIndex={-1}
              className="truncate text-lg font-semibold tracking-tight"
            >
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
              {host.agent_version ? `Agent ${host.agent_version}` : "Agent"}
            </Badge>
          )}
          <span>
            {host.agent_id && host.last_report ? (
              <>
                Reported <Age value={host.last_report} lower /> · collected{" "}
                <Age value={host.last_collected_at} lower />
              </>
            ) : (
              <>
                Seen <Age value={host.last_seen} lower />
              </>
            )}
          </span>
        </div>
      </header>
      <div className="details-body">
        {error && (
          <Alert variant="destructive">
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}
        <Tabs defaultValue="summary" className="host-tabs">
          <TabsList aria-label="Host sections" className="host-tab-list">
            <TabsTrigger value="summary">Summary</TabsTrigger>
            {host.agent_id && (
              <TabsTrigger value="metrics">Metrics</TabsTrigger>
            )}
            {host.agent_id && (
              <TabsTrigger value="containers">Containers</TabsTrigger>
            )}
            <TabsTrigger value="services">Services</TabsTrigger>
            <TabsTrigger value="settings">Settings</TabsTrigger>
          </TabsList>
          <TabsContent value="summary" className="host-tab-content">
            {issues.length > 0 && (
              <div className="flex flex-col gap-1.5">
                {issues.map((issue) => (
                  <div
                    key={`${issue.kind}:${issue.message}`}
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
                  Discovery can only see what answers on the network. Install
                  the agent for hardware, storage, GPU and live usage with
                  history.
                </p>
                <Button size="sm" onClick={install}>
                  <ArrowDownToLine data-icon="inline-start" />
                  Install reporting agent
                </Button>
              </div>
            )}
            {host.agent_id && (
              <section aria-label="Current readings" className="host-readings">
                <ReadingMeter label="CPU" value={cpuPercent(host)} />
                <ReadingMeter label="Memory" value={memoryUsage(host)} />
                <ReadingMeter
                  label="Fullest filesystem"
                  value={worstDisk(host)?.percent ?? null}
                  detail={worstDisk(host)?.disk.mount}
                />
                <p className="text-xs text-muted-foreground">
                  Latest reported values
                  {state.key !== "reporting" ? " · Agent is not reporting" : ""}
                </p>
              </section>
            )}
            <DeviceSection
              host={host}
              hosts={hosts}
              onType={(value) => void setType(value)}
            />
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
                  <dd className="mono">
                    {host.macs.join(", ") || "Unavailable"}
                  </dd>
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
                  <dd className="mono">
                    {host.macs.join(", ") || "Unavailable"}
                  </dd>
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
            {(metrics?.gpus ?? inventory?.gpus ?? []).length > 0 && (
              <section>
                <h3 className="detail-heading">
                  <Zap />
                  Graphics
                </h3>
                <div className="flex flex-wrap gap-1.5">
                  {(metrics?.gpus ?? inventory?.gpus ?? []).map((g, i) => (
                    <span
                      key={g.id ?? `${g.name}-${i}`}
                      className="service-chip"
                    >
                      {g.name}
                      {g.temperature_celsius != null && (
                        <b>{g.temperature_celsius.toFixed(0)}°C</b>
                      )}
                    </span>
                  ))}
                </div>
              </section>
            )}
          </TabsContent>
          <TabsContent value="metrics" className="host-tab-content">
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
          </TabsContent>
          <TabsContent value="containers" className="host-tab-content">
            <section aria-label="Docker containers">
              <h3 className="detail-heading">
                <Boxes />
                Docker containers
                <span className="spacer" />
                <span className="text-xs font-normal text-muted-foreground">
                  {metrics?.containers.length ?? 0}
                </span>
              </h3>
              {(metrics?.containers.length ?? 0) > 0 ? (
                <div className="container-grid">
                  {metrics?.containers.map((container) => (
                    <article className="container-card" key={container.id}>
                      <div className="flex min-w-0 items-start justify-between gap-3">
                        <div className="min-w-0">
                          <strong className="block truncate">
                            {container.name}
                          </strong>
                          <span className="mono block truncate text-xs text-muted-foreground">
                            {container.image}
                          </span>
                        </div>
                        <span
                          className="status-pill shrink-0"
                          data-tone={
                            container.state === "running" ? "good" : "neutral"
                          }
                        >
                          {container.state}
                        </span>
                      </div>
                      <div className="container-meta">
                        <span>{container.status}</span>
                        {container.ports && <span>{container.ports}</span>}
                        <span className="mono">
                          {container.id.slice(0, 12)}
                        </span>
                      </div>
                    </article>
                  ))}
                </div>
              ) : (
                <p className="text-sm text-muted-foreground">
                  {host.collectors.find(
                    (collector) => collector.name === "docker_containers",
                  )?.available
                    ? "No containers found."
                    : "Docker is unavailable or its daemon is not running."}
                </p>
              )}
            </section>
          </TabsContent>
          <TabsContent value="services" className="host-tab-content">
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
                          <span className="block truncate">
                            {unit.description}
                          </span>
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
                          {unit.sub_state !== unit.state &&
                            ` · ${unit.sub_state}`}
                        </span>
                      </div>
                    ))}
                  </div>
                ) : (
                  <p className="text-xs text-muted-foreground">
                    No failed units. Add names to HOPE_WATCH_SERVICES on the
                    agent to always show them here.
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
                Open ports
                <span className="spacer" />
                <span className="text-xs font-normal text-muted-foreground">
                  {host.ports_scanned_at ? (
                    <>
                      Common ports scanned{" "}
                      <Age value={host.ports_scanned_at} lower />
                    </>
                  ) : host.agent_id ? (
                    ""
                  ) : (
                    "Port scan pending"
                  )}
                </span>
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
                  No open ports found.
                </p>
              )}
            </section>
          </TabsContent>
          <TabsContent value="settings" className="host-tab-content">
            {update && (
              <div
                className="rounded-xl border p-3.5"
                data-testid="agent-update"
              >
                <p className="mb-2.5 text-xs leading-relaxed text-muted-foreground">
                  Agent {update.target} is available (this host runs{" "}
                  {host.agent_version}).{" "}
                  {update.state === "ready" &&
                    "The agent installs it itself, but only after checking the release's signature against the key it was installed with. It keeps its identity and history."}
                  {update.state === "pending" && (
                    <>
                      Update requested{" "}
                      <Age value={host.update_requested_at} lower />. It starts
                      when the agent next reports.
                    </>
                  )}
                  {update.state === "manual" &&
                    "This agent has no updater yet. Run this command on the host once; after that, updates can start from here."}
                </p>
                {update.failure && (
                  <p className="mb-2.5 text-xs text-destructive">
                    {update.failure}. The previous version is still running.
                  </p>
                )}
                {update.state === "manual" ? (
                  <>
                    <div className="install-command mb-2.5">
                      <code>{upgradeCommand}</code>
                    </div>
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={!upgradeCommand}
                      onClick={async () => {
                        try {
                          await navigator.clipboard.writeText(upgradeCommand);
                          setCopied(true);
                        } catch {
                          setError(
                            "Clipboard access failed. Select and copy the command above.",
                          );
                        }
                      }}
                    >
                      {copied ? (
                        <Check data-icon="inline-start" />
                      ) : (
                        <Copy data-icon="inline-start" />
                      )}
                      {copied ? "Copied" : "Copy command"}
                    </Button>
                  </>
                ) : (
                  <Button size="sm" variant="outline" onClick={requestUpdate}>
                    <ArrowDownToLine data-icon="inline-start" />
                    {update.state === "pending" ? "Ask again" : "Update now"}
                  </Button>
                )}
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
              <>
                {host.collectors.some(
                  (c) => !c.available && c.name !== "agent_update",
                ) && (
                  <section>
                    <h3 className="detail-heading">
                      <MemoryStick />
                      Unavailable collectors
                    </h3>
                    {host.collectors
                      .filter((c) => !c.available && c.name !== "agent_update")
                      .map((c) => (
                        <p
                          key={c.name}
                          className="text-xs text-muted-foreground"
                        >
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
                    Merge a duplicate discovery into this agent after verifying
                    it is the same machine. History and map links are preserved.
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
                    <Button
                      variant="outline"
                      disabled={!source}
                      onClick={merge}
                    >
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
                          This agent will no longer be able to send reports.
                          History stays; a new installation command re-enrolls
                          it.
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
            {!host.agent_id && (
              <p className="text-sm text-muted-foreground">
                Install a reporting agent to manage updates and access for this
                host.
              </p>
            )}
          </TabsContent>
        </Tabs>
      </div>
    </aside>
  );
}

/** What the device is, why the server thinks so, and a way to correct it. */
function DeviceSection({
  host,
  hosts,
  onType,
}: {
  host: Host;
  hosts: Host[];
  onType: (value: string) => void;
}) {
  const device = host.device;
  const label = deviceLabel(host) ?? "Unknown device";
  const parent = host.parent_id
    ? hosts.find((h) => h.id === host.parent_id)
    : undefined;
  const e = host.evidence;
  const announced = [e.mdns_name, ...e.mdns_instances, e.upnp_friendly_name]
    .filter((v): v is string => !!v)
    .filter((v, i, all) => all.indexOf(v) === i);
  const model = [e.upnp_manufacturer, e.upnp_model ?? e.mdns_model]
    .filter(Boolean)
    .join(" ");
  return (
    <section data-testid="device">
      <h3 className="detail-heading">
        <Tag />
        Device
      </h3>
      <dl className="detail-list">
        <dt>Type</dt>
        <dd>
          {label}
          {device && device.kind !== "unknown" && (
            <span className="ml-1.5 text-muted-foreground">
              ·{" "}
              {device.manual
                ? "set by you"
                : confidenceLabel(device.confidence).toLowerCase()}
            </span>
          )}
        </dd>
        {host.vendor && (
          <>
            <dt>Manufacturer</dt>
            <dd>{host.vendor}</dd>
          </>
        )}
        {parent && (
          <>
            <dt>Behind</dt>
            <dd title={host.parent_reason ?? undefined}>
              {parent.name}
              {parent.name !== parent.addresses[0] && (
                <span className="mono ml-1.5 text-muted-foreground">
                  {parent.addresses[0]}
                </span>
              )}
            </dd>
          </>
        )}
        {announced.length > 0 && (
          <>
            <dt>Announces</dt>
            <dd>{announced.slice(0, 3).join(" · ")}</dd>
          </>
        )}
        {model && (
          <>
            <dt>Model</dt>
            <dd>{model}</dd>
          </>
        )}
        {e.http_title && (
          <>
            <dt>Web interface</dt>
            <dd>{e.http_title}</dd>
          </>
        )}
      </dl>
      {device && !device.manual && device.reasons.length > 0 && (
        <ul className="device-reasons" aria-label="Why this type">
          {device.reasons.map((reason) => (
            <li key={reason}>{reason}</li>
          ))}
        </ul>
      )}
      {device?.kind === "unknown" && !host.agent_id && (
        <p className="text-xs text-muted-foreground">
          Nothing identifies this device yet. Types come from its manufacturer,
          open ports, web interface and what it announces on the network.
        </p>
      )}
      <Select
        value={device?.manual ? device.kind : "auto"}
        onValueChange={(value) => value && onType(value)}
      >
        <SelectTrigger aria-label="Device type" className="w-full">
          <SelectValue>
            {(value: string) =>
              value === "auto" ? "Automatic detection" : kindLabel(value)
            }
          </SelectValue>
        </SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectItem value="auto">Automatic detection</SelectItem>
            {DEVICE_KINDS.filter((k) => k.kind !== "unknown").map((k) => (
              <SelectItem key={k.kind} value={k.kind}>
                {k.label}
              </SelectItem>
            ))}
          </SelectGroup>
        </SelectContent>
      </Select>
    </section>
  );
}

function ReadingMeter({
  label,
  value,
  detail,
}: {
  label: string;
  value: number | null;
  detail?: string;
}) {
  const bounded = value == null ? null : Math.min(100, Math.max(0, value));
  const tone =
    bounded == null
      ? "unknown"
      : bounded >= 90
        ? "critical"
        : bounded >= 75
          ? "warning"
          : "good";
  return (
    <div className="reading-meter-card">
      <div className="reading-meter-heading">
        <span>{label}</span>
        <strong>{percent0(value)}</strong>
      </div>
      <div
        className="reading-meter-track"
        role="progressbar"
        aria-label={`${label} usage`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={bounded ?? undefined}
        aria-valuetext={
          value == null ? "Unavailable" : `${Math.round(value)} percent`
        }
        data-tone={tone}
      >
        {bounded != null && <span style={{ width: `${bounded}%` }} />}
      </div>
      <span className="reading-meter-detail">{detail || "Current usage"}</span>
    </div>
  );
}
