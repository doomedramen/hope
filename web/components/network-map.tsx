"use client";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Background,
  BackgroundVariant,
  Controls,
  ControlButton,
  Handle,
  MiniMap,
  Position,
  ReactFlow,
  useNodesInitialized,
  useReactFlow,
  useStore,
  getViewportForBounds,
  type Node,
  type NodeProps,
  type NodeChange,
  type Connection,
  type Edge,
} from "@xyflow/react";
import {
  Focus,
  Minus,
  Plus,
  Router,
  Scan,
  Network as NetworkIcon,
} from "lucide-react";
import { ipValue } from "@/lib/fleet";
import type { Host, MapEdit, MapState, Network } from "@/lib/protocol";
import { percent0 } from "@/lib/api";
import { Meter } from "@/components/ui/meter";
import { HostIcon } from "@/components/host-table";
import { deviceLabel, hostDisplayName } from "@/lib/devices";
import {
  cpuPercent,
  hostState,
  hostIssues,
  hasCurrentMetrics,
  memoryUsage,
  worstDisk,
  type Tone,
} from "@/lib/fleet";
import { topology } from "@/lib/network-topology";
import { useNow } from "@/lib/clock";
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
  const now = useNow(5);
  const state = data.host ? hostState(data.host, now) : null;
  const current = data.host ? hasCurrentMetrics(data.host, now) : false;
  const issue = data.host ? hostIssues(data.host, now)[0] : null;
  const tone = issue?.severity ?? state?.tone ?? "neutral";
  const cpu = data.host && current ? cpuPercent(data.host) : null;
  const memory = data.host && current ? memoryUsage(data.host) : null;
  const disk = data.host && current ? worstDisk(data.host) : null;
  return (
    <div
      className={`map-device ${selected ? "selected" : ""}`}
      data-kind={data.kind}
      data-tone={tone}
      data-host-id={data.host?.id}
    >
      <Handle type="target" position={Position.Top} />
      <div className="map-node-identity flex items-center gap-3">
        <div className="device-icon">
          {data.kind === "gateway" ? (
            <Router size={18} />
          ) : data.host ? (
            <HostIcon host={data.host} size={18} />
          ) : (
            <NetworkIcon size={18} />
          )}
        </div>
        <div className="min-w-0">
          <div className="truncate font-semibold" title={data.label}>
            {data.label}
          </div>
          <div
            className="mono truncate text-[11px] text-muted-foreground"
            title={data.subtitle}
          >
            {data.subtitle}
          </div>
        </div>
      </div>
      {state && (
        <div className="mt-2.5 flex items-center gap-2 text-[11px]">
          <span className="status-dot" data-tone={tone} aria-hidden="true" />
          <span className={`node-status tone-${tone}`} title={issue?.message}>
            {state.key === "stale"
              ? state.label
              : (issue?.message ?? state.label)}
          </span>
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
function MapViewport({
  metrics,
  inventoryKey,
}: {
  metrics: boolean;
  inventoryKey: string;
}) {
  const initialized = useNodesInitialized();
  const width = useStore((state) => state.width);
  const height = useStore((state) => state.height);
  const { getNodes, getEdges, getNodesBounds, setViewport, zoomIn, zoomOut } =
    useReactFlow<HopeNode>();
  const fitted = useRef<string>("");
  const fitAll = useCallback(() => {
    if (!width || !height) return;
    void setViewport(
      getViewportForBounds(getNodesBounds(getNodes()), width, height, 0.02, 1, {
        top: "24px",
        right: "24px",
        bottom: "76px",
        left: "24px",
      }),
    );
  }, [getNodes, getNodesBounds, setViewport, width, height]);
  const readableView = useCallback(() => {
    const nodes = getNodes().filter((node) => node.type === "hope");
    if (!nodes.length || !width || !height) return;
    const minimumZoom = 0.92;
    const all = getViewportForBounds(
      getNodesBounds(nodes),
      width,
      height,
      0.02,
      1,
      { top: "24px", right: "24px", bottom: "76px", left: "24px" },
    );
    if (all.zoom >= minimumZoom) {
      void setViewport(all);
      return;
    }
    const edges = getEdges();
    const score = (node: HopeNode) => {
      if (node.selected) return 100;
      if (node.type !== "hope" || !node.data.host) return 0;
      const issue = hostIssues(node.data.host)[0];
      const severity = issue?.severity === "critical" ? 3 : issue ? 2 : 0;
      const related = edges.some(
        (edge) => edge.source === node.id || edge.target === node.id,
      );
      return severity * 10 + Number(related);
    };
    const anchor = [...nodes].sort((a, b) => score(b) - score(a))[0];
    const familyIds = new Set([anchor.id]);
    for (const edge of edges) {
      if (edge.source === anchor.id) familyIds.add(edge.target);
      if (edge.target === anchor.id) familyIds.add(edge.source);
    }
    const family = nodes.filter((node) => familyIds.has(node.id));
    const viewport = getViewportForBounds(
      getNodesBounds(family),
      width,
      height,
      minimumZoom,
      1,
      0.12,
    );
    // A wide family can extend off screen. Its focal host must remain fully usable.
    const anchorBounds = getNodesBounds([anchor]);
    const screenX = anchorBounds.x * viewport.zoom + viewport.x;
    const screenY = anchorBounds.y * viewport.zoom + viewport.y;
    const right = Math.max(24, width - anchorBounds.width * viewport.zoom - 24);
    const bottom = Math.max(
      24,
      height - anchorBounds.height * viewport.zoom - 76,
    );
    viewport.x += Math.min(right, Math.max(24, screenX)) - screenX;
    viewport.y += Math.min(bottom, Math.max(24, screenY)) - screenY;
    void setViewport(viewport);
  }, [getNodes, getEdges, getNodesBounds, setViewport, width, height]);
  useEffect(() => {
    const key = `${inventoryKey}:${metrics}:${width}:${height}`;
    if (!initialized || fitted.current === key) return;
    const frame = requestAnimationFrame(() => {
      fitted.current = key;
      readableView();
    });
    return () => cancelAnimationFrame(frame);
  }, [initialized, inventoryKey, metrics, width, height, readableView]);
  return (
    <Controls
      showInteractive={false}
      showFitView={false}
      showZoom={false}
      orientation="horizontal"
      aria-label="Map navigation"
    >
      <ControlButton
        onClick={() => void zoomIn()}
        title="Zoom in"
        aria-label="Zoom in"
      >
        <Plus size={16} />
      </ControlButton>
      <ControlButton
        onClick={() => void zoomOut()}
        title="Zoom out"
        aria-label="Zoom out"
      >
        <Minus size={16} />
      </ControlButton>
      <ControlButton
        className="map-viewport-action"
        onClick={readableView}
        title="Show readable host cards, starting with selected or attention hosts"
        aria-label="Readable view"
      >
        <Focus size={15} />
        <span>Readable view</span>
      </ControlButton>
      <ControlButton
        className="map-viewport-action"
        onClick={fitAll}
        title="Fit all hosts; large inventories may appear small"
        aria-label="Fit all"
      >
        <Scan size={15} />
        <span>Fit all</span>
      </ControlButton>
    </Controls>
  );
}

/** The logical hierarchy has a new coordinate system; subnet positions remain relative. */
function positionKey(mode: string, node: HopeNode) {
  return `${mode === "topology" ? "topology:v2" : mode}:${node.id}`;
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
  showMetrics,
  selected,
  onSelect,
  onSave,
}: {
  hosts: Host[];
  networks: Network[];
  layout: MapState;
  mode: "topology" | "subnets";
  showMetrics: boolean;
  selected: string | null;
  onSelect: (id: string) => void;
  onSave: (edit: MapEdit) => Promise<void>;
}) {
  const now = useNow(5);
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
    const prefix = mode === "topology" ? "topology:v2" : mode;
    const position = (id: string, fallback: { x: number; y: number }) =>
      moving[`${prefix}:${id}`] ??
      layout.positions.find((p) => p.id === `${prefix}:${id}`) ??
      fallback;
    const tree = topology(hosts);
    const node = (
      host: Host,
      extra: Partial<Node<Data, "hope">> = {},
    ): Node<Data, "hope"> => ({
      id: host.id,
      type: "hope",
      position: { x: 0, y: 0 },
      selected: selected === host.id,
      ariaLabel: `Inspect ${hostDisplayName(host)}`,
      data: {
        label: hostDisplayName(host),
        subtitle:
          host.addresses.join(", ") || deviceLabel(host) || describe(host),
        host,
        kind: "host",
        health: showMetrics,
        tone: hostIssues(host, now)[0]?.severity ?? hostState(host, now).tone,
      },
      ...extra,
    });
    if (mode === "topology") {
      for (const host of hosts)
        nodes.push(
          node(host, {
            position: position(
              host.id,
              tree.positions.get(host.id) ?? { x: 0, y: 0 },
            ),
          }),
        );
      for (const link of tree.links)
        edges.push({
          id: `topology-${link.source}-${link.target}`,
          source: link.source,
          target: link.target,
          type: "smoothstep",
          className: `topology-edge ${link.kind}-edge`,
          ariaLabel: link.reason,
          data: { provenance: link.kind, reason: link.reason },
          label: link.kind === "reported" ? "Reported" : "Inferred",
        });
    } else {
      const rowPitch = showMetrics ? 226 : 154;
      const groups = [
        ...networks.map((n) => ({ id: n.id, label: n.name, cidr: n.cidr })),
        {
          id: "ungrouped",
          label: "Other hosts",
          cidr: "outside configured ranges",
        },
      ];
      let offset = 0;
      for (const group of groups) {
        const members = hosts
          .filter(
            (h) =>
              (networks.find((n) => h.network_ids.includes(n.id))?.id ??
                "ungrouped") === group.id,
          )
          .sort(
            (a, b) =>
              ipValue(a.addresses[0]) - ipValue(b.addresses[0]) ||
              a.id.localeCompare(b.id),
          );
        if (!members.length) continue;
        const columns = Math.min(
          6,
          Math.max(2, Math.ceil(Math.sqrt(members.length))),
        );
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
        const height = Math.max(...placed.map((p) => p.y + rowPitch));
        const root = `network-${group.id}`;
        nodes.push({
          id: root,
          type: "subnet",
          position: { x: offset, y: 0 },
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
        offset += width + 80;
        members.forEach((host, index) =>
          nodes.push(
            node(host, {
              parentId: root,
              extent: "parent",
              position: placed[index],
            }),
          ),
        );
      }
    }
    if (mode === "topology")
      for (const link of layout.links) {
        if (visibleIds.has(link.source) && visibleIds.has(link.target))
          edges.push({
            id: `manual-${link.id}`,
            source: link.source,
            target: link.target,
            label: `${link.label || "Connection"} · Manual`,
            type: "smoothstep",
            className: "manual-edge",
            ariaLabel: `${link.label || "Connection"} · Manual`,
          });
      }
    return {
      nodes: nodes.map((node) => ({
        ...node,
        measured: measurements[node.id],
      })),
      edges,
    };
  }, [
    hosts,
    networks,
    layout,
    mode,
    showMetrics,
    moving,
    selected,
    measurements,
    now,
  ]);
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
        <MapViewport
          metrics={showMetrics}
          inventoryKey={hosts
            .map((host) => host.id)
            .sort()
            .join("|")}
        />
        <Background
          variant={BackgroundVariant.Dots}
          gap={22}
          size={1.2}
          color="var(--grid)"
        />
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
        {mode === "subnets" ? (
          <span>
            <i className="legend-box" />
            Subnet membership
          </span>
        ) : (
          <>
            <span>
              <i className="legend-line reported" />
              Reported relationship
            </span>
            <span>
              <i className="legend-line inferred" />
              Inferred routing
            </span>
            <span>
              <i className="legend-line manual" />
              Manual link
            </span>
            <details className="map-explanation">
              <summary>About this map</summary>
              <div>
                <p>
                  Links describe logical relationships, not physical switch
                  ports. Some hosts have no known relationship yet.
                </p>
                <p>
                  Drag the canvas to explore. Use Readable view for legible
                  cards or Fit all to see the whole inventory.
                </p>
                <p>
                  Drag handles to link hosts; double-click a manual link to
                  remove it. Host positions and manual links save automatically.
                </p>
              </div>
            </details>
          </>
        )}
      </div>
    </div>
  );
}
