"use client";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MiniMap,
  Position,
  ReactFlow,
  useNodesInitialized,
  useReactFlow,
  type Node,
  type NodeProps,
  type NodeChange,
  type Connection,
  type Edge,
} from "@xyflow/react";
import { Bot, Router, Network as NetworkIcon } from "lucide-react";
import { ipValue } from "@/lib/fleet";
import type { Host, MapEdit, MapState, Network } from "@/lib/protocol";
import { percent0 } from "@/lib/api";
import { Meter } from "@/components/ui/meter";
import { HostIcon } from "@/components/host-table";
import { deviceLabel } from "@/lib/devices";
import {
  cpuPercent,
  hostState,
  memoryUsage,
  worstDisk,
  type Tone,
} from "@/lib/fleet";
import "@xyflow/react/dist/style.css";

type Data = {
  label: string;
  subtitle: string;
  host?: Host;
  kind: "host" | "network" | "gateway";
  health: boolean;
  tone: Tone;
};
type SubnetData = {
  label: string;
  cidr: string;
  members: number;
  responding: number;
};
type HopeNode = Node<Data, "hope"> | Node<SubnetData, "subnet">;
/** A range drawn as a container: membership is a grouping, not a relationship, so it needs no edges. */
function SubnetNode({ data }: NodeProps<Node<SubnetData, "subnet">>) {
  return (
    <div className="subnet-group" data-testid="subnet-group">
      <Handle type="target" position={Position.Top} isConnectable={false} />
      <header>
        <NetworkIcon size={14} />
        <strong>{data.label}</strong>
        <span className="mono">{data.cidr}</span>
        <span className="ml-auto">
          {data.responding} / {data.members} responding
        </span>
      </header>
    </div>
  );
}
function DeviceNode({ data, selected }: NodeProps<Node<Data, "hope">>) {
  const state = data.host ? hostState(data.host) : null;
  const cpu = data.host ? cpuPercent(data.host) : null;
  const memory = data.host ? memoryUsage(data.host) : null;
  const disk = data.host ? worstDisk(data.host) : null;
  return (
    <div
      className={`map-device ${selected ? "selected" : ""}`}
      data-kind={data.kind}
    >
      <Handle type="target" position={Position.Top} />
      <div className="flex items-center gap-3">
        <div
          className="device-icon"
          data-tone={data.kind === "host" ? data.tone : undefined}
        >
          {data.kind === "gateway" ? (
            <Router size={18} />
          ) : data.host ? (
            <HostIcon host={data.host} size={18} />
          ) : (
            <NetworkIcon size={18} />
          )}
        </div>
        <div className="min-w-0">
          <div className="truncate font-semibold">{data.label}</div>
          <div className="mono truncate text-[11px] text-muted-foreground">
            {data.subtitle}
          </div>
        </div>
      </div>
      {state && (
        <div className="mt-2.5 flex items-center gap-2 text-[11px]">
          <span
            className="status-dot"
            data-tone={state.tone}
            aria-hidden="true"
          />
          <span className={`mr-auto tone-${state.tone}`}>{state.label}</span>
          {data.host?.agent_id && !data.host.agent_revoked && (
            <span className="inline-flex items-center gap-1 text-muted-foreground">
              <Bot size={12} />
              Agent
            </span>
          )}
        </div>
      )}
      {data.health && data.host && (
        <div className="node-health">
          <span>
            CPU <Meter value={cpu} thin />
            <strong>{percent0(cpu)}</strong>
          </span>
          <span>
            RAM <Meter value={memory} thin />
            <strong>{percent0(memory)}</strong>
          </span>
          <span>
            Disk <Meter value={disk?.percent} thin />
            <strong>{percent0(disk?.percent)}</strong>
          </span>
        </div>
      )}
      <Handle type="source" position={Position.Bottom} />
    </div>
  );
}
const nodeTypes = { hope: DeviceNode, subnet: SubnetNode };
const NODE_WIDTH = 232;
const COLUMN_PITCH = 256;
const GROUP_PADDING = 20;
const GROUP_HEADER = 54;
/** What a discovered host is, when its name is only its address. */
function describe(host: Host) {
  if (host.inventory) return host.inventory.os;
  const method = host.discovery_method.toUpperCase();
  return host.services.length
    ? `${method} · ${host.services.length} ${host.services.length === 1 ? "service" : "services"}`
    : `Found by ${method}`;
}
function InitialViewport() {
  const initialized = useNodesInitialized();
  const { fitView } = useReactFlow();
  const fitted = useRef(false);
  useEffect(() => {
    if (!initialized || fitted.current) return;
    const frame = requestAnimationFrame(() => {
      fitted.current = true;
      void fitView({ padding: 0.25, maxZoom: 1 });
    });
    return () => cancelAnimationFrame(frame);
  }, [initialized, fitView]);
  return null;
}

