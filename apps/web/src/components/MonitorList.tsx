import { useQuery } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import {
  ActivityIcon,
  ArrowLeftIcon,
  CheckIcon,
  CircleAlertIcon,
  CircleHelpIcon,
  Clock3Icon,
  ExternalLinkIcon,
  SearchIcon,
  TriangleAlertIcon,
} from "lucide-react";
import { useMemo, useRef, useState } from "react";
import {
  fetchIncidents,
  fetchMonitor,
  fetchMonitorResults,
  fetchMonitors,
  getUserFacingError,
  type Incident,
  type Monitor,
  type MonitorResult,
} from "@/lib/api";
import { formatDate, formatRelative, labelize, shortId } from "@/lib/format";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";

type MonitorFilter = "all" | "up" | "degraded" | "down" | "stale";

const EMPTY_MONITORS: Monitor[] = [];

export function MonitorList({
  onOpenIncidentHistory,
}: {
  onOpenIncidentHistory?: () => void;
} = {}) {
  const navigate = useNavigate({ from: "/monitoring" });
  const { monitor: requestedMonitorId } = useSearch({ from: "/monitoring" });
  const [filter, setFilter] = useState<MonitorFilter>("all");
  const [search, setSearch] = useState("");
  const monitorsQuery = useQuery({
    queryKey: ["monitors"],
    queryFn: () => fetchMonitors(),
    refetchInterval: 15_000,
  });
  const monitors = monitorsQuery.data?.items ?? EMPTY_MONITORS;
  const visibleMonitors = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return monitors.filter((monitor) => {
      const matchesFilter = filter === "all" || monitor.state === filter;
      if (!matchesFilter) return false;
      if (!needle) return true;
      const target = monitorTarget(monitor);
      return [
        target.name,
        target.endpoint,
        monitor.monitor_type,
        monitor.service_product,
      ].some((value) => value?.toLowerCase().includes(needle));
    });
  }, [filter, monitors, search]);
  const selectedFromList =
    monitors.find((monitor) => monitor.id === requestedMonitorId) ?? null;
  const requestedMonitorQuery = useQuery({
    queryKey: ["monitor", requestedMonitorId ?? "none"],
    queryFn: () => fetchMonitor(requestedMonitorId!),
    enabled: Boolean(requestedMonitorId && !selectedFromList),
    retry: false,
  });
  const selectedMonitor =
    selectedFromList ?? requestedMonitorQuery.data ?? null;

  const selectMonitor = (id: string | null) => {
    void navigate({
      search: (previous) => ({
        ...previous,
        monitor: id ?? undefined,
      }),
    });
  };

  return (
    <div
      aria-busy={monitorsQuery.isLoading}
      className={
        selectedMonitor
          ? "grid min-w-0 gap-6 lg:grid-cols-[minmax(17rem,0.72fr)_minmax(0,1.65fr)]"
          : "min-w-0"
      }
    >
      <section
        aria-label="Monitor list"
        className={selectedMonitor ? "hidden min-w-0 lg:block" : "min-w-0"}
      >
        <Card className="overflow-hidden">
          <CardHeader className="gap-4 border-b">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div>
                <CardTitle>Monitors</CardTitle>
                <CardDescription className="mt-1">
                  Services checked from their known endpoints.
                </CardDescription>
              </div>
              <Badge variant={monitors.length ? "outline" : "secondary"}>
                {monitors.length}
              </Badge>
            </div>
            <div className="flex flex-col gap-3">
              <label className="relative block">
                <span className="sr-only">Search monitors</span>
                <SearchIcon
                  aria-hidden="true"
                  className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground"
                />
                <Input
                  className="pl-9"
                  onChange={(event) => setSearch(event.target.value)}
                  placeholder="Search monitors"
                  value={search}
                />
              </label>
              <ToggleGroup
                aria-label="Filter monitors by state"
                className="w-full justify-start overflow-x-auto"
                onValueChange={(values) => {
                  const next = values[0] as MonitorFilter | undefined;
                  if (next) setFilter(next);
                }}
                size="sm"
                value={[filter]}
                variant="outline"
              >
                <ToggleGroupItem value="all">All</ToggleGroupItem>
                <ToggleGroupItem value="up">Passing</ToggleGroupItem>
                <ToggleGroupItem value="down">Down</ToggleGroupItem>
                <ToggleGroupItem value="degraded">Degraded</ToggleGroupItem>
                <ToggleGroupItem value="stale">Stale</ToggleGroupItem>
              </ToggleGroup>
              {monitors.length >= 100 ? (
                <p className="text-xs text-muted-foreground">
                  Showing up to 100 monitors. Search covers this loaded list.
                </p>
              ) : null}
            </div>
          </CardHeader>
          {monitorsQuery.isLoading ? (
            <MonitorListLoading />
          ) : monitorsQuery.isError ? (
            <CardContent className="pt-6">
              <Alert variant="destructive">
                <CircleAlertIcon />
                <AlertTitle>Monitors unavailable</AlertTitle>
                <AlertDescription>
                  {getUserFacingError(
                    monitorsQuery.error,
                    "The monitor list could not be loaded.",
                  )}
                </AlertDescription>
                <Button
                  onClick={() => monitorsQuery.refetch()}
                  size="sm"
                  variant="outline"
                >
                  Retry
                </Button>
              </Alert>
            </CardContent>
          ) : monitors.length === 0 ? (
            <CardContent className="pt-6">
              <Empty>
                <EmptyHeader>
                  <EmptyMedia variant="icon">
                    <ActivityIcon />
                  </EmptyMedia>
                  <EmptyTitle>No monitors yet</EmptyTitle>
                  <EmptyDescription>
                    Approve a suggested check to start watching a service.
                  </EmptyDescription>
                </EmptyHeader>
              </Empty>
            </CardContent>
          ) : visibleMonitors.length === 0 ? (
            <CardContent className="pt-6">
              <Empty>
                <EmptyHeader>
                  <EmptyMedia variant="icon">
                    <SearchIcon />
                  </EmptyMedia>
                  <EmptyTitle>No matching monitors</EmptyTitle>
                  <EmptyDescription>
                    Clear search or choose another state filter.
                  </EmptyDescription>
                </EmptyHeader>
              </Empty>
            </CardContent>
          ) : (
            <div className="divide-y">
              {visibleMonitors.map((monitor) => (
                <MonitorListItem
                  key={monitor.id}
                  monitor={monitor}
                  onSelect={() => selectMonitor(monitor.id)}
                  selected={monitor.id === requestedMonitorId}
                />
              ))}
            </div>
          )}
        </Card>
      </section>

      {requestedMonitorId &&
      !selectedMonitor &&
      !monitorsQuery.isLoading &&
      !requestedMonitorQuery.isLoading ? (
        <section className="min-w-0" aria-label="Selected monitor">
          <Card>
            <CardContent className="pt-6">
              <Alert variant="destructive">
                <CircleAlertIcon />
                <AlertTitle>Monitor unavailable</AlertTitle>
                <AlertDescription>
                  {getUserFacingError(
                    requestedMonitorQuery.error,
                    "This monitor is not present in the current monitor list.",
                  )}
                </AlertDescription>
                <Button
                  onClick={() => selectMonitor(null)}
                  size="sm"
                  variant="outline"
                >
                  Back to monitors
                </Button>
              </Alert>
            </CardContent>
          </Card>
        </section>
      ) : selectedMonitor ? (
        <MonitorDetail
          key={selectedMonitor.id}
          monitor={selectedMonitor}
          monitorListError={monitorsQuery.isError ? monitorsQuery.error : null}
          onListRetry={() => monitorsQuery.refetch()}
          onOpenIncidentHistory={onOpenIncidentHistory}
          onBack={() => selectMonitor(null)}
        />
      ) : null}
    </div>
  );
}

