import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  CheckIcon,
  CircleAlertIcon,
  FileSearchIcon,
  FingerprintIcon,
  ShieldQuestionIcon,
  XIcon,
} from "lucide-react";
import { useState } from "react";
import {
  fetchServiceReviews,
  resolveServiceReview,
  type FingerprintCandidate,
  type ServiceReviewItem,
} from "@/lib/api";
import {
  displayValue,
  formatDate,
  formatRelative,
  labelize,
  shortId,
} from "@/lib/format";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogMedia,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import {
  Progress,
  ProgressLabel,
  ProgressValue,
} from "@/components/ui/progress";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { Spinner } from "@/components/ui/spinner";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

export function ServiceReviewQueue() {
  const queryClient = useQueryClient();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [rejectOpen, setRejectOpen] = useState(false);
  const reviewsQuery = useQuery({
    queryKey: ["service-reviews", "pending"],
    queryFn: () => fetchServiceReviews("pending"),
  });
  const resolveMutation = useMutation({
    mutationFn: ({
      id,
      decision,
    }: {
      id: string;
      decision: "confirm" | "reject";
    }) => resolveServiceReview(id, decision),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ["service-reviews", "pending"],
      });
      setRejectOpen(false);
    },
  });

  const reviews = reviewsQuery.data?.items ?? [];
  const selected =
    reviews.find((review) => review.id === selectedId) ?? reviews[0] ?? null;

  function resolve(decision: "confirm" | "reject") {
    if (!selected) return;
    resolveMutation.mutate({ id: selected.id, decision });
  }

  return (
    <Card className="overflow-hidden">
      <CardHeader className="border-b">
        <div className="flex items-start gap-3">
          <span className="grid size-9 shrink-0 place-items-center rounded-lg bg-muted text-muted-foreground">
            <ShieldQuestionIcon className="size-4" />
          </span>
          <div>
            <CardTitle>Service identity review</CardTitle>
            <CardDescription>
              Decide when a fingerprint should become part of the inventory.
            </CardDescription>
          </div>
        </div>
        <Badge
          className="ml-auto"
          variant={reviews.length ? "outline" : "secondary"}
        >
          {reviews.length} pending
        </Badge>
      </CardHeader>
      {reviewsQuery.isLoading ? (
        <QueueLoading />
      ) : reviewsQuery.isError ? (
        <CardContent className="pt-6">
          <Alert variant="destructive">
            <CircleAlertIcon />
            <AlertTitle>Review queue unavailable</AlertTitle>
            <AlertDescription>
              {reviewsQuery.error instanceof Error
                ? reviewsQuery.error.message
                : "The service review queue could not be loaded."}
            </AlertDescription>
            <Button
              onClick={() => reviewsQuery.refetch()}
              size="sm"
              variant="outline"
            >
              Retry
            </Button>
          </Alert>
        </CardContent>
      ) : reviews.length === 0 ? (
        <CardContent className="pt-6">
          <Empty>
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <CheckIcon />
              </EmptyMedia>
              <EmptyTitle>No service reviews pending</EmptyTitle>
              <EmptyDescription>
                New ambiguous fingerprints will appear here with their source
                evidence.
              </EmptyDescription>
            </EmptyHeader>
          </Empty>
        </CardContent>
      ) : (
        <div className="grid lg:grid-cols-[minmax(15rem,0.8fr)_minmax(0,1.6fr)]">
          <div className="divide-y border-b lg:border-r lg:border-b-0">
            {reviews.map((review) => (
              <ReviewListItem
                key={review.id}
                onSelect={() => setSelectedId(review.id)}
                review={review}
                selected={review.id === selected?.id}
              />
            ))}
          </div>
          {selected ? (
            <ReviewDetail
              onConfirm={() => resolve("confirm")}
              onReject={() => setRejectOpen(true)}
              pending={resolveMutation.isPending}
              review={selected}
            />
          ) : null}
        </div>
      )}
      {resolveMutation.isError ? (
        <div className="border-t px-6 py-4">
          <Alert variant="destructive">
            <CircleAlertIcon />
            <AlertTitle>Decision was not saved</AlertTitle>
            <AlertDescription>
              {resolveMutation.error instanceof Error
                ? resolveMutation.error.message
                : "Try again."}
            </AlertDescription>
          </Alert>
        </div>
      ) : null}
      <AlertDialog onOpenChange={setRejectOpen} open={rejectOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogMedia>
              <XIcon />
            </AlertDialogMedia>
            <AlertDialogTitle>Reject this fingerprint?</AlertDialogTitle>
            <AlertDialogDescription>
              The unchanged candidate will stay rejected and will not return on
              the next scan.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={resolveMutation.isPending}>
              Keep for review
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={resolveMutation.isPending}
              onClick={() => resolve("reject")}
              variant="destructive"
            >
              {resolveMutation.isPending ? <Spinner /> : null}
              Reject candidate
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Card>
  );
}

