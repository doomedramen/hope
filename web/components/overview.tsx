"use client";
import { Age } from "@/components/age";
import {
  Activity,
  ArrowDownToLine,
  ArrowRight,
  Check,
  CircleCheck,
  Cpu,
  HardDrive,
  Map,
  MemoryStick,
  Network,
  Plus,
  Radar,
  Server,
  TriangleAlert,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Meter } from "@/components/ui/meter";
import { StatTile } from "@/components/stat-tile";
import { bytes, percent0, rate } from "@/lib/api";
import {
  cpuPercent,
  fleetSummary,
  fleetUpdates,
  memoryUsage,
  worstDisk,
  type Issue,
} from "@/lib/fleet";
import type { Bootstrap, Host } from "@/lib/protocol";

export function Overview({
  data,
  onSelect,
  onDiscover,
  onAddNetwork,
  onInstall,
  onView,
  onUpdateAll,
  busy = false,
}: {
  data: Bootstrap;
  onSelect: (host: Host) => void;
  onDiscover: () => void;
  onAddNetwork: () => void;
  onInstall: () => void;
  onView: (view: "map" | "hosts", filter?: "attention") => void;
  /** Asks every agent that can update itself to do so. */
  onUpdateAll: () => void;
  busy?: boolean;
}) {
  const { hosts, networks, scans } = data;
  const summary = fleetSummary(hosts, scans);
  const updates = fleetUpdates(hosts, data.agent_release);
  const agentsInstalled = hosts.some((h) => h.agent_id);
  const scanned = scans.length > 0 || hosts.length > 0;
  const onboarding = !networks.length || !scanned || !agentsInstalled;
  const affectedHosts = new Set(summary.issues.map((issue) => issue.host.id))
    .size;
  const attentionTone = summary.issues.some((i) => i.severity === "critical")
    ? "critical"
    : summary.issues.length
      ? "warning"
      : "good";
  return (
    <div className="page">
      <div className="page-heading">
        <div>
          <h2>Network status</h2>
          <p>
            {hosts.length
              ? `${summary.online} of ${hosts.length} hosts are responding${
                  summary.issues.length
                    ? `, ${summary.issues.length} ${summary.issues.length === 1 ? "issue" : "issues"} across ${affectedHosts} ${affectedHosts === 1 ? "host" : "hosts"}`
                    : " and nothing needs attention"
                }.`
              : "Add a range to start discovering what is on your network."}
          </p>
        </div>
        {summary.lastScan && (
          <p className="text-xs text-muted-foreground">
            Last discovery pass{" "}
            <Age
              value={
                summary.lastScan.finished_at ?? summary.lastScan.started_at
              }
              lower
            />
          </p>
        )}
      </div>

      {!hosts.length && (
        <section className="hero-empty" data-testid="hero-empty">
          <div>
            <h2>No devices yet</h2>
            <p>
              Add a CIDR range to find every reachable device, or install the
              reporting agent on a machine to see its hardware, storage and live
              health.
            </p>
            <div className="hero-actions">
              <Button size="lg" onClick={onAddNetwork}>
                <Plus data-icon="inline-start" />
                Add a network
              </Button>
              <Button size="lg" variant="outline" onClick={onInstall}>
                Install an agent
              </Button>
            </div>
          </div>
          <div className="panel">
            <div className="panel-header">
              <h3>
                <Radar />
                What happens next
              </h3>
            </div>
            <div className="panel-body text-sm text-muted-foreground leading-relaxed">
              Discovery probes each address in the range with ICMP, ARP on the
              local link, and a handful of TCP ports, then enriches responsive
              hosts with names and services. An unanswered probe never marks a
              host offline; H.O.P.E only reports what it observed.
            </div>
          </div>
        </section>
      )}

      {updates.total > 0 && (
        <section className="panel" data-testid="fleet-updates">
          <div className="panel-header">
            <h3>
              <ArrowDownToLine />
              Agent updates
            </h3>
          </div>
          <div className="panel-body flex flex-wrap items-center justify-between gap-3 text-sm">
            <p className="text-muted-foreground">
              {updates.total}{" "}
              {updates.total === 1 ? "agent runs" : "agents run"} an older
              release than {data.agent_release}.
              {updates.ready > 0 && ` ${updates.ready} can update from here.`}
              {updates.pending > 0 &&
                ` ${updates.pending} already ${updates.pending === 1 ? "has" : "have"} been asked.`}
              {updates.manual > 0 &&
                ` ${updates.manual} need${updates.manual === 1 ? "s" : ""} the upgrade command run on ${updates.manual === 1 ? "its host" : "their hosts"} once (open the host).`}
            </p>
            {updates.ready > 0 && (
              <Button size="sm" disabled={busy} onClick={onUpdateAll}>
                <ArrowDownToLine data-icon="inline-start" />
                {`Update ${updates.ready} ${updates.ready === 1 ? "agent" : "agents"}`}
              </Button>
            )}
          </div>
        </section>
      )}

      <div className="stat-grid">
        <StatTile
          label="Hosts responding"
          icon={<Server />}
          value={summary.online}
          unit={`/ ${hosts.length}`}
          hint={
            hosts.length
              ? `${hosts.length - summary.online} quiet or stale`
              : "No hosts discovered yet"
          }
          onClick={() => onView("hosts")}
          testId="tile-hosts"
        />
        <StatTile
          label="Agents reporting"
          icon={<Activity />}
          value={summary.reporting}
          unit={`/ ${summary.agents}`}
          hint={
            summary.agents
              ? summary.reporting === summary.agents
                ? "Every agent is on schedule"
                : `${summary.agents - summary.reporting} not reporting`
              : "Install an agent for live metrics"
          }
          tone={
            summary.agents && summary.reporting < summary.agents
              ? "warning"
              : undefined
          }
          onClick={() => onView("hosts", undefined)}
          testId="tile-agents"
        />
        <StatTile
          label="Hosts needing attention"
          icon={<TriangleAlert />}
          value={affectedHosts}
          hint={
            summary.issues.length
              ? `${summary.issues.length} issues · ${summary.issues.filter((i) => i.severity === "critical").length} critical`
              : "No issues detected"
          }
          tone={hosts.length ? attentionTone : undefined}
          onClick={() => onView("hosts", "attention")}
          testId="tile-attention"
        />
        <StatTile
          label="Discovery"
          icon={<Radar />}
          value={
            summary.runningScans.length
              ? summary.runningScans.length
              : networks.filter((n) => n.enabled).length
          }
          unit={
            summary.runningScans.length
              ? summary.runningScans.length === 1
                ? "scan running"
                : "scans running"
              : `of ${networks.length} ${networks.length === 1 ? "range" : "ranges"} watched`
          }
          hint={
            summary.lastScan ? (
              <>
                Last pass{" "}
                <Age
                  value={
                    summary.lastScan.finished_at ?? summary.lastScan.started_at
                  }
                  lower
                />
              </>
            ) : (
              "No discovery pass yet"
            )
          }
          onClick={() => onView("map")}
          testId="tile-discovery"
        />
      </div>

      {onboarding && hosts.length > 0 && (
        <section className="panel" aria-label="Getting started">
          <div className="panel-header">
            <h3>
              <CircleCheck />
              Getting started
            </h3>
          </div>
          <div className="checklist mt-3">
            <Step
              index={1}
              done={networks.length > 0}
              title="Add a network range"
              detail="Tell H.O.P.E which IPv4 ranges to watch."
              action={
                <Button size="sm" variant="outline" onClick={onAddNetwork}>
                  Add a range
                </Button>
              }
            />
            <Step
              index={2}
              done={scanned}
              title="Run discovery"
              detail="Find every reachable device and its open services."
              action={
                <Button size="sm" variant="outline" onClick={onDiscover}>
                  Discover
                </Button>
              }
            />
            <Step
              index={3}
              done={agentsInstalled}
              title="Install a reporting agent"
              detail="See CPU, memory, storage, GPU and traffic with seven days of history."
              action={
                <Button size="sm" variant="outline" onClick={onInstall}>
                  Add agent
                </Button>
              }
            />
          </div>
        </section>
      )}

      {hosts.length > 0 && (
        <div className="overview-grid">
          <div className="overview-column">
            <section className="panel" aria-label="Needs attention">
              <div className="panel-header">
                <div>
                  <h3>
                    <TriangleAlert />
                    Needs attention
                  </h3>
                  <p>Stale agents, unobserved hosts and resources above 90%.</p>
                </div>
                {summary.issues.length > 0 && (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => onView("hosts", "attention")}
                  >
                    View all
                    <ArrowRight data-icon="inline-end" />
                  </Button>
                )}
              </div>
              <div className="issue-list mt-3">
                {summary.issues.length ? (
                  summary.issues
                    .slice(0, 8)
                    .map((issue) => (
                      <IssueRow
                        key={`${issue.host.id}:${issue.kind}`}
                        issue={issue}
                        onSelect={onSelect}
                      />
                    ))
                ) : (
                  <div className="all-clear">
                    <Check size={18} />
                    Everything is reporting on time and within limits.
                  </div>
                )}
              </div>
            </section>
            <section className="panel" aria-label="Fleet usage">
              <div className="panel-header">
                <div>
                  <h3>
                    <Activity />
                    Fleet usage
                  </h3>
                  <p>
                    Latest sample from {summary.reporting} reporting{" "}
                    {summary.reporting === 1 ? "agent" : "agents"}.
                  </p>
                </div>
              </div>
              <div className="panel-body">
                {summary.reporting ? (
                  <div className="fleet-usage">
                    <div>
                      <div className="label">
                        <span>Average CPU</span>
                        <strong>{percent0(summary.cpuAverage)}</strong>
                      </div>
                      <Meter value={summary.cpuAverage} label="Average CPU" />
                    </div>
                    <div>
                      <div className="label">
                        <span>Memory in use</span>
                        <strong>
                          {summary.memoryTotal
                            ? `${bytes(summary.memoryUsed)} of ${bytes(summary.memoryTotal)}`
                            : "—"}
                        </strong>
                      </div>
                      <Meter
                        value={
                          summary.memoryTotal
                            ? (summary.memoryUsed / summary.memoryTotal) * 100
                            : null
                        }
                        label="Memory in use"
                      />
                    </div>
                    <div>
                      <div className="label">
                        <span>Storage in use</span>
                        <strong>
                          {summary.diskTotal
                            ? `${bytes(summary.diskUsed)} of ${bytes(summary.diskTotal)}`
                            : "—"}
                        </strong>
                      </div>
                      <Meter
                        value={
                          summary.diskTotal
                            ? (summary.diskUsed / summary.diskTotal) * 100
                            : null
                        }
                        label="Storage in use"
                      />
                    </div>
                    <div>
                      <div className="label">
                        <span>Traffic</span>
                        <strong>
                          ↓ {rate(summary.rx)} · ↑ {rate(summary.tx)}
                        </strong>
                      </div>
                      <p className="text-xs text-muted-foreground">
                        Sum of every agent&rsquo;s primary interfaces.
                      </p>
                    </div>
                  </div>
                ) : (
                  <p className="text-sm text-muted-foreground">
                    No agent is reporting right now.{" "}
                    <button
                      className="text-primary underline-offset-4 hover:underline"
                      onClick={onInstall}
                    >
                      Install one
                    </button>{" "}
                    to see fleet-wide usage.
                  </p>
                )}
              </div>
            </section>
          </div>
          <div className="overview-column">
            <Leaders
              title="Busiest CPUs"
              icon={<Cpu />}
              hosts={hosts}
              value={cpuPercent}
              onSelect={onSelect}
            />
            <Leaders
              title="Most memory in use"
              icon={<MemoryStick />}
              hosts={hosts}
              value={memoryUsage}
              onSelect={onSelect}
            />
            <Leaders
              title="Fullest filesystems"
              icon={<HardDrive />}
              hosts={hosts}
              value={(host) => worstDisk(host)?.percent ?? null}
              detail={(host) => worstDisk(host)?.disk.mount}
              onSelect={onSelect}
            />
            <section className="panel" aria-label="Networks">
              <div className="panel-header">
                <h3>
                  <Network />
                  Networks
                </h3>
                <Button variant="ghost" size="sm" onClick={() => onView("map")}>
                  Open map
                  <Map data-icon="inline-end" />
                </Button>
              </div>
              <div className="panel-body flex flex-col gap-3">
                {networks.length ? (
                  networks.map((network) => {
                    const members = hosts.filter((h) =>
                      h.network_ids.includes(network.id),
                    ).length;
                    return (
                      <div
                        key={network.id}
                        className="flex items-center gap-3 text-sm"
                      >
                        <span
                          className="status-dot"
                          data-tone={network.enabled ? "good" : "neutral"}
                        />
                        <span className="min-w-0 flex-1 truncate font-medium">
                          {network.name}
                        </span>
                        <span className="mono text-muted-foreground">
                          {network.cidr}
                        </span>
                        <span className="tabular w-16 text-right text-xs text-muted-foreground">
                          {members} {members === 1 ? "host" : "hosts"}
                        </span>
                      </div>
                    );
                  })
                ) : (
                  <p className="text-sm text-muted-foreground">
                    No ranges yet.
                  </p>
                )}
              </div>
            </section>
          </div>
        </div>
      )}
    </div>
  );
}