/** Saved-position key: devices inside a range are relative to it; routers above ranges are not. */
function positionKey(mode: string, node: HopeNode) {
  return node.parentId ? `${mode}:${node.id}` : `${mode}:router:${node.id}`;
}

const TONE_COLORS: Record<Tone, string> = {
  good: "var(--status-good)",
  warning: "var(--status-warning)",
  critical: "var(--status-critical)",
  neutral: "var(--status-neutral)",
};

export function NetworkMap({
  hosts,
  networks,
  layout,
  mode,
  selected,
  onSelect,
  onSave,
}: {
  hosts: Host[];
  networks: Network[];
  layout: MapState;
  mode: "topology" | "subnets" | "health";
  selected: string | null;
  onSelect: (id: string) => void;
  onSave: (edit: MapEdit) => Promise<void>;
}) {
  const [moving, setMoving] = useState<
    Record<string, { x: number; y: number }>
  >({});
  const [measurements, setMeasurements] = useState<
    Record<string, { width: number; height: number }>
  >({});
  const measure = (changes: NodeChange<HopeNode>[]) => {
    const dimensions = changes.filter(
      (change) => change.type === "dimensions" && change.dimensions,
    );
    if (!dimensions.length) return;
    setMeasurements((previous) => {
      const next = { ...previous };
      let changed = false;
      for (const change of dimensions) {
        if (change.type !== "dimensions" || !change.dimensions) continue;
        if (
          previous[change.id]?.width !== change.dimensions.width ||
          previous[change.id]?.height !== change.dimensions.height
        ) {
          next[change.id] = change.dimensions;
          changed = true;
        }
      }
      return changed ? next : previous;
    });
  };
  const { nodes, edges } = useMemo(() => {
    const nodes: HopeNode[] = [];
    const edges: Edge[] = [];
    const visibleIds = new Set(hosts.map((h) => h.id));
    const position = (id: string, fallback: { x: number; y: number }) =>
      moving[`${mode}:${id}`] ??
      layout.positions.find((p) => p.id === `${mode}:${id}`) ??
      fallback;
    const rowPitch = mode === "health" ? 186 : 118;
    const groups = [
      ...networks.map((n) => ({ id: n.id, label: n.name, cidr: n.cidr })),
      {
        id: "ungrouped",
        label: "Other hosts",
        cidr: "outside configured ranges",
      },
    ];
    // Outside "subnets" mode the map is a hierarchy: a device's parent is the gateway or upstream
    // router the server inferred (or, for an agent, its reported default gateway). Routers that
    // have children are lifted out of their range and drawn above the ranges they serve, so one
    // edge per range replaces a line to every device.
    const hierarchical = mode !== "subnets";
    const byId = new Map(hosts.map((h) => [h.id, h]));
    const parentOf = (host: Host): string | null => {
      if (
        host.parent_id &&
        byId.has(host.parent_id) &&
        host.parent_id !== host.id
      )
        return host.parent_id;
      const gateway = host.inventory?.interfaces.find(
        (i) => i.gateway,
      )?.gateway;
      const gatewayHost = gateway
        ? hosts.find((h) => h.addresses.includes(gateway))
        : undefined;
      return gatewayHost && gatewayHost.id !== host.id ? gatewayHost.id : null;
    };
    const routers = new Set<string>();
    if (hierarchical)
      for (const host of hosts) {
        const parent = parentOf(host);
        if (parent) routers.add(parent);
      }
    const node = (
      host: Host,
      extra: Partial<Node<Data, "hope">>,
    ): Node<Data, "hope"> => {
      const label = deviceLabel(host);
      const address = host.addresses[0];
      return {
        id: host.id,
        type: "hope",
        position: { x: 0, y: 0 },
        selected: selected === host.id,
        data: {
          label: host.name,
          subtitle:
            host.name === address
              ? (label ?? describe(host))
              : [label, host.addresses.join(", ")].filter(Boolean).join(" · "),
          host,
          kind: "host",
          health: mode === "health",
          tone: hostState(host).tone,
        },
        ...extra,
      };
    };
    // Range id (or "ungrouped") → centre x, for placing the routers that serve it.
    const centres = new Map<string, number>();
    // Range id → the parent most of its devices sit behind.
    const feeds = new Map<string, string>();
    let offset = 0;
    for (const group of groups) {
      const members = hosts
        .filter(
          (h) =>
            !routers.has(h.id) &&
            (networks.find((n) => h.network_ids.includes(n.id))?.id ??
              "ungrouped") === group.id,
        )
        .sort(
          (a, b) =>
            ipValue(a.addresses[0]) - ipValue(b.addresses[0]) ||
            a.name.localeCompare(b.name),
        );
      if (!members.length && group.id === "ungrouped") continue;
      const columns = Math.min(
        6,
        Math.max(2, Math.ceil(Math.sqrt(members.length || 1))),
      );
      const rows = Math.max(1, Math.ceil(members.length / columns));
      // Positions are relative to the container; a host dragged past the grid enlarges it.
      const placed = members.map((host, index) =>
        position(host.id, {
          x: GROUP_PADDING + (index % columns) * COLUMN_PITCH,
          y:
            GROUP_HEADER +
            GROUP_PADDING +
            Math.floor(index / columns) * rowPitch,
        }),
      );
      const width = Math.max(
        GROUP_PADDING * 2 + (columns - 1) * COLUMN_PITCH + NODE_WIDTH,
        ...placed.map((p) => p.x + NODE_WIDTH + GROUP_PADDING),
      );
      const height = Math.max(
        GROUP_HEADER + GROUP_PADDING + rows * rowPitch + GROUP_PADDING / 2,
        ...placed.map((p) => p.y + rowPitch),
      );
      const x = offset;
      offset += width + 80;
      centres.set(group.id, x + width / 2);
      const root = `network-${group.id}`;
      const counts = new Map<string, number>();
      if (hierarchical)
        for (const host of members) {
          const parent = parentOf(host);
          if (parent) counts.set(parent, (counts.get(parent) ?? 0) + 1);
        }
      const dominant = [...counts.entries()].sort(
        (a, b) => b[1] - a[1],
      )[0]?.[0];
      if (dominant) feeds.set(group.id, dominant);
      nodes.push({
        id: root,
        type: "subnet",
        position: { x, y: 0 },
        style: { width, height },
        data: {
          label: group.label,
          cidr: group.cidr,
          members: members.length,
          responding: members.filter((h) => hostState(h).tone === "good")
            .length,
        },
        draggable: false,
        selectable: false,
        connectable: false,
      });
      if (dominant)
        edges.push({
          id: `feeds-${dominant}-${group.id}`,
          source: dominant,
          target: root,
          type: "smoothstep",
          className: "gateway-edge",
        });
      members.forEach((host, index) => {
        nodes.push(
          node(host, {
            parentId: root,
            extent: "parent",
            position: placed[index],
          }),
        );
        const parent = hierarchical ? parentOf(host) : null;
        // A device behind a different router than the rest of its range gets its own edge.
        if (parent && parent !== dominant)
          edges.push({
            id: `parent-${parent}-${host.id}`,
            source: parent,
            target: host.id,
            type: "smoothstep",
            className: "gateway-edge",
          });
      });
    }
    if (hierarchical && routers.size) {
      // Depth 0 is the top of the chain (a router with no router above it).
      const depth = (id: string, seen = new Set<string>()): number => {
        const host = byId.get(id);
        const parent = host ? parentOf(host) : null;
        if (!parent || seen.has(parent) || !routers.has(parent)) return 0;
        seen.add(id);
        return 1 + depth(parent, seen);
      };
      const levels = [...routers].map((id) => ({ id, depth: depth(id) }));
      const deepest = Math.max(...levels.map((l) => l.depth));
      const xOf = new Map<string, number>();
      // Bottom-up: a router sits over the ranges it feeds, an upstream router over its routers.
      for (let level = deepest; level >= 0; level--) {
        let free = offset;
        for (const { id } of levels.filter((l) => l.depth === level)) {
          const below = [
            ...[...feeds.entries()]
              .filter(([, parent]) => parent === id)
              .map(([group]) => centres.get(group) ?? 0),
            ...[...routers]
              .filter((r) => {
                const host = byId.get(r);
                return host && parentOf(host) === id && xOf.has(r);
              })
              .map((r) => (xOf.get(r) ?? 0) + NODE_WIDTH / 2),
          ];
          const centre = below.length
            ? below.reduce((a, b) => a + b, 0) / below.length
            : (free += COLUMN_PITCH) - COLUMN_PITCH / 2;
          xOf.set(id, centre - NODE_WIDTH / 2);
        }
      }
      for (const { id, depth: level } of levels) {
        const host = byId.get(id);
        if (!host) continue;
        nodes.push(
          node(host, {
            // Saved apart from its position inside a range: the coordinates mean different things.
            position: position(`router:${id}`, {
              x: xOf.get(id) ?? 0,
              y: -(deepest - level + 1) * (rowPitch + 60),
            }),
          }),
        );
        const parent = parentOf(host);
        if (parent && routers.has(parent))
          edges.push({
            id: `upstream-${parent}-${id}`,
            source: parent,
            target: id,
            type: "smoothstep",
            className: "gateway-edge",
          });
      }
    }
    if (mode !== "subnets")
      for (const link of layout.links)
        if (visibleIds.has(link.source) && visibleIds.has(link.target))
          edges.push({
            id: `manual-${link.id}`,
            source: link.source,
            target: link.target,
            label: `${link.label || "Connection"} · Manual`,
            type: "smoothstep",
            className: "manual-edge",
          });
    return {
      nodes: nodes.map((node) => ({
        ...node,
        measured: measurements[node.id],
      })),
      edges,
    };
  }, [hosts, networks, layout, mode, moving, selected, measurements]);
  const connect = (connection: Connection) => {
    if (
      !connection.source ||
      !connection.target ||
      connection.source === connection.target
    )
      return;
    if (
      !hosts.some((h) => h.id === connection.source) ||
      !hosts.some((h) => h.id === connection.target)
    )
      return;
    void onSave({
      operation: "add_link",
      link: {
        id: crypto.randomUUID(),
        source: connection.source,
        target: connection.target,
        label: "Connection",
      },
    });
  };
  return (
    <div className="network-canvas" data-testid="network-map">
      <ReactFlow<HopeNode>
        nodes={nodes}
        onNodesChange={measure}
        edges={edges}
        nodeTypes={nodeTypes}
        minZoom={0.02}
        maxZoom={1.6}
        onNodeClick={(_, node) => {
          if (node.type === "hope" && (node.data as Data).host)
            onSelect(node.id);
        }}
        onNodeDrag={(_, node) =>
          setMoving((previous) => ({
            ...previous,
            [positionKey(mode, node)]: node.position,
          }))
        }
        onNodeDragStop={(_, node) => {
          const key = positionKey(mode, node);
          const position = node.position;
          void onSave({
            operation: "set_position",
            position: { id: key, ...position },
          }).then(() => {
            setMoving((previous) => {
              if (
                previous[key]?.x !== position.x ||
                previous[key]?.y !== position.y
              )
                return previous;
              const next = { ...previous };
              delete next[key];
              return next;
            });
          });
        }}
        onConnect={connect}
        onEdgeDoubleClick={(_, edge) => {
          if (edge.id.startsWith("manual-"))
            void onSave({
              operation: "remove_link",
              id: edge.id.slice("manual-".length),
            });
        }}
        deleteKeyCode={null}
        proOptions={{ hideAttribution: false }}
      >
        <InitialViewport />
        <Background
          variant={BackgroundVariant.Dots}
          gap={22}
          size={1.2}
          color="var(--grid)"
        />
        <Controls showInteractive={false} />
        {nodes.length > 8 && (
          <MiniMap
            pannable
            zoomable
            nodeColor={(node) =>
              node.type === "subnet"
                ? "var(--muted)"
                : TONE_COLORS[(node.data as Data).tone]
            }
            nodeStrokeWidth={0}
            maskColor="color-mix(in srgb, var(--background) 70%, transparent)"
          />
        )}
      </ReactFlow>
      <div className="map-legend">
        <span>
          <i className="legend-box" />
          Range (hosts grouped by subnet)
        </span>
        {mode !== "subnets" && (
          <>
            <span>
              <i className="legend-line" />
              Gateway or upstream router
            </span>
            <span>
              <i className="legend-line manual" />
              Manual link
            </span>
            <span>
              Drag handles to add manual links. Double-click a manual link to
              remove it.
            </span>
          </>
        )}
      </div>
    </div>
  );
}