function MonitorListItem({
  monitor,
  onSelect,
  selected,
}: {
  monitor: Monitor;
  onSelect: () => void;
  selected: boolean;
}) {
  const target = monitorTarget(monitor);
  return (
    <button
      aria-pressed={selected}
      className="w-full px-4 py-4 text-left outline-none transition-colors hover:bg-muted/50 focus-visible:ring-3 focus-visible:ring-inset focus-visible:ring-ring/50 aria-pressed:bg-muted"
      onClick={onSelect}
      type="button"
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex min-w-0 items-center gap-2">
            <MonitorStateBadge state={monitor.state} />
            <span className="truncate font-medium">{target.name}</span>
          </div>
          <p className="mt-1 truncate text-sm text-muted-foreground">
            {target.endpoint}
          </p>
        </div>
        <span className="shrink-0 text-xs text-muted-foreground">
          {labelize(monitor.monitor_type)}
        </span>
      </div>
      <div className="mt-3 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
        <span>Last result: {formatRelative(monitor.last_result_at)}</span>
        <span>{formatInterval(monitor.interval_seconds)} interval</span>
      </div>
    </button>
  );
}

function MonitorDetail({
  monitor,
  monitorListError,
  onListRetry,
  onOpenIncidentHistory,
  onBack,
}: {
  monitor: Monitor;
  monitorListError: Error | null;
  onListRetry: () => void;
  onOpenIncidentHistory?: () => void;
  onBack: () => void;
}) {
  const target = monitorTarget(monitor);
  const technicalDetailsRef = useRef<HTMLDetailsElement>(null);
  const technicalDetailsSummaryRef = useRef<HTMLElement>(null);
  const [technicalDetailsOpen, setTechnicalDetailsOpen] = useState(false);
  const resultsQuery = useQuery({
    queryKey: ["monitor-results", monitor.id],
    queryFn: () => fetchMonitorResults(monitor.id),
    refetchInterval: 15_000,
  });
  const incidentsQuery = useQuery({
    queryKey: ["incidents", "monitor", monitor.id],
    queryFn: () => fetchIncidents(),
    refetchInterval: 15_000,
  });
  const results = useMemo(
    () => sortResults(resultsQuery.data?.items ?? []),
    [resultsQuery.data?.items],
  );
  const incidents = useMemo(
    () =>
      sortIncidents(
        (incidentsQuery.data?.items ?? []).filter(
          (incident) => incident.monitor_id === monitor.id,
        ),
      ),
    [incidentsQuery.data?.items, monitor.id],
  );
  const latestResult = results[0] ?? null;
  const relevantIncident =
    incidents.find((incident) => incident.state === "open") ??
    incidents[0] ??
    null;
  const incidentsAreBounded = incidentsQuery.data?.items.length === 100;
  const resultsAreBounded = resultsQuery.data?.items.length === 100;

  const revealTechnicalDetails = () => {
    setTechnicalDetailsOpen(true);
    if (technicalDetailsRef.current) {
      technicalDetailsRef.current.open = true;
      technicalDetailsRef.current.scrollIntoView?.({ block: "nearest" });
      technicalDetailsSummaryRef.current?.focus();
    }
  };

  return (
    <section aria-label="Selected monitor" className="min-w-0">
      <Card className="overflow-hidden">
        <CardHeader className="gap-4 border-b">
          {monitorListError ? (
            <div className="lg:hidden">
              <LocalUnavailable
                title="Monitor list unavailable"
                description={getUserFacingError(
                  monitorListError,
                  "The monitor list could not be refreshed. This selected monitor may be stale.",
                )}
                onRetry={onListRetry}
              />
            </div>
          ) : null}
          <Button
            className="w-fit lg:hidden"
            onClick={onBack}
            size="sm"
            variant="ghost"
          >
            <ArrowLeftIcon data-icon="inline-start" />
            Back to monitors
          </Button>
          <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
            <div className="min-w-0">
              <div className="flex flex-wrap items-center gap-2">
                <MonitorStateBadge state={monitor.state} />
                <Badge variant="outline">
                  {labelize(monitor.monitor_type)}
                </Badge>
              </div>
              <CardTitle className="mt-3 truncate text-2xl">
                {target.name}
              </CardTitle>
              <CardDescription className="mt-1 break-words">
                {target.endpoint}
                {monitor.config.path && typeof monitor.config.path === "string"
                  ? ` ${monitor.config.path}`
                  : ""}
              </CardDescription>
            </div>
            <div className="flex shrink-0 flex-wrap gap-2">
              <button
                aria-controls="monitor-technical-details"
                aria-expanded={technicalDetailsOpen}
                className={buttonVariants({ size: "sm", variant: "outline" })}
                onClick={revealTechnicalDetails}
                type="button"
              >
                Check details
              </button>
              <a
                className={buttonVariants({ size: "sm", variant: "ghost" })}
                href="#monitor-notifications"
              >
                Notifications
                <ExternalLinkIcon data-icon="inline-end" />
              </a>
            </div>
          </div>
          <div className="flex flex-wrap gap-x-4 gap-y-1 text-sm text-muted-foreground">
            <span>Last check {formatRelative(monitor.last_result_at)}</span>
            <span>Every {formatInterval(monitor.interval_seconds)}</span>
          </div>
        </CardHeader>

        <CardContent className="space-y-6 pt-6">
          <section aria-labelledby="monitor-latest-result">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <h2 className="font-medium" id="monitor-latest-result">
                Latest result
              </h2>
              {latestResult ? (
                <span className="text-sm text-muted-foreground">
                  {formatDate(latestResult.observed_at)}
                </span>
              ) : null}
            </div>
            {resultsQuery.isLoading ? (
              <ResultLoading />
            ) : resultsQuery.isError ? (
              <LocalUnavailable
                title="Latest result unavailable"
                description={getUserFacingError(
                  resultsQuery.error,
                  "Results for this monitor could not be loaded.",
                )}
                onRetry={() => resultsQuery.refetch()}
              />
            ) : latestResult ? (
              <LatestResult result={latestResult} />
            ) : (
              <div className="mt-3 rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
                No result recorded yet. This monitor is waiting for its first
                check.
              </div>
            )}
          </section>

          <section aria-labelledby="monitor-history">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <h2 className="font-medium" id="monitor-history">
                Recent checks
              </h2>
              <span className="text-sm text-muted-foreground">
                {resultsQuery.isLoading
                  ? "Loading"
                  : resultsQuery.isError
                    ? "Unavailable"
                    : results.length
                      ? resultsAreBounded
                        ? `${results.length} latest checks`
                        : `${results.length} recorded`
                      : "No recorded checks"}
              </span>
            </div>
            {resultsQuery.isLoading ? (
              <ResultLoading />
            ) : resultsQuery.isError ? (
              <p className="mt-3 text-sm text-muted-foreground">
                History is unavailable until results can be loaded.
              </p>
            ) : (
              <ResultHistory results={results} />
            )}
          </section>

          <section aria-labelledby="monitor-incident">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <h2 className="font-medium" id="monitor-incident">
                Current incident / recent state changes
              </h2>
              {onOpenIncidentHistory ? (
                <button
                  className="text-sm text-primary underline-offset-4 hover:underline"
                  onClick={onOpenIncidentHistory}
                  type="button"
                >
                  Incident history
                </button>
              ) : (
                <a
                  className="text-sm text-primary underline-offset-4 hover:underline"
                  href="#incident-history"
                >
                  Incident history
                </a>
              )}
            </div>
            {incidentsQuery.isLoading ? (
              <Skeleton className="mt-3 h-16 w-full" />
            ) : incidentsQuery.isError ? (
              <LocalUnavailable
                title="Incident history unavailable"
                description={getUserFacingError(
                  incidentsQuery.error,
                  "Incident history for this monitor could not be loaded.",
                )}
                onRetry={() => incidentsQuery.refetch()}
              />
            ) : relevantIncident ? (
              <IncidentSummary incident={relevantIncident} />
            ) : (
              <p className="mt-3 rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
                {incidentsAreBounded
                  ? "No incident found in the latest 100 incidents returned. History may be incomplete."
                  : "No incident recorded for this monitor."}
              </p>
            )}
          </section>

          <details
            className="rounded-lg border"
            id="monitor-technical-details"
            onToggle={(event) =>
              setTechnicalDetailsOpen(event.currentTarget.open)
            }
            open={technicalDetailsOpen}
            ref={technicalDetailsRef}
          >
            <summary
              className="cursor-pointer px-4 py-3 font-medium outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
              ref={technicalDetailsSummaryRef}
            >
              Technical details
            </summary>
            <dl className="grid gap-4 border-t px-4 py-4 sm:grid-cols-2">
              <Detail
                label="Interval"
                value={formatInterval(monitor.interval_seconds)}
              />
              <Detail label="Timeout" value={`${monitor.timeout_ms}ms`} />
              <Detail
                label="Failure threshold"
                value={String(monitor.failure_threshold)}
              />
              <Detail
                label="Recovery threshold"
                value={String(monitor.recovery_threshold)}
              />
              <Detail
                label="Consecutive failures"
                value={String(monitor.consecutive_failures)}
              />
              <Detail
                label="Consecutive successes"
                value={String(monitor.consecutive_successes)}
              />
              <Detail label="Monitor ID" mono value={shortId(monitor.id)} />
              <Detail
                label="Endpoint ID"
                mono
                value={shortId(monitor.endpoint_id)}
              />
            </dl>
          </details>

          <div
            className="flex flex-wrap items-center gap-2 rounded-lg border bg-muted/30 p-4 text-sm"
            id="monitor-notifications"
          >
            <span className="text-muted-foreground">
              Notifications: Configure in settings.
            </span>
            <a
              className="font-medium text-primary underline-offset-4 hover:underline"
              href="/settings"
            >
              Notification settings
            </a>
          </div>
        </CardContent>
      </Card>
    </section>
  );
}