function Step({
  index,
  done,
  title,
  detail,
  action,
}: {
  index: number;
  done: boolean;
  title: string;
  detail: string;
  action: React.ReactNode;
}) {
  return (
    <div className="checklist-step" data-done={done}>
      <span className="step-index" aria-hidden="true">
        {done ? <Check size={14} /> : index}
      </span>
      <div className="min-w-0">
        <div className="step-title">{title}</div>
        <div className="step-detail">{detail}</div>
      </div>
      {done ? (
        <span className="text-xs font-medium tone-good">Done</span>
      ) : (
        action
      )}
    </div>
  );
}

function IssueRow({
  issue,
  onSelect,
}: {
  issue: Issue;
  onSelect: (host: Host) => void;
}) {
  return (
    <button
      type="button"
      className="issue-row"
      onClick={() => onSelect(issue.host)}
    >
      <span
        className="status-dot"
        data-tone={issue.severity}
        aria-label={issue.severity}
      />
      <span className="min-w-0">
        <span className="issue-host">{issue.host.name}</span>
        <span className="issue-message block">{issue.message}</span>
      </span>
      <span className="issue-when">
        <Age
          value={Math.max(issue.host.last_report ?? 0, issue.host.last_seen)}
        />
      </span>
    </button>
  );
}

function Leaders({
  title,
  icon,
  hosts,
  value,
  detail,
  onSelect,
}: {
  title: string;
  icon: React.ReactNode;
  hosts: Host[];
  value: (host: Host) => number | null;
  detail?: (host: Host) => string | undefined;
  onSelect: (host: Host) => void;
}) {
  const rows = hosts
    .map((host) => ({ host, value: value(host) }))
    .filter((row): row is { host: Host; value: number } => row.value != null)
    .sort((a, b) => b.value - a.value)
    .slice(0, 5);
  return (
    <section className="panel" aria-label={title}>
      <div className="panel-header">
        <h3>
          {icon}
          {title}
        </h3>
      </div>
      <div className="panel-body">
        {rows.length ? (
          <div className="leader-list">
            {rows.map(({ host, value }) => (
              <button
                key={host.id}
                type="button"
                className="leader-row"
                onClick={() => onSelect(host)}
              >
                <span className="leader-name">
                  {host.name}
                  {detail?.(host) && (
                    <span className="ml-1.5 text-muted-foreground">
                      {detail(host)}
                    </span>
                  )}
                </span>
                <Meter value={value} thin />
                <span className="leader-value">{percent0(value)}</span>
              </button>
            ))}
          </div>
        ) : (
          <p className="text-sm text-muted-foreground">
            Appears once an agent reports.
          </p>
        )}
      </div>
    </section>
  );
}
