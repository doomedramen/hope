import { useQueries, useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  ActivityIcon,
  ArrowRightIcon,
  BellRingIcon,
  CircleAlertIcon,
  GitCompareArrowsIcon,
  NetworkIcon,
  RadioTowerIcon,
  ServerIcon,
  ShieldCheckIcon,
  TriangleAlertIcon,
} from "lucide-react";
import {
  Area,
  AreaChart,
  CartesianGrid,
  Line,
  LineChart,
  XAxis,
  YAxis,
} from "recharts";
import {
  fetchAgents,
  fetchChanges,
  fetchDevices,
  fetchIdentitySuggestions,
  fetchMonitorProposals,
  fetchMonitorResults,
  fetchMonitors,
  fetchNetworks,
  fetchServiceReviews,
  type ChangeEvent,
  type MonitorResult,
} from "@/lib/api";
import { formatDate, labelize, shortId } from "@/lib/format";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
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
import { Empty, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import { StatusDot, type StatusTone } from "@/components/ui/status-dot";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

const healthChartConfig = {
  healthy: { label: "Healthy", color: "var(--chart-1)" },
} as const;
const changeChartConfig = {
  changes: { label: "Changes", color: "var(--chart-2)" },
} as const;

type AttentionItem = {
  description: string;
  id: string;
  label: string;
  tone: StatusTone;
  to: "/infrastructure" | "/monitoring";
};

function OverviewDashboard() {
  const devicesQuery = useQuery({
    queryKey: ["devices"],
    queryFn: fetchDevices,
  });
  const networksQuery = useQuery({
    queryKey: ["networks"],
    queryFn: fetchNetworks,
  });
  const agentsQuery = useQuery({ queryKey: ["agents"], queryFn: fetchAgents });
  const monitorsQuery = useQuery({
    queryKey: ["monitors"],
    queryFn: () => fetchMonitors(),
  });
  const suggestionsQuery = useQuery({
    queryKey: ["identity-suggestions"],
    queryFn: fetchIdentitySuggestions,
  });
  const serviceReviewsQuery = useQuery({
    queryKey: ["service-reviews", "pending"],
    queryFn: () => fetchServiceReviews("pending"),
  });
  const monitorProposalsQuery = useQuery({
    queryKey: ["monitor-proposals", "pending"],
    queryFn: () => fetchMonitorProposals("pending"),
  });
  const changesQuery = useQuery({
    queryKey: ["changes", "overview"],
    queryFn: () => fetchChanges(),
  });
  const dashboardQueries = [
    devicesQuery,
    networksQuery,
    agentsQuery,
    monitorsQuery,
    suggestionsQuery,
    serviceReviewsQuery,
    monitorProposalsQuery,
    changesQuery,
  ];
  const monitors = monitorsQuery.data?.items ?? [];
  const resultQueries = useQueries({
    queries: monitors.slice(0, 12).map((monitor) => ({
      queryKey: ["monitor-results", monitor.id],
      queryFn: () => fetchMonitorResults(monitor.id),
      staleTime: 30_000,
    })),
  });
  const results = resultQueries.flatMap((query) => query.data?.items ?? []);
  const devices = devicesQuery.data?.items ?? [];
  const networks = networksQuery.data?.items ?? [];
  const agents = agentsQuery.data?.items ?? [];
  const suggestions = suggestionsQuery.data?.items ?? [];
  const serviceReviews = serviceReviewsQuery.data?.items ?? [];
  const monitorProposals = monitorProposalsQuery.data?.items ?? [];
  const changes = changesQuery.data?.items ?? [];
  const healthy = monitors.filter((monitor) => monitor.state === "up").length;
  const degraded = monitors.filter(
    (monitor) => monitor.state === "degraded",
  ).length;
  const down = monitors.filter((monitor) => monitor.state === "down").length;
  const pendingReviews =
    suggestions.length + serviceReviews.length + monitorProposals.length;
  const dashboardError = [...dashboardQueries, ...resultQueries].find(
    (query) => query.isError,
  )?.error;
  const loading = dashboardQueries.some((query) => query.isLoading);
  const attentionItems: AttentionItem[] = [
    ...suggestions.map((item) => ({
      id: item.id,
      label: `Candidate ${item.candidate_device_id}`,
      description: `${Math.round(item.score * 100)}% match confidence`,
      tone: "critical" as const,
      to: "/infrastructure" as const,
    })),
    ...serviceReviews.map((item) => ({
      id: item.id,
      label: item.candidate.product ?? "Unrecognized service",
      description: `${Math.round(item.confidence * 100)}% fingerprint confidence`,
      tone: "attention" as const,
      to: "/monitoring" as const,
    })),
    ...monitorProposals.map((item) => ({
      id: item.id,
      label: item.target_identity,
      description: `Suggested ${item.check_type.toUpperCase()} monitor`,
      tone: "info" as const,
      to: "/monitoring" as const,
    })),
  ];

  return (
    <div className="mx-auto flex w-full max-w-[1440px] flex-col gap-5">
      <section className="flex flex-col gap-4 md:flex-row md:items-end md:justify-between">
        <div>
          <h1 className="text-3xl font-semibold tracking-tight">
            Infrastructure overview
          </h1>
        </div>
        <p className="flex items-center gap-2 text-sm text-muted-foreground">
          <StatusDot tone="healthy" />
          Data refreshes as monitors report
        </p>
      </section>
      {dashboardError ? (
        <Alert variant="destructive">
          <CircleAlertIcon />
          <AlertTitle>Some dashboard data is unavailable</AlertTitle>
          <AlertDescription>
            {dashboardError instanceof Error
              ? dashboardError.message
              : "Refresh to try loading the missing data again."}
          </AlertDescription>
          <Button
            onClick={() => {
              void Promise.all(
                [...dashboardQueries, ...resultQueries].map((query) =>
                  query.refetch(),
                ),
              );
            }}
            size="sm"
            variant="outline"
          >
            Retry
          </Button>
        </Alert>
      ) : null}
      <section className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <MetricCard
          detail={`${networks.length} networks`}
          icon={ServerIcon}
          label="Devices"
          to="/infrastructure"
          value={devices.length}
        />
        <MetricCard
          detail={healthDetail(healthy, monitors.length)}
          icon={ShieldCheckIcon}
          label="Healthy services"
          to="/monitoring"
          value={`${healthy} / ${monitors.length}`}
        />
        <MetricCard
          detail="Identity, service, and monitor reviews"
          icon={TriangleAlertIcon}
          label="Pending review"
          to="/monitoring"
          value={pendingReviews}
        />
        <MetricCard
          detail="Recorded change events"
          icon={GitCompareArrowsIcon}
          label="Recent changes"
          to="/changes"
          value={changes.length}
        />
      </section>
      <section className="grid gap-4 xl:grid-cols-[minmax(0,1.7fr)_minmax(320px,0.78fr)]">
        <div className="grid gap-4">
          <ServiceHealthCard
            degraded={degraded}
            down={down}
            healthy={healthy}
            loading={
              monitorsQuery.isLoading ||
              resultQueries.some((query) => query.isLoading)
            }
            series={buildHealthSeries(results)}
          />
          <div className="grid gap-4 lg:grid-cols-[minmax(0,1.25fr)_minmax(280px,0.75fr)]">
            <InfrastructureSignalsCard
              loading={changesQuery.isLoading}
              series={buildChangeSeries(changes)}
            />
            <TopologyCard
              agents={agents.length}
              devices={devices.length}
              networks={networks.length}
            />
          </div>
        </div>
        <NeedsAttentionCard items={attentionItems} loading={loading} />
      </section>
      <RecentChangesCard changes={changes} loading={changesQuery.isLoading} />
    </div>
  );
}

function MetricCard({
  detail,
  icon: Icon,
  label,
  to,
  value,
}: {
  detail: string;
  icon: typeof ServerIcon;
  label: string;
  to: "/infrastructure" | "/monitoring" | "/changes";
  value: number | string;
}) {
  return (
    <Link
      className="group rounded-xl outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
      to={to}
    >
      <Card className="h-full gap-3 transition-colors group-hover:bg-muted/40">
        <CardContent className="flex items-start gap-3">
          <span className="grid size-10 place-items-center rounded-xl bg-accent text-accent-foreground">
            <Icon className="size-5" />
          </span>
          <div className="min-w-0 flex-1">
            <p className="text-sm font-medium text-muted-foreground">{label}</p>
            <p className="mt-1 text-2xl font-semibold tracking-tight tabular-nums">
              {value}
            </p>
            <p className="mt-1 truncate text-xs text-muted-foreground">
              {detail}
            </p>
          </div>
          <ArrowRightIcon
            aria-hidden="true"
            className="mt-1 size-4 text-muted-foreground transition-transform group-hover:translate-x-0.5"
          />
        </CardContent>
      </Card>
    </Link>
  );
}

function ServiceHealthCard({
  degraded,
  down,
  healthy,
  loading,
  series,
}: {
  degraded: number;
  down: number;
  healthy: number;
  loading: boolean;
  series: Array<{ health: number; time: string }>;
}) {
  const total = healthy + degraded + down;
  const percentage = total ? Math.round((healthy / total) * 100) : 0;
  return (
    <Card className="gap-4">
      <CardHeader>
        <div className="flex items-center gap-2">
          <ActivityIcon className="size-5 text-primary" />
          <div>
            <CardTitle render={<h2 />}>Service health</CardTitle>
            <CardDescription>
              Availability from reported monitor results.
            </CardDescription>
          </div>
        </div>
        <CardAction>
          <Badge variant="healthy">Live status</Badge>
        </CardAction>
      </CardHeader>
      <CardContent className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_150px]">
        {loading ? (
          <Skeleton className="h-48 w-full" />
        ) : series.length ? (
          <ChartContainer className="h-48 w-full" config={healthChartConfig}>
            <AreaChart
              accessibilityLayer
              data={series}
              margin={{ left: -22, right: 8, top: 8 }}
            >
              <defs>
                <linearGradient id="health-fill" x1="0" x2="0" y1="0" y2="1">
                  <stop
                    offset="5%"
                    stopColor="var(--color-healthy)"
                    stopOpacity={0.28}
                  />
                  <stop
                    offset="95%"
                    stopColor="var(--color-healthy)"
                    stopOpacity={0.02}
                  />
                </linearGradient>
              </defs>
              <CartesianGrid vertical={false} />
              <XAxis
                axisLine={false}
                dataKey="time"
                tickLine={false}
                tickMargin={8}
                minTickGap={24}
              />
              <YAxis
                axisLine={false}
                domain={[0, 100]}
                tickFormatter={(value) => `${value}%`}
                tickLine={false}
                width={34}
              />
              <ChartTooltip
                content={<ChartTooltipContent indicator="line" />}
                cursor={false}
              />
              <Area
                dataKey="health"
                fill="url(#health-fill)"
                stroke="var(--color-healthy)"
                strokeWidth={2}
                type="monotone"
              />
            </AreaChart>
          </ChartContainer>
        ) : (
          <EmptyChart label="No monitor history yet." />
        )}
        <div className="border-t pt-4 lg:border-t-0 lg:border-l lg:pt-0 lg:pl-5">
          <p className="text-3xl font-semibold tracking-tight tabular-nums">
            {percentage}%
          </p>
          <p className="text-sm text-muted-foreground">Healthy now</p>
          <div className="mt-5 grid gap-2 text-sm">
            <HealthCount count={healthy} label="healthy" tone="healthy" />
            <HealthCount count={degraded} label="degraded" tone="attention" />
            <HealthCount count={down} label="down" tone="critical" />
          </div>
        </div>
      </CardContent>
    </Card>
  );
}