function LatestResult({ result }: { result: MonitorResult }) {
  return (
    <div className="mt-3 flex flex-col gap-2 rounded-lg border p-4 sm:flex-row sm:items-start sm:justify-between">
      <div className="flex items-start gap-3">
        <ResultStatusIcon status={result.status} />
        <div>
          <p className="font-medium">{resultStatusLabel(result.status)}</p>
          <p className="mt-1 text-sm text-muted-foreground">
            {result.error ?? resultMessage(result.status)}
          </p>
        </div>
      </div>
      <div className="text-left text-sm text-muted-foreground sm:text-right">
        <p>
          {result.latency_ms === null
            ? "Latency unavailable"
            : `${result.latency_ms}ms`}
        </p>
        <p>{formatRelative(result.observed_at)}</p>
      </div>
    </div>
  );
}

function ResultHistory({ results }: { results: MonitorResult[] }) {
  if (!results.length) {
    return (
      <p className="mt-3 rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
        No recent checks to show.
      </p>
    );
  }
  return (
    <div
      aria-label="Recent check history"
      className="mt-3 flex min-h-12 items-end gap-1 rounded-lg border p-3"
      role="list"
    >
      {results.slice(0, 20).map((result) => (
        <span
          aria-label={`${resultStatusLabel(result.status)} at ${formatDate(result.observed_at)}`}
          className={`h-6 min-w-2 flex-1 rounded-sm ${resultStatusClass(result.status)}`}
          key={result.id}
          role="listitem"
          title={`${resultStatusLabel(result.status)} · ${formatDate(result.observed_at)}`}
        />
      ))}
    </div>
  );
}