function ReviewListItem({
  onSelect,
  review,
  selected,
}: {
  onSelect: () => void;
  review: ServiceReviewItem;
  selected: boolean;
}) {
  return (
    <button
      aria-pressed={selected}
      className="w-full px-4 py-4 text-left transition-colors hover:bg-muted/50 aria-pressed:bg-muted"
      onClick={onSelect}
      type="button"
    >
      <div className="flex items-start justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <FingerprintIcon className="size-4 shrink-0 text-muted-foreground" />
          <span className="truncate font-medium">
            {review.candidate.product ?? labelize(review.candidate.protocol)}
          </span>
        </div>
        <Badge variant={review.reason === "conflict" ? "outline" : "secondary"}>
          {review.reason === "conflict" ? "Conflict" : "Low confidence"}
        </Badge>
      </div>
      <div className="mt-2 flex items-center justify-between gap-2 text-xs text-muted-foreground">
        <span className="font-mono">{shortId(review.service_id)}</span>
        <span>{Math.round(review.confidence * 100)}%</span>
      </div>
      <p className="mt-1 text-xs text-muted-foreground">
        {formatRelative(review.created_at)}
      </p>
    </button>
  );
}

function ReviewDetail({
  onConfirm,
  onReject,
  pending,
  review,
}: {
  onConfirm: () => void;
  onReject: () => void;
  pending: boolean;
  review: ServiceReviewItem;
}) {
  const candidate = review.candidate;
  const version = candidate.product_version ?? candidate.version;

  return (
    <div className="min-w-0 p-5 md:p-6">
      <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant="outline">{labelize(candidate.protocol)}</Badge>
            <Badge
              variant={review.reason === "conflict" ? "outline" : "secondary"}
            >
              {review.reason === "conflict"
                ? "Existing value differs"
                : "Below policy threshold"}
            </Badge>
          </div>
          <h3 className="mt-3 text-xl font-semibold tracking-tight">
            {candidate.product ?? "Unidentified service"}
          </h3>
          <p className="mt-1 text-sm text-muted-foreground">
            Service{" "}
            <span className="font-mono">{shortId(review.service_id)}</span>
            {version ? ` · version ${version}` : ""}
          </p>
        </div>
        <div className="flex shrink-0 gap-2">
          <Button
            disabled={pending}
            onClick={onReject}
            size="sm"
            variant="outline"
          >
            <XIcon data-icon="inline-start" />
            Reject
          </Button>
          <Button disabled={pending} onClick={onConfirm} size="sm">
            {pending ? <Spinner /> : <CheckIcon data-icon="inline-start" />}
            Confirm
          </Button>
        </div>
      </div>

      <div className="mt-6 grid gap-4 sm:grid-cols-3">
        <DetailValue label="Rule" value={candidate.rule_id} />
        <DetailValue label="Fixture" value={`v${candidate.fixture_version}`} />
        <DetailValue label="Observed" value={formatDate(review.created_at)} />
      </div>

      <div className="mt-6 rounded-lg border bg-muted/25 p-4">
        <Progress value={review.confidence * 100}>
          <div className="flex w-full items-center gap-2">
            <ProgressLabel>Fingerprint confidence</ProgressLabel>
            <ProgressValue />
          </div>
        </Progress>
        <p className="mt-2 text-xs text-muted-foreground">
          Policy auto-apply threshold:{" "}
          {Math.round(review.policy_threshold * 100)}%
        </p>
      </div>

      <Separator className="my-6" />

      <div className="grid gap-6 xl:grid-cols-2">
        <section>
          <div className="flex items-center gap-2">
            <FileSearchIcon className="size-4 text-muted-foreground" />
            <h4 className="font-medium">Why this match</h4>
          </div>
          <ul className="mt-3 flex flex-col gap-2 text-sm">
            {candidate.reasons.map((reason) => (
              <li className="flex gap-2" key={reason}>
                <span className="mt-1.5 size-1.5 shrink-0 rounded-full bg-foreground" />
                <span>{reason}</span>
              </li>
            ))}
          </ul>
          <div className="mt-5">
            <p className="text-xs font-medium text-muted-foreground">
              Evidence fields
            </p>
            <div className="mt-2 divide-y rounded-lg border">
              {candidate.evidence_fields.map((field) => (
                <div
                  className="flex flex-wrap justify-between gap-2 px-3 py-2 text-sm"
                  key={`${field.path}-${field.value}`}
                >
                  <span className="font-mono text-xs text-muted-foreground">
                    {field.path}
                  </span>
                  <span className="text-right">{field.value}</span>
                </div>
              ))}
            </div>
          </div>
        </section>
        <section>
          <div className="flex items-center gap-2">
            <ShieldQuestionIcon className="size-4 text-muted-foreground" />
            <h4 className="font-medium">Ranked candidates</h4>
          </div>
          <div className="mt-3 flex flex-col gap-2">
            {review.candidates.map((ranked, index) => (
              <CandidateRow
                candidate={ranked}
                current={index === 0}
                key={`${ranked.rule_id}-${ranked.confidence}`}
              />
            ))}
          </div>
        </section>
      </div>

      <Separator className="my-6" />

      <section>
        <h4 className="font-medium">Source observations</h4>
        <div className="mt-3 grid gap-3 xl:grid-cols-2">
          <Observation title="M2 classification" value={review.evidence.m2} />
          <Observation
            title="Fingerprint evidence"
            value={review.evidence.fingerprint}
          />
        </div>
      </section>
      <p className="mt-5 text-xs text-muted-foreground">
        Source run:{" "}
        <span className="font-mono">
          {review.source_instance ? shortId(review.source_instance) : "—"}
        </span>
        {" · "}
        Review item <span className="font-mono">{shortId(review.id)}</span>
      </p>
    </div>
  );
}

