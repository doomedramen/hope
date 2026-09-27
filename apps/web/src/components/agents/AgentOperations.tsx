import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import {
  fetchAgentLogs,
  fetchAgentCollection,
  saveAgentCollection,
  fetchAgentUpdates,
  saveAgentUpdatePolicy,
  updateAgent,
  getUserFacingError,
  type AgentCollectionSettings,
  type AgentUpdatePolicy,
  type AgentLogEntry,
} from "@/lib/api";

export type AgentLocation = {
  agent?: string;
  tab?: string;
  q?: string;
  status?: string;
  cursor?: string;
  range?: string;
  logq?: string;
  source?: string;
  severity?: string;
  from?: string;
  to?: string;
};
export type NavigateAgent = (patch: Partial<AgentLocation>) => void;
const selectClass =
  "h-10 rounded-md border bg-background px-3 text-sm focus-visible:ring-2 focus-visible:ring-ring";
const activeStates = new Set([
  "pending",
  "verifying",
  "downloading",
  "installing",
  "restarting",
  "awaiting_health",
]);
function ErrorMessage({ error }: { error: unknown }) {
  return error ? (
    <p role="alert" className="text-sm text-destructive">
      {getUserFacingError(
        error,
        "Could not load or save this section. Try again.",
      )}
    </p>
  ) : null;
}

export function AgentUpdatePanel({ agentId }: { agentId: string }) {
  const query = useQuery({
    queryKey: ["agent-updates", agentId],
    queryFn: () => fetchAgentUpdates(agentId),
    refetchInterval: 10_000,
  });
  const update = useMutation({
    mutationFn: (version: string) => updateAgent(agentId, version),
    onSuccess: () => query.refetch(),
  });
  const data = query.data;
  const active = data?.operations?.find((operation) =>
    activeStates.has(operation.state),
  );
  return (
    <section
      className="space-y-3 rounded-lg border p-4"
      aria-label="Agent updates"
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="font-medium">Agent updates</h2>
        {data?.target_version &&
          data.target_version !== data.current_version && (
            <Button
              disabled={update.isPending || Boolean(active)}
              onClick={() => update.mutate(data.target_version!)}
            >
              Update to {data.target_version}
            </Button>
          )}
      </div>
      {query.isLoading && <p role="status">Checking releases…</p>}
      <ErrorMessage error={query.error || update.error} />
      {data && (
        <>
          <p className="text-sm text-muted-foreground">
            Installed {data.current_version ?? "version unavailable"} ·{" "}
            {data.policy?.mode ?? "notify"} · {data.policy?.channel ?? "stable"}
          </p>
          {data.target_version === data.current_version &&
            data.target_version && (
              <p className="text-sm">Agent is up to date.</p>
            )}
          {data.blocked_reason && (
            <p className="text-sm">{data.blocked_reason}</p>
          )}
          {data.manifest?.release_notes && (
            <details>
              <summary className="cursor-pointer text-sm">
                Release notes
              </summary>
              <p className="mt-2 whitespace-pre-wrap text-sm">
                {data.manifest.release_notes}
              </p>
            </details>
          )}
          {active && (
            <p role="status" className="text-sm">
              {active.state.replaceAll("_", " ")} ·{" "}
              {active.progress?.detail || "Waiting for the agent"}
            </p>
          )}
          {update.isSuccess && !active && (
            <p role="status" className="text-sm">
              Update requested. Offline agents pick it up when they reconnect.
            </p>
          )}
          <details>
            <summary className="cursor-pointer text-sm">
              Update history ({data.operations?.length ?? 0})
            </summary>
            <ul className="mt-3 divide-y">
              {data.operations?.map((operation) => (
                <li key={operation.id} className="space-y-1 py-3 text-sm">
                  <div className="flex flex-wrap justify-between gap-2">
                    <span>
                      {operation.previous_version} → {operation.target_version}
                    </span>
                    <Badge variant="outline">
                      {operation.state.replaceAll("_", " ")}
                    </Badge>
                  </div>
                  <p className="text-muted-foreground">
                    {new Date(operation.created_at).toLocaleString()} ·{" "}
                    {operation.transport === "agent"
                      ? "Managed update"
                      : "SSH update"}
                  </p>
                  {(operation.last_error || operation.rollback_reason) && (
                    <p className="text-destructive">
                      {operation.last_error || operation.rollback_reason}
                    </p>
                  )}
                </li>
              ))}
            </ul>
          </details>
        </>
      )}
    </section>
  );
}