function IncidentSummary({ incident }: { incident: Incident }) {
  const open = incident.state === "open";
  return (
    <div className="mt-3 rounded-lg border p-4">
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant={open ? "destructive" : "outline"}>
          {open ? "Open" : labelize(incident.state)}
        </Badge>
        <Badge variant="secondary">{labelize(incident.severity)}</Badge>
      </div>
      <p className="mt-3 font-medium">
        {incident.summary ?? `${labelize(incident.monitor_type)} state change`}
      </p>
      <p className="mt-1 text-sm text-muted-foreground">
        {incident.failure_count} failed check
        {incident.failure_count === 1 ? "" : "s"}
        {" · "}
        Last event {formatRelative(incident.last_event_at)}
      </p>
    </div>
  );
}

function LocalUnavailable({
  description,
  onRetry,
  title,
}: {
  description: string;
  onRetry: () => void;
  title: string;
}) {
  return (
    <Alert className="mt-3" variant="destructive">
      <CircleAlertIcon />
      <AlertTitle>{title}</AlertTitle>
      <AlertDescription>{description}</AlertDescription>
      <Button onClick={onRetry} size="sm" variant="outline">
        Retry
      </Button>
    </Alert>
  );
}

function ResultLoading() {
  return (
    <div className="mt-3 space-y-2">
      <Skeleton className="h-16 w-full" />
      <Skeleton className="h-12 w-full" />
    </div>
  );
}

