"use client";

import {
  ArrowDownLeft,
  ArrowLeft,
  ArrowUpRight,
  Boxes,
  Cpu,
  HardDrive,
  MemoryStick,
  Network,
  Waypoints,
} from "lucide-react";
import { ContainerAppIcon } from "@/components/container-app-icon";
import { Button } from "@/components/ui/button";
import type { Container, Host } from "@/lib/protocol";
import { bytes, percentage } from "@/lib/api";
import { hostDisplayName } from "@/lib/devices";
import { ContainerMetricHistory } from "@/components/metric-history";

export function ContainerDetailsPage({
  host,
  container,
  containerId,
  onBack,
}: {
  host: Host;
  container?: Container;
  containerId: string;
  onBack: () => void;
}) {
  const history = (
    <ContainerMetricHistory
      containerId={containerId}
      hostId={host.id}
      collectedAt={host.last_collected_at}
      latest={host.metrics}
    />
  );
  if (!container)
    return (
      <section className="container-page" aria-label="Container details">
        <Button variant="outline" size="sm" onClick={onBack}>
          <ArrowLeft data-icon="inline-start" />
          Back to {hostDisplayName(host)}
        </Button>
        <h1>Container not in the latest report</h1>
        <p className="text-muted-foreground">
          It may have stopped, been removed, or its collector may be
          unavailable. Its stored history remains available below.
        </p>
        {history}
      </section>
    );
  const memory =
    container.memory_used_bytes == null
      ? "Unavailable"
      : `${bytes(container.memory_used_bytes)} / ${bytes(container.memory_limit_bytes)}`;

  return (
    <section className="container-page" aria-label="Container details">
      <Button variant="outline" size="sm" onClick={onBack}>
        <ArrowLeft data-icon="inline-start" />
        Back to {hostDisplayName(host)}
      </Button>
      <header className="container-page-header">
        <ContainerAppIcon image={container.image} name={container.name} />
        <div className="min-w-0 flex-1">
          <p className="eyebrow">Container on {hostDisplayName(host)}</p>
          <h1 className="truncate">{container.name}</h1>
          <p className="mono break-all text-sm text-muted-foreground">
            {container.image}
          </p>
        </div>
        <span
          className="status-pill shrink-0"
          data-tone={container.state === "running" ? "good" : "neutral"}
        >
          {container.state}
        </span>
      </header>

      <section
        aria-label="Current resource usage"
        className="container-stat-grid"
      >
        <article className="container-stat">
          <span>
            <Cpu size={15} /> CPU
          </span>
          <strong>{percentage(container.cpu_percent)}</strong>
        </article>
        <article className="container-stat">
          <span>
            <MemoryStick size={15} /> Memory
          </span>
          <strong>{memory}</strong>
          <small>{percentage(container.memory_percent)} of limit</small>
        </article>
        <article className="container-stat">
          <span>
            <Network size={15} /> Network I/O
          </span>
          <strong>
            <ArrowDownLeft size={14} /> {bytes(container.network_rx_bytes)}
          </strong>
          <small>
            <ArrowUpRight size={14} /> {bytes(container.network_tx_bytes)}
          </small>
        </article>
        <article className="container-stat">
          <span>
            <HardDrive size={15} /> Block I/O
          </span>
          <strong>
            <ArrowDownLeft size={14} /> {bytes(container.block_read_bytes)}
          </strong>
          <small>
            <ArrowUpRight size={14} /> {bytes(container.block_write_bytes)}
          </small>
        </article>
      </section>

      <section className="container-info-section">
        <h2>
          <Boxes size={16} /> Container
        </h2>
        <dl className="container-info-grid">
          <div>
            <dt>Host</dt>
            <dd>{hostDisplayName(host)}</dd>
          </div>
          <div>
            <dt>Status</dt>
            <dd>{container.status || container.state}</dd>
          </div>
          <div>
            <dt>Container ID</dt>
            <dd className="mono break-all">{container.id}</dd>
          </div>
          <div>
            <dt>Processes</dt>
            <dd>{container.pids ?? "Unavailable"}</dd>
          </div>
          <div className="container-info-wide">
            <dt>Published ports</dt>
            <dd>{container.ports || "None"}</dd>
          </div>
          <div className="container-info-wide">
            <dt>Image</dt>
            <dd className="mono break-all">{container.image}</dd>
          </div>
        </dl>
      </section>
      <p className="container-stats-note">
        <Waypoints size={14} /> Stats reflect the agent’s latest report. Network
        and block I/O are totals since the container started.
      </p>
      {history}
    </section>
  );
}
