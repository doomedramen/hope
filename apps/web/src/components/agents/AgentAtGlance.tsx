import { useMutation, useQuery } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import {
  fetchAgentCollection,
  fetchAgentUpdates,
  updateAgent,
  getUserFacingError,
  type AgentDetail,
  type AgentMetricsResponse,
} from "@/lib/api";
import { formatRelative } from "@/lib/format";
import type { NavigateAgent } from "./AgentOperations";

type Delivery = {
  queued_records?: number;
  queued_bytes?: number;
  oldest_at?: number;
  loss?: { records?: number; reason?: string };
  budgets?: Record<
    string,
    { dropped_records: number; last_dropped_at: number }
  >;
  sources?: Record<string, number>;
};
const activeStates = new Set([
  "pending",
  "verifying",
  "downloading",
  "installing",
  "restarting",
  "awaiting_health",
]);
function percent(value: unknown) {
  return typeof value === "number" && Number.isFinite(value)
    ? `${value.toFixed(1)}%`
    : "Unavailable";
}

export function AgentAtGlance({
  agent,
  metrics,
  loading,
  error,
  navigate,
}: {
  agent: AgentDetail;
  metrics?: AgentMetricsResponse;
  loading: boolean;
  error: unknown;
  navigate: NavigateAgent;
}) {
  const collection = useQuery({
    queryKey: ["agent-collection", agent.id],
    queryFn: () => fetchAgentCollection(agent.id),
    refetchInterval: 15_000,
  });
  const updates = useQuery({
    queryKey: ["agent-updates", agent.id],
    queryFn: () => fetchAgentUpdates(agent.id),
    refetchInterval: 10_000,
  });
  const update = useMutation({
    mutationFn: (version: string) => updateAgent(agent.id, version),
    onSuccess: () => updates.refetch(),
  });
  const sample = metrics?.latest;
  const values = sample?.metrics;
  const cpu = (values?.cpu as { usage_percent?: number } | undefined)
    ?.usage_percent;
  const memory = (values?.memory as { used_percent?: number } | undefined)
    ?.used_percent;
  const delivery = values?.delivery as
    { metrics?: Delivery; logs?: Delivery } | undefined;
  const sources = collection.data?.config
    ? [
        ...collection.data.config.journal_units.map(
          (name) => `journal:${name}`,
        ),
        ...collection.data.config.docker_containers.map(
          (name) => `docker:${name}`,
        ),
      ]
    : undefined;
  const healthySources =
    sources?.filter(
      (source) =>
        (sample ? Date.parse(sample.collected_at) / 1000 : Date.now() / 1000) -
          (delivery?.logs?.sources?.[source] ?? 0) <
        90,
    ).length ?? 0;
  const queued =
    (delivery?.metrics?.queued_records ?? 0) +
    (delivery?.logs?.queued_records ?? 0);
  const throttled = Object.values(delivery?.logs?.budgets ?? {}).some(
    (budget) => budget.last_dropped_at * 1000 > Date.now() - 60000,
  );
  const lost =
    (delivery?.metrics?.loss?.records ?? 0) +
    (delivery?.logs?.loss?.records ?? 0);
  const fresh =
    metrics?.freshness.state === "fresh" &&
    Boolean(sample) &&
    Date.now() - Date.parse(sample!.collected_at) <= 45_000;
  const lastUpdate = updates.data?.operations?.[0];
  const active = updates.data?.operations?.find((operation) =>
    activeStates.has(operation.state),
  );
  const rolledBack = lastUpdate?.state === "rolled_back";
  const updateFailed = lastUpdate?.state === "failed" || rolledBack;
  const logsFailing = Boolean(
    sources?.length &&
    collection.data?.applied_revision === collection.data?.config.revision &&
    healthySources < sources.length,
  );
  const missing =
    agent.collector_status?.filter(
      (collector) => collector.status !== "available",
    ) ?? [];
  const snapshot = agent.inventory_snapshot;
  const inventoryDelayed =
    snapshot != null &&
    (!Number.isFinite(Date.parse(snapshot.collected_at)) ||
      Date.now() - Date.parse(snapshot.collected_at) > 45 * 60_000);
  let condition = "Data arriving normally";
  let message = "Metrics are current. No delivery backlog reported.";
  let action: { label: string; tab: string; source?: string } | undefined;
  let attention = false;
  if (agent.status !== "online") {
    attention = true;
    condition =
      agent.status === "revoked"
        ? "Agent access revoked"
        : "Agent is not reporting";
    message = `Last contact ${formatRelative(agent.last_seen)}. Values below are the last received readings.`;
    action = {
      label: "Inspect connection events",
      tab: "logs",
      source: "agent",
    };
  } else if (!sample) {
    condition = error
      ? "Data status unavailable"
      : loading
        ? "Checking agent data…"
        : "Waiting for first metrics";
    message = error
      ? "Could not load the latest readings. Try refreshing."
      : loading
        ? "Loading readings and collection status."
        : "The agent connected. Resource readings have not arrived yet.";
  } else if (!fresh) {
    attention = true;
    condition = "Metrics are delayed";
    message = `Last sample ${formatRelative(sample.collected_at)}. Check collection and delivery.`;
    action = { label: "Inspect agent events", tab: "logs", source: "agent" };
  } else if (updateFailed) {
    attention = true;
    condition = rolledBack ? "Update rolled back" : "Update failed";
    message = rolledBack
      ? `Version ${agent.agent_version ?? "previous"} restored. Collection has resumed.`
      : (lastUpdate?.last_error ?? "Review the failed update before retrying.");
    action = { label: "Review update", tab: "settings" };
  } else if (logsFailing) {
    attention = true;
    condition = "Log collection needs attention";
    message = `${healthySources} of ${sources!.length} selected sources are collecting.`;
    action = { label: "Inspect log diagnostics", tab: "logs", source: "agent" };
  } else if ((cpu ?? 0) >= 90 || (memory ?? 0) >= 90) {
    attention = true;
    condition = "High resource use";
    message = `${(cpu ?? 0) >= 90 ? "CPU" : "Memory"} usage is above 90%. Open metrics to check the trend.`;
    action = { label: "Inspect resource history", tab: "metrics" };
  } else if (throttled) {
    attention = true;
    condition = "Log source limit reached";
    message =
      "Some log records exceeded a configured source budget. Review the gap diagnostic or increase the limit.";
    action = { label: "Review log gaps", tab: "logs" };
  } else if (lost > 0) {
    attention = true;
    condition = "Recorded data gaps";
    message = `${lost} records lost within the local delivery history. Current readings are arriving.`;
    action = { label: "Inspect delivery details", tab: "settings" };
  } else if (snapshot === null) {
    attention = true;
    condition = "Inventory has not arrived";
    message = "Metrics are current, but no host inventory has been received.";
    action = { label: "Inspect agent events", tab: "logs", source: "agent" };
  } else if (inventoryDelayed) {
    attention = true;
    condition = "Inventory is delayed";
    message = `Metrics are current. Inventory was collected ${formatRelative(snapshot!.collected_at)}; host details may be out of date.`;
    action = { label: "Inspect agent events", tab: "logs", source: "agent" };
  } else if (missing.length > 0 || snapshot?.complete === false) {
    attention = true;
    condition = "Inventory coverage is limited";
    message = "Metrics are current. Some host details could not be collected.";
  } else if (queued > 2) {
    condition = "Catching up on buffered data";
    message = `${queued} records waiting to send at the last sample.`;
  }
  const logStatus = !sources
    ? "Checking…"
    : !sources.length
      ? "Not enabled"
      : !sample
        ? loading
          ? "Checking…"
          : "Not received"
        : collection.data?.applied_revision !== collection.data?.config.revision
          ? "Applying settings"
          : !fresh || agent.status !== "online"
            ? `${healthySources}/${sources.length} at last sample`
            : `${healthySources}/${sources.length} collecting`;
  const target = updates.data?.target_version;
  return (
    <section aria-label="Agent at a glance" className="space-y-4">
      <div
        className={`border-l-4 pl-3 ${attention ? "border-amber-500" : !sample || error ? "border-muted-foreground" : "border-emerald-600"}`}
      >
        <h2 className="text-base font-semibold">{condition}</h2>
        <p className="mt-1 text-sm text-muted-foreground">{message}</p>
        {action && (
          <button
            className="mt-1.5 text-sm font-medium underline underline-offset-4 focus-visible:outline focus-visible:outline-ring"
            onClick={() => navigate({ tab: action.tab, source: action.source })}
          >
            {action.label}
          </button>
        )}
      </div>
      <dl
        className="grid grid-cols-2 gap-x-6 gap-y-3 sm:grid-cols-4"
        aria-label="Current readings"
      >
        <div>
          <dt className="text-xs text-muted-foreground">
            CPU{sample && !fresh ? " · last reading" : ""}
          </dt>
          <dd className="mt-1 text-xl font-semibold tabular-nums">
            {loading ? "…" : percent(cpu)}
          </dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">
            Memory{sample && !fresh ? " · last reading" : ""}
          </dt>
          <dd className="mt-1 text-xl font-semibold tabular-nums">
            {loading ? "…" : percent(memory)}
          </dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">Latest metrics</dt>
          <dd className="mt-1 text-sm font-medium">
            {sample
              ? formatRelative(sample.collected_at)
              : loading
                ? "Loading…"
                : "Not received"}
          </dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">
            Host & container logs
          </dt>
          <dd className="mt-1 text-sm font-medium">{logStatus}</dd>
        </div>
      </dl>
      <div className="flex flex-wrap items-center justify-between gap-2 border-t pt-3 text-sm">
        <span>
          {active
            ? `Update ${active.state.replaceAll("_", " ")}`
            : `Version ${agent.agent_version ?? "unknown"}`}
          {!active && target === agent.agent_version ? " · up to date" : ""}
        </span>
        {target && target !== agent.agent_version && !active ? (
          <Button
            size="sm"
            variant={attention ? "outline" : "default"}
            disabled={update.isPending}
            onClick={() => update.mutate(target)}
          >
            {updateFailed && lastUpdate?.target_version === target
              ? "Retry"
              : "Update to"}{" "}
            {target}
          </Button>
        ) : !sources?.length && sources ? (
          <Button
            size="sm"
            variant="outline"
            onClick={() => navigate({ tab: "settings" })}
          >
            Choose log sources
          </Button>
        ) : agent.device_id ? (
          <a
            className="underline underline-offset-4"
            href={`/devices?device=${agent.device_id}`}
          >
            Open device
          </a>
        ) : null}
      </div>
      {updates.data?.blocked_reason?.startsWith("Bootstrap") && (
        <p className="text-xs text-muted-foreground">
          One-time installer upgrade required for managed updates.
        </p>
      )}
      {missing.length > 0 && (
        <div className="text-sm text-muted-foreground">
          <p>
            Inventory coverage limited:{" "}
            {missing.map((item) => item.capability).join(", ")}.
          </p>
          <details className="mt-1">
            <summary className="cursor-pointer underline underline-offset-4 focus-visible:outline focus-visible:outline-ring">
              View reasons
            </summary>
            <ul className="mt-2 space-y-1">
              {missing.map((item) => (
                <li key={item.capability}>
                  <span className="font-medium">{item.capability}</span>:{" "}
                  {item.error || `${item.status}. No reason reported.`}
                </li>
              ))}
            </ul>
          </details>
        </div>
      )}
      {(Boolean(error) ||
        collection.isError ||
        updates.isError ||
        update.isError) && (
        <p role="status" className="text-sm text-destructive">
          {update.error
            ? getUserFacingError(update.error, "Update request failed")
            : "Some status could not refresh. Showing the last received data."}
        </p>
      )}
    </section>
  );
}