function InfrastructureSignalsCard({
  loading,
  series,
}: {
  loading: boolean;
  series: Array<{ changes: number; time: string }>;
}) {
  return (
    <Card className="gap-4">
      <CardHeader>
        <div className="flex items-center gap-2">
          <ActivityIcon className="size-5 text-primary" />
          <div>
            <CardTitle>Infrastructure signals</CardTitle>
            <CardDescription>
              Recent recorded change activity across your estate.
            </CardDescription>
          </div>
        </div>
      </CardHeader>
      <CardContent>
        {loading ? (
          <Skeleton className="h-40 w-full" />
        ) : series.length ? (
          <ChartContainer className="h-40 w-full" config={changeChartConfig}>
            <LineChart
              accessibilityLayer
              data={series}
              margin={{ left: -22, right: 8, top: 8 }}
            >
              <CartesianGrid vertical={false} />
              <XAxis
                axisLine={false}
                dataKey="time"
                tickLine={false}
                tickMargin={8}
                minTickGap={24}
              />
              <YAxis
                allowDecimals={false}
                axisLine={false}
                tickLine={false}
                width={24}
              />
              <ChartTooltip
                content={<ChartTooltipContent indicator="line" />}
                cursor={false}
              />
              <Line
                dataKey="changes"
                dot={false}
                stroke="var(--color-changes)"
                strokeWidth={2}
                type="monotone"
              />
            </LineChart>
          </ChartContainer>
        ) : (
          <EmptyChart label="No changes in this range." />
        )}
      </CardContent>
    </Card>
  );
}

