import { useQuery } from "@tanstack/react-query";
import { CheckIcon, CircleAlertIcon, TriangleAlertIcon } from "lucide-react";
import { useState } from "react";
import { fetchIncidents, type Incident } from "@/lib/api";
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
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

type IncidentFilter = "open" | "recovered" | "all";

export function IncidentList() {
  const [filter, setFilter] = useState<IncidentFilter>("open");
  const state = filter === "all" ? undefined : filter;
  const incidentsQuery = useQuery({
    queryKey: ["incidents", state],
    queryFn: () => fetchIncidents(state),
    refetchInterval: 15_000,
  });
  const incidents = incidentsQuery.data?.items ?? [];

  return (
    <Card aria-busy={incidentsQuery.isLoading} className="overflow-hidden">
      <CardHeader className="border-b">
        <CardTitle>Incidents</CardTitle>
        <CardAction>
          <ToggleGroup
            aria-label="Filter incidents by state"
            onValueChange={(values) => {
              const next = values[0] as IncidentFilter | undefined;
              if (next) setFilter(next);
            }}
            size="sm"
            value={[filter]}
            variant="outline"
          >
            <ToggleGroupItem value="open">Open</ToggleGroupItem>
            <ToggleGroupItem value="recovered">Recovered</ToggleGroupItem>
            <ToggleGroupItem value="all">All</ToggleGroupItem>
          </ToggleGroup>
        </CardAction>
      </CardHeader>
      {incidentsQuery.isLoading ? (
        <IncidentListLoading />
      ) : incidentsQuery.isError ? (
        <CardContent className="pt-6">
          <Alert variant="destructive">
            <CircleAlertIcon />
            <AlertTitle>Incidents unavailable</AlertTitle>
            <AlertDescription>
              {incidentsQuery.error instanceof Error
                ? incidentsQuery.error.message
                : "The incident list could not be loaded."}
            </AlertDescription>
            <Button
              onClick={() => incidentsQuery.refetch()}
              size="sm"
              variant="outline"
            >
              Retry
            </Button>
          </Alert>
        </CardContent>
      ) : incidents.length === 0 ? (
        <CardContent className="pt-6">
          <Empty>
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <CheckIcon />
              </EmptyMedia>
              <EmptyTitle>No incidents</EmptyTitle>
            </EmptyHeader>
          </Empty>
        </CardContent>
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>State</TableHead>
              <TableHead>Target</TableHead>
              <TableHead>Severity</TableHead>
              <TableHead>Opened</TableHead>
              <TableHead>Last event</TableHead>
              <TableHead className="text-right">Failures</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {incidents.map((incident) => (
              <IncidentRow incident={incident} key={incident.id} />
            ))}
          </TableBody>
        </Table>
      )}
    </Card>
  );
}

function IncidentRow({ incident }: { incident: Incident }) {
  const name =
    incident.service_name ??
    incident.service_product ??
    "Service " + shortId(incident.service_id);
  const endpoint =
    incident.endpoint_url ??
    incident.endpoint_dns_name ??
    formatSocketTarget(incident.endpoint_address, incident.endpoint_port) ??
    "Endpoint " + shortId(incident.endpoint_id);

  return (
    <TableRow>
      <TableCell>
        {incident.state === "open" ? (
          <Badge variant="destructive">
            <CircleAlertIcon />
            Open
          </Badge>
        ) : (
          <Badge variant="outline">
            <CheckIcon className="text-emerald-600 dark:text-emerald-400" />
            Recovered
          </Badge>
        )}
      </TableCell>
      <TableCell className="min-w-56">
        <div className="flex min-w-0 flex-col gap-0.5">
          <span className="truncate font-medium">{name}</span>
          <span className="truncate text-xs text-muted-foreground">
            {endpoint}
          </span>
          <span className="truncate text-xs text-muted-foreground">
            {incident.summary ?? labelize(incident.monitor_type)}
          </span>
        </div>
      </TableCell>
      <TableCell>
        {incident.severity === "critical" ? (
          <Badge variant="destructive">Critical</Badge>
        ) : (
          <Badge variant="outline">
            <TriangleAlertIcon className="text-amber-600 dark:text-amber-400" />
            {labelize(incident.severity)}
          </Badge>
        )}
      </TableCell>
      <TableCell title={formatDate(incident.opened_at)}>
        {formatRelative(incident.opened_at)}
      </TableCell>
      <TableCell title={formatDate(incident.last_event_at)}>
        {formatRelative(incident.last_event_at)}
      </TableCell>
      <TableCell className="text-right font-medium">
        {incident.failure_count}
      </TableCell>
    </TableRow>
  );
}

function formatSocketTarget(address: string | null, port: number | null) {
  if (!address) return null;
  const hostAddress = address.replace(/\/\d+$/, "");
  const host = hostAddress.includes(":")
    ? "[" + hostAddress + "]"
    : hostAddress;
  return port !== null ? host + ":" + port : host;
}

function IncidentListLoading() {
  return (
    <CardContent className="space-y-3 pt-6">
      {Array.from({ length: 4 }, (_, index) => (
        <Skeleton className="h-12 w-full" key={index} />
      ))}
    </CardContent>
  );
}
