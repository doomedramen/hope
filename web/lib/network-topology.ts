import type { Host } from "@/lib/protocol";
import { hostDisplayName } from "@/lib/devices";
import { ipValue } from "@/lib/fleet";

export type TopologyLink = {
  source: string;
  target: string;
  kind: "reported" | "inferred";
  reason: string;
};

/** Logical relationships only: a gateway or runs_on edge never claims a switch port. */
export function topology(hosts: Host[]) {
  const aliases = new Map<string, Host>();
  for (const host of hosts) {
    aliases.set(host.id, host);
    if (host.source_device_id) aliases.set(host.source_device_id, host);
  }
  const ordered = [...hosts].sort(
    (a, b) =>
      ipValue(a.addresses[0]) - ipValue(b.addresses[0]) ||
      hostDisplayName(a).localeCompare(hostDisplayName(b)) ||
      a.id.localeCompare(b.id),
  );
  const parents = new Map<string, TopologyLink>();
  for (const host of ordered) {
    const reported = (host.relationships ?? []).find(
      (link) =>
        link.kind === "runs_on" &&
        aliases.get(link.from_device_id)?.id === host.id &&
        aliases.has(link.to_device_id),
    );
    const inferred = host.parent_id ? aliases.get(host.parent_id) : undefined;
    const gateway = host.inventory?.interfaces.find(
      (item) => item.gateway,
    )?.gateway;
    const reportedGateway = gateway
      ? ordered.find((item) => item.addresses.includes(gateway))
      : undefined;
    const parent = reported
      ? aliases.get(reported.to_device_id)
      : (inferred ?? reportedGateway);
    if (!parent || parent.id === host.id) continue;
    parents.set(host.id, {
      source: parent.id,
      target: host.id,
      kind:
        reported || (!inferred && reportedGateway) ? "reported" : "inferred",
      reason: reported
        ? `Runs on ${hostDisplayName(parent)} · Reported`
        : inferred
          ? `${host.parent_reason ?? "Gateway or upstream router"} · Inferred`
          : `Default gateway reported by ${hostDisplayName(host)} · Reported`,
    });
  }
  // Malformed or conflicting upstream observations must not trap layout in a cycle.
  for (const host of ordered) {
    const path = [host.id];
    let current = host.id;
    while (parents.has(current)) {
      const next = parents.get(current)!.source;
      if (path.includes(next)) {
        const cycle = path
          .slice(path.indexOf(next))
          .map((id) => parents.get(id)!);
        cycle.sort(
          (a, b) =>
            Number(b.kind === "inferred") - Number(a.kind === "inferred") ||
            a.target.localeCompare(b.target),
        );
        parents.delete(cycle[0].target);
        break;
      }
      path.push(next);
      current = next;
    }
  }
  const children = new Map<string, Host[]>();
  for (const host of ordered) {
    const parent = parents.get(host.id);
    if (parent)
      children.set(parent.source, [
        ...(children.get(parent.source) ?? []),
        host,
      ]);
  }
  const nodeWidth = 232;
  const columnGap = 48;
  // Reserve expanded-card space so toggling metrics never shifts the hierarchy.
  const cardHeight = 190;
  const rowPitch = cardHeight + 44;
  const maxColumns = 4;
  const sizes = new Map<string, { width: number; height: number }>();
  function measure(host: Host): { width: number; height: number } {
    const cached = sizes.get(host.id);
    if (cached) return cached;
    const below = children.get(host.id) ?? [];
    const grid =
      below.length > maxColumns &&
      below.every((child) => !children.has(child.id));
    const childSizes = below.map(measure);
    const size = {
      width: Math.max(
        nodeWidth,
        grid
          ? maxColumns * (nodeWidth + columnGap) - columnGap
          : childSizes.reduce(
              (sum, child) => sum + child.width + columnGap,
              -columnGap,
            ),
      ),
      height: !below.length
        ? cardHeight
        : grid
          ? Math.ceil(below.length / maxColumns) * rowPitch + cardHeight
          : rowPitch + Math.max(...childSizes.map((child) => child.height)),
    };
    sizes.set(host.id, size);
    return size;
  }
  const positions = new Map<string, { x: number; y: number }>();
  function place(host: Host, x: number, y: number) {
    const size = measure(host);
    positions.set(host.id, { x: x + (size.width - nodeWidth) / 2, y });
    const below = children.get(host.id) ?? [];
    const grid =
      below.length > maxColumns &&
      below.every((child) => !children.has(child.id));
    let offset = x;
    below.forEach((child, index) => {
      place(
        child,
        grid ? x + (index % maxColumns) * (nodeWidth + columnGap) : offset,
        y + rowPitch * (grid ? 1 + Math.floor(index / maxColumns) : 1),
      );
      offset += measure(child).width + columnGap;
    });
  }
  let x = 0,
    y = 0,
    rowHeight = 0;
  for (const root of ordered.filter((host) => !parents.has(host.id))) {
    const size = measure(root);
    if (x && x + size.width > maxColumns * (nodeWidth + columnGap)) {
      x = 0;
      y += rowHeight + 60;
      rowHeight = 0;
    }
    place(root, x, y);
    x += size.width + columnGap;
    rowHeight = Math.max(rowHeight, size.height);
  }
  return {
    positions,
    links: [...parents.values()],
  };
}