function TopologyCard({
  agents,
  devices,
  networks,
}: {
  agents: number;
  devices: number;
  networks: number;
}) {
  return (
    <Card className="gap-4">
      <CardHeader>
        <div className="flex items-center gap-2">
          <NetworkIcon className="size-5 text-primary" />
          <div>
            <CardTitle>Infrastructure topology</CardTitle>
            <CardDescription>Current estate footprint.</CardDescription>
          </div>
        </div>
        <CardAction>
          <Link
            className="rounded-sm text-sm font-medium text-primary outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
            to="/infrastructure"
          >
            View map
          </Link>
        </CardAction>
      </CardHeader>
      <CardContent className="grid gap-4">
        <TopologyNode icon={NetworkIcon} label="Networks" value={networks} />
        <span className="ml-5 h-3 border-l border-dashed border-border" />
        <TopologyNode icon={ServerIcon} label="Devices" value={devices} />
        <span className="ml-5 h-3 border-l border-dashed border-border" />
        <TopologyNode icon={RadioTowerIcon} label="Agents" value={agents} />
      </CardContent>
    </Card>
  );
}

function NeedsAttentionCard({
  items,
  loading,
}: {
  items: AttentionItem[];
  loading: boolean;
}) {
  return (
    <Card className="gap-4">
      <CardHeader>
        <div className="flex items-center gap-2">
          <BellRingIcon className="size-5 text-status-critical-fg" />
          <CardTitle render={<h2 />}>Needs attention</CardTitle>
        </div>
        <CardAction>
          <Link
            className="rounded-sm text-sm font-medium text-primary outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
            to="/monitoring"
          >
            View all
          </Link>
        </CardAction>
      </CardHeader>
      <CardContent>
        {loading ? (
          <LoadingRows count={5} />
        ) : items.length ? (
          <div className="divide-y">
            <p className="pb-2 text-sm font-medium">
              Review queue{" "}
              <Badge className="ml-1" variant="info">
                {items.length}
              </Badge>
            </p>
            {items.slice(0, 6).map((item) => (
              <div className="flex items-center gap-3 py-3" key={item.id}>
                <StatusDot tone={item.tone} />
                <div className="min-w-0 flex-1">
                  <p className="truncate text-sm font-medium">{item.label}</p>
                  <p className="truncate text-xs text-muted-foreground">
                    {item.description}
                  </p>
                </div>
                <Link
                  className={buttonVariants({ variant: "outline", size: "sm" })}
                  to={item.to}
                >
                  Review
                </Link>
              </div>
            ))}
          </div>
        ) : (
          <EmptyState label="Nothing needs review right now." />
        )}
      </CardContent>
    </Card>
  );
}