export function AgentSettings({ agentId }: { agentId: string }) {
  const collection = useQuery({
    queryKey: ["agent-collection", agentId],
    queryFn: () => fetchAgentCollection(agentId),
    refetchInterval: 15_000,
  });
  const updates = useQuery({
    queryKey: ["agent-updates", agentId],
    queryFn: () => fetchAgentUpdates(agentId),
  });
  return (
    <div className="space-y-6">
      <ErrorMessage error={collection.error || updates.error} />
      {collection.data?.config && (
        <CollectionForm
          key={agentId}
          agentId={agentId}
          settings={collection.data}
        />
      )}
      {updates.data?.policy && (
        <PolicyForm
          key={agentId + "-policy"}
          agentId={agentId}
          initial={updates.data.policy}
        />
      )}
    </div>
  );
}
function CollectionForm({
  agentId,
  settings,
}: {
  agentId: string;
  settings: AgentCollectionSettings;
}) {
  const cache = useQueryClient();
  const [draft, setDraft] = useState(settings);
  const [units, setUnits] = useState(settings.config.journal_units.join("\n"));
  const [containers, setContainers] = useState(
    settings.config.docker_containers.join("\n"),
  );
  const [redact, setRedact] = useState(settings.config.redact.join("\n"));
  const lines = (value: string) => [
    ...new Set(
      value
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean),
    ),
  ];
  const save = useMutation({
    mutationFn: () =>
      saveAgentCollection(agentId, {
        ...draft,
        config: {
          ...draft.config,
          journal_units: lines(units),
          docker_containers: lines(containers),
          redact: lines(redact),
        },
      }),
    onSuccess: async (result) => {
      setDraft((old) => ({
        ...old,
        config: { ...old.config, revision: result.revision },
      }));
      await cache.invalidateQueries({
        queryKey: ["agent-collection", agentId],
      });
    },
  });
  return (
    <form
      className="space-y-4"
      onSubmit={(event) => {
        event.preventDefault();
        save.mutate();
      }}
    >
      <div>
        <h2 className="font-medium">Log collection</h2>
        <p className="text-sm text-muted-foreground">
          Choose exact sources. Empty lists disable host and container logs.
          Agent diagnostics remain available.
        </p>
      </div>
      <div className="grid gap-4 sm:grid-cols-2">
        <label className="space-y-2 text-sm">
          Journal units, one per line
          <textarea
            className="min-h-28 w-full rounded-md border bg-background p-3 font-mono"
            value={units}
            onChange={(event) => setUnits(event.target.value)}
            placeholder="hope-agent.service"
          />
        </label>
        <label className="space-y-2 text-sm">
          Docker containers, one per line
          <textarea
            className="min-h-28 w-full rounded-md border bg-background p-3 font-mono"
            value={containers}
            onChange={(event) => setContainers(event.target.value)}
            placeholder="jellyfin"
          />
        </label>
      </div>
      <p className="text-xs text-muted-foreground">
        The service account needs permission to read each source. Unavailable
        sources report a diagnostic; other collectors continue.
      </p>
      <label className="block space-y-2 text-sm">
        Redact exact strings, one per line
        <textarea
          className="min-h-20 w-full rounded-md border bg-background p-3"
          value={redact}
          onChange={(event) => setRedact(event.target.value)}
          autoComplete="off"
        />
      </label>
      <label className="block space-y-2 text-sm">
        Log retention in days
        <Input
          type="number"
          min={1}
          max={90}
          value={draft.log_retention_days}
          onChange={(event) =>
            setDraft({
              ...draft,
              log_retention_days: Number(event.target.value),
            })
          }
          className="max-w-32"
        />
      </label>
      <p className="text-sm">
        Configuration {settings.config.revision}:{" "}
        {settings.applied_revision === settings.config.revision
          ? "Applied by agent"
          : "Waiting for agent acknowledgement"}
      </p>
      <ErrorMessage error={save.error} />
      <Button type="submit" disabled={save.isPending}>
        Save log sources
      </Button>
      {save.isSuccess && (
        <p role="status" className="text-sm">
          Saved. The agent applies this configuration on its next heartbeat.
        </p>
      )}
    </form>
  );
}
function PolicyForm({
  agentId,
  initial,
}: {
  agentId: string;
  initial: AgentUpdatePolicy;
}) {
  const cache = useQueryClient();
  const [policy, setPolicy] = useState(initial);
  const save = useMutation({
    mutationFn: () => saveAgentUpdatePolicy(agentId, policy),
    onSuccess: () =>
      cache.invalidateQueries({ queryKey: ["agent-updates", agentId] }),
  });
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        save.mutate();
      }}
      className="space-y-4 border-t pt-5"
    >
      <h2 className="font-medium">Update policy</h2>
      <div className="grid gap-4 sm:grid-cols-2">
        <label className="grid gap-2 text-sm">
          Updates
          <select
            className={selectClass}
            value={policy.mode}
            onChange={(event) =>
              setPolicy({ ...policy, mode: event.target.value })
            }
          >
            <option value="notify">Notify only</option>
            <option value="manual">Manual</option>
            <option value="automatic">Automatic</option>
          </select>
        </label>
        <label className="grid gap-2 text-sm">
          Release channel
          <select
            className={selectClass}
            value={policy.channel}
            onChange={(event) =>
              setPolicy({ ...policy, channel: event.target.value })
            }
          >
            <option value="stable">Stable</option>
            <option value="canary">Canary</option>
          </select>
        </label>
        <label className="grid gap-2 text-sm">
          Pin version (optional)
          <Input
            value={policy.pinned_version ?? ""}
            placeholder="Use latest compatible release"
            onChange={(event) =>
              setPolicy({
                ...policy,
                pinned_version: event.target.value || null,
              })
            }
          />
        </label>
        <label className="grid gap-2 text-sm">
          Automatic rollout percentage
          <Input
            type="number"
            min={0}
            max={100}
            value={policy.rollout_percent}
            onChange={(event) =>
              setPolicy({
                ...policy,
                rollout_percent: Number(event.target.value),
              })
            }
          />
        </label>
        <label className="grid gap-2 text-sm">
          Window starts (UTC hour)
          <Input
            type="number"
            min={0}
            max={23}
            value={policy.window_start_utc}
            onChange={(event) =>
              setPolicy({
                ...policy,
                window_start_utc: Number(event.target.value),
              })
            }
          />
        </label>
        <label className="grid gap-2 text-sm">
          Window ends (UTC hour)
          <Input
            type="number"
            min={0}
            max={23}
            value={policy.window_end_utc}
            onChange={(event) =>
              setPolicy({
                ...policy,
                window_end_utc: Number(event.target.value),
              })
            }
          />
        </label>
      </div>
      <p className="text-xs text-muted-foreground">
        Equal hours allow updates all day. Automatic rollout starts with
        canaries, waits 10 minutes after a healthy update, and pauses after
        failure. At most three agents update at once.
      </p>
      <ErrorMessage error={save.error} />
      <Button disabled={save.isPending}>Save update policy</Button>
      {save.isSuccess && (
        <p role="status" className="text-sm">
          Update policy saved.
        </p>
      )}
    </form>
  );
}

