"use client";
import { useState } from "react";
import { Bot, Plus } from "lucide-react";
import { Age } from "@/components/age";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Meter } from "@/components/ui/meter";
import { percent0 } from "@/lib/api";
import {
  cpuPercent,
  hostState,
  memoryUsage,
  worstDisk,
} from "@/lib/fleet";
import type { Host } from "@/lib/protocol";

const FILTERS = [
  ["all", "All agents"],
  ["reporting", "Reporting"],
  ["stale", "Not reporting"],
  ["never", "Never reported"],
  ["revoked", "Revoked"],
] as const;
type Filter = (typeof FILTERS)[number][0];
function agentState(host: Host): Exclude<Filter, "all"> {
  if (host.agent_revoked) return "revoked";
  if (!host.last_report) return "never";
  return hostState(host).key === "reporting" ? "reporting" : "stale";
}

export function AgentsPage({
  hosts,
  onSelect,
  onInstall,
}: {
  hosts: Host[];
  onSelect: (host: Host) => void;
  onInstall: () => void;
}) {
  const [filter, setFilter] = useState<Filter>("all");
  const [search, setSearch] = useState("");
  // Enrollment creates the host record before any report arrives. Keep revoked records too.
  const agents = hosts.filter((host) => host.agent_id);
  const matching = agents.filter((host) =>
    [host.name, host.agent_id, host.agent_version, ...host.addresses]
      .join(" ")
      .toLowerCase()
      .includes(search.trim().toLowerCase()),
  );
  const visible = matching.filter(
    (host) => filter === "all" || agentState(host) === filter,
  );
  return (
    <div className="page">
      <div className="page-heading">
        <p>
          {agents.length} enrolled {agents.length === 1 ? "agent" : "agents"},
          including agents that have stopped reporting or have been revoked.
        </p>
        <Button onClick={onInstall}>
          <Plus />
          Add agent
        </Button>
      </div>
      <div className="flex flex-col gap-3">
        <Input
          aria-label="Search agents"
          placeholder="Search by host, address, agent ID or version…"
          value={search}
          onChange={(event) => setSearch(event.target.value)}
        />
        <div
          className="flex flex-wrap gap-2"
          role="group"
          aria-label="Agent status"
        >
          {FILTERS.map(([key, label]) => (
            <button
              key={key}
              type="button"
              className="chip"
              aria-pressed={filter === key}
              onClick={() => setFilter(key)}
            >
              {label}
              <span className="count">
                {key === "all"
                  ? matching.length
                  : matching.filter((host) => agentState(host) === key).length}
              </span>
            </button>
          ))}
        </div>
      </div>
      {visible.length ? (
        <div className="agent-grid">
          {visible.map((host) => {
            const state = agentState(host);
            const cpu = cpuPercent(host);
            const memory = memoryUsage(host);
            const storage = worstDisk(host);
            const tone =
              state === "reporting"
                ? "good"
                : state === "revoked"
                  ? "neutral"
                  : "warning";
            return (
              <article
                key={host.id}
                className="panel agent-card"
                aria-label={host.name}
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <Button
                    variant="link"
                    className="p-0 text-base"
                    onClick={() => onSelect(host)}
                  >
                    <Bot />
                    {host.name}
                  </Button>
                  <span className="status-pill" data-tone={tone}>
                    {FILTERS.find(([key]) => key === state)?.[1]}
                  </span>
                </div>
                <dl className="detail-list mt-4">
                  <dt>Address</dt>
                  <dd className="mono break-all">
                    {host.addresses.join(" · ") || "Unknown"}
                  </dd>
                  <dt>Agent version</dt>
                  <dd>{host.agent_version ?? "Not reported"}</dd>
                  <dt>Last report</dt>
                  <dd>
                    {host.last_report ? (
                      <Age value={host.last_report} />
                    ) : (
                      "Never"
                    )}
                  </dd>
                  <dt>Agent ID</dt>
                  <dd className="mono break-all">{host.agent_id}</dd>
                </dl>
                <section className="agent-usage" aria-label="Resource usage">
                  <div className="agent-usage-heading">
                    <h3>Resource usage</h3>
                    <span>
                      {state === "reporting"
                        ? "Latest report"
                        : host.last_report
                          ? "Last report"
                          : "No report yet"}
                    </span>
                  </div>
                  <div className="agent-usage-grid">
                    <div className="metric-cell">
                      <span>CPU {percent0(cpu)}</span>
                      <Meter value={cpu} thin label="CPU usage" />
                    </div>
                    <div className="metric-cell">
                      <span>Memory {percent0(memory)}</span>
                      <Meter value={memory} thin label="Memory usage" />
                    </div>
                    <div className="metric-cell">
                      <span title={storage?.disk.mount ?? undefined}>
                        Storage {percent0(storage?.percent)}
                      </span>
                      <Meter
                        value={storage?.percent}
                        thin
                        label="Storage usage"
                      />
                    </div>
                  </div>
                  {state !== "reporting" && host.last_report != null && (
                    <p className="agent-usage-note">
                      Values may be out of date; last report{" "}
                      <Age value={host.last_report} />.
                    </p>
                  )}
                  {!host.last_report && (
                    <p className="agent-usage-note">
                      CPU and memory readings appear after the first report.
                    </p>
                  )}
                </section>
                {state === "never" && (
                  <p className="mt-3 text-xs text-muted-foreground">
                    Enrolled, but no reports have been received.
                  </p>
                )}
                {state === "revoked" && (
                  <p className="mt-3 text-xs text-muted-foreground">
                    Access revoked. Open the host to re-enroll this agent.
                  </p>
                )}
              </article>
            );
          })}
        </div>
      ) : (
        <div className="panel p-6 text-sm">
          <h2 className="font-semibold">
            {agents.length ? "No matching agents" : "No agents enrolled"}
          </h2>
          <p className="mt-2 text-muted-foreground">
            {agents.length
              ? "Try another status or search term."
              : "Install an agent on a host to collect hardware and usage data."}
          </p>
          {agents.length > 0 && (
            <Button
              variant="outline"
              className="mt-3"
              onClick={() => {
                setFilter("all");
                setSearch("");
              }}
            >
              Clear filters
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
