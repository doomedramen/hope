import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ActivityIcon,
  CheckCheckIcon,
  CheckIcon,
  CircleAlertIcon,
  ListChecksIcon,
  XIcon,
} from "lucide-react";
import { useState } from "react";
import {
  fetchMonitorProposals,
  getUserFacingError,
  resolveMonitorProposal,
  type MonitorProposal,
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
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Empty,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
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

export function MonitorProposalQueue() {
  const queryClient = useQueryClient();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [bulkApproveOpen, setBulkApproveOpen] = useState(false);
  const [rejectOpen, setRejectOpen] = useState(false);
  const [bulkFailures, setBulkFailures] = useState<string[]>([]);
  const [bulkApprovedCount, setBulkApprovedCount] = useState(0);
  const proposalsQuery = useQuery({
    queryKey: ["monitor-proposals", "pending"],
    queryFn: () => fetchMonitorProposals("pending"),
  });
  const resolveMutation = useMutation({
    mutationFn: ({
      id,
      decision,
    }: {
      id: string;
      decision: "approve" | "reject";
    }) => resolveMonitorProposal(id, decision),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: ["monitor-proposals", "pending"],
        }),
        queryClient.invalidateQueries({ queryKey: ["monitors"] }),
      ]);
      setRejectOpen(false);
    },
  });
  const bulkApproveMutation = useMutation({
    mutationFn: async () => {
      const proposalsToApprove = proposalsQuery.data?.items ?? [];
      const settled = await Promise.all(
        proposalsToApprove.map(async (proposal) => {
          try {
            await resolveMonitorProposal(proposal.id, "approve");
            return { proposal, error: null };
          } catch (error) {
            return { proposal, error };
          }
        }),
      );
      return {
        approvedCount: settled.filter((item) => item.error === null).length,
        failedTargets: settled
          .filter((item) => item.error !== null)
          .map((item) => item.proposal.target_identity),
      };
    },
    onMutate: () => {
      setBulkFailures([]);
      setBulkApprovedCount(0);
    },
    onSuccess: async ({ approvedCount, failedTargets }) => {
      setBulkApprovedCount(approvedCount);
      setBulkFailures(failedTargets);
      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: ["monitor-proposals", "pending"],
        }),
        queryClient.invalidateQueries({ queryKey: ["monitors"] }),
      ]);
    },
  });

  const proposals = proposalsQuery.data?.items ?? [];
  const selected =
    proposals.find((proposal) => proposal.id === selectedId) ??
    proposals[0] ??
    null;

  function resolve(decision: "approve" | "reject") {
    if (!selected) return;
    resolveMutation.mutate({ id: selected.id, decision });
  }

  return (
    <Card aria-busy={proposalsQuery.isLoading} className="overflow-hidden">
      <CardHeader className="border-b">
        <div className="flex min-w-0 flex-wrap items-center gap-3">
          <span className="grid size-9 shrink-0 place-items-center rounded-lg bg-muted text-muted-foreground">
            <ListChecksIcon className="size-4" />
          </span>
          <CardTitle>Pending monitor proposals</CardTitle>
        </div>
        <div className="ml-auto flex flex-wrap items-center justify-end gap-2">
          {proposals.length > 1 ? (
            <Button
              disabled={bulkApproveMutation.isPending}
              onClick={() => setBulkApproveOpen(true)}
              size="sm"
              variant="outline"
            >
              <CheckCheckIcon />
              {bulkApproveMutation.isPending ? "Approving..." : "Approve all"}
            </Button>
          ) : null}
          <Badge variant={proposals.length ? "outline" : "secondary"}>
            {proposals.length} pending
          </Badge>
        </div>
      </CardHeader>
      {proposalsQuery.isLoading ? (
        <ProposalLoading />
      ) : proposalsQuery.isError ? (
        <CardContent className="pt-6">
          <Alert variant="destructive">
            <CircleAlertIcon />
            <AlertTitle>Monitor proposals unavailable</AlertTitle>
            <AlertDescription>
              {getUserFacingError(
                proposalsQuery.error,
                "The monitor proposal queue could not be loaded.",
              )}
            </AlertDescription>
            <Button
              onClick={() => proposalsQuery.refetch()}
              size="sm"
              variant="outline"
            >
              Retry
            </Button>
          </Alert>
        </CardContent>
      ) : proposals.length === 0 ? (
        <CardContent className="pt-6">
          <Empty>
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <CheckIcon />
              </EmptyMedia>
              <EmptyTitle>No pending monitor proposals</EmptyTitle>
            </EmptyHeader>
          </Empty>
        </CardContent>
      ) : (
        <div className="grid lg:grid-cols-[minmax(15rem,0.8fr)_minmax(0,1.6fr)]">
          <div className="divide-y border-b lg:border-r lg:border-b-0">
            {proposals.map((proposal) => (
              <ProposalListItem
                key={proposal.id}
                onSelect={() => setSelectedId(proposal.id)}
                proposal={proposal}
                selected={proposal.id === selected?.id}
              />
            ))}
          </div>
          {selected ? (
            <ProposalDetail
              onApprove={() => resolve("approve")}
              onReject={() => setRejectOpen(true)}
              pending={resolveMutation.isPending}
              proposal={selected}
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
              {getUserFacingError(resolveMutation.error, "Try again.")}
            </AlertDescription>
          </Alert>
        </div>
      ) : null}
      {bulkFailures.length ? (
        <div className="border-t px-6 py-4">
          <Alert variant="destructive">
            <CircleAlertIcon />
            <AlertTitle>
              {bulkApprovedCount
                ? "Some monitor proposals need attention"
                : "Monitor proposals were not approved"}
            </AlertTitle>
            <AlertDescription>
              {bulkApprovedCount} proposal
              {bulkApprovedCount === 1 ? " was" : "s were"} approved and
              refreshed. Failed targets remain pending:{" "}
              {bulkFailures.join(", ")}.
            </AlertDescription>
          </Alert>
        </div>
      ) : null}
      {bulkApproveMutation.isError ? (
        <div className="border-t px-6 py-4">
          <Alert variant="destructive">
            <CircleAlertIcon />
            <AlertTitle>Bulk approval was not completed</AlertTitle>
            <AlertDescription>
              {getUserFacingError(bulkApproveMutation.error, "Try again.")}
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
            <AlertDialogTitle>Reject this monitor proposal?</AlertDialogTitle>
            <AlertDialogDescription>
              Rejecting it prevents monitor creation.
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
              Reject proposal
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <AlertDialog onOpenChange={setBulkApproveOpen} open={bulkApproveOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogMedia>
              <CheckCheckIcon />
            </AlertDialogMedia>
            <AlertDialogTitle>Approve all monitor proposals?</AlertDialogTitle>
            <AlertDialogDescription>
              This will approve {proposals.length} pending proposal
              {proposals.length === 1 ? "" : "s"} and create the suggested
              monitors. Review the individual proposals first if any target is
              uncertain.
            </AlertDialogDescription>
            <div className="rounded-lg border bg-muted/30 p-3 text-sm">
              <p className="font-medium">Targets to approve</p>
              <ul className="mt-2 grid gap-1 text-muted-foreground">
                {proposals.slice(0, 5).map((proposal) => (
                  <li
                    className="flex items-center justify-between gap-3"
                    key={proposal.id}
                  >
                    <span className="min-w-0 truncate">
                      {proposal.target_identity}
                    </span>
                    <span className="shrink-0 text-xs">
                      {proposal.check_type.toUpperCase()}
                    </span>
                  </li>
                ))}
              </ul>
              {proposals.length > 5 ? (
                <p className="mt-2 text-xs text-muted-foreground">
                  …and {proposals.length - 5} more pending proposals.
                </p>
              ) : null}
            </div>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={bulkApproveMutation.isPending}>
              Keep for review
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={bulkApproveMutation.isPending}
              onClick={() => {
                bulkApproveMutation.mutate();
                setBulkApproveOpen(false);
              }}
            >
              {bulkApproveMutation.isPending ? <Spinner /> : null}
              Approve all proposals
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Card>
  );
}

function ProposalListItem({
  onSelect,
  proposal,
  selected,
}: {
  onSelect: () => void;
  proposal: MonitorProposal;
  selected: boolean;
}) {
  return (
    <button
      aria-pressed={selected}
      className="w-full px-4 py-4 text-left outline-none transition-colors hover:bg-muted/50 focus-visible:ring-3 focus-visible:ring-ring/50 aria-pressed:bg-muted"
      onClick={onSelect}
      type="button"
    >
      <div className="flex items-start justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <ActivityIcon className="size-4 shrink-0 text-muted-foreground" />
          <span className="truncate font-medium">
            {proposal.product ?? labelize(proposal.check_type)}
          </span>
        </div>
        <Badge variant="secondary">{labelize(proposal.check_type)}</Badge>
      </div>
      <div className="mt-2 flex items-center justify-between gap-2 text-xs text-muted-foreground">
        <span className="font-mono">{shortId(proposal.endpoint_id)}</span>
        <span>
          {proposal.auto_create_allowed
            ? "Policy allows auto-create"
            : "Approval required"}
        </span>
      </div>
      <p className="mt-1 text-xs text-muted-foreground">
        {formatRelative(proposal.created_at)}
      </p>
    </button>
  );
}

function ProposalDetail({
  onApprove,
  onReject,
  pending,
  proposal,
}: {
  onApprove: () => void;
  onReject: () => void;
  pending: boolean;
  proposal: MonitorProposal;
}) {
  return (
    <div className="min-w-0 p-5 md:p-6">
      <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant="outline">{labelize(proposal.check_type)}</Badge>
            <Badge
              variant={proposal.auto_create_allowed ? "default" : "secondary"}
            >
              {proposal.auto_create_allowed
                ? "Auto-create allowed"
                : "Approval required"}
            </Badge>
          </div>
          <h3 className="mt-3 text-xl font-semibold tracking-tight">
            {proposal.product ?? `${labelize(proposal.check_type)} check`}
          </h3>
          <p className="mt-1 text-sm text-muted-foreground">
            Endpoint{" "}
            <span className="font-mono">{shortId(proposal.endpoint_id)}</span>
            {proposal.product_version
              ? ` · version ${proposal.product_version}`
              : ""}
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
          <Button disabled={pending} onClick={onApprove} size="sm">
            {pending ? <Spinner /> : <CheckIcon data-icon="inline-start" />}
            Approve
          </Button>
        </div>
      </div>

      <div className="mt-6 grid gap-4 sm:grid-cols-3">
        <DetailValue
          label="Rule"
          value={`${proposal.rule_id} · v${proposal.rule_version}`}
        />
        <DetailValue label="Service" value={shortId(proposal.service_id)} />
        <DetailValue label="Created" value={formatDate(proposal.created_at)} />
      </div>

      <Separator className="my-6" />

      <div className="grid gap-6 xl:grid-cols-2">
        <ValueTable title="Target" value={proposal.target} />
        <ValueTable title="Check configuration" value={proposal.check_config} />
      </div>

      <Separator className="my-6" />

      <section>
        <h4 className="font-medium">Resolved from</h4>
        <ValueTable
          className="mt-3"
          title="Service state"
          value={proposal.resolved_from}
        />
      </section>
      {Object.keys(proposal.user_overrides).length ? (
        <section className="mt-6">
          <h4 className="font-medium">User overrides</h4>
          <ValueTable
            className="mt-3"
            title="Overrides"
            value={proposal.user_overrides}
          />
        </section>
      ) : null}
      <p className="mt-5 text-xs text-muted-foreground">
        Proposal <span className="font-mono">{shortId(proposal.id)}</span>
        {" · "}
        {proposal.source_evidence_id
          ? `Evidence ${shortId(proposal.source_evidence_id)}`
          : "No source evidence"}
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

function ValueTable({
  className,
  title,
  value,
}: {
  className?: string;
  title: string;
  value: Record<string, unknown>;
}) {
  return (
    <div className={className}>
      <div className="overflow-hidden rounded-lg border">
        <div className="border-b bg-muted/40 px-3 py-2 text-sm font-medium">
          {title}
        </div>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Field</TableHead>
              <TableHead>Value</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {Object.entries(value).map(([key, entry]) => (
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
      </div>
    </div>
  );
}

function ProposalLoading() {
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