function DetailValue({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <p className="text-xs text-muted-foreground">{label}</p>
      <p className="mt-1 truncate font-mono text-sm" title={value}>
        {value}
      </p>
    </div>
  );
}

function CandidateRow({
  candidate,
  current,
}: {
  candidate: FingerprintCandidate;
  current: boolean;
}) {
  return (
    <div className="flex items-center gap-3 rounded-lg border px-3 py-2">
      <span className="grid size-6 shrink-0 place-items-center rounded-full bg-muted text-xs font-medium">
        {current ? "1" : "·"}
      </span>
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium">
          {candidate.product ?? labelize(candidate.protocol)}
        </p>
        <p className="truncate font-mono text-xs text-muted-foreground">
          {candidate.rule_id}
        </p>
      </div>
      <Badge variant={current ? "default" : "secondary"}>
        {Math.round(candidate.confidence * 100)}%
      </Badge>
    </div>
  );
}

function Observation({
  title,
  value,
}: {
  title: string;
  value: Record<string, unknown>;
}) {
  const entries = Object.entries(value).filter(([key]) => key !== "candidates");
  return (
    <div className="overflow-hidden rounded-lg border">
      <div className="border-b bg-muted/40 px-3 py-2 text-sm font-medium">
        {title}
      </div>
      {entries.length ? (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Field</TableHead>
              <TableHead>Value</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {entries.map(([key, entry]) => (
              <TableRow key={key}>
                <TableCell className="font-mono text-xs text-muted-foreground">
                  {key}
                </TableCell>
                <TableCell className="max-w-72 whitespace-normal text-xs">
                  {displayValue(entry)}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : (
        <p className="p-3 text-sm text-muted-foreground">
          No bounded fields recorded.
        </p>
      )}
    </div>
  );
}

function QueueLoading() {
  return (
    <div className="grid gap-5 p-5 lg:grid-cols-[minmax(15rem,0.8fr)_minmax(0,1.6fr)]">
      <div className="flex flex-col gap-3">
        {Array.from({ length: 4 }, (_, index) => (
          <Skeleton className="h-20 w-full" key={index} />
        ))}
      </div>
      <div className="flex flex-col gap-4">
        <Skeleton className="h-8 w-2/5" />
        <Skeleton className="h-24 w-full" />
        <Skeleton className="h-48 w-full" />
      </div>
    </div>
  );
}
