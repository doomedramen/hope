"use client";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  Background,
  Controls,
  Handle,
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
import { Computer, Router, Network as NetworkIcon } from "lucide-react";
import type { Host, MapEdit, MapState, Network } from "@/lib/protocol";
import { percentage, status } from "@/lib/api";
import { Badge } from "@/components/ui/badge";
import "@xyflow/react/dist/style.css";

type Data = {
  label: string;
  subtitle: string;
  host?: Host;
  kind: "host" | "network" | "gateway";
  health: boolean;
};
type HopeNode = Node<Data, "hope">;
function DeviceNode({ data, selected }: NodeProps<HopeNode>) {
  const Icon =
    data.kind === "gateway"
      ? Router
      : data.kind === "network"
        ? NetworkIcon
        : Computer;
  const state = data.host ? status(data.host) : null;
  return (
    <div
      className={`map-device ${selected ? "selected" : ""}`}
      data-kind={data.kind}
    >
      <Handle type="target" position={Position.Top} />
      <div className="flex items-center gap-3">
        <div className="device-icon">
          <Icon size={20} />
        </div>
        <div className="min-w-0">
          <div className="truncate font-medium">{data.label}</div>
          <div className="truncate text-xs text-muted-foreground">
            {data.subtitle}
          </div>
        </div>
      </div>
      {state && (
        <div className="mt-3 flex items-center justify-between gap-2">
          <span
            className={`status-dot ${state === "Reporting" || state === "Observed" ? "live" : "stale"}`}
          />
          <span className="mr-auto text-xs text-muted-foreground">{state}</span>
          {data.host?.agent_id && <Badge variant="secondary">Agent</Badge>}
        </div>
      )}
      {data.health && data.host && (
        <div className="node-health">
          <span>
            CPU <strong>{percentage(data.host.metrics?.cpu_percent)}</strong>
          </span>
          <span>
            RAM{" "}
            <strong>
              {data.host.metrics?.memory_total_bytes &&
              data.host.metrics.memory_used_bytes != null
                ? percentage(
                    (data.host.metrics.memory_used_bytes /
                      data.host.metrics.memory_total_bytes) *
                      100,
                  )
                : "—"}
            </strong>
          </span>
        </div>
      )}
      <Handle type="source" position={Position.Bottom} />
    </div>
  );
}
const nodeTypes = { hope: DeviceNode };
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
    const groups = [
      ...networks.map((n) => ({ id: n.id, label: n.name, cidr: n.cidr })),
      {
        id: "ungrouped",
        label: "Other hosts",
        cidr: "Outside configured ranges",
      },
    ];
    let offset = 0;
    for (const group of groups) {
      const members = hosts.filter(
        (h) =>
          (networks.find((n) => h.network_ids.includes(n.id))?.id ??
            "ungrouped") === group.id,
      );
      if (!members.length && group.id === "ungrouped") continue;
      const columns = Math.min(
        8,
        Math.max(2, Math.ceil(Math.sqrt(members.length))),
      );
      const x = offset;
      const center = (columns - 1) * 140 + 20;
      offset += columns * 280 + 60;
      const root = `network-${group.id}`;
      nodes.push({
        id: root,
        type: "hope",
        position: position(root, {
          x: x + center,
          y: mode === "subnets" ? 0 : -300,
        }),
        data: {
          label: group.label,
          subtitle: group.cidr,
          kind: "network",
          health: false,
        },
        draggable: false,
        connectable: false,
      });
      members.forEach((host, index) => {
        nodes.push({
          id: host.id,
          type: "hope",
          position: position(host.id, {
            x: x + (index % columns) * 280,
            y: 160 + Math.floor(index / columns) * 180,
          }),
          selected: selected === host.id,
          data: {
            label: host.name,
            subtitle: host.addresses.join(", "),
            host,
            kind: "host",
            health: mode === "health",
          },
        });
        if (mode === "subnets") {
          if (group.id !== "ungrouped")
            edges.push({
              id: `${root}-${host.id}`,
              source: root,
              target: host.id,
              label: "Member",
              className: "membership-edge",
              type: "smoothstep",
            });
        } else {
          const gateway = host.inventory?.interfaces.find(
            (i) => i.gateway,
          )?.gateway;
          if (gateway) {
            const gatewayHost = hosts.find((h) =>
              h.addresses.includes(gateway),
            );
            const source = gatewayHost?.id ?? `gateway-${group.id}-${gateway}`;
            if (!gatewayHost && !nodes.some((n) => n.id === source))
              nodes.push({
                id: source,
                type: "hope",
                position: position(source, { x: x + center, y: -150 }),
                data: {
                  label: gateway,
                  subtitle: "Reported default gateway",
                  kind: "gateway",
                  health: false,
                },
                connectable: false,
              });
            if (source !== host.id)
              edges.push({
                id: `gateway-${source}-${host.id}`,
                source,
                target: host.id,
                label: "Reported gateway",
                type: "smoothstep",
              });
          } else if (group.id !== "ungrouped")
            edges.push({
              id: `${root}-${host.id}`,
              source: root,
              target: host.id,
              label: "Subnet membership",
              className: "membership-edge",
              type: "smoothstep",
            });
        }
      });
    }
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
          if (node.data.host) onSelect(node.id);
        }}
        onNodeDrag={(_, node) =>
          setMoving((previous) => ({
            ...previous,
            [`${mode}:${node.id}`]: node.position,
          }))
        }
        onNodeDragStop={(_, node) => {
          const key = `${mode}:${node.id}`;
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
        <Background gap={24} size={1} color="var(--border)" />
        <Controls showInteractive={false} />
      </ReactFlow>
      <div className="map-legend">
        <span>
          <i className="legend-line" />
          Reported relationship
        </span>
        <span>
          <i className="legend-line dashed" />
          Subnet membership
        </span>
        <span>
          Drag handles to add manual links. Double-click a manual link to remove
          it.
        </span>
      </div>
    </div>
  );
}
