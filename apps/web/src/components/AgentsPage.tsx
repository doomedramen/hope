import { useMutation, useQuery } from "@tanstack/react-query";
import {
  ActivityIcon,
  CheckIcon,
  CircleAlertIcon,
  CircleHelpIcon,
  Clock3Icon,
  CopyIcon,
  ContainerIcon,
  CpuIcon,
  DatabaseIcon,
  DownloadIcon,
  FileTextIcon,
  HardDriveIcon,
  NetworkIcon,
  RefreshCwIcon,
  SearchIcon,
  ServerIcon,
  ShieldCheckIcon,
  XIcon,
} from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  createAgentEnrollment,
  fetchAgent,
  fetchAgents,
  getUserFacingError,
  type Agent,
  type AgentContainerInventory,
  type AgentDetail,
  type AgentFilesystemInventory,
  type AgentHostInventory,
  type AgentInventorySummary,
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
import { agentConnectionAddress, saveAgentConnectionAddress } from "@/lib/agentConnection";

const EMPTY_AGENTS: Agent[] = [];
const EMPTY_INVENTORY_SUMMARY: AgentInventorySummary = {
  interfaces: 0,
  filesystems: 0,
  processes: 0,
  sockets: 0,
  containers: 0,
};

export function AgentsPage() {
  const [requestedAgentId, setRequestedAgentId] = useState<string | null>(null);
  const [enrollmentOpen, setEnrollmentOpen] = useState(false);
  const [search, setSearch] = useState("");
  const agentsQuery = useQuery({
    queryKey: ["agents"],
    queryFn: fetchAgents,
    refetchInterval: 15_000,
  });
  const agents = agentsQuery.data?.items ?? EMPTY_AGENTS;
  const isEmptyState =
    !agentsQuery.isLoading && !agentsQuery.isError && agents.length === 0;
  const selectedAgentId = agents.some((agent) => agent.id === requestedAgentId)
    ? requestedAgentId
    : (agents[0]?.id ?? null);
  const selectedAgent =
    agents.find((agent) => agent.id === selectedAgentId) ?? null;
  const detailQuery = useQuery({
    queryKey: ["agent", selectedAgentId],
    queryFn: () => fetchAgent(selectedAgentId!),
    enabled: Boolean(selectedAgentId),
    refetchInterval: 15_000,
  });

  const visibleAgents = useMemo(() => {
    const needle = search.trim().toLowerCase();
    if (!needle) return agents;
    return agents.filter((agent) =>
      [
        agent.hostname,
        agent.id,
        agent.agent_version,
        agent.os,
        agent.arch,
        ...agent.capabilities,
      ].some((value) => value?.toLowerCase().includes(needle)),
    );
  }, [agents, search]);

  const onlineCount = agents.filter(
    (agent) => agent.status === "online",
  ).length;
  const staleCount = agents.filter((agent) => agent.status === "stale").length;
  const offlineCount = agents.filter(
    (agent) => agent.status === "offline",
  ).length;

  return (
    <div className="mx-auto flex max-w-7xl flex-col gap-6">
      <div className="flex flex-col justify-between gap-4 md:flex-row md:items-end">
        <div>
          <h1 className="text-3xl font-semibold tracking-tight">Agents</h1>
        </div>
        <div className="flex items-center gap-2">
          <Button
            onClick={() => {
              void agentsQuery.refetch();
              if (selectedAgentId) void detailQuery.refetch();
            }}
            size="sm"
            variant="outline"
          >
            <RefreshCwIcon data-icon="inline-start" />
            Refresh
          </Button>
          {!isEmptyState ? (
            <Button onClick={() => setEnrollmentOpen(true)} size="sm">
              <DownloadIcon data-icon="inline-start" />
              Enroll agent
            </Button>
          ) : null}
        </div>
      </div>

      {!isEmptyState ? (
        <div className="order-2 grid gap-4 sm:order-none sm:grid-cols-2 xl:grid-cols-4">
          <MetricCard
            detail="Enrolled agents"
            icon={<ServerIcon />}
            label="Enrolled"
            value={agents.length}
          />
          <MetricCard
            detail="Heartbeat within threshold"
            icon={<ActivityIcon />}
            label="Online"
            value={onlineCount}
          />
          <MetricCard
            detail="Heartbeat is delayed"
            icon={<Clock3Icon />}
            label="Stale"
            value={staleCount}
          />
          <MetricCard
            detail="No current connection"
            icon={<CircleAlertIcon />}
            label="Offline"
            value={offlineCount}
          />
        </div>
      ) : null}

      <div className="order-1 grid gap-6 sm:order-none xl:grid-cols-[minmax(0,1fr)_minmax(22rem,0.8fr)]">
        <Card aria-busy={agentsQuery.isLoading} className="min-w-0">
          <CardHeader className="border-b max-sm:grid-cols-1">
            <CardTitle>Enrolled agents</CardTitle>
            <CardDescription>
              {isEmptyState
                ? "Enroll your first Linux agent to collect host inventory."
                : `${visibleAgents.length} shown of ${agents.length}`}
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
          ) : agentsQuery.isError ? (
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
            />
          )}
        </Card>

        <AgentDetailPanel
          detail={detailQuery.data}
          error={detailQuery.error}
          loading={detailQuery.isLoading}
          onRetry={() => detailQuery.refetch()}
          selectedAgent={selectedAgent}
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
  const scriptOrigin = new URL(addressError ? agentConnectionAddress() : baseUrl);
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

  const shellQuote = (value: string) =>
    "'" + value.replaceAll("'", "'\\''") + "'";
  const enrollment = enrollmentMutation.data;
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
        <div className="grid gap-2">
          <label className="text-sm font-medium" htmlFor="agent-connection-address">Agent connection address</label>
          <Input id="agent-connection-address" value={baseUrl} onChange={(event) => {
            setBaseUrl(event.target.value);
            try { saveAgentConnectionAddress(event.target.value); setAddressError(null); }
            catch { setAddressError("Enter the HTTP LAN address or HTTPS proxy address agents can reach."); }
          }} />
          <p className="text-xs text-muted-foreground">Use the address reachable from your devices. An HTTP LAN address uses Hope’s pinned HTTPS on port 443.</p>
          {addressError ? <p className="text-xs text-destructive">{addressError}</p> : null}
        </div>
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
        ) : installCommand && enrollment && !addressError ? (
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
          {enrollmentMutation.error ? (
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

function MetricCard({
  detail,
  icon,
  label,
  value,
}: {
  detail: string;
  icon: ReactNode;
  label: string;
  value: number;
}) {
  return (
    <Card size="sm">
      <CardContent className="flex items-center gap-3">
        <span className="grid size-9 place-items-center rounded-lg bg-muted text-muted-foreground">
          {icon}
        </span>
        <div>
          <p className="text-xs text-muted-foreground">{label}</p>
          <p className="text-2xl font-semibold tracking-tight">{value}</p>
          <p className="text-xs text-muted-foreground">{detail}</p>
        </div>
      </CardContent>
    </Card>
  );
}

function AgentTable({
  agents,
  selectedAgentId,
  selectAgent,
}: {
  agents: Agent[];
  selectedAgentId: string | null;
  selectAgent: (id: string) => void;
}) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Agent</TableHead>
          <TableHead>State</TableHead>
          <TableHead>Version</TableHead>
          <TableHead>Platform</TableHead>
          <TableHead>Capabilities</TableHead>
          <TableHead>Last seen</TableHead>
          <TableHead>Inventory</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {agents.map((agent) => (
          <TableRow
            aria-selected={agent.id === selectedAgentId}
            data-state={agent.id === selectedAgentId ? "selected" : undefined}
            key={agent.id}
          >
            <TableCell className="min-w-48">
              <button
                aria-label={`Select ${agent.hostname || "agent"}`}
                aria-pressed={agent.id === selectedAgentId}
                className="flex min-w-0 w-full items-center gap-2 rounded-md text-left outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
                onClick={() => selectAgent(agent.id)}
                type="button"
              >
                <span className="grid size-7 shrink-0 place-items-center rounded-md bg-muted">
                  <ServerIcon aria-hidden="true" className="size-3.5" />
                </span>
                <div className="min-w-0">
                  <p className="truncate font-medium">
                    {agent.hostname || "Unnamed agent"}
                  </p>
                  <p className="truncate font-mono text-xs text-muted-foreground">
                    {shortId(agent.id)}
                  </p>
                </div>
              </button>
            </TableCell>
            <TableCell>
              <AgentStatusBadge status={agent.status} />
            </TableCell>
            <TableCell className="font-mono text-xs">
              {agent.agent_version ?? "—"}
            </TableCell>
            <TableCell>
              <div className="flex flex-col gap-0.5">
                <span>{agent.os ?? "—"}</span>
                <span className="text-xs text-muted-foreground">
                  {agent.arch ?? "—"}
                </span>
              </div>
            </TableCell>
            <TableCell className="min-w-48">
              <CapabilityBadges capabilities={agent.capabilities} />
            </TableCell>
            <TableCell title={formatDate(agent.last_seen)}>
              {formatRelative(agent.last_seen)}
            </TableCell>
            <TableCell>
              <InventorySummary
                summary={agent.inventory_summary ?? EMPTY_INVENTORY_SUMMARY}
                compact
              />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

function AgentDetailPanel({
  detail,
  error,
  loading,
  onRetry,
  selectedAgent,
}: {
  detail: AgentDetail | undefined;
  error: unknown;
  loading: boolean;
  onRetry: () => void;
  selectedAgent: Agent | null;
}) {
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

  if (error) {
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
          <CardTitle className="truncate">
            {detail.hostname || "Unnamed agent"}
          </CardTitle>
          <CardDescription className="flex flex-wrap gap-x-2 gap-y-1">
            <span className="font-mono">{shortId(detail.id)}</span>
            <span>{detail.os ?? "unknown OS"}</span>
            <span>{detail.arch ?? "unknown arch"}</span>
          </CardDescription>
        </div>
        <CardAction>
          <AgentStatusBadge status={detail.status} />
        </CardAction>
      </CardHeader>
      <CardContent className="flex min-w-0 flex-col gap-5">
        <div className="grid gap-3 sm:grid-cols-2">
          <InfoPair label="Version" value={detail.agent_version} mono />
          <InfoPair
            label="Last seen"
            value={formatRelative(detail.last_seen)}
            title={formatDate(detail.last_seen)}
          />
          <InfoPair
            label="Device"
            value={detail.device_id ? shortId(detail.device_id) : "Unlinked"}
            mono={Boolean(detail.device_id)}
          />
          <InfoPair
            label="Protocol"
            value={
              detail.protocol_version === null
                ? null
                : `v${detail.protocol_version}`
            }
            mono
          />
        </div>

        <Tabs className="min-w-0" defaultValue="overview">
          <TabsList
            aria-label="Agent inventory views"
            className="w-full flex-wrap justify-start"
            variant="line"
          >
            <TabsTrigger value="overview">
              <ShieldCheckIcon data-icon="inline-start" />
              Evidence
            </TabsTrigger>
            <TabsTrigger value="host">
              <ServerIcon data-icon="inline-start" />
              Host
            </TabsTrigger>
            <TabsTrigger value="network">
              <NetworkIcon data-icon="inline-start" />
              Network
            </TabsTrigger>
            <TabsTrigger value="filesystems">
              <HardDriveIcon data-icon="inline-start" />
              Filesystems
            </TabsTrigger>
            <TabsTrigger value="processes">
              <CpuIcon data-icon="inline-start" />
              Processes
            </TabsTrigger>
            <TabsTrigger value="sockets">
              <ActivityIcon data-icon="inline-start" />
              Sockets
            </TabsTrigger>
            <TabsTrigger value="containers">
              <ContainerIcon data-icon="inline-start" />
              Containers
            </TabsTrigger>
          </TabsList>

          <TabsContent className="flex flex-col gap-5 pt-4" value="overview">
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
                <Badge variant="secondary">{detail.capabilities.length}</Badge>
              </div>
              <CapabilityBadges capabilities={detail.capabilities} expanded />
            </section>
            <ReachabilityExplanation />
            <EvidenceList evidence={detail.evidence} />
          </TabsContent>

          <TabsContent className="pt-4" value="host">
            <HostInventory host={detail.host} />
          </TabsContent>
          <TabsContent className="pt-4" value="network">
            <NetworkInventory network={detail.network} />
          </TabsContent>
          <TabsContent className="pt-4" value="filesystems">
            <FilesystemInventory filesystems={detail.filesystems} />
          </TabsContent>
          <TabsContent className="pt-4" value="processes">
            <ProcessInventory processes={detail.processes} />
          </TabsContent>
          <TabsContent className="pt-4" value="sockets">
            <SocketInventory sockets={detail.sockets} />
          </TabsContent>
          <TabsContent className="pt-4" value="containers">
            <ContainerInventory containers={detail.containers} />
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
                      {socket.reachability.endpoint}
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
