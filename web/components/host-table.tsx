"use client";
import { Age } from "@/components/age";
import Image from "next/image";
import { createElement, useMemo, useState } from "react";
import { ArrowDown, ArrowUp, ArrowUpDown, Bot } from "lucide-react";
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
import { percent0 } from "@/lib/api";
import {
  compareHosts,
  cpuPercent,
  hostState,
  hostIssues,
  memoryUsage,
  worstDisk,
  type SortKey,
} from "@/lib/fleet";
import type { Host } from "@/lib/protocol";
import { deviceFingerprintId, deviceIcon, deviceLabel } from "@/lib/devices";

/** A glyph for what the host most likely is (the server's device guess). */
export function HostIcon({ host, size }: { host: Host; size: number }) {
  const fingerprint = deviceFingerprintId(host);
  if (fingerprint) {
    return (
      <FingerprintIcon
        key={fingerprint}
        fingerprint={fingerprint}
        host={host}
        size={size}
      />
    );
  }
  return createElement(deviceIcon(host), {
    size,
    "aria-label": deviceLabel(host) ?? undefined,
  });
}

function FingerprintIcon({
  fingerprint,
  host,
  size,
}: {
  fingerprint: string;
  host: Host;
  size: number;
}) {
  const [failed, setFailed] = useState(false);
  if (failed) {
    return createElement(deviceIcon(host), {
      size,
      "aria-label": deviceLabel(host) ?? undefined,
    });
  }
  return (
    <Image
      src={`https://static.ui.com/fingerprint/0/${fingerprint}_${size <= 18 ? 101 : 257}x${size <= 18 ? 101 : 257}.png`}
      width={size}
      height={size}
      alt={deviceLabel(host) ?? ""}
      unoptimized
      onError={() => setFailed(true)}
    />
  );
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
    key: "address",
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
          {header("address", "Address")}
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
          const issues = hostIssues(h);
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
                        {[deviceLabel(h), h.inventory?.os ?? h.vendor ?? null]
                          .filter(Boolean)
                          .join(" · ") || `Discovered by ${h.discovery_method}`}
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
                {issues.length > 0 && (
                  <div
                    className={`mt-1 text-xs font-medium tone-${issues.some((issue) => issue.severity === "critical") ? "critical" : "warning"}`}
                    title={issues.map((issue) => issue.message).join("\n")}
                  >
                    {issues.length} {issues.length === 1 ? "issue" : "issues"}
                  </div>
                )}
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
                <Age value={Math.max(h.last_report ?? 0, h.last_seen)} />
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}