export function AgentLogs({
  agentId,
  location,
  navigate,
}: {
  agentId: string;
  location: AgentLocation;
  navigate: NavigateAgent;
}) {
  const [paused, setPaused] = useState(false);
  const [frozen, setFrozen] = useState<AgentLogEntry[]>();
  const [cursor, setCursor] = useState<string>();
  const [older, setOlder] = useState<AgentLogEntry[]>([]);
  const [copyState, setCopyState] = useState("");
  const [queryText, setQueryText] = useState(location.logq ?? "");
  const params: Record<string, string> = { limit: "100" };
  for (const [key, value] of Object.entries({
    q: location.logq,
    source: location.source,
    severity: location.severity,
    from: location.from,
    to: location.to,
    cursor,
  })) {
    if (value) params[key] = value;
  }
  const query = useQuery({
    queryKey: ["agent-logs", agentId, params],
    queryFn: () => fetchAgentLogs(agentId, params),
    refetchInterval: cursor ? false : 5000,
  });
  const currentEntries = [...older, ...(query.data?.items ?? [])].filter(
    (entry, index, all) =>
      all.findIndex((other) => other.id === entry.id) === index,
  );
  const entries = paused && !cursor && frozen ? frozen : currentEntries;
  const newEntries =
    paused && frozen
      ? (query.data?.items ?? []).filter(
          (entry) => !frozen.some((old) => old.id === entry.id),
        ).length
      : 0;
  function pause() {
    if (!paused) {
      setFrozen(entries);
      setPaused(true);
    }
  }
  function resume() {
    setFrozen(undefined);
    setOlder([]);
    setCursor(undefined);
    setPaused(false);
    void query.refetch();
  }
  function filter(patch: Partial<AgentLocation>) {
    setCursor(undefined);
    setOlder([]);
    setFrozen(undefined);
    navigate(patch);
  }
  function exportEntries() {
    const url = URL.createObjectURL(
      new Blob(
        entries.map((entry) => JSON.stringify(entry) + "\n"),
        { type: "application/x-ndjson" },
      ),
    );
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = `hope-${agentId}-logs.ndjson`;
    anchor.click();
    URL.revokeObjectURL(url);
  }
  return (
    <section className="space-y-4" aria-label="Logs and activity">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="font-medium">Logs & activity</h2>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="outline"
            onClick={() => (paused || cursor ? resume() : pause())}
          >
            {paused || cursor
              ? `Resume live${newEntries ? ` (${newEntries}+ new)` : ""}`
              : "Pause live"}
          </Button>
          <Button
            variant="outline"
            onClick={async () => {
              try {
                await navigator.clipboard.writeText(window.location.href);
                setCopyState("Link copied");
              } catch {
                setCopyState("Could not copy link");
              }
            }}
          >
            Copy link
          </Button>
          <Button
            variant="outline"
            disabled={!entries.length}
            onClick={exportEntries}
          >
            Export loaded entries ({entries.length})
          </Button>
        </div>
      </div>
      {copyState && (
        <p role="status" className="text-sm">
          {copyState}
        </p>
      )}
      <form
        className="flex flex-wrap gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          filter({ logq: queryText || undefined });
        }}
      >
        <Input
          aria-label="Search logs"
          placeholder="Search log messages"
          value={queryText}
          onChange={(event) => setQueryText(event.target.value)}
          className="min-w-40 flex-1"
        />
        <Button type="submit">Search</Button>
        <select
          aria-label="Log severity"
          className={selectClass}
          value={location.severity ?? ""}
          onChange={(event) =>
            filter({ severity: event.target.value || undefined })
          }
        >
          <option value="">All severities</option>
          {[
            "critical",
            "error",
            "warning",
            "notice",
            "info",
            "debug",
            "unknown",
          ].map((severity) => (
            <option key={severity}>{severity}</option>
          ))}
        </select>
        <Input
          aria-label="Log source"
          placeholder="Source: agent, journal:unit, docker:name"
          value={location.source ?? ""}
          onChange={(event) =>
            filter({ source: event.target.value || undefined })
          }
          className="sm:w-72"
        />
      </form>
      <div className="flex flex-wrap gap-3 text-sm">
        <Button
          size="sm"
          variant="outline"
          onClick={() => filter({ source: "agent" })}
        >
          Agent diagnostics
        </Button>
        <Button
          size="sm"
          variant="outline"
          onClick={() =>
            filter({ source: undefined, from: undefined, to: undefined })
          }
        >
          All sources · last 24 hours
        </Button>
        {location.from && (
          <p>
            From {new Date(Number(location.from) * 1000).toLocaleString()}
            {location.to
              ? ` to ${new Date(Number(location.to) * 1000).toLocaleString()}`
              : ""}
          </p>
        )}
      </div>
      <ErrorMessage error={query.error} />
      {query.isLoading && <p role="status">Loading logs…</p>}
      {!query.isLoading && !query.isError && !entries.length && (
        <div className="rounded-lg border border-dashed p-6">
          <p>No log entries in this range.</p>
          <p className="mt-2 text-sm text-muted-foreground">
            Choose journal units or containers in Inventory & settings. Logs
            begin after the agent applies those sources.
          </p>
        </div>
      )}
      <div
        className="divide-y rounded-lg border"
        onWheel={pause}
        onPointerDown={pause}
        onFocusCapture={pause}
      >
        {entries.map((entry) => (
          <details key={entry.id} className="p-3 text-sm">
            <summary className="cursor-pointer list-none space-y-1 focus-visible:outline focus-visible:outline-ring">
              <span className="flex flex-wrap gap-2 text-xs text-muted-foreground">
                <time dateTime={entry.observed_at}>
                  {new Date(entry.observed_at).toLocaleString()}
                </time>
                <span>{entry.source}</span>
                <span>{entry.severity}</span>
              </span>
              <span className="block whitespace-pre-wrap break-words font-mono">
                {entry.message}
              </span>
            </summary>
            <pre className="mt-3 overflow-auto rounded bg-muted p-3 text-xs">
              {JSON.stringify(
                {
                  received_at: entry.received_at,
                  event_id: entry.event_id,
                  ...entry.attributes,
                },
                null,
                2,
              )}
            </pre>
          </details>
        ))}
      </div>
      {query.data?.next_cursor && (
        <Button
          variant="outline"
          disabled={query.isFetching}
          onClick={() => {
            setPaused(true);
            setOlder(entries);
            setCursor(query.data!.next_cursor!);
          }}
        >
          Load older entries
        </Button>
      )}
      <p className="text-xs text-muted-foreground">
        {paused || cursor
          ? "Live updates paused while reading history."
          : "Live · refreshes every 5 seconds."}{" "}
        Export includes only loaded entries.
      </p>
    </section>
  );
}
