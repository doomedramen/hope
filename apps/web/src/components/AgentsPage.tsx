import { AgentAtGlance } from "@/components/agents/AgentAtGlance";
import {
  AgentUpdatePanel,
  AgentBulkUpdates,
  AgentSettings,
  AgentLogs,
  type AgentLocation,
  type NavigateAgent,
} from "@/components/agents/AgentOperations";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  CheckIcon,
  CircleAlertIcon,
  CircleHelpIcon,
  Clock3Icon,
  CopyIcon,
  DatabaseIcon,
  DownloadIcon,
  FileTextIcon,
  NetworkIcon,
  RefreshCwIcon,
  SearchIcon,
  ServerIcon,
  ShieldCheckIcon,
  XIcon,
} from "lucide-react";
import { CartesianGrid, Line, LineChart, XAxis, YAxis } from "recharts";
import { useEffect, useState, type ReactNode } from "react";
import {
  createAgentEnrollment,
  fetchAgentEnrollment,
  fetchAgent,
  fetchAgentMetrics,
  fetchAgents,
  getUserFacingError,
  type Agent,
  type AgentContainerInventory,
  type AgentDetail,
  type AgentFilesystemInventory,
  type AgentHostInventory,
  type AgentInventorySummary,
  type AgentMetricRange,
  type AgentMetricsResponse,
  type AgentNetworkInventory,
  type AgentProcessInventory,
  type AgentReachabilityState,
  type AgentSocketInventory,
  type AgentStatus,
} from "@/lib/api";
import {
  displayValue,
  formatDate,
  formatRelative,
  labelize,
  shortId,
} from "@/lib/format";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
} from "@/components/ui/chart";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "cn";
import {
  agentConnectionAddress,
  saveAgentConnectionAddress,
} from "@/lib/agentConnection";

const EMPTY_AGENTS: Agent[] = [];
const EMPTY_INVENTORY_SUMMARY: AgentInventorySummary = {
  interfaces: 0,
  filesystems: 0,
  processes: 0,
  sockets: 0,
  containers: 0,
};

export function AgentsPage({
  location: externalLocation,
  onNavigate,
}: { location?: AgentLocation; onNavigate?: NavigateAgent } = {}) {
  const queryClient = useQueryClient();
  const [localLocation, setLocalLocation] = useState<AgentLocation>({});
  const location = externalLocation ?? localLocation;
  const navigate: NavigateAgent = (patch) => {
    if (onNavigate) onNavigate(patch);
    else setLocalLocation((old) => ({ ...old, ...patch }));
  };
  const requestedAgentId = location.agent ?? null;
  const setRequestedAgentId = (id: string | null) =>
    navigate({ agent: id ?? undefined, tab: undefined });
  const [enrollmentOpen, setEnrollmentOpen] = useState(false);
  const [bulkSelected, setBulkSelected] = useState<string[]>([]);
  const toggleBulk = (id: string) =>
    setBulkSelected((old) =>
      old.includes(id) ? old.filter((item) => item !== id) : [...old, id],
    );
  const search = location.q ?? "";
  const setSearch = (q: string) =>
    navigate({ q: q || undefined, cursor: undefined });
  const agentsQuery = useQuery({
    queryKey: ["agents", search, location.status, location.cursor],
    queryFn: () =>
      fetchAgents({
        q: search,
        status: location.status,
        cursor: location.cursor,
      }),
    refetchInterval: 15_000,
  });
  const agents = agentsQuery.data?.items ?? EMPTY_AGENTS;
  const isEmptyState =
    !agentsQuery.isLoading &&
    !agentsQuery.isError &&
    agents.length === 0 &&
    !search &&
    !location.status &&
    !location.cursor;
  const selectedAgentId = requestedAgentId;
  const detailQuery = useQuery({
    queryKey: ["agent", selectedAgentId],
    queryFn: () => fetchAgent(selectedAgentId!),
    enabled: Boolean(selectedAgentId),
    refetchInterval: 15_000,
  });

  const visibleAgents = agents;
  const counts = agentsQuery.data?.counts;

  return (
    <div className="mx-auto flex max-w-7xl flex-col gap-4">
      <div
        className={
          selectedAgentId
            ? "flex items-center justify-between gap-3"
            : "flex flex-col justify-between gap-4 md:flex-row md:items-end"
        }
      >
        <div>
          {selectedAgentId ? (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setRequestedAgentId(null)}
            >
              ← Agents
            </Button>
          ) : (
            <h1 className="text-3xl font-semibold tracking-tight">Agents</h1>
          )}
        </div>
        <div className="flex items-center gap-2">
          <Button
            onClick={() => {
              void agentsQuery.refetch();
              if (selectedAgentId) {
                void detailQuery.refetch();
                for (const key of [
                  "agent-metrics",
                  "agent-collection",
                  "agent-updates",
                  "agent-logs",
                ])
                  void queryClient.invalidateQueries({
                    queryKey: [key, selectedAgentId],
                  });
              }
            }}
            size="sm"
            variant="outline"
          >
            <RefreshCwIcon data-icon="inline-start" />
            Refresh
          </Button>
          {!isEmptyState && !selectedAgentId ? (
            <Button onClick={() => setEnrollmentOpen(true)} size="sm">
              <DownloadIcon data-icon="inline-start" />
              Enroll agent
            </Button>
          ) : null}
        </div>
      </div>

      {!isEmptyState && !selectedAgentId && (
        <div
          className="flex flex-wrap gap-2"
          aria-label="Filter agents by status"
        >
          {["all", "online", "stale", "offline", "revoked"].map((status) => (
            <Button
              key={status}
              variant={
                (location.status ?? "all") === status ? "secondary" : "outline"
              }
              size="sm"
              aria-pressed={(location.status ?? "all") === status}
              onClick={() =>
                navigate({
                  status: status === "all" ? undefined : status,
                  cursor: undefined,
                })
              }
            >
              {labelize(status)}
              {counts?.[status] !== undefined ? ` (${counts[status]})` : ""}
            </Button>
          ))}
        </div>
      )}
      {!selectedAgentId && (
        <AgentBulkUpdates
          selected={bulkSelected}
          clear={() => setBulkSelected([])}
        />
      )}
      <div className="grid gap-6">
        {!selectedAgentId && (
          <Card aria-busy={agentsQuery.isLoading} className="min-w-0">
            <CardHeader className="border-b max-sm:grid-cols-1">
              <CardTitle>Enrolled agents</CardTitle>
              <CardDescription>
                {isEmptyState
                  ? "Enroll your first Linux agent to collect host inventory."
                  : `${visibleAgents.length} shown${agentsQuery.data?.total !== undefined ? ` of ${agentsQuery.data.total}` : ""}`}
              </CardDescription>
              {!isEmptyState ? (
                <CardAction className="max-sm:col-start-1 max-sm:row-start-2 max-sm:justify-self-stretch">
                  <div className="relative">
                    <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
                    <Input
                      aria-label="Search agents"
                      className="w-full pl-8 sm:w-56"
                      onChange={(event) => setSearch(event.target.value)}
                      placeholder="Search agents"
                      value={search}
                    />
                  </div>
                </CardAction>
              ) : null}
            </CardHeader>
            {agentsQuery.isLoading ? (
              <AgentListLoading />
            ) : agentsQuery.isError && !agentsQuery.data ? (
              <CardContent className="pt-6">
                <LoadError
                  error={agentsQuery.error}
                  retry={() => agentsQuery.refetch()}
                  title="Agents unavailable"
                />
              </CardContent>
            ) : visibleAgents.length === 0 ? (
              <CardContent className="pt-6">
                <Empty>
                  <EmptyHeader>
                    <EmptyMedia variant="icon">
                      <ServerIcon />
                    </EmptyMedia>
                    <EmptyTitle>
                      {search.trim()
                        ? "No matching agents"
                        : "No enrolled agents"}
                    </EmptyTitle>
                    <EmptyDescription>
                      {search.trim()
                        ? "Clear search to view all enrolled agents."
                        : "Enroll a Linux agent to collect host inventory."}
                    </EmptyDescription>
                    {!search.trim() ? (
                      <Button onClick={() => setEnrollmentOpen(true)}>
                        <DownloadIcon data-icon="inline-start" />
                        Enroll agent
                      </Button>
                    ) : null}
                  </EmptyHeader>
                </Empty>
              </CardContent>
            ) : (
              <AgentTable
                agents={visibleAgents}
                selectedAgentId={selectedAgentId}
                selectAgent={setRequestedAgentId}
                bulkSelected={bulkSelected}
                toggleBulk={toggleBulk}
              />
            )}
            {agentsQuery.data?.next_cursor && (
              <CardContent>
                <Button
                  variant="outline"
                  onClick={() =>
                    navigate({ cursor: agentsQuery.data!.next_cursor! })
                  }
                >
                  Next page
                </Button>
              </CardContent>
            )}
            {location.cursor && (
              <CardContent>
                <Button
                  variant="outline"
                  onClick={() => navigate({ cursor: undefined })}
                >
                  First page
                </Button>
              </CardContent>
            )}
          </Card>
        )}

        <AgentDetailPanel
          detail={detailQuery.data}
          error={detailQuery.error}
          loading={detailQuery.isLoading}
          onRetry={() => detailQuery.refetch()}
          selectedAgent={detailQuery.data ?? null}
          location={location}
          navigate={navigate}
        />
      </div>
      <EnrollmentDialog
        onOpenChange={setEnrollmentOpen}
        open={enrollmentOpen}
      />
    </div>
  );
}

function EnrollmentDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [baseUrl, setBaseUrl] = useState(agentConnectionAddress);
  const [addressError, setAddressError] = useState<string | null>(null);
  const scriptOrigin = new URL(
    addressError ? agentConnectionAddress() : baseUrl,
  );
  scriptOrigin.protocol = "https:";
  const scriptUrl = scriptOrigin.origin + "/agent/install.sh";
  const enrollmentMutation = useMutation({
    mutationFn: createAgentEnrollment,
  });
  const { mutate: createEnrollment, reset: resetEnrollment } =
    enrollmentMutation;
  const [copied, setCopied] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setBaseUrl(agentConnectionAddress());
    resetEnrollment();
    createEnrollment();
  }, [createEnrollment, open, resetEnrollment]);

  const shellQuote = (value: string | null | undefined) =>
    "'" + String(value ?? "").replaceAll("'", "'\\''") + "'";
  const enrollment = enrollmentMutation.data;
  const progress = useQuery({
    queryKey: ["agent-enrollment", enrollment?.attempt_id],
    queryFn: () => fetchAgentEnrollment(enrollment!.attempt_id),
    enabled: open && Boolean(enrollment?.attempt_id),
    refetchInterval: 3000,
  });
  const setup = progress.data;
  const installCommand = enrollment
    ? "curl -fsSL " +
      (baseUrl.startsWith("http:")
        ? "--insecure --pinnedpubkey " + shellQuote(enrollment.tls_pin) + " "
        : "") +
      shellQuote(scriptUrl) +
      " | sudo env HOPE_SERVER=" +
      shellQuote(baseUrl) +
      (baseUrl.startsWith("http:")
        ? " HOPE_TLS_PIN=" + shellQuote(enrollment.tls_pin)
        : "") +
      " HOPE_ENROLLMENT_CODE=" +
      shellQuote(enrollment.code) +
      " bash"
    : null;

  async function copy(label: string, value: string) {
    if (!navigator.clipboard) return;
    await navigator.clipboard.writeText(value);
    setCopied(label);
    window.setTimeout(() => setCopied(null), 2_000);
  }

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent className="min-w-0 max-w-2xl">
        <DialogHeader>
          <DialogTitle>Enroll a Linux agent</DialogTitle>
          <DialogDescription>
            Generate a single-use command, then paste it into the Linux host
            where the agent should run. The signed installer derives the
            enrollment and gateway endpoints from the Hope server origin.
          </DialogDescription>
        </DialogHeader>
        {enrollment && (
          <section
            aria-label="Enrollment progress"
            className="space-y-2 rounded-md border p-3"
            aria-live="polite"
          >
            <p className="font-medium">
              {setup?.revoked_at
                ? "Agent access revoked"
                : setup?.expired
                  ? "Command expired — create another"
                  : setup?.complete
                    ? `${setup.hostname || "Agent"} is sending data`
                    : "Waiting for this command’s agent"}
            </p>
            <ol className="grid gap-1 text-sm sm:grid-cols-2">
              {[
                ["Command created", true],
                ["Agent authenticated", setup?.authenticated_at],
                ["First inventory accepted", setup?.inventory_at],
                ["First metrics accepted", setup?.metrics_at],
              ].map(([label, done]) => (
                <li key={String(label)}>
                  {done ? "✓" : "○"} {label}
                </li>
              ))}
            </ol>
            {setup?.log_sources.length ? (
              <p className="text-xs">
                Selected log sources:{" "}
                {setup.log_sources.filter((source) => source.receiving).length}/
                {setup.log_sources.length} receiving. Logs do not block setup.
              </p>
            ) : (
              <p className="text-xs text-muted-foreground">
                Host and container logs are optional. Choose sources in agent
                Settings.
              </p>
            )}
            {progress.error && (
              <p role="alert" className="text-sm text-destructive">
                Progress unavailable. Retrying; setup is not yet verified.
              </p>
            )}
            {setup?.agent_id && (
              <a
                className="inline-block text-sm underline"
                href={`/agents?agent=${encodeURIComponent(setup.agent_id)}`}
              >
                Open this agent
              </a>
            )}
          </section>
        )}
        {!setup?.complete && (
          <div className="grid gap-2">
            <label
              className="text-sm font-medium"
              htmlFor="agent-connection-address"
            >
              Agent connection address
            </label>
            <Input
              id="agent-connection-address"
              value={baseUrl}
              onChange={(event) => {
                setBaseUrl(event.target.value);
                try {
                  saveAgentConnectionAddress(event.target.value);
                  setAddressError(null);
                } catch {
                  setAddressError(
                    "Enter the HTTP LAN address or HTTPS proxy address agents can reach.",
                  );
                }
              }}
            />
            <p className="text-xs text-muted-foreground">
              Use the address reachable from your devices. An HTTP LAN address
              uses Hope’s pinned HTTPS on port 443.
            </p>
            {addressError ? (
              <p className="text-xs text-destructive">{addressError}</p>
            ) : null}
          </div>
        )}
        {enrollmentMutation.isPending ? (
          <p aria-live="polite" className="text-sm text-muted-foreground">
            Preparing a one-time installer command…
          </p>
        ) : enrollmentMutation.error ? (
          <Alert variant="destructive">
            <CircleAlertIcon />
            <AlertTitle>Could not prepare the installer</AlertTitle>
            <AlertDescription>
              {getUserFacingError(
                enrollmentMutation.error,
                "The server did not return an enrollment code.",
              )}
            </AlertDescription>
          </Alert>
        ) : installCommand &&
          enrollment &&
          !addressError &&
          !setup?.complete &&
          !setup?.expired ? (
          <div className="grid gap-3">
            <p className="text-sm font-medium">Run this on the target host</p>
            <CommandBlock
              copied={copied === "install"}
              label="agent install command"
              onCopy={() => copy("install", installCommand)}
              value={installCommand}
            />
            <p className="text-xs text-muted-foreground">
              This command downloads the signed installer, enrolls the host,
              enables the agent service, and expires in{" "}
              {enrollment.expires_in_minutes} minutes. Treat it like a password
              and do not paste it into a shared terminal.
            </p>
          </div>
        ) : null}
        <DialogFooter>
          {enrollmentMutation.error || setup?.expired ? (
            <Button
              onClick={() => enrollmentMutation.mutate()}
              variant="outline"
            >
              Try again
            </Button>
          ) : null}
          <DialogClose render={<Button variant="outline" />}>Done</DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function CommandBlock({
  value,
  label,
  copied,
  onCopy,
}: {
  value: string;
  label: string;
  copied: boolean;
  onCopy: () => void;
}) {
  return (
    <div className="flex min-w-0 items-start gap-2 rounded-lg border bg-muted/40 p-2">
      <pre className="min-w-0 flex-1 overflow-x-auto whitespace-pre-wrap p-1 font-mono text-xs leading-relaxed">
        {value}
      </pre>
      <Button
        aria-label={`Copy ${label}`}
        onClick={onCopy}
        size="icon-sm"
        type="button"
        variant="ghost"
      >
        {copied ? <CheckIcon /> : <CopyIcon />}
      </Button>
    </div>
  );
}

