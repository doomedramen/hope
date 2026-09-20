import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  CheckIcon,
  CircleAlertIcon,
  NetworkIcon,
  PlusIcon,
  PlayIcon,
  XIcon,
} from "lucide-react";
import { useMemo, useState } from "react";
import {
  cancelScanRun,
  confirmDiscoveryScope,
  draftDiscoveryScope,
  fetchScanRun,
  launchNetworkScan,
  type DiscoveryScope,
  type Network,
  type ScanRun,
} from "@/lib/api";
import { isValidCidr } from "@/lib/network-validation";
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
import { Checkbox } from "@/components/ui/checkbox";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import {
  Field,
  FieldContent,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";

type ScanProfile = "normal" | "low_impact";

interface ScopeDraftState {
  excludedCidrs: string;
  scanProfile: ScanProfile;
  scope: DiscoveryScope | null;
  appliedExcludedCidrs: string[] | null;
  appliedScanProfile: ScanProfile | null;
  acknowledged: boolean;
  scanRun: ScanRun | null;
}

const DEFAULT_SCOPE_STATE: ScopeDraftState = {
  excludedCidrs: "",
  scanProfile: "normal",
  scope: null,
  appliedExcludedCidrs: null,
  appliedScanProfile: null,
  acknowledged: false,
  scanRun: null,
};

export function DiscoveryScopeSetup({
  networks,
  loading,
  error,
  onAddNetwork,
}: {
  networks: Network[];
  loading: boolean;
  error: unknown;
  onAddNetwork?: () => void;
}) {
  const [selectedNetworkId, setSelectedNetworkId] = useState<string | null>(
    null,
  );
  const [scopeStates, setScopeStates] = useState<
    Record<string, ScopeDraftState>
  >({});
  const selectedNetwork =
    networks.find((network) => network.id === selectedNetworkId) ??
    networks[0] ??
    null;
  const selectedState = selectedNetwork
    ? (scopeStates[selectedNetwork.id] ?? DEFAULT_SCOPE_STATE)
    : DEFAULT_SCOPE_STATE;

  const draftMutation = useMutation({
    mutationFn: ({
      networkId,
      excludedCidrs: nextExcludedCidrs,
      scanProfile,
    }: {
      networkId: string;
      excludedCidrs: string[];
      scanProfile: ScanProfile;
    }) =>
      draftDiscoveryScope(networkId, {
        excluded_cidrs: nextExcludedCidrs,
        scan_profile: scanProfile,
      }),
    onSuccess: (scope, variables) => {
      setScopeStates((current) => ({
        ...current,
        [variables.networkId]: {
          excludedCidrs: variables.excludedCidrs.join("\n"),
          scanProfile: variables.scanProfile,
          scope,
          appliedExcludedCidrs: variables.excludedCidrs,
          appliedScanProfile: variables.scanProfile,
          acknowledged: false,
          scanRun: null,
        },
      }));
    },
  });
  const confirmMutation = useMutation({
    mutationFn: ({
      networkId,
      targetCount,
    }: {
      networkId: string;
      targetCount: number;
    }) => confirmDiscoveryScope(networkId, targetCount),
    onSuccess: (scope, variables) => {
      setScopeStates((current) => ({
        ...current,
        [variables.networkId]: {
          ...(current[variables.networkId] ?? DEFAULT_SCOPE_STATE),
          scope,
          acknowledged: false,
          scanRun: null,
        },
      }));
    },
  });
  const scanMutation = useMutation({
    mutationFn: ({
      networkId,
      idempotencyKey,
    }: {
      networkId: string;
      idempotencyKey: string;
    }) =>
      launchNetworkScan(networkId, {
        kind: "initial_discovery",
        idempotencyKey,
      }),
    onSuccess: (scanRun, variables) => {
      setScopeStates((current) => ({
        ...current,
        [variables.networkId]: {
          ...(current[variables.networkId] ?? DEFAULT_SCOPE_STATE),
          scanRun,
        },
      }));
    },
  });

  const setSelectedState = (
    update: (current: ScopeDraftState) => ScopeDraftState,
  ) => {
    if (!selectedNetwork) return;
    setScopeStates((current) => ({
      ...current,
      [selectedNetwork.id]: update(
        current[selectedNetwork.id] ?? DEFAULT_SCOPE_STATE,
      ),
    }));
  };

  const excludedCidrs = useMemo(
    () => parseExcludedCidrs(selectedState.excludedCidrs),
    [selectedState.excludedCidrs],
  );
  const invalidExcludedCidr = excludedCidrs.find(
    (excludedCidr) => !isValidCidr(excludedCidr),
  );
  const scopeIsDirty =
    selectedState.scope !== null &&
    (selectedState.appliedExcludedCidrs?.join("\n") !==
      excludedCidrs.join("\n") ||
      selectedState.appliedScanProfile !== selectedState.scanProfile);
  const canConfirm =
    selectedState.scope !== null &&
    !scopeIsDirty &&
    selectedState.acknowledged &&
    !confirmMutation.isPending;

  if (loading) {
    return (
      <Card>
        <CardContent className="flex items-center gap-3 py-6 text-sm text-muted-foreground">
          <Spinner />
          Loading networks…
        </CardContent>
      </Card>
    );
  }
  if (error) {
    return (
      <Card>
        <CardContent className="pt-6">
          <ScopeError error={error} />
        </CardContent>
      </Card>
    );
  }

  return (
    <Card>
      <CardHeader className="border-b">
        <CardTitle>Networks</CardTitle>
        <CardDescription>
          Choose the private addresses that discovery can scan.
        </CardDescription>
        <CardAction>
          <div className="flex items-center gap-2">
            <Badge variant="outline">{networks.length} configured</Badge>
            {onAddNetwork ? (
              <Button onClick={onAddNetwork} size="sm" variant="outline">
                <PlusIcon data-icon="inline-start" />
                Add network
              </Button>
            ) : null}
          </div>
        </CardAction>
      </CardHeader>
      <CardContent className="grid gap-6 pt-5 lg:grid-cols-[minmax(13rem,0.8fr)_minmax(0,1.2fr)]">
        {networks.length === 0 ? (
          <div className="lg:col-span-2">
            <Empty>
              <EmptyHeader>
                <EmptyMedia variant="icon">
                  <NetworkIcon />
                </EmptyMedia>
                <EmptyTitle>No networks configured</EmptyTitle>
                <EmptyDescription>
                  Add a network before setting up discovery scope.
                </EmptyDescription>
                {onAddNetwork ? (
                  <Button onClick={onAddNetwork} variant="outline">
                    <PlusIcon data-icon="inline-start" />
                    Add network
                  </Button>
                ) : null}
              </EmptyHeader>
            </Empty>
          </div>
        ) : (
          <>
            <div className="flex flex-col gap-2">
              <p className="text-sm font-medium">Network boundary</p>
              <div className="flex flex-col gap-1">
                {networks.map((network) => {
                  const state = scopeStates[network.id];
                  const confirmed = state
                    ? isScopeConfirmed(
                        state.scope,
                        isScopeDirty(
                          state,
                          parseExcludedCidrs(state.excludedCidrs),
                        ),
                      )
                    : false;
                  return (
                    <Button
                      aria-pressed={network.id === selectedNetwork?.id}
                      className="h-auto justify-start px-3 py-2 text-left"
                      key={network.id}
                      onClick={() => {
                        setSelectedNetworkId(network.id);
                        draftMutation.reset();
                        confirmMutation.reset();
                        scanMutation.reset();
                      }}
                      variant={
                        network.id === selectedNetwork?.id
                          ? "secondary"
                          : "ghost"
                      }
                    >
                      <NetworkIcon data-icon="inline-start" />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate font-medium">
                          {network.name || "Unnamed network"}
                        </span>
                        <span className="block font-mono text-xs text-muted-foreground">
                          {network.cidr}
                        </span>
                      </span>
                      {confirmed ? <CheckIcon data-icon="inline-end" /> : null}
                    </Button>
                  );
                })}
              </div>
            </div>
            {selectedNetwork ? (
              <ScopeForm
                acknowledged={selectedState.acknowledged}
                canConfirm={canConfirm}
                confirmPending={confirmMutation.isPending}
                draftPending={draftMutation.isPending}
                error={draftMutation.error ?? confirmMutation.error}
                excludedCidrs={selectedState.excludedCidrs}
                invalidExcludedCidr={invalidExcludedCidr ?? null}
                isDirty={scopeIsDirty}
                network={selectedNetwork}
                scanError={
                  scanMutation.variables?.networkId === selectedNetwork.id
                    ? scanMutation.error
                    : null
                }
                scanPending={
                  scanMutation.isPending &&
                  scanMutation.variables?.networkId === selectedNetwork.id
                }
                scanRun={selectedState.scanRun}
                onAcknowledge={(acknowledged) =>
                  setSelectedState((current) => ({
                    ...current,
                    acknowledged,
                  }))
                }
                onExcludedCidrsChange={(excludedCidrs) => {
                  scanMutation.reset();
                  setSelectedState((current) => ({
                    ...current,
                    excludedCidrs,
                    acknowledged: false,
                  }));
                }}
                onProfileChange={(scanProfile) => {
                  scanMutation.reset();
                  setSelectedState((current) => ({
                    ...current,
                    scanProfile,
                    acknowledged: false,
                  }));
                }}
                onConfirm={() => {
                  if (selectedState.scope) {
                    scanMutation.reset();
                    confirmMutation.mutate({
                      networkId: selectedNetwork.id,
                      targetCount: selectedState.scope.target_count,
                    });
                  }
                }}
                onDraft={() => {
                  scanMutation.reset();
                  if (invalidExcludedCidr) return;
                  draftMutation.mutate({
                    networkId: selectedNetwork.id,
                    excludedCidrs,
                    scanProfile: selectedState.scanProfile,
                  });
                }}
                onLaunch={() => {
                  if (
                    selectedState.scope &&
                    isScopeConfirmed(selectedState.scope, scopeIsDirty)
                  ) {
                    scanMutation.mutate({
                      networkId: selectedNetwork.id,
                      idempotencyKey: createIdempotencyKey(),
                    });
                  }
                }}
                profile={selectedState.scanProfile}
                scope={selectedState.scope}
              />
            ) : null}
          </>
        )}
      </CardContent>
    </Card>
  );
}

function ScopeForm({
  network,
  scope,
  excludedCidrs,
  invalidExcludedCidr,
  profile,
  acknowledged,
  isDirty,
  canConfirm,
  draftPending,
  confirmPending,
  error,
  scanError,
  scanPending,
  scanRun,
  onExcludedCidrsChange,
  onProfileChange,
  onAcknowledge,
  onDraft,
  onConfirm,
  onLaunch,
}: {
  network: Network;
  scope: DiscoveryScope | null;
  excludedCidrs: string;
  invalidExcludedCidr: string | null;
  profile: ScanProfile;
  acknowledged: boolean;
  isDirty: boolean;
  canConfirm: boolean;
  draftPending: boolean;
  confirmPending: boolean;
  error: unknown;
  scanError: unknown;
  scanPending: boolean;
  scanRun: ScanRun | null;
  onExcludedCidrsChange: (value: string) => void;
  onProfileChange: (value: ScanProfile) => void;
  onAcknowledge: (value: boolean) => void;
  onDraft: () => void;
  onConfirm: () => void;
  onLaunch: () => void;
}) {
  const confirmed = isScopeConfirmed(scope, isDirty);
  const targetCount = scope ? formatCount(scope.target_count) : null;
  const status = confirmed
    ? "Confirmed"
    : isDirty
      ? "Needs calculation"
      : scope
        ? "Draft"
        : "Not set up";

  return (
    <form
      className="flex flex-col gap-5"
      onSubmit={(event) => {
        event.preventDefault();
        onDraft();
      }}
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <p className="font-medium">{network.name || "Unnamed network"}</p>
          <p className="font-mono text-sm text-muted-foreground">
            {network.cidr}
            {network.gateway ? ` · gateway ${network.gateway}` : ""}
          </p>
        </div>
        <Badge variant={confirmed ? "secondary" : "outline"}>{status}</Badge>
      </div>

      <FieldGroup>
        <Field>
          <FieldLabel htmlFor={`scope-exclusions-${network.id}`}>
            Excluded addresses or ranges
          </FieldLabel>
          <Textarea
            aria-invalid={Boolean(invalidExcludedCidr)}
            id={`scope-exclusions-${network.id}`}
            onChange={(event) => onExcludedCidrsChange(event.target.value)}
            placeholder="One CIDR per line, for example 192.168.1.10/32"
            rows={3}
            value={excludedCidrs}
          />
          <FieldError>
            {invalidExcludedCidr
              ? "CIDR must be a valid network range."
              : undefined}
          </FieldError>
          <FieldDescription>
            Leave blank to include every scannable address in this network.
          </FieldDescription>
        </Field>
        <Field>
          <FieldLabel htmlFor={`scope-profile-${network.id}`}>
            Scan profile
          </FieldLabel>
          <Select
            onValueChange={(value) => {
              if (value === "normal" || value === "low_impact") {
                onProfileChange(value);
              }
            }}
            value={profile}
          >
            <SelectTrigger id={`scope-profile-${network.id}`}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem value="normal">Normal</SelectItem>
                <SelectItem value="low_impact">Low impact</SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
          <FieldDescription>
            Low impact limits scan pressure for fragile devices.
          </FieldDescription>
        </Field>
      </FieldGroup>

      <div className="flex flex-wrap items-center gap-2">
        <Button
          disabled={draftPending || Boolean(invalidExcludedCidr)}
          type="submit"
          variant="outline"
        >
          {draftPending ? <Spinner data-icon="inline-start" /> : null}
          Calculate targets
        </Button>
        {isDirty ? (
          <span className="text-xs text-muted-foreground">
            Recalculate after changing scope settings.
          </span>
        ) : null}
      </div>

      {scope ? (
        <Alert>
          <NetworkIcon />
          <AlertTitle>{targetCount} scan targets</AlertTitle>
          <AlertDescription>
            {isDirty
              ? "Scope settings changed. Calculate targets again before confirming."
              : `Server calculated this count from ${network.cidr} and your exclusions. Review it before confirming.`}
          </AlertDescription>
        </Alert>
      ) : null}

      {scope && !confirmed ? (
        <Field orientation="horizontal">
          <Checkbox
            checked={acknowledged}
            id={`scope-confirm-${network.id}`}
            onCheckedChange={(checked) => onAcknowledge(checked === true)}
          />
          <FieldContent>
            <FieldLabel htmlFor={`scope-confirm-${network.id}`}>
              I reviewed the target count
            </FieldLabel>
            <FieldDescription>
              Confirm the scope to enable discovery.
            </FieldDescription>
          </FieldContent>
        </Field>
      ) : null}

      <ScopeError error={error} />

      {scope && !confirmed ? (
        <Button disabled={!canConfirm} onClick={onConfirm} type="button">
          {confirmPending ? (
            <Spinner data-icon="inline-start" />
          ) : (
            <CheckIcon data-icon="inline-start" />
          )}
          Confirm scope
        </Button>
      ) : null}
      {confirmed ? (
        <>
          <p className="flex items-center gap-2 text-sm text-muted-foreground">
            <CheckIcon className="text-primary" />
            Discovery scope confirmed for {targetCount} targets.
          </p>
          <ScanLaunch
            error={scanError}
            network={network}
            onLaunch={onLaunch}
            pending={scanPending}
            run={scanRun}
            targetCount={targetCount}
          />
        </>
      ) : null}
    </form>
  );
}

const SCAN_RUN_POLL_INTERVAL_MS = 2_500;

export function ScanLaunch({
  network,
  run,
  targetCount,
  pending,
  error,
  onLaunch,
}: {
  network: Network;
  run: ScanRun | null;
  targetCount: string | null;
  pending: boolean;
  error: unknown;
  onLaunch: () => void;
}) {
  const queryClient = useQueryClient();
  const runQuery = useQuery({
    queryKey: ["scan-run", run?.id],
    queryFn: () => fetchScanRun(run!.id),
    enabled: Boolean(run),
    refetchInterval: (query) => {
      const currentRun = query.state.data ?? run;
      return currentRun && isActiveScanRun(currentRun.status)
        ? SCAN_RUN_POLL_INTERVAL_MS
        : false;
    },
  });
  const cancelMutation = useMutation({
    mutationFn: cancelScanRun,
    onSuccess: (updatedRun) => {
      queryClient.setQueryData(["scan-run", updatedRun.id], updatedRun);
    },
  });
  const currentRun = runQuery.data ?? run;
  const canCancel = currentRun && isActiveScanRun(currentRun.status);

  return (
    <div className="flex flex-col gap-4 rounded-lg border bg-muted/20 p-4">
      <div className="flex flex-col justify-between gap-3 sm:flex-row sm:items-start">
        <div>
          <p className="font-medium">Initial discovery scan</p>
          <p className="mt-1 text-sm text-muted-foreground">
            Scan approved addresses in {network.cidr}. This launches a full TCP
            port sweep for the confirmed scope.
          </p>
        </div>
        <Button disabled={pending} onClick={onLaunch} type="button">
          {pending ? (
            <Spinner data-icon="inline-start" />
          ) : (
            <PlayIcon data-icon="inline-start" />
          )}
          {pending ? "Queueing initial discovery…" : "Launch initial discovery"}
        </Button>
      </div>

      {pending ? (
        <p aria-live="polite" className="text-sm text-muted-foreground">
          Submitting scan request for confirmed scope. The server will queue
          work only for approved targets.
        </p>
      ) : null}
      {error ? <ScanLaunchError error={error} /> : null}
      {currentRun ? (
        <ScanRunStatus run={currentRun} targetCount={targetCount} />
      ) : null}
      {canCancel ? (
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="text-xs text-muted-foreground">
            {currentRun.cancellation_requested
              ? "Cancellation requested. Waiting for the worker to stop."
              : "You can cancel this scan while it is queued or running."}
          </p>
          <Button
            disabled={
              cancelMutation.isPending || currentRun.cancellation_requested
            }
            onClick={() => cancelMutation.mutate(currentRun.id)}
            type="button"
            variant="outline"
          >
            {cancelMutation.isPending ? (
              <Spinner data-icon="inline-start" />
            ) : (
              <XIcon data-icon="inline-start" />
            )}
            {cancelMutation.isPending
              ? "Requesting cancellation…"
              : currentRun.cancellation_requested
                ? "Cancellation requested"
                : "Cancel scan"}
          </Button>
        </div>
      ) : null}
      {cancelMutation.error ? (
        <Alert variant="destructive">
          <CircleAlertIcon />
          <AlertTitle>Scan cancellation failed</AlertTitle>
          <AlertDescription>
            {cancelMutation.error instanceof Error
              ? cancelMutation.error.message
              : "Server returned no cancellation error details."}
          </AlertDescription>
        </Alert>
      ) : null}
      {runQuery.error ? (
        <Alert variant="destructive">
          <CircleAlertIcon />
          <AlertTitle>Scan status update failed</AlertTitle>
          <AlertDescription>
            {runQuery.error instanceof Error
              ? runQuery.error.message
              : "Server returned no scan status error details."}
          </AlertDescription>
        </Alert>
      ) : null}
    </div>
  );
}

function ScanRunStatus({
  run,
  targetCount,
}: {
  run: ScanRun;
  targetCount: string | null;
}) {
  const completedTargets = formatCount(run.targets_completed);
  const plannedTargets = targetCount ?? formatCount(run.targets_planned);
  const portProgress = `${formatCount(run.ports_completed)} of ${formatCount(run.ports_planned)} ports completed.`;

  switch (run.status) {
    case "pending":
      return (
        <Alert aria-live="polite">
          <NetworkIcon />
          <AlertTitle>Initial discovery queued</AlertTitle>
          <AlertDescription>
            Server accepted scan for {plannedTargets} confirmed targets. Work
            has not started yet. {portProgress}
          </AlertDescription>
        </Alert>
      );
    case "running":
      return (
        <Alert aria-live="polite">
          <NetworkIcon />
          <AlertTitle>Initial discovery running</AlertTitle>
          <AlertDescription>
            {completedTargets} of {plannedTargets} confirmed targets completed.{" "}
            {portProgress}
          </AlertDescription>
        </Alert>
      );
    case "succeeded":
      return (
        <Alert aria-live="polite">
          <CheckIcon />
          <AlertTitle>Initial discovery completed</AlertTitle>
          <AlertDescription>
            {run.authoritative && run.complete
              ? `Full TCP scan completed for ${plannedTargets} confirmed targets. Results are authoritative.`
              : "Server reported success without a complete authoritative result."}
          </AlertDescription>
        </Alert>
      );
    case "failed":
      return (
        <Alert aria-live="polite" variant="destructive">
          <CircleAlertIcon />
          <AlertTitle>Initial discovery failed</AlertTitle>
          <AlertDescription>
            {run.error ??
              "Server marked scan run failed without an error message."}{" "}
            {portProgress} Partial results are not authoritative.
          </AlertDescription>
        </Alert>
      );
    case "cancelled":
      return (
        <Alert aria-live="polite" variant="destructive">
          <CircleAlertIcon />
          <AlertTitle>Initial discovery cancelled</AlertTitle>
          <AlertDescription>
            Scan stopped after {completedTargets} of {plannedTargets} confirmed
            targets completed. {portProgress} Partial results are not
            authoritative.
          </AlertDescription>
        </Alert>
      );
  }
}

function isActiveScanRun(status: ScanRun["status"]): boolean {
  return status === "pending" || status === "running";
}

function ScanLaunchError({ error }: { error: unknown }) {
  return (
    <Alert variant="destructive">
      <CircleAlertIcon />
      <AlertTitle>Initial discovery launch failed</AlertTitle>
      <AlertDescription>
        {error instanceof Error
          ? error.message
          : "Server rejected initial discovery launch without an error message."}
      </AlertDescription>
    </Alert>
  );
}

function ScopeError({ error }: { error: unknown }) {
  if (!error) return null;
  return (
    <Alert variant="destructive">
      <CircleAlertIcon />
      <AlertTitle>Scope action failed</AlertTitle>
      <AlertDescription>
        {error instanceof Error
          ? error.message
          : "Scope server returned no error details."}
      </AlertDescription>
    </Alert>
  );
}

function parseExcludedCidrs(value: string): string[] {
  return value
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
}

function isScopeDirty(
  state: ScopeDraftState,
  excludedCidrs: string[],
): boolean {
  return (
    state.scope !== null &&
    (state.appliedExcludedCidrs?.join("\n") !== excludedCidrs.join("\n") ||
      state.appliedScanProfile !== state.scanProfile)
  );
}

function isScopeConfirmed(
  scope: DiscoveryScope | null,
  isDirty: boolean,
): boolean {
  return Boolean(
    scope &&
    !isDirty &&
    scope.enabled &&
    scope.confirmed_at &&
    scope.confirmed_target_count === scope.target_count,
  );
}

function createIdempotencyKey(): string {
  if (typeof globalThis.crypto?.randomUUID === "function") {
    return globalThis.crypto.randomUUID();
  }
  return `scan-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function formatCount(value: number): string {
  return new Intl.NumberFormat().format(value);
}
