import { useQuery } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import {
  ActivityIcon,
  CircleAlertIcon,
  ExternalLinkIcon,
  ShieldAlertIcon,
} from "lucide-react";
import { useMemo, useState, type ReactNode } from "react";
import { fetchChanges, type ChangeEvent } from "@/lib/api";
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
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";

export const Route = createFileRoute("/changes")({
  component: ChangesPage,
});

const SEVERITIES = ["all", "info", "notice", "warning", "critical"] as const;
const EMPTY_CHANGES: ChangeEvent[] = [];

function ChangesPage() {
  const [severity, setSeverity] = useState<(typeof SEVERITIES)[number]>("all");
  const [category, setCategory] = useState("all");
  const changesQuery = useQuery({
    queryKey: ["changes", category, severity],
    queryFn: () =>
      fetchChanges({
        category: category === "all" ? undefined : category,
        severity: severity === "all" ? undefined : severity,
      }),
  });
  const changes = changesQuery.data?.items ?? EMPTY_CHANGES;
  const categories = useMemo(
    () => ["all", ...new Set(changes.map((change) => change.category))],
    [changes],
  );

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-6">
      <div>
        <h1 className="text-3xl font-semibold tracking-tight">Changes</h1>
      </div>

      <div className="grid gap-4 sm:grid-cols-3">
        <Summary
          icon={<ActivityIcon />}
          label="Events"
          value={changes.length}
          detail="Matching changes"
        />
        <Summary
          icon={<ShieldAlertIcon />}
          label="Critical"
          value={
            changes.filter((change) => change.severity === "critical").length
          }
          detail="Critical events"
        />
        <Summary
          icon={<CircleAlertIcon />}
          label="Review"
          value={
            changes.filter(
              (change) =>
                change.severity === "notice" || change.severity === "warning",
            ).length
          }
          detail="Notice or warning"
        />
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Change history</CardTitle>
          <CardAction>
            <Badge variant="outline">{changes.length} events</Badge>
          </CardAction>
        </CardHeader>
        <CardContent className="flex flex-col gap-5">
          <div className="flex flex-col justify-between gap-3 md:flex-row md:items-center">
            <ToggleGroup
              onValueChange={(values) => {
                const next = values[0];
                if (next) setSeverity(next as (typeof SEVERITIES)[number]);
              }}
              size="sm"
              value={[severity]}
              variant="outline"
            >
              {SEVERITIES.map((item) => (
                <ToggleGroupItem key={item} value={item}>
                  {labelize(item)}
                </ToggleGroupItem>
              ))}
            </ToggleGroup>
            <Select
              onValueChange={(value) => {
                if (value) setCategory(value);
              }}
              value={category}
            >
              <SelectTrigger className="w-full md:w-48">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  {categories.map((item) => (
                    <SelectItem key={item} value={item}>
                      {item === "all" ? "All categories" : labelize(item)}
                    </SelectItem>
                  ))}
                </SelectGroup>
              </SelectContent>
            </Select>
          </div>
          {changesQuery.isLoading ? (
            <LoadingFeed />
          ) : changesQuery.isError ? (
            <Alert variant="destructive">
              <CircleAlertIcon />
              <AlertTitle>Changes unavailable</AlertTitle>
              <AlertDescription>{changesQuery.error.message}</AlertDescription>
              <Button
                onClick={() => changesQuery.refetch()}
                size="sm"
                variant="outline"
              >
                Retry
              </Button>
            </Alert>
          ) : changes.length === 0 ? (
            <Empty>
              <EmptyHeader>
                <EmptyMedia variant="icon">
                  <ActivityIcon />
                </EmptyMedia>
                <EmptyTitle>No matching changes</EmptyTitle>
                <EmptyDescription>
                  No events match the selected filters.
                </EmptyDescription>
              </EmptyHeader>
            </Empty>
          ) : (
            <div className="divide-y">
              {changes.map((change) => (
                <ChangeRow change={change} key={change.id} />
              ))}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

function Summary({
  icon,
  label,
  value,
  detail,
}: {
  icon: ReactNode;
  label: string;
  value: number;
  detail: string;
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

function ChangeRow({ change }: { change: ChangeEvent }) {
  const summary = summarize(change.before, change.after);
  const severityVariant =
    change.severity === "critical"
      ? "destructive"
      : change.severity === "warning"
        ? "outline"
        : "secondary";
  return (
    <article className="flex gap-3 py-4">
      <span className="mt-0.5 grid size-8 shrink-0 place-items-center rounded-lg bg-muted text-muted-foreground">
        <ActivityIcon className="size-4" />
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant={severityVariant}>{labelize(change.severity)}</Badge>
          <span className="text-xs text-muted-foreground">
            {labelize(change.category)}
          </span>
          <time
            className="ml-auto text-xs text-muted-foreground"
            title={formatDate(change.occurred_at)}
          >
            {formatRelative(change.occurred_at)}
          </time>
        </div>
        <h2 className="mt-2 font-medium">
          {labelize(change.entity_kind)} record changed
        </h2>
        <p className="mt-1 text-sm text-muted-foreground">
          {summary || "An event was recorded without field snapshots."}
        </p>
        <div className="mt-2 flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
          <span className="font-mono">{shortId(change.entity_id)}</span>
          <span>
            Source:{" "}
            {change.evidence_source
              ? labelize(change.evidence_source)
              : "system"}
          </span>
          {change.entity_kind === "devices" ? (
            <Link
              className="inline-flex items-center gap-1 text-foreground hover:underline"
              to="/infrastructure"
            >
              Open inventory <ExternalLinkIcon className="size-3" />
            </Link>
          ) : null}
        </div>
      </div>
    </article>
  );
}

function summarize(
  before: Record<string, unknown> | null,
  after: Record<string, unknown> | null,
): string {
  if (!before && !after) return "";
  const keys = new Set([
    ...Object.keys(before ?? {}),
    ...Object.keys(after ?? {}),
  ]);
  const changes = [...keys].filter(
    (key) => JSON.stringify(before?.[key]) !== JSON.stringify(after?.[key]),
  );
  return changes.length
    ? changes
        .slice(0, 3)
        .map((key) => `${labelize(key)} updated`)
        .join(" · ")
    : "Snapshot recorded.";
}

function LoadingFeed() {
  return (
    <div className="flex flex-col gap-4">
      {Array.from({ length: 6 }, (_, index) => (
        <Skeleton className="h-20 w-full" key={index} />
      ))}
    </div>
  );
}