function AgentTable({
  agents,
  selectedAgentId,
  selectAgent,
  bulkSelected,
  toggleBulk,
}: {
  agents: Agent[];
  bulkSelected: string[];
  toggleBulk: (id: string) => void;
  selectedAgentId: string | null;
  selectAgent: (id: string) => void;
}) {
  const percent = (value: number | undefined) =>
    typeof value === "number" && Number.isFinite(value)
      ? `${value.toFixed(0)}%`
      : "—";
  const dataAge = (agent: Agent) =>
    agent.telemetry
      ? formatRelative(agent.telemetry.collected_at)
      : "No metrics yet";
  const stale = (agent: Agent) =>
    !agent.telemetry ||
    Date.now() - Date.parse(agent.telemetry.collected_at) > 45_000;
  const backlog = (agent: Agent) =>
    (agent.telemetry?.delivery?.metrics?.queued_records ?? 0) +
    (agent.telemetry?.delivery?.logs?.queued_records ?? 0);
  return (
    <>
      <div className="divide-y sm:hidden" aria-label="Agents on this page">
        {agents.map((agent) => (
          <article key={agent.id} className="space-y-2 px-4 py-3">
            <div className="flex items-center gap-2">
              <input
                type="checkbox"
                className="size-4 shrink-0 accent-primary"
                aria-label={`Include ${agent.hostname || agent.id} in update`}
                checked={bulkSelected.includes(agent.id)}
                disabled={agent.status === "revoked"}
                onChange={() => toggleBulk(agent.id)}
              />
              <button
                className="min-w-0 flex-1 truncate text-left font-medium focus-visible:outline focus-visible:outline-ring"
                onClick={() => selectAgent(agent.id)}
                aria-label={`Open ${agent.hostname || "agent"}`}
              >
                {agent.hostname || "Unnamed agent"}
              </button>
              <AgentStatusBadge status={agent.status} />
            </div>
            <div className="flex justify-between gap-2 text-sm">
              <span>CPU {percent(agent.telemetry?.cpu_percent)}</span>
              <span>Memory {percent(agent.telemetry?.memory_percent)}</span>
              <span className="text-muted-foreground">
                v{agent.agent_version ?? "?"}
              </span>
            </div>
            <p
              className={`text-xs ${stale(agent) ? "text-amber-700 dark:text-amber-400" : "text-muted-foreground"}`}
            >
              {agent.telemetry ? `Metrics ${dataAge(agent)}` : dataAge(agent)}
              {backlog(agent) > 2 ? ` · ${backlog(agent)} queued` : ""}
            </p>
          </article>
        ))}
      </div>
      <div className="hidden sm:block">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>
                <span className="sr-only">Select for updates</span>
              </TableHead>
              <TableHead>Agent</TableHead>
              <TableHead>Connection</TableHead>
              <TableHead>CPU / memory</TableHead>
              <TableHead>Data delivery</TableHead>
              <TableHead>Version</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {agents.map((agent) => (
              <TableRow
                key={agent.id}
                aria-selected={agent.id === selectedAgentId}
              >
                <TableCell>
                  <input
                    type="checkbox"
                    className="size-4 accent-primary"
                    aria-label={`Include ${agent.hostname || agent.id} in update`}
                    checked={bulkSelected.includes(agent.id)}
                    disabled={agent.status === "revoked"}
                    onChange={() => toggleBulk(agent.id)}
                  />
                </TableCell>
                <TableCell className="min-w-40">
                  <button
                    aria-label={`Select ${agent.hostname || "agent"}`}
                    className="block max-w-64 truncate rounded text-left font-medium focus-visible:outline focus-visible:outline-ring"
                    onClick={() => selectAgent(agent.id)}
                  >
                    {agent.hostname || "Unnamed agent"}
                  </button>
                  <p className="mt-1 text-xs text-muted-foreground">
                    {agent.os ? labelize(agent.os) : "Platform not reported"}
                  </p>
                </TableCell>
                <TableCell>
                  <AgentStatusBadge status={agent.status} />
                  <p
                    className="mt-1 text-xs text-muted-foreground"
                    title={formatDate(agent.last_seen)}
                  >
                    {formatRelative(agent.last_seen)}
                  </p>
                </TableCell>
                <TableCell className="tabular-nums">
                  <span>
                    {percent(agent.telemetry?.cpu_percent)} /{" "}
                    {percent(agent.telemetry?.memory_percent)}
                  </span>
                  {stale(agent) && agent.telemetry && (
                    <p className="text-xs text-muted-foreground">
                      Last readings
                    </p>
                  )}
                </TableCell>
                <TableCell>
                  <p
                    className={
                      stale(agent) ? "text-amber-700 dark:text-amber-400" : ""
                    }
                  >
                    {agent.telemetry
                      ? `Metrics ${dataAge(agent)}`
                      : dataAge(agent)}
                  </p>
                  {backlog(agent) > 2 && (
                    <p className="text-xs text-muted-foreground">
                      {backlog(agent)} records queued
                    </p>
                  )}
                </TableCell>
                <TableCell className="text-sm">
                  {agent.agent_version ?? "Not reported"}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </>
  );
}

function AgentDetailPanel({
  detail,
  error,
  loading,
  onRetry,
  selectedAgent,
  location,
  navigate,
}: {
  detail: AgentDetail | undefined;
  error: unknown;
  loading: boolean;
  onRetry: () => void;
  selectedAgent: Agent | null;
  location: AgentLocation;
  navigate: NavigateAgent;
}) {
  const activeTab = location.tab ?? "overview";
  const setActiveTab = (tab: string) => navigate({ tab });
  const metricRange: AgentMetricRange = [
    "1h",
    "6h",
    "24h",
    "7d",
    "30d",
    "180d",
  ].includes(location.range ?? "")
    ? (location.range as AgentMetricRange)
    : "1h";
  const setMetricRange = (range: AgentMetricRange) => navigate({ range });
  const metricsQuery = useQuery({
    queryKey: ["agent-metrics", detail?.id, metricRange],
    queryFn: () => fetchAgentMetrics(detail!.id, metricRange),
    enabled: Boolean(detail?.id),
    refetchInterval: 15_000,
  });

  if (loading) {
    return (
      <Card>
        <CardContent className="flex flex-col gap-4">
          <Skeleton className="h-8 w-2/3" />
          <Skeleton className="h-16 w-full" />
          <Skeleton className="h-48 w-full" />
        </CardContent>
      </Card>
    );
  }

  if (error && !detail) {
    return (
      <Card>
        <CardContent className="pt-6">
          <LoadError
            error={error}
            retry={onRetry}
            title="Agent detail unavailable"
          />
        </CardContent>
      </Card>
    );
  }

  if (!detail || !selectedAgent) {
    if (!loading && !error) return null;
    return (
      <Card>
        <CardContent>
          <Empty>
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <ServerIcon />
              </EmptyMedia>
              <EmptyTitle>Select an agent</EmptyTitle>
              <EmptyDescription>
                View host inventory, socket reachability, and evidence state.
              </EmptyDescription>
            </EmptyHeader>
          </Empty>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card className="min-w-0">
      <CardHeader className="border-b">
        <div className="min-w-0">
          <h1
            className="truncate text-2xl font-semibold tracking-tight"
            title={detail.hostname ?? undefined}
          >
            {detail.hostname || "Unnamed agent"}
          </h1>
          <CardDescription className="flex flex-wrap gap-x-2 gap-y-1">
            <span>{detail.os ? labelize(detail.os) : "Unknown platform"}</span>
            <span>Last contact {formatRelative(detail.last_seen)}</span>
          </CardDescription>
        </div>
        <CardAction>
          <AgentStatusBadge status={detail.status} />
        </CardAction>
      </CardHeader>
      <CardContent className="flex min-w-0 flex-col gap-5">
        <Tabs
          className="min-w-0"
          onValueChange={setActiveTab}
          value={activeTab}
        >
          <TabsList
            aria-label="Agent views"
            className="grid w-full grid-cols-4 gap-1 group-data-horizontal/tabs:h-auto"
            variant="line"
          >
            <TabsTrigger className="min-h-10" value="overview">
              Overview
            </TabsTrigger>
            <TabsTrigger className="min-h-10" value="metrics">
              Metrics
            </TabsTrigger>
            <TabsTrigger className="min-h-10" value="logs">
              Logs
            </TabsTrigger>
            <TabsTrigger className="min-h-10" value="settings">
              Settings
            </TabsTrigger>
          </TabsList>

          <TabsContent className="pt-4" value="overview">
            <AgentAtGlance
              agent={detail}
              metrics={metricsQuery.data}
              loading={metricsQuery.isLoading}
              error={error || metricsQuery.error}
              navigate={navigate}
            />
          </TabsContent>

          <TabsContent className="space-y-6 pt-4" value="settings">
            <AgentSettings agentId={detail.id} />
            <AgentUpdatePanel agentId={detail.id} />
            <details className="space-y-4 rounded-lg border p-4">
              <summary className="cursor-pointer font-medium">
                Agent identity and delivery details
              </summary>
              <div className="grid gap-3 pt-4 sm:grid-cols-2">
                <InfoPair label="Agent ID" value={detail.id} mono />
                <InfoPair
                  label="Platform"
                  value={`${detail.os ?? "unknown"} ${detail.arch ?? ""}`}
                />
                <InfoPair
                  label="Protocol"
                  value={`v${detail.protocol_version ?? "unknown"}`}
                />
                <InfoPair
                  label="Last heartbeat"
                  value={formatDate(detail.last_heartbeat_at)}
                />
              </div>

              <section
                className="space-y-3 rounded-lg border p-4"
                aria-label="Data delivery"
              >
                <h2 className="font-medium">Data delivery</h2>
                {metricsQuery.error && (
                  <p role="status" className="text-sm text-destructive">
                    Could not refresh delivery status. Cached values remain
                    visible.
                  </p>
                )}
                <p className="text-sm">
                  Last sample:{" "}
                  {metricsQuery.data?.latest
                    ? formatRelative(metricsQuery.data.latest.collected_at)
                    : "Waiting for first metrics"}{" "}
                  · {metricsQuery.data?.freshness.state ?? "unknown"}
                </p>
                <div className="grid gap-3 sm:grid-cols-2">
                  {["metrics", "logs"].map((stream) => {
                    const delivery = metricsQuery.data?.latest?.metrics
                      .delivery as
                      | Record<
                          string,
                          {
                            queued_records?: number;
                            queued_bytes?: number;
                            oldest_at?: number;
                            loss?: { records?: number; reason?: string };
                          }
                        >
                      | undefined;
                    const status = delivery?.[stream];
                    return (
                      <div key={stream} className="rounded border p-3 text-sm">
                        <h3 className="font-medium">{labelize(stream)}</h3>
                        {status ? (
                          <>
                            <p>
                              {status.queued_records ?? 0} queued ·{" "}
                              {formatBytes(status.queued_bytes ?? 0)}
                            </p>
                            {status.oldest_at && (
                              <p className="text-muted-foreground">
                                Oldest queued:{" "}
                                {new Date(
                                  status.oldest_at * 1000,
                                ).toLocaleString()}
                              </p>
                            )}
                            {Boolean(status.loss?.records) && (
                              <p className="text-destructive">
                                {status.loss?.records} records lost:{" "}
                                {status.loss?.reason}
                              </p>
                            )}
                          </>
                        ) : (
                          <p className="text-muted-foreground">
                            Delivery counters unavailable for this agent.
                          </p>
                        )}
                      </div>
                    );
                  })}
                </div>
                <p className="text-xs text-muted-foreground">
                  Queue counts reflect the last received sample. Metrics and
                  logs each retain up to 64 MiB or 24 hours locally.
                </p>
              </section>
              {Boolean(error) && (
                <p role="status" className="text-sm text-destructive">
                  Refresh failed. Showing the last loaded agent data.
                </p>
              )}
              {detail.device_id && (
                <a
                  href={`/devices?device=${encodeURIComponent(detail.device_id)}`}
                  className="text-sm underline"
                >
                  Open associated device
                </a>
              )}
              {detail.collector_status
                ?.filter((collector) => collector.status !== "available")
                .map((collector) => (
                  <p key={collector.capability} className="text-sm">
                    {labelize(collector.capability)}: {collector.status} —{" "}
                    {collector.error || "No diagnostic provided"}
                  </p>
                ))}
              <section className="flex flex-col gap-2">
                <h2 className="text-sm font-medium">Inventory summary</h2>
                <InventorySummary
                  summary={detail.inventory_summary ?? EMPTY_INVENTORY_SUMMARY}
                />
              </section>
              <ReconciliationSummary detail={detail} />
              <section className="flex flex-col gap-2">
                <div className="flex items-end justify-between gap-3">
                  <div>
                    <h2 className="text-sm font-medium">Capabilities</h2>
                    <p className="text-xs text-muted-foreground">
                      Collectors reported by agent.
                    </p>
                  </div>
                  <Badge variant="secondary">
                    {detail.capabilities.length}
                  </Badge>
                </div>
                <CapabilityBadges capabilities={detail.capabilities} expanded />
              </section>
              <ReachabilityExplanation />
              <EvidenceList evidence={detail.evidence} />
            </details>
            <section
              aria-label="Host inventory"
              className="space-y-3 border-t pt-4"
            >
              <h2 className="font-medium">Inventory</h2>
              {[
                ["Host", <HostInventory host={detail.host} />],
                ["Network", <NetworkInventory network={detail.network} />],
                [
                  "Filesystems",
                  <FilesystemInventory filesystems={detail.filesystems} />,
                ],
                [
                  "Processes",
                  <ProcessInventory processes={detail.processes} />,
                ],
                ["Sockets", <SocketInventory sockets={detail.sockets} />],
                [
                  "Containers",
                  <ContainerInventory containers={detail.containers} />,
                ],
              ].map(([title, content]) => (
                <details key={String(title)} className="rounded-lg border p-4">
                  <summary className="cursor-pointer font-medium">
                    {title}
                  </summary>
                  <div className="pt-4">{content}</div>
                </details>
              ))}
            </section>
          </TabsContent>
          <TabsContent className="pt-4" value="logs">
            <AgentLogs
              key={`${detail.id}-${location.from}-${location.to}-${location.logq}-${location.source}-${location.severity}`}
              agentId={detail.id}
              location={location}
              navigate={navigate}
            />
          </TabsContent>
          <TabsContent className="space-y-4 pt-4" value="metrics">
            <Button
              variant="outline"
              onClick={() => {
                const end = Math.floor(Date.now() / 1000);
                const seconds = {
                  "1h": 3600,
                  "6h": 21600,
                  "24h": 86400,
                  "7d": 604800,
                  "30d": 2592000,
                  "180d": 15552000,
                }[metricRange];
                navigate({
                  tab: "logs",
                  from: String(end - seconds),
                  to: String(end),
                });
              }}
            >
              View logs for this time range
            </Button>
            <AgentMetricsView
              data={metricsQuery.data}
              error={metricsQuery.error}
              loading={metricsQuery.isLoading}
              onRangeChange={setMetricRange}
              range={metricRange}
              retry={() => void metricsQuery.refetch()}
            />
          </TabsContent>
        </Tabs>
      </CardContent>
    </Card>
  );
}

function ReconciliationSummary({ detail }: { detail: AgentDetail }) {
  const isConflict = detail.reconciliation.status === "conflict";
  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-sm font-medium">Evidence reconciliation</h2>
      <Alert variant={isConflict ? "destructive" : undefined}>
        {isConflict ? <CircleAlertIcon /> : <ShieldCheckIcon />}
        <AlertTitle>
          {labelize(detail.reconciliation.status)}
          {detail.reconciliation.confidence === null
            ? ""
            : ` · ${Math.round(detail.reconciliation.confidence * 100)}% confidence`}
        </AlertTitle>
        <AlertDescription>
          {detail.reconciliation.explanation ??
            "Agent evidence has no reconciliation explanation."}
          <div className="mt-2 flex flex-wrap gap-1.5">
            {detail.reconciliation.device_id ? (
              <Badge variant="outline">
                Device {shortId(detail.reconciliation.device_id)}
              </Badge>
            ) : null}
            {detail.reconciliation.matched_identifiers.map((identifier) => (
              <Badge key={identifier} variant="secondary">
                {identifier}
              </Badge>
            ))}
          </div>
          {detail.reconciliation.conflicts.length ? (
            <ul className="mt-2 list-disc pl-4">
              {detail.reconciliation.conflicts.map((conflict) => (
                <li key={conflict}>{conflict}</li>
              ))}
            </ul>
          ) : null}
          {detail.reconciliation.last_reconciled_at ? (
            <p className="mt-2 text-xs">
              Reconciled{" "}
              {formatRelative(detail.reconciliation.last_reconciled_at)}
            </p>
          ) : null}
        </AlertDescription>
      </Alert>
    </section>
  );
}

function AgentMetricsView({
  data,
  error,
  loading,
  onRangeChange,
  range,
  retry,
}: {
  data: AgentMetricsResponse | undefined;
  error: unknown;
  loading: boolean;
  onRangeChange: (range: AgentMetricRange) => void;
  range: AgentMetricRange;
  retry: () => void;
}) {
  const networkInterfaces = data?.dimensions.network_interfaces ?? [];
  const diskDevices = data?.dimensions.disk_devices ?? [];
  const gpuDevices = data?.dimensions.gpu_devices ?? [];
  const [interfaceSelection, setInterfaceSelection] = useState("");
  const [diskSelection, setDiskSelection] = useState("");
  const [gpuSelection, setGpuSelection] = useState("");
  const selectedInterface = networkInterfaces.includes(interfaceSelection)
    ? interfaceSelection
    : (networkInterfaces[0] ?? "");
  const selectedDisk = diskDevices.includes(diskSelection)
    ? diskSelection
    : (diskDevices[0] ?? "");
  const selectedGpu = gpuDevices.some((device) => device.id === gpuSelection)
    ? gpuSelection
    : (gpuDevices[0]?.id ?? "");

  if (loading) {
    return (
      <div className="grid gap-4">
        <Skeleton className="h-9 w-56" />
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          {Array.from({ length: 4 }, (_, index) => (
            <Skeleton className="h-24" key={index} />
          ))}
        </div>
        <Skeleton className="h-64 w-full" />
      </div>
    );
  }

  if (error && !data) {
    return (
      <LoadError error={error} retry={retry} title="Metrics unavailable" />
    );
  }

  if (!data) {
    return (
      <MetricEmpty
        description="No resource telemetry response has been received."
        title="No metrics"
      />
    );
  }

  const latestMetrics = data.latest?.metrics;
  const cpu = latestSeriesValue(data, "cpu.usage_percent");
  const memory = metricNumber(latestMetrics, ["memory", "used_percent"]);
  const network = sumLatestDeviceMetric(
    latestMetrics,
    ["network", "interfaces"],
    "rx_bytes_per_sec",
  );
  const disk = sumLatestDeviceMetric(
    latestMetrics,
    ["disk", "devices"],
    "utilization_percent",
  );
  const pressure = metricNumber(latestMetrics, [
    "pressure",
    "io",
    "some_avg10",
  ]);
  const gpu = selectedGpu
    ? latestDeviceMetric(
        latestMetrics,
        ["gpu", "devices"],
        selectedGpu,
        "utilization_percent",
      )
    : null;
  const stale = data.freshness.state === "stale";
  const partial = !["available", "empty"].includes(data.availability.status);

  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-sm font-medium">Resource telemetry</h2>
          <p className="text-xs text-muted-foreground">
            Host and device samples. No per-process or container series in this
            view.
          </p>
        </div>
        <div
          aria-label="Metric range"
          className="flex flex-wrap gap-1"
          role="group"
        >
          {(
            [
              ["1h", "1 hour"],
              ["6h", "6 hours"],
              ["24h", "24 hours"],
              ["7d", "7 days"],
              ["30d", "30 days"],
              ["180d", "180 days"],
            ] as const
          ).map(([value, label]) => (
            <Button
              aria-pressed={range === value}
              key={value}
              onClick={() => onRangeChange(value)}
              size="sm"
              variant={range === value ? "default" : "outline"}
            >
              {label}
            </Button>
          ))}
        </div>
      </div>

      <div
        className="space-y-1 text-xs text-muted-foreground"
        aria-live="polite"
      >
        <p>
          {(data.resolution_seconds ?? 15) < 60
            ? `${data.resolution_seconds ?? 15}-second`
            : (data.resolution_seconds ?? 15) < 3600
              ? `${(data.resolution_seconds ?? 15) / 60}-minute`
              : `${(data.resolution_seconds ?? 15) / 3600}-hour`}{" "}
          buckets · Average with minimum/maximum · Latest sample{" "}
          {data.latest
            ? formatRelative(data.latest.collected_at)
            : "not received"}
        </p>
        {stale && (
          <p className="text-amber-700 dark:text-amber-400">
            Samples are stale. Readings below are historical.
          </p>
        )}
        {partial && (
          <p>Partial telemetry. Unavailable readings remain blank.</p>
        )}
        {Boolean(error) && (
          <p className="text-destructive">
            Refresh failed. Showing previously loaded readings.{" "}
            <button type="button" onClick={retry} className="underline">
              Retry
            </button>
          </p>
        )}
      </div>

      <div className="grid grid-cols-2 gap-2 lg:grid-cols-3">
        <MetricValueCard label="CPU used" value={formatMetricPercent(cpu)} />
        <MetricValueCard
          label="Memory used"
          value={formatMetricPercent(memory)}
        />
        <MetricValueCard label="Network receive" value={formatRate(network)} />
        <MetricValueCard label="Disk busy" value={formatMetricPercent(disk)} />
        <MetricValueCard
          label="I/O pressure (some)"
          value={formatMetricPercent(pressure)}
        />
        {gpuDevices.length > 0 && (
          <MetricValueCard
            label="GPU utilization"
            value={formatMetricPercent(gpu)}
          />
        )}
      </div>

      <MetricChart
        color="var(--chart-1)"
        data={seriesFor(data, "cpu.usage_percent")}
        description="CPU busy percentage over the selected range."
        id="agent-cpu-metrics"
        title="CPU"
        unit="percent"
      />
      <MetricChart
        color="var(--chart-2)"
        data={seriesFor(data, "memory.used_percent")}
        description="Memory utilization over the selected range."
        id="agent-memory-metrics"
        title="Memory"
        unit="percent"
      />
      <MetricChart
        color="var(--chart-3)"
        data={
          selectedInterface
            ? seriesFor(
                data,
                `network.interfaces.${selectedInterface}.rx_bytes_per_sec`,
              )
            : []
        }
        description="Receive throughput for the selected interface."
        id="agent-network-metrics"
        selector={
          networkInterfaces.length > 1 ? (
            <MetricSelector
              ariaLabel="Network interface"
              onChange={setInterfaceSelection}
              options={networkInterfaces}
              value={selectedInterface}
            />
          ) : null
        }
        title="Network receive"
        unit="bytes"
      />
      <MetricChart
        color="var(--chart-4)"
        data={
          selectedDisk
            ? seriesFor(
                data,
                `disk.devices.${selectedDisk}.utilization_percent`,
              )
            : []
        }
        description="Disk utilization for the selected block device."
        id="agent-disk-metrics"
        selector={
          diskDevices.length > 1 ? (
            <MetricSelector
              ariaLabel="Disk device"
              onChange={setDiskSelection}
              options={diskDevices}
              value={selectedDisk}
            />
          ) : null
        }
        title="Disk I/O"
        unit="percent"
      />
      <MetricChart
        color="var(--chart-5)"
        data={seriesFor(data, "pressure.io.some_avg10")}
        description="Linux I/O pressure some average over ten seconds."
        id="agent-pressure-metrics"
        title="I/O pressure"
        unit="percent"
      />
      <MetricChart
        color="var(--chart-1)"
        data={
          selectedGpu
            ? seriesFor(data, `gpu.devices.${selectedGpu}.utilization_percent`)
            : []
        }
        description="GPU utilization for the selected device."
        emptyLabel={
          gpuDevices.length ? undefined : "No supported GPU telemetry reported."
        }
        id="agent-gpu-metrics"
        selector={
          gpuDevices.length > 1 ? (
            <MetricSelector
              ariaLabel="GPU device"
              labelForOption={(id) =>
                gpuDevices.find((device) => device.id === id)?.name ?? id
              }
              onChange={setGpuSelection}
              options={gpuDevices.map((device) => device.id)}
              value={selectedGpu}
            />
          ) : null
        }
        title="GPU"
        unit="percent"
      />

      <div className="flex flex-wrap gap-2 text-xs text-muted-foreground">
        <span>Collectors:</span>
        {Object.entries(data.availability.collectors).map(
          ([collector, status]) => (
            <Badge
              key={collector}
              variant={status === "available" ? "secondary" : "outline"}
            >
              {labelize(collector)}: {labelize(status)}
            </Badge>
          ),
        )}
      </div>
    </div>
  );
}

function MetricValueCard({ label, value }: { label: string; value: string }) {
  return (
    <Card className="gap-2 py-4">
      <CardContent>
        <p className="text-xs text-muted-foreground">{label}</p>
        <p className="mt-1 text-xl font-semibold tabular-nums">{value}</p>
      </CardContent>
    </Card>
  );
}

function MetricChart({
  color,
  data,
  description,
  emptyLabel = "No samples in this range.",
  id,
  selector,
  title,
  unit,
}: {
  color: string;
  data: Array<{
    time: number;
    value: number | null;
    minimum?: number;
    maximum?: number;
  }>;
  description: string;
  emptyLabel?: string;
  id: string;
  selector?: ReactNode;
  title: string;
  unit: "bytes" | "percent";
}) {
  return (
    <section className="rounded-lg border p-4">
      <div className="mb-3 flex flex-wrap items-start justify-between gap-3">
        <div>
          <h3 className="text-sm font-medium">{title}</h3>
          <p className="text-xs text-muted-foreground">{description}</p>
        </div>
        {selector}
      </div>
      {data.some((point) => point.value !== null) ? (
        <>
          <ChartContainer
            aria-describedby={`${id}-summary`}
            aria-label={`${title} chart`}
            className="h-52 w-full"
            config={{ value: { label: title, color } }}
            role="img"
          >
            <LineChart
              accessibilityLayer
              data={data}
              margin={{ left: -18, right: 8, top: 8 }}
            >
              <CartesianGrid vertical={false} />
              <XAxis
                axisLine={false}
                dataKey="time"
                type="number"
                domain={["dataMin", "dataMax"]}
                tickFormatter={(value) =>
                  formatMetricTime(
                    new Date(value).toISOString(),
                    data.length > 1 &&
                      data[data.length - 1].time - data[0].time >= 86400000,
                  )
                }
                minTickGap={28}
                tickLine={false}
                tickMargin={8}
              />
              <YAxis
                axisLine={false}
                tickFormatter={(value) => formatMetricAxis(value, unit)}
                tickLine={false}
                width={52}
              />
              <ChartTooltip
                content={
                  <ChartTooltipContent
                    formatter={(value) => formatMetricAxis(Number(value), unit)}
                    indicator="line"
                    labelFormatter={(value) =>
                      new Date(Number(value)).toLocaleString()
                    }
                  />
                }
                cursor={false}
              />
              <Line
                dataKey="minimum"
                name="Minimum"
                dot={false}
                stroke="var(--color-value)"
                strokeOpacity={0.4}
                strokeDasharray="3 3"
                type="linear"
                connectNulls={false}
              />
              <Line
                dataKey="maximum"
                name="Maximum"
                dot={false}
                stroke="var(--color-value)"
                strokeOpacity={0.4}
                strokeDasharray="3 3"
                type="linear"
                connectNulls={false}
              />
              <Line
                dataKey="value"
                dot={false}
                stroke="var(--color-value)"
                strokeWidth={2}
                type="linear"
                connectNulls={false}
              />
            </LineChart>
          </ChartContainer>
          <p
            className="mt-2 text-xs text-muted-foreground"
            id={`${id}-summary`}
          >
            {summarizeMetricChart(data, title, unit)}
          </p>
        </>
      ) : (
        <MetricEmpty description={emptyLabel} title="No chart data" />
      )}
    </section>
  );
}

function MetricSelector({
  ariaLabel,
  labelForOption,
  onChange,
  options,
  value,
}: {
  ariaLabel: string;
  labelForOption?: (value: string) => string;
  onChange: (value: string) => void;
  options: string[];
  value: string;
}) {
  return (
    <select
      aria-label={ariaLabel}
      className="h-8 rounded-md border bg-background px-2 text-xs"
      onChange={(event) => onChange(event.target.value)}
      value={value}
    >
      {options.map((option) => (
        <option key={option} value={option}>
          {labelForOption?.(option) ?? option}
        </option>
      ))}
    </select>
  );
}

function MetricEmpty({
  description,
  title,
}: {
  description: string;
  title: string;
}) {
  return (
    <div className="rounded-lg border border-dashed p-8 text-center">
      <p className="text-sm font-medium">{title}</p>
      <p className="mt-1 text-sm text-muted-foreground">{description}</p>
    </div>
  );
}

function seriesFor(data: AgentMetricsResponse, key: string) {
  const result: Array<{
    time: number;
    value: number | null;
    minimum?: number;
    maximum?: number;
  }> = [];
  const cadence = (data.resolution_seconds ?? 15) * 1000;
  for (const point of data.series) {
    const time = Date.parse(point.timestamp);
    if (!Number.isFinite(time)) continue;
    const previous = result.at(-1);
    if (previous && time - previous.time > cadence * 1.5)
      result.push({ time: previous.time + cadence, value: null });
    const stats = point.values[key];
    result.push({
      time,
      value: stats && Number.isFinite(stats.average) ? stats.average : null,
      minimum: stats?.minimum,
      maximum: stats?.maximum,
    });
  }
  return result;
}

function latestSeriesValue(
  data: AgentMetricsResponse,
  key: string,
): number | null {
  const latest = metricNumber(data.latest?.metrics, key.split("."));
  if (latest !== null) return latest;
  const value = data.series.at(-1)?.values[key]?.latest;
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function metricNumber(
  metrics: Record<string, unknown> | undefined,
  path: string[],
): number | null {
  let current: unknown = metrics;
  for (const part of path) {
    if (!current || typeof current !== "object") return null;
    current = (current as Record<string, unknown>)[part];
  }
  return typeof current === "number" && Number.isFinite(current)
    ? current
    : null;
}

function latestDeviceMetric(
  metrics: Record<string, unknown> | undefined,
  path: string[],
  id: string,
  field: string,
): number | null {
  const devices = metricNumberArray(metrics, path);
  const device = devices.find(
    (item) => String(item.id ?? item.name ?? item.index) === id,
  );
  return typeof device?.[field] === "number" ? (device[field] as number) : null;
}

function sumLatestDeviceMetric(
  metrics: Record<string, unknown> | undefined,
  path: string[],
  field: string,
): number | null {
  const values = metricNumberArray(metrics, path)
    .map((item) => item[field])
    .filter(
      (value): value is number =>
        typeof value === "number" && Number.isFinite(value),
    );
  return values.length ? values.reduce((sum, value) => sum + value, 0) : null;
}

function metricNumberArray(
  metrics: Record<string, unknown> | undefined,
  path: string[],
): Array<Record<string, unknown>> {
  let current: unknown = metrics;
  for (const part of path) {
    if (!current || typeof current !== "object") return [];
    current = (current as Record<string, unknown>)[part];
  }
  return Array.isArray(current)
    ? current.filter(
        (item): item is Record<string, unknown> =>
          Boolean(item) && typeof item === "object" && !Array.isArray(item),
      )
    : [];
}

function formatMetricPercent(value: number | null): string {
  return value === null ? "—" : `${value.toFixed(1)}%`;
}

function formatRate(value: number | null): string {
  if (value === null) return "—";
  if (value < 1024) return `${value.toFixed(0)} B/s`;
  return `${formatBytes(value)}/s`;
}

function formatMetricTime(value: string, showDate = false): string {
  const date = new Date(value);
  return Number.isNaN(date.valueOf())
    ? value
    : showDate
      ? date.toLocaleDateString([], { month: "short", day: "numeric" })
      : date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function formatMetricAxis(value: number, unit: "bytes" | "percent"): string {
  if (unit === "percent") return `${value.toFixed(0)}%`;
  return formatRate(value);
}

function summarizeMetricChart(
  data: Array<{
    time: number;
    value: number | null;
    minimum?: number;
    maximum?: number;
  }>,
  title: string,
  unit: "bytes" | "percent",
): string {
  const values = data
    .map((point) => point.value)
    .filter((value): value is number => value !== null);
  if (!values.length) return `${title}: no numeric samples.`;
  const minimum = Math.min(
    ...data
      .map((point) => point.minimum ?? point.value)
      .filter((value): value is number => value !== null),
  );
  const maximum = Math.max(
    ...data
      .map((point) => point.maximum ?? point.value)
      .filter((value): value is number => value !== null),
  );
  const latest = values.at(-1) ?? minimum;
  return `${title}: ${values.length} populated buckets; minimum ${formatMetricAxis(minimum, unit)}, maximum ${formatMetricAxis(maximum, unit)}, last bucket average ${formatMetricAxis(latest, unit)}.`;
}

function ReachabilityExplanation() {
  return (
    <Alert>
      <NetworkIcon />
      <AlertTitle>Internal listener vs worker reachability</AlertTitle>
      <AlertDescription>
        Listening means the agent observed a local socket. Reachable means the
        monitoring worker completed an independent network check. These states
        can differ.
      </AlertDescription>
    </Alert>
  );
}

function EvidenceList({ evidence }: { evidence: AgentDetail["evidence"] }) {
  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-end justify-between gap-3">
        <div>
          <h2 className="text-sm font-medium">Recent evidence</h2>
          <p className="text-xs text-muted-foreground">
            Agent observations and source.
          </p>
        </div>
        <Badge variant={evidence.length ? "outline" : "secondary"}>
          {evidence.length}
        </Badge>
      </div>
      {evidence.length ? (
        <div className="flex flex-col divide-y rounded-lg border">
          {evidence.slice(0, 6).map((item) => (
            <div
              className="flex flex-wrap items-center gap-2 p-3"
              key={item.id}
            >
              <DatabaseIcon className="size-4 text-muted-foreground" />
              <div className="min-w-0 flex-1">
                <p className="font-medium">
                  {item.attribute} <span className="font-normal">from</span>{" "}
                  {labelize(item.source)}
                </p>
                <p className="max-w-full truncate font-mono text-xs text-muted-foreground">
                  {displayValue(item.value)}
                </p>
              </div>
              <Badge variant={item.confirmed ? "default" : "secondary"}>
                {item.confirmed
                  ? "Confirmed"
                  : `${Math.round(item.confidence * 100)}%`}
              </Badge>
              <time
                className="text-xs text-muted-foreground"
                title={formatDate(item.observed_at)}
              >
                {formatRelative(item.observed_at)}
              </time>
            </div>
          ))}
        </div>
      ) : (
        <InventoryEmpty
          description="No evidence has been reported by this agent."
          icon={<FileTextIcon />}
          title="No evidence"
        />
      )}
    </section>
  );
}

function HostInventory({ host }: { host: AgentHostInventory | null }) {
  if (!host) {
    return (
      <InventoryEmpty
        description="Host collector has not reported."
        title="No host inventory"
      />
    );
  }
  return (
    <section className="flex flex-col gap-3">
      <div>
        <h2 className="text-sm font-medium">Host inventory</h2>
        <p className="text-xs text-muted-foreground">
          Operating system and machine facts reported by agent.
        </p>
      </div>
      <dl className="grid gap-x-4 gap-y-3 rounded-lg border p-4 sm:grid-cols-2">
        <InfoPair label="Hostname" value={host.hostname} />
        <InfoPair label="Operating system" value={host.os} />
        <InfoPair label="Distribution" value={host.distribution} />
        <InfoPair label="Kernel" value={host.kernel} mono />
        <InfoPair label="Architecture" value={host.arch} />
        <InfoPair label="Uptime" value={formatDuration(host.uptime_seconds)} />
        <InfoPair label="CPU" value={host.cpu_model} />
        <InfoPair label="CPU count" value={formatNumber(host.cpu_count)} />
        <InfoPair label="Load (1m)" value={formatDecimal(host.load_1m)} />
        <InfoPair
          label="Memory"
          value={`${formatBytes(host.memory_used_bytes)} / ${formatBytes(host.memory_total_bytes)}`}
        />
        <InfoPair label="Boot ID" value={host.boot_id} mono />
        <InfoPair label="Machine ID hash" value={host.machine_id_hash} mono />
      </dl>
    </section>
  );
}

function NetworkInventory({
  network,
}: {
  network: AgentNetworkInventory | null;
}) {
  if (!network) {
    return (
      <InventoryEmpty
        description="Network collector has not reported."
        title="No network inventory"
      />
    );
  }
  return (
    <section className="flex flex-col gap-5">
      <div className="flex flex-col gap-2">
        <div>
          <h2 className="text-sm font-medium">Interfaces</h2>
          <p className="text-xs text-muted-foreground">
            Addresses and link state from host.
          </p>
        </div>
        {network.interfaces.length ? (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead>Addresses</TableHead>
                <TableHead>MAC</TableHead>
                <TableHead>State</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {network.interfaces.map((networkInterface) => (
                <TableRow key={networkInterface.name}>
                  <TableCell className="font-medium">
                    {networkInterface.name}
                  </TableCell>
                  <TableCell className="max-w-48 truncate">
                    {networkInterface.addresses.join(", ") || "—"}
                  </TableCell>
                  <TableCell className="font-mono text-xs">
                    {networkInterface.mac ?? "—"}
                  </TableCell>
                  <TableCell>{networkInterface.state ?? "—"}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        ) : (
          <InventoryEmpty
            description="No interfaces reported."
            title="No interfaces"
          />
        )}
      </div>
      <div className="flex flex-col gap-2">
        <h2 className="text-sm font-medium">Routes</h2>
        {network.routes.length ? (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Destination</TableHead>
                <TableHead>Gateway</TableHead>
                <TableHead>Interface</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {network.routes.map((route, index) => (
                <TableRow key={`${route.destination}-${index}`}>
                  <TableCell className="font-mono text-xs">
                    {route.destination}
                  </TableCell>
                  <TableCell className="font-mono text-xs">
                    {route.gateway ?? "—"}
                  </TableCell>
                  <TableCell>{route.interface_name ?? "—"}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        ) : (
          <p className="text-sm text-muted-foreground">No routes reported.</p>
        )}
      </div>
    </section>
  );
}

function FilesystemInventory({
  filesystems,
}: {
  filesystems: AgentFilesystemInventory[];
}) {
  if (!filesystems.length) {
    return (
      <InventoryEmpty
        description="Filesystem collector has not reported."
        title="No filesystems"
      />
    );
  }
  return (
    <section className="flex flex-col gap-3">
      <div>
        <h2 className="text-sm font-medium">Filesystems</h2>
        <p className="text-xs text-muted-foreground">
          Capacity and inode use at last collection.
        </p>
      </div>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Mount</TableHead>
            <TableHead>Filesystem</TableHead>
            <TableHead>Used</TableHead>
            <TableHead>Inodes</TableHead>
            <TableHead>Mode</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {filesystems.map((filesystem) => (
            <TableRow key={filesystem.mount_point}>
              <TableCell className="font-mono text-xs font-medium">
                {filesystem.mount_point}
              </TableCell>
              <TableCell>
                {filesystem.filesystem ?? filesystem.device ?? "—"}
              </TableCell>
              <TableCell>
                {formatBytes(filesystem.used_bytes)} /{" "}
                {formatBytes(filesystem.total_bytes)}
              </TableCell>
              <TableCell>
                {formatNumber(filesystem.inode_used)} /{" "}
                {formatNumber(filesystem.inode_total)}
              </TableCell>
              <TableCell>
                <Badge variant={filesystem.read_only ? "outline" : "secondary"}>
                  {filesystem.read_only ? "Read only" : "Writable"}
                </Badge>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </section>
  );
}

function ProcessInventory({
  processes,
}: {
  processes: AgentProcessInventory[];
}) {
  if (!processes.length) {
    return (
      <InventoryEmpty
        description="Process collector has not reported."
        title="No processes"
      />
    );
  }
  return (
    <section className="flex flex-col gap-3">
      <div>
        <h2 className="text-sm font-medium">Processes</h2>
        <p className="text-xs text-muted-foreground">
          Processes reported by agent.
        </p>
      </div>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>PID</TableHead>
            <TableHead>Name</TableHead>
            <TableHead>User</TableHead>
            <TableHead>State</TableHead>
            <TableHead>CPU</TableHead>
            <TableHead>Memory</TableHead>
            <TableHead>Command</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {processes.map((process) => (
            <TableRow key={process.pid}>
              <TableCell className="font-mono text-xs">{process.pid}</TableCell>
              <TableCell className="font-medium">{process.name}</TableCell>
              <TableCell>{process.user ?? "—"}</TableCell>
              <TableCell>{process.state ?? "—"}</TableCell>
              <TableCell>{formatPercent(process.cpu_percent)}</TableCell>
              <TableCell>{formatBytes(process.memory_bytes)}</TableCell>
              <TableCell className="max-w-56 truncate font-mono text-xs">
                {process.command ?? "—"}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </section>
  );
}

function SocketInventory({ sockets }: { sockets: AgentSocketInventory[] }) {
  if (!sockets.length) {
    return (
      <InventoryEmpty
        description="Socket collector has not reported."
        title="No sockets"
      />
    );
  }
  return (
    <section className="flex flex-col gap-3">
      <div>
        <h2 className="text-sm font-medium">Sockets</h2>
        <p className="text-xs text-muted-foreground">
          Local listeners and worker reachability.
        </p>
      </div>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Transport</TableHead>
            <TableHead>Local listener</TableHead>
            <TableHead>Process</TableHead>
            <TableHead>Worker reachability</TableHead>
            <TableHead>Checked</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {sockets.map((socket) => (
            <TableRow
              key={`${socket.protocol}-${socket.local_address}-${socket.local_port}`}
            >
              <TableCell>
                <div className="flex flex-col gap-0.5">
                  <span className="font-medium">
                    {socket.protocol.toUpperCase()}
                  </span>
                  <span className="font-mono text-xs text-muted-foreground">
                    {socket.state ?? "—"}
                  </span>
                </div>
              </TableCell>
              <TableCell>
                <div className="flex flex-col gap-1">
                  <Badge variant={socket.listening ? "outline" : "secondary"}>
                    {socket.listening ? "Listening" : "Not listening"}
                  </Badge>
                  <span className="font-mono text-xs text-muted-foreground">
                    {formatSocketAddress(
                      socket.local_address,
                      socket.local_port,
                    )}
                  </span>
                </div>
              </TableCell>
              <TableCell>
                <div className="flex flex-col gap-0.5">
                  <span>{socket.process_name ?? "—"}</span>
                  <span className="font-mono text-xs text-muted-foreground">
                    {socket.process_id === null
                      ? "—"
                      : `pid ${socket.process_id}`}
                  </span>
                </div>
              </TableCell>
              <TableCell>
                <div className="flex flex-col gap-1">
                  <ReachabilityBadge state={socket.reachability.state} />
                  {socket.reachability.endpoint ? (
                    <span className="font-mono text-xs text-muted-foreground">
                      {typeof socket.reachability.endpoint === "string"
                        ? socket.reachability.endpoint
                        : socket.reachability.endpoint.address &&
                            socket.reachability.endpoint.port !== null
                          ? formatSocketAddress(
                              socket.reachability.endpoint.address,
                              socket.reachability.endpoint.port,
                            )
                          : "No worker endpoint"}
                    </span>
                  ) : null}
                </div>
              </TableCell>
              <TableCell title={formatDate(socket.reachability.checked_at)}>
                {formatRelative(socket.reachability.checked_at)}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </section>
  );
}

function ContainerInventory({
  containers,
}: {
  containers: AgentContainerInventory[];
}) {
  if (!containers.length) {
    return (
      <InventoryEmpty
        description="Container collector has not reported."
        title="No containers"
      />
    );
  }
  return (
    <section className="flex flex-col gap-3">
      <div>
        <h2 className="text-sm font-medium">Containers</h2>
        <p className="text-xs text-muted-foreground">
          Runtime, image, networks, mounts, and published ports.
        </p>
      </div>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Container</TableHead>
            <TableHead>Image</TableHead>
            <TableHead>Status</TableHead>
            <TableHead>Health</TableHead>
            <TableHead>Published ports</TableHead>
            <TableHead>Networks</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {containers.map((container) => (
            <TableRow key={container.id}>
              <TableCell>
                <div className="flex flex-col gap-0.5">
                  <span className="font-medium">{container.name}</span>
                  <span className="font-mono text-xs text-muted-foreground">
                    {shortId(container.id)}
                  </span>
                </div>
              </TableCell>
              <TableCell className="max-w-40 truncate">
                {container.image ?? "—"}
              </TableCell>
              <TableCell>{container.status ?? "—"}</TableCell>
              <TableCell>{container.health ?? "—"}</TableCell>
              <TableCell className="max-w-40 truncate">
                {container.published_ports.join(", ") || "—"}
              </TableCell>
              <TableCell className="max-w-40 truncate">
                {container.networks.join(", ") || "—"}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </section>
  );
}

function InventorySummary({
  compact = false,
  summary,
}: {
  compact?: boolean;
  summary: Agent["inventory_summary"];
}) {
  const items = [
    ["Interfaces", summary.interfaces],
    ["Filesystems", summary.filesystems],
    ["Processes", summary.processes],
    ["Sockets", summary.sockets],
    ["Containers", summary.containers],
  ] as const;
  if (compact) {
    return (
      <div className="flex min-w-40 flex-wrap gap-x-2 gap-y-0.5 text-xs text-muted-foreground">
        {items.map(([label, value]) => (
          <span key={label} title={label}>
            <span className="font-medium text-foreground">{value}</span>{" "}
            {label.toLowerCase()}
          </span>
        ))}
      </div>
    );
  }
  return (
    <div className="grid gap-2 sm:grid-cols-5">
      {items.map(([label, value]) => (
        <div className="rounded-lg border p-3" key={label}>
          <p className="text-xs text-muted-foreground">{label}</p>
          <p className="mt-1 text-xl font-semibold tracking-tight">{value}</p>
        </div>
      ))}
    </div>
  );
}

function CapabilityBadges({
  capabilities,
  expanded = false,
}: {
  capabilities: string[];
  expanded?: boolean;
}) {
  if (!capabilities.length) {
    return <span className="text-sm text-muted-foreground">None reported</span>;
  }
  const shown = expanded ? capabilities : capabilities.slice(0, 3);
  const remaining = capabilities.length - shown.length;
  return (
    <div className="flex flex-wrap gap-1.5">
      {shown.map((capability) => (
        <Badge key={capability} variant="secondary">
          {labelize(capability)}
        </Badge>
      ))}
      {remaining > 0 ? <Badge variant="outline">+{remaining}</Badge> : null}
    </div>
  );
}

function AgentStatusBadge({ status }: { status: AgentStatus }) {
  if (status === "online") {
    return (
      <Badge variant="outline">
        <CheckIcon />
        Online
      </Badge>
    );
  }
  if (status === "offline") {
    return (
      <Badge variant="destructive">
        <XIcon />
        Offline
      </Badge>
    );
  }
  if (status === "stale") {
    return (
      <Badge variant="outline">
        <Clock3Icon />
        Stale
      </Badge>
    );
  }
  return (
    <Badge variant="secondary">
      <CircleHelpIcon />
      {labelize(status)}
    </Badge>
  );
}

function ReachabilityBadge({ state }: { state: AgentReachabilityState }) {
  if (state === "reachable") {
    return (
      <Badge variant="outline">
        <CheckIcon />
        Reachable
      </Badge>
    );
  }
  if (state === "unreachable") {
    return (
      <Badge variant="destructive">
        <XIcon />
        Unreachable
      </Badge>
    );
  }
  if (state === "not_checked") {
    return (
      <Badge variant="secondary">
        <CircleHelpIcon />
        Not checked
      </Badge>
    );
  }
  return (
    <Badge variant="outline">
      <Clock3Icon />
      {labelize(state)}
    </Badge>
  );
}

function InfoPair({
  label,
  mono = false,
  title,
  value,
}: {
  label: string;
  mono?: boolean;
  title?: string;
  value: string | null;
}) {
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className={cn("truncate", mono && "font-mono text-xs")} title={title}>
        {value ?? "—"}
      </dd>
    </div>
  );
}

function InventoryEmpty({
  description,
  icon = <DatabaseIcon />,
  title,
}: {
  description: string;
  icon?: ReactNode;
  title: string;
}) {
  return (
    <Empty className="min-h-40">
      <EmptyHeader>
        <EmptyMedia variant="icon">{icon}</EmptyMedia>
        <EmptyTitle>{title}</EmptyTitle>
        <EmptyDescription>{description}</EmptyDescription>
      </EmptyHeader>
    </Empty>
  );
}

function LoadError({
  error,
  retry,
  title,
}: {
  error: unknown;
  retry?: () => void;
  title: string;
}) {
  return (
    <Alert variant="destructive">
      <CircleAlertIcon />
      <AlertTitle>{title}</AlertTitle>
      <AlertDescription>
        {getUserFacingError(error, "Request failed.")}
      </AlertDescription>
      {retry ? (
        <Button onClick={retry} size="sm" variant="outline">
          Retry
        </Button>
      ) : null}
    </Alert>
  );
}

function AgentListLoading() {
  return (
    <CardContent className="flex flex-col gap-3 pt-6">
      {Array.from({ length: 5 }, (_, index) => (
        <Skeleton className="h-12 w-full" key={index} />
      ))}
    </CardContent>
  );
}

function formatBytes(value: number | null): string {
  if (value === null) return "—";
  if (value < 1024) return `${value} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let amount = value;
  let unitIndex = -1;
  while (amount >= 1024 && unitIndex < units.length - 1) {
    amount /= 1024;
    unitIndex += 1;
  }
  return `${amount.toFixed(amount >= 10 ? 0 : 1)} ${units[unitIndex]}`;
}

function formatDuration(seconds: number | null): string | null {
  if (seconds === null) return null;
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  if (days) return `${days}d ${hours}h`;
  if (hours) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

function formatNumber(value: number | null): string | null {
  return value === null ? null : new Intl.NumberFormat().format(value);
}

function formatDecimal(value: number | null): string | null {
  return value === null ? null : value.toFixed(2);
}

function formatPercent(value: number | null): string | null {
  return value === null ? null : `${value.toFixed(1)}%`;
}

function formatSocketAddress(address: string, port: number): string {
  const host =
    address.includes(":") && !address.startsWith("[")
      ? `[${address}]`
      : address;
  return `${host}:${port}`;
}
