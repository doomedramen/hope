import { useQuery } from "@tanstack/react-query";
import {
  ActivityIcon,
  CircleAlertIcon,
  CircleHelpIcon,
  Clock3Icon,
  CheckIcon,
  TriangleAlertIcon,
} from "lucide-react";
import { fetchMonitors, type Monitor } from "@/lib/api";
import { formatDate, formatRelative, labelize, shortId } from "@/lib/format";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Empty,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

export function MonitorList() {
  const monitorsQuery = useQuery({
    queryKey: ["monitors"],
    queryFn: () => fetchMonitors(),
    refetchInterval: 15_000,
  });
  const monitors = monitorsQuery.data?.items ?? [];

  return (
    <Card aria-busy={monitorsQuery.isLoading} className="overflow-hidden">
      <CardHeader className="border-b">
        <CardTitle>Monitors</CardTitle>
        <CardAction>
          <Badge variant={monitors.length ? "outline" : "secondary"}>
            {monitors.length}
          </Badge>
        </CardAction>
      </CardHeader>
      {monitorsQuery.isLoading ? (
        <MonitorListLoading />
      ) : monitorsQuery.isError ? (
        <CardContent className="pt-6">
          <Alert variant="destructive">
            <CircleAlertIcon />
            <AlertTitle>Monitors unavailable</AlertTitle>
            <AlertDescription>
              {monitorsQuery.error instanceof Error
                ? monitorsQuery.error.message
                : "The monitor list could not be loaded."}
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
              <EmptyTitle>No monitors</EmptyTitle>
            </EmptyHeader>
          </Empty>
        </CardContent>
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>State</TableHead>
              <TableHead>Target</TableHead>
              <TableHead>Check</TableHead>
              <TableHead>Last result</TableHead>
              <TableHead>Next run</TableHead>
              <TableHead className="text-right">Failures</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {monitors.map((monitor) => (
              <MonitorRow key={monitor.id} monitor={monitor} />
            ))}
          </TableBody>
        </Table>
      )}
    </Card>
  );
}

function MonitorRow({ monitor }: { monitor: Monitor }) {
  const target = monitorTarget(monitor);
  const path =
    typeof monitor.config.path === "string" ? monitor.config.path : null;

  return (
    <TableRow>
      <TableCell>
        <MonitorStateBadge state={monitor.state} />
      </TableCell>
      <TableCell className="min-w-56">
        <div className="flex min-w-0 flex-col gap-0.5">
          <span className="truncate font-medium">{target.name}</span>
          <span className="truncate text-xs text-muted-foreground">
            {target.endpoint}
            {path ? ` ${path}` : ""}
          </span>
          <span className="font-mono text-[0.7rem] text-muted-foreground">
            {shortId(monitor.service_id)} / {shortId(monitor.endpoint_id)}
          </span>
        </div>
      </TableCell>
      <TableCell>
        <div className="flex flex-col gap-0.5">
          <span>{labelize(monitor.monitor_type)}</span>
          <span className="text-xs text-muted-foreground">
            {formatInterval(monitor.interval_seconds)}
          </span>
        </div>
      </TableCell>
      <TableCell title={formatDate(monitor.last_result_at)}>
        {formatRelative(monitor.last_result_at)}
      </TableCell>
      <TableCell title={formatDate(monitor.next_run_at)}>
        {formatRelative(monitor.next_run_at)}
      </TableCell>
      <TableCell
        className={
          monitor.consecutive_failures > 0
            ? "text-right font-medium text-destructive"
            : "text-right text-muted-foreground"
        }
      >
        {monitor.consecutive_failures}
      </TableCell>
    </TableRow>
  );
}

function MonitorStateBadge({ state }: { state: string }) {
  if (state === "up") {
    return (
      <Badge variant="outline">
        <CheckIcon className="text-emerald-600 dark:text-emerald-400" />
        Up
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

function MonitorListLoading() {
  return (
    <CardContent className="space-y-3 pt-6">
      {Array.from({ length: 4 }, (_, index) => (
        <Skeleton className="h-12 w-full" key={index} />
      ))}
    </CardContent>
  );
}