function RecentChangesCard({
  changes,
  loading,
}: {
  changes: ChangeEvent[];
  loading: boolean;
}) {
  return (
    <Card className="gap-4">
      <CardHeader>
        <div className="flex items-center gap-2">
          <GitCompareArrowsIcon className="size-5 text-primary" />
          <div>
            <CardTitle render={<h2 />}>Recent changes</CardTitle>
            <CardDescription>
              Latest observed infrastructure changes and updates.
            </CardDescription>
          </div>
        </div>
        <CardAction>
          <Link
            className="rounded-sm text-sm font-medium text-primary outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
            to="/changes"
          >
            View all changes
          </Link>
        </CardAction>
      </CardHeader>
      <CardContent>
        {loading ? (
          <LoadingRows count={4} />
        ) : changes.length ? (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Time</TableHead>
                <TableHead>Device</TableHead>
                <TableHead>Event</TableHead>
                <TableHead className="hidden md:table-cell">Source</TableHead>
                <TableHead>Status</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {changes.slice(0, 6).map((change) => (
                <TableRow key={change.id}>
                  <TableCell className="text-xs text-muted-foreground">
                    {formatDate(change.occurred_at)}
                  </TableCell>
                  <TableCell className="font-mono text-xs">
                    {shortId(change.entity_id)}
                  </TableCell>
                  <TableCell>
                    <span className="flex items-center gap-2">
                      <StatusDot tone={changeTone(change.severity)} />
                      {labelize(change.category)}
                    </span>
                  </TableCell>
                  <TableCell className="hidden text-muted-foreground md:table-cell">
                    {change.evidence_source
                      ? labelize(change.evidence_source)
                      : "—"}
                  </TableCell>
                  <TableCell>
                    <Badge variant={badgeTone(change.severity)}>
                      {labelize(change.severity)}
                    </Badge>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        ) : (
          <EmptyState label="No change events yet." />
        )}
      </CardContent>
    </Card>
  );
}

function TopologyNode({
  icon: Icon,
  label,
  value,
}: {
  icon: typeof ServerIcon;
  label: string;
  value: number;
}) {
  return (
    <div className="flex items-center gap-3">
      <span className="grid size-10 place-items-center rounded-full bg-status-info-bg text-status-info-fg">
        <Icon className="size-4" />
      </span>
      <div>
        <p className="text-sm font-medium">{label}</p>
        <p className="text-xs text-muted-foreground">
          {value} {value === 1 ? "record" : "records"}
        </p>
      </div>
    </div>
  );
}
function HealthCount({
  count,
  label,
  tone,
}: {
  count: number;
  label: string;
  tone: StatusTone;
}) {
  return (
    <p className="flex items-center gap-2">
      <StatusDot tone={tone} />
      <span className="font-medium tabular-nums">{count}</span>
      <span className="text-muted-foreground">{label}</span>
    </p>
  );
}
function EmptyChart({ label }: { label: string }) {
  return (
    <Empty className="h-48 min-h-0 rounded-lg p-4">
      <EmptyHeader>
        <EmptyTitle>{label}</EmptyTitle>
      </EmptyHeader>
    </Empty>
  );
}
function EmptyState({ label }: { label: string }) {
  return (
    <Empty className="min-h-32 rounded-lg p-4">
      <EmptyHeader>
        <EmptyTitle>{label}</EmptyTitle>
      </EmptyHeader>
    </Empty>
  );
}
function LoadingRows({ count }: { count: number }) {
  return (
    <div
      aria-label="Loading dashboard data"
      className="grid gap-3"
      role="status"
    >
      {Array.from({ length: count }, (_, index) => (
        <Skeleton className="h-10 w-full" key={index} />
      ))}
    </div>
  );
}
function healthDetail(healthy: number, total: number) {
  return total
    ? `${Math.round((healthy / total) * 100)}% healthy now`
    : "No monitors configured";
}
function changeTone(severity: ChangeEvent["severity"]): StatusTone {
  if (severity === "critical") return "critical";
  if (severity === "warning") return "attention";
  if (severity === "info" || severity === "notice") return "info";
  return "neutral";
}
function badgeTone(
  severity: ChangeEvent["severity"],
): "critical" | "attention" | "info" | "neutral" {
  const tone = changeTone(severity);
  return tone === "critical" || tone === "attention" || tone === "info"
    ? tone
    : "neutral";
}
function buildHealthSeries(results: MonitorResult[]) {
  const buckets = new Map<
    string,
    { total: number; successful: number; timestamp: number }
  >();
  results.forEach((result) => {
    const date = new Date(result.observed_at);
    if (Number.isNaN(date.getTime())) return;
    const timestamp = Math.floor(date.getTime() / 3_600_000) * 3_600_000;
    const key = String(timestamp);
    const bucket = buckets.get(key) ?? { total: 0, successful: 0, timestamp };
    bucket.total += 1;
    bucket.successful += result.status === "success" ? 1 : 0;
    buckets.set(key, bucket);
  });
  return [...buckets.values()]
    .sort((a, b) => a.timestamp - b.timestamp)
    .map((bucket) => ({
      time: new Intl.DateTimeFormat(undefined, {
        hour: "2-digit",
        minute: "2-digit",
      }).format(bucket.timestamp),
      health: Math.round((bucket.successful / bucket.total) * 100),
    }));
}
function buildChangeSeries(changes: ChangeEvent[]) {
  const buckets = new Map<string, { count: number; timestamp: number }>();
  changes.forEach((change) => {
    const date = new Date(change.occurred_at);
    if (Number.isNaN(date.getTime())) return;
    const timestamp = Math.floor(date.getTime() / 3_600_000) * 3_600_000;
    const key = String(timestamp);
    const bucket = buckets.get(key) ?? { count: 0, timestamp };
    bucket.count += 1;
    buckets.set(key, bucket);
  });
  return [...buckets.values()]
    .sort((a, b) => a.timestamp - b.timestamp)
    .map((bucket) => ({
      time: new Intl.DateTimeFormat(undefined, {
        hour: "2-digit",
        minute: "2-digit",
      }).format(bucket.timestamp),
      changes: bucket.count,
    }));
}

export { OverviewDashboard };
