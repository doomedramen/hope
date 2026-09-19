import { useQuery } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import {
  ArrowUpRightIcon,
  GitMergeIcon,
  NetworkIcon,
  ServerIcon,
  ShieldCheckIcon,
} from "lucide-react";
import type { ReactNode } from "react";
import {
  fetchChanges,
  fetchDevices,
  fetchIdentitySuggestions,
  fetchMonitorProposals,
  fetchServiceReviews,
} from "@/lib/api";
import { formatRelative, labelize, shortId } from "@/lib/format";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";

export const Route = createFileRoute("/")({
  component: OverviewPage,
});

function OverviewPage() {
  const devicesQuery = useQuery({
    queryKey: ["devices"],
    queryFn: fetchDevices,
  });
  const suggestionsQuery = useQuery({
    queryKey: ["identity-suggestions"],
    queryFn: fetchIdentitySuggestions,
  });
  const serviceReviewsQuery = useQuery({
    queryKey: ["service-reviews", "pending"],
    queryFn: () => fetchServiceReviews(),
  });
  const monitorProposalsQuery = useQuery({
    queryKey: ["monitor-proposals", "pending"],
    queryFn: () => fetchMonitorProposals(),
  });
  const changesQuery = useQuery({
    queryKey: ["changes", "overview"],
    queryFn: () => fetchChanges(),
  });
  const devices = devicesQuery.data?.items ?? [];
  const suggestions = suggestionsQuery.data?.items ?? [];
  const changes = changesQuery.data?.items ?? [];
  const pendingReviews =
    suggestions.length +
    (serviceReviewsQuery.data?.items.length ?? 0) +
    (monitorProposalsQuery.data?.items.length ?? 0);
  const loading =
    devicesQuery.isLoading ||
    suggestionsQuery.isLoading ||
    serviceReviewsQuery.isLoading ||
    monitorProposalsQuery.isLoading ||
    changesQuery.isLoading;

  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-6">
      <div className="max-w-2xl">
        <p className="text-sm text-muted-foreground">Overview / M1 inventory</p>
        <h1 className="mt-1 text-3xl font-semibold tracking-tight">
          Infrastructure inventory
        </h1>
        <p className="mt-2 text-muted-foreground">
          Devices, network interfaces, IP addresses, and identity evidence.
        </p>
      </div>
      <div className="grid gap-4 md:grid-cols-3">
        <OverviewMetric
          icon={<ServerIcon />}
          label="Devices"
          value={devices.length}
          detail="Device records"
          to="/infrastructure"
        />
        <OverviewMetric
          icon={<ShieldCheckIcon />}
          label="Pending review"
          value={pendingReviews}
          detail="Identity, service, and monitor reviews"
          to="/monitoring"
        />
        <OverviewMetric
          icon={<NetworkIcon />}
          label="Recent changes"
          value={changes.length}
          detail="Recorded events"
          to="/changes"
        />
      </div>
      <div className="grid gap-6 lg:grid-cols-[1.4fr_1fr]">
        <Card>
          <CardHeader>
            <CardTitle>Recent changes</CardTitle>
            <CardDescription>Recent inventory events.</CardDescription>
            <CardAction>
              <Link className="text-sm hover:underline" to="/changes">
                View all
              </Link>
            </CardAction>
          </CardHeader>
          <CardContent>
            {loading ? (
              <LoadingRows />
            ) : changes.length ? (
              <div className="divide-y">
                {changes.slice(0, 5).map((change) => (
                  <div className="flex items-center gap-3 py-3" key={change.id}>
                    <span className="grid size-8 place-items-center rounded-lg bg-muted">
                      <GitMergeIcon className="size-4 text-muted-foreground" />
                    </span>
                    <div className="min-w-0 flex-1">
                      <p className="font-medium">
                        {labelize(change.category)} on{" "}
                        {labelize(change.entity_kind)}
                      </p>
                      <p className="font-mono text-xs text-muted-foreground">
                        {shortId(change.entity_id)}
                      </p>
                    </div>
                    <time className="text-xs text-muted-foreground">
                      {formatRelative(change.occurred_at)}
                    </time>
                  </div>
                ))}
              </div>
            ) : (
              <p className="text-sm text-muted-foreground">
                No change events yet.
              </p>
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle>Identity review</CardTitle>
            <CardDescription>
              Review low-confidence device matches.
            </CardDescription>
            <CardAction>
              <Badge variant={suggestions.length ? "outline" : "secondary"}>
                {suggestions.length} pending
              </Badge>
            </CardAction>
          </CardHeader>
          <CardContent>
            {loading ? (
              <LoadingRows />
            ) : suggestions.length ? (
              <div className="flex flex-col gap-3">
                {suggestions.slice(0, 4).map((suggestion) => (
                  <div className="rounded-lg border p-3" key={suggestion.id}>
                    <div className="flex items-center justify-between gap-2">
                      <p className="font-medium">
                        Candidate {shortId(suggestion.candidate_device_id)}
                      </p>
                      <Badge variant="outline">
                        {Math.round(suggestion.score * 100)}%
                      </Badge>
                    </div>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {suggestion.explanation.matched
                        .map((match) => labelize(match.rule_type))
                        .join(", ")}
                    </p>
                  </div>
                ))}
              </div>
            ) : (
              <p className="text-sm text-muted-foreground">
                No unresolved identity suggestions.
              </p>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

function OverviewMetric({
  icon,
  label,
  value,
  detail,
  to,
}: {
  icon: ReactNode;
  label: string;
  value: number;
  detail: string;
  to: "/" | "/infrastructure" | "/monitoring" | "/changes";
}) {
  return (
    <Link to={to}>
      <Card className="transition-colors hover:bg-muted/40">
        <CardContent className="flex items-center gap-3">
          <span className="grid size-10 place-items-center rounded-lg bg-muted text-muted-foreground">
            {icon}
          </span>
          <div>
            <p className="text-sm text-muted-foreground">{label}</p>
            <p className="text-2xl font-semibold tracking-tight">{value}</p>
            <p className="text-xs text-muted-foreground">{detail}</p>
          </div>
          <ArrowUpRightIcon className="ml-auto size-4 text-muted-foreground" />
        </CardContent>
      </Card>
    </Link>
  );
}

function LoadingRows() {
  return (
    <div className="flex flex-col gap-3">
      {Array.from({ length: 4 }, (_, index) => (
        <Skeleton className="h-12 w-full" key={index} />
      ))}
    </div>
  );
}
