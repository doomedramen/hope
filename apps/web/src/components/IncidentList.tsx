import { useQuery } from "@tanstack/react-query";
import { CheckIcon, CircleAlertIcon, TriangleAlertIcon } from "lucide-react";
import { useState } from "react";
import { fetchIncidents, getUserFacingError, type Incident } from "@/lib/api";
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

type IncidentFilter = "open" | "recovered" | "all";

export function IncidentList() {
  const [filter, setFilter] = useState<IncidentFilter>("open");
  const [selectedIncident, setSelectedIncident] = useState<Incident | null>(
    null,
  );
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
              {getUserFacingError(
                incidentsQuery.error,
                "The incident list could not be loaded.",
              )}
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
        <>
          <div className="hidden overflow-x-auto md:block">
            <Table className="min-w-[58rem]">
              <TableHeader>
                <TableRow>
                  <TableHead>State</TableHead>
                  <TableHead>Target</TableHead>
                  <TableHead>Severity</TableHead>
                  <TableHead>Opened</TableHead>
                  <TableHead>Last event</TableHead>
                  <TableHead className="text-right">Failures</TableHead>
                  <TableHead className="text-right">Action</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {incidents.map((incident) => (
                  <IncidentRow
                    incident={incident}
                    key={incident.id}
                    onReview={() => setSelectedIncident(incident)}
                  />
                ))}
              </TableBody>
            </Table>
          </div>
          <div className="space-y-3 p-4 md:hidden">
            {incidents.map((incident) => (
              <IncidentMobileCard
                incident={incident}
                key={incident.id}
                onReview={() => setSelectedIncident(incident)}
              />
            ))}
          </div>
        </>
      )}
      <Dialog
        onOpenChange={(open) => {
          if (!open) setSelectedIncident(null);
        }}
        open={selectedIncident !== null}
      >
        {selectedIncident ? (
          <IncidentDetails incident={selectedIncident} />
        ) : null}
      </Dialog>
    </Card>
  );
}

function IncidentRow({
  incident,
  onReview,
}: {
  incident: Incident;
  onReview: () => void;
}) {
  const { name, endpoint } = incidentTarget(incident);

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
      <TableCell className="text-right">
        <Button onClick={onReview} size="sm" variant="outline">
          Review
        </Button>
      </TableCell>
    </TableRow>
  );
}

function IncidentMobileCard({
  incident,
  onReview,
}: {
  incident: Incident;
  onReview: () => void;
}) {
  const { name, endpoint } = incidentTarget(incident);

  return (
    <article className="rounded-lg border p-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="mb-1 flex flex-wrap items-center gap-2">
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
            {incident.severity === "critical" ? (
              <Badge variant="destructive">Critical</Badge>
            ) : (
              <Badge variant="outline">
                <TriangleAlertIcon className="text-amber-600 dark:text-amber-400" />
                {labelize(incident.severity)}
              </Badge>
            )}
          </div>
          <h3 className="truncate font-medium">{name}</h3>
          <p className="break-words text-sm text-muted-foreground">
            {endpoint}
          </p>
          <p className="mt-1 text-sm text-muted-foreground">
            {incident.summary ?? labelize(incident.monitor_type)}
          </p>
        </div>
        <Button onClick={onReview} size="sm" variant="outline">
          Review
        </Button>
      </div>
      <dl className="mt-3 grid grid-cols-2 gap-x-4 gap-y-2 border-t pt-3 text-sm">
        <Detail label="Opened" value={formatRelative(incident.opened_at)} />
        <Detail
          label="Last event"
          value={formatRelative(incident.last_event_at)}
        />
        <Detail label="Failures" value={String(incident.failure_count)} />
        <Detail label="Monitor" mono value={shortId(incident.monitor_id)} />
      </dl>
    </article>
  );
}

function IncidentDetails({ incident }: { incident: Incident }) {
  const { name, endpoint } = incidentTarget(incident);

  return (
    <DialogContent className="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>Incident details</DialogTitle>
        <DialogDescription>
          {name} · {endpoint}
        </DialogDescription>
      </DialogHeader>
      <dl className="grid gap-3 rounded-lg border p-4 sm:grid-cols-2">
        <Detail label="State" value={labelize(incident.state)} />
        <Detail label="Severity" value={labelize(incident.severity)} />
        <Detail label="Summary" value={incident.summary ?? "No summary"} />
        <Detail label="Failures" value={String(incident.failure_count)} />
        <Detail label="Opened" value={formatDate(incident.opened_at)} />
        <Detail label="Last event" value={formatDate(incident.last_event_at)} />
        <Detail label="Incident ID" mono value={shortId(incident.id)} />
        <Detail label="Monitor" mono value={shortId(incident.monitor_id)} />
      </dl>
      <DialogFooter showCloseButton />
    </DialogContent>
  );
}

function incidentTarget(incident: Incident) {
  const agentMonitor = incident.monitor_type.startsWith("agent_");
  const name =
    incident.service_name ??
    incident.service_product ??
    (incident.service_id
      ? `Service ${shortId(incident.service_id)}`
      : agentMonitor
        ? "Agent monitor"
        : "Service unavailable");
  const endpoint =
    incident.endpoint_url ??
    incident.endpoint_dns_name ??
    formatSocketTarget(incident.endpoint_address, incident.endpoint_port) ??
    (incident.endpoint_id
      ? `Endpoint ${shortId(incident.endpoint_id)}`
      : agentMonitor
        ? "Agent target"
        : "Endpoint unavailable");

  return { name, endpoint };
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