function Detail({
  label,
  mono = false,
  value,
}: {
  label: string;
  mono?: boolean;
  value: string;
}) {
  return (
    <div className="min-w-0">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className={mono ? "truncate font-mono text-sm" : "text-sm"}>
        {value}
      </dd>
    </div>
  );
}

function MonitorStateBadge({ state }: { state: string }) {
  if (state === "up") {
    return (
      <Badge variant="outline">
        <CheckIcon className="text-emerald-600 dark:text-emerald-400" />
        Passing
      </Badge>
    );
  }
  if (state === "degraded") {
    return (
      <Badge variant="outline">
        <TriangleAlertIcon className="text-amber-600 dark:text-amber-400" />
        Degraded
      </Badge>
    );
  }
  if (state === "down") {
    return (
      <Badge variant="destructive">
        <CircleAlertIcon />
        Down
      </Badge>
    );
  }
  if (state === "stale") {
    return (
      <Badge variant="outline">
        <Clock3Icon className="text-amber-600 dark:text-amber-400" />
        Stale
      </Badge>
    );
  }
  return (
    <Badge variant="secondary">
      <CircleHelpIcon />
      Unknown
    </Badge>
  );
}

function ResultStatusIcon({ status }: { status: string }) {
  if (status === "success") {
    return <CheckIcon aria-hidden="true" className="mt-0.5 text-emerald-600" />;
  }
  if (status === "failure" || status === "timeout") {
    return (
      <CircleAlertIcon aria-hidden="true" className="mt-0.5 text-destructive" />
    );
  }
  return (
    <CircleHelpIcon
      aria-hidden="true"
      className="mt-0.5 text-muted-foreground"
    />
  );
}

