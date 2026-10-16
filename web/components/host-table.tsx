"use client";
import { useMemo, useState } from "react";
import {
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  Bot,
  Laptop,
  Monitor,
  Router,
  Server,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Meter } from "@/components/ui/meter";
import { StatusPill } from "@/components/status-pill";
import { age, percent0 } from "@/lib/api";
import {
  compareHosts,
  cpuPercent,
  hostState,
  memoryUsage,
  worstDisk,
  type SortKey,
} from "@/lib/fleet";
import type { Host } from "@/lib/protocol";

/** A glyph for what the host most likely is, from its OS, agent and observed services. */
export function HostIcon({ host, size }: { host: Host; size: number }) {
  const os = host.inventory?.os.toLowerCase() ?? "";
  if (os.includes("mac")) return <Laptop size={size} />;
  if (host.agent_id) return <Server size={size} />;
  if (host.services.some((s) => s.port === 80 || s.port === 443))
    return <Router size={size} />;
  return <Monitor size={size} />;
}

export function HostTable({
  hosts,
  selected,
  select,
}: {
  hosts: Host[];
  selected: string | null;
  select: (id: string) => void;
}) {
  const [sort, setSort] = useState<{ key: SortKey; descending: boolean }>({
    key: "status",
    descending: false,
  });
  const sorted = useMemo(() => {
    const compare = compareHosts(sort.key);
    const rows = [...hosts].sort(compare);
    return sort.descending ? rows.reverse() : rows;
  }, [hosts, sort]);
  const toggle = (key: SortKey) =>
    setSort((current) =>
      current.key === key
        ? { key, descending: !current.descending }
        : { key, descending: false },
    );
  const header = (key: SortKey, label: string, className?: string) => {
    const active = sort.key === key;
    const Icon = !active ? ArrowUpDown : sort.descending ? ArrowDown : ArrowUp;
    return (
      <TableHead
        className={className}
        aria-sort={
          active ? (sort.descending ? "descending" : "ascending") : undefined
        }
      >
        <button
          type="button"
          onClick={() => toggle(key)}
          data-active={active || undefined}
        >
          {label}
          <Icon size={12} aria-hidden="true" />
        </button>
      </TableHead>
    );
  };
  return (
    <Table className="host-table">
      <TableHeader>
        <TableRow>
          {header("name", "Host")}
          <TableHead>Address</TableHead>
          {header("status", "Status")}
          {header("cpu", "CPU")}
          {header("memory", "Memory")}
          {header("disk", "Disk")}
          <TableHead>Services</TableHead>
          {header("seen", "Last seen", "text-right")}
        </TableRow>
      </TableHeader>
      <TableBody>
        {sorted.map((h) => {
          const state = hostState(h);
          const cpu = cpuPercent(h);
          const memory = memoryUsage(h);
          const disk = worstDisk(h);
          return (
            <TableRow
              key={h.id}
              data-state={h.id === selected ? "selected" : undefined}
              onClick={() => select(h.id)}
            >
              <TableCell>
                <div className="host-cell">
                  <span className="host-avatar" data-tone={state.tone}>
                    <HostIcon host={h} size={16} />
                  </span>
                  <div className="min-w-0">
                    <Button
                      variant="link"
                      className="name"
                      onClick={(e) => {
                        e.stopPropagation();
                        select(h.id);
                      }}
                    >
                      {h.name}
                    </Button>
                    <div className="sub">
                      {h.agent_id && (
                        <Bot size={12} aria-label="Agent installed" />
                      )}
                      <span className="truncate">
                        {h.inventory?.os ??
                          `Discovered by ${h.discovery_method}`}
                      </span>
                    </div>
                  </div>
                </div>
              </TableCell>
              <TableCell>
                <span className="mono">{h.addresses[0] ?? "Unknown"}</span>
                {h.addresses.length > 1 && (
                  <span className="ml-1 text-xs text-muted-foreground">
                    +{h.addresses.length - 1}
                  </span>
                )}
              </TableCell>
              <TableCell>
                <StatusPill host={h} />
              </TableCell>
              <TableCell>
                <div className="metric-cell">
                  <span>{percent0(cpu)}</span>
                  <Meter value={cpu} thin />
                </div>
              </TableCell>
              <TableCell>
                <div className="metric-cell">
                  <span>{percent0(memory)}</span>
                  <Meter value={memory} thin />
                </div>
              </TableCell>
              <TableCell>
                <div className="metric-cell">
                  <span>
                    {percent0(disk?.percent)}
                    {disk && (
                      <span className="ml-1 text-muted-foreground">
                        {disk.disk.mount}
                      </span>
                    )}
                  </span>
                  <Meter value={disk?.percent} thin />
                </div>
              </TableCell>
              <TableCell>
                <span className="tabular text-muted-foreground">
                  {h.services.length || "—"}
                </span>
              </TableCell>
              <TableCell className="text-right text-muted-foreground">
                {age(Math.max(h.last_report ?? 0, h.last_seen))}
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}