function monitorTarget(monitor: Monitor) {
  const name =
    monitor.service_name ??
    monitor.service_product ??
    `Service ${shortId(monitor.service_id)}`;
  const endpoint =
    monitor.endpoint_url ??
    monitor.endpoint_dns_name ??
    formatSocketTarget(monitor.endpoint_address, monitor.endpoint_port) ??
    `Endpoint ${shortId(monitor.endpoint_id)}`;
  return { name, endpoint };
}

function formatSocketTarget(address: string | null, port: number | null) {
  if (!address) return null;
  const hostAddress = address.replace(/\/\d+$/, "");
  const host = hostAddress.includes(":") ? `[${hostAddress}]` : hostAddress;
  return port !== null ? `${host}:${port}` : host;
}

function formatInterval(seconds: number) {
  if (seconds < 60) return `${seconds}s`;
  if (seconds % 3600 === 0) return `${seconds / 3600}h`;
  if (seconds % 60 === 0) return `${seconds / 60}m`;
  return `${Math.floor(seconds / 60)}m`;
}

function sortResults(results: MonitorResult[]) {
  return [...results].sort(
    (left, right) =>
      new Date(right.observed_at).getTime() -
      new Date(left.observed_at).getTime(),
  );
}

function sortIncidents(incidents: Incident[]) {
  return [...incidents].sort(
    (left, right) =>
      new Date(right.last_event_at).getTime() -
      new Date(left.last_event_at).getTime(),
  );
}

function resultStatusLabel(status: string) {
  if (status === "success") return "Passing";
  if (status === "failure") return "Failed";
  if (status === "timeout") return "Timed out";
  if (status === "error") return "Error";
  return labelize(status);
}

function resultMessage(status: string) {
  return status === "success"
    ? "Check completed without an error."
    : "No error detail was recorded.";
}

function resultStatusClass(status: string) {
  if (status === "success") return "bg-emerald-500";
  if (status === "failure" || status === "timeout") return "bg-destructive";
  return "bg-muted-foreground/50";
}

function MonitorListLoading() {
  return (
    <CardContent className="space-y-3 pt-6">
      {Array.from({ length: 4 }, (_, index) => (
        <Skeleton className="h-20 w-full" key={index} />
      ))}
    </CardContent>
  );
}
