import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  CheckIcon,
  CircleAlertIcon,
  NetworkIcon,
  PencilIcon,
  PlusIcon,
  PlayIcon,
  Trash2Icon,
  XIcon,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  cancelScanRun,
  confirmDiscoveryScope,
  draftDiscoveryScope,
  fetchDiscoveryState,
  fetchNetworkScans,
  fetchScanRun,
  getUserFacingError,
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
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import {
  Field,
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
const SCAN_RUN_POLL_INTERVAL_MS = 2_500;

interface ScopeDraftState {
  excludedCidrs: string;
  scanProfile: ScanProfile;
  scope: DiscoveryScope | null;
  appliedExcludedCidrs: string[] | null;
  appliedScanProfile: ScanProfile | null;
  scanRun: ScanRun | null;
}

const DEFAULT_SCOPE_STATE: ScopeDraftState = {
  excludedCidrs: "",
  scanProfile: "normal",
  scope: null,
  appliedExcludedCidrs: null,
  appliedScanProfile: null,
  scanRun: null,
};

export function DiscoveryScopeSetup({
  networks,
  loading,
  error,
  actionError,
  onAddNetwork,
  onDeleteNetwork,
  onEditNetwork,
}: {
  networks: Network[];
  loading: boolean;
  error: unknown;
  actionError?: unknown;
  onAddNetwork?: () => void;
  onDeleteNetwork?: (network: Network) => void;
  onEditNetwork?: (network: Network) => void;
}) {
  const [selectedNetworkId, setSelectedNetworkId] = useState<string | null>(
    null,
  );
  const [scopeStates, setScopeStates] = useState<
    Record<string, ScopeDraftState>
  >({});
  const hydratedNetworkIds = useRef(new Set<string>());
  const launchKeysByNetwork = useRef(new Map<string, string>());
  const selectedNetwork =
    networks.find((network) => network.id === selectedNetworkId) ??
    networks[0] ??
    null;
  const selectedState = selectedNetwork
    ? (scopeStates[selectedNetwork.id] ?? DEFAULT_SCOPE_STATE)
    : DEFAULT_SCOPE_STATE;
  const discoveryStateQuery = useQuery({
    queryKey: ["discovery-state", selectedNetwork?.id],
    queryFn: () => fetchDiscoveryState(selectedNetwork!.id),
    enabled: Boolean(selectedNetwork),
  });
  const scansQuery = useQuery({
    queryKey: ["network-scans", selectedNetwork?.id],
    queryFn: () => fetchNetworkScans(selectedNetwork!.id),
    enabled: Boolean(selectedNetwork),
    refetchInterval: (query) => {
      const active = query.state.data?.active_scan;
      return active && isActiveScanRun(active.status)
        ? SCAN_RUN_POLL_INTERVAL_MS
        : false;
    },
  });

  useEffect(() => {
    if (!selectedNetwork || !discoveryStateQuery.data) return;
    const networkId = selectedNetwork.id;
    if (hydratedNetworkIds.current.has(networkId)) return;
    hydratedNetworkIds.current.add(networkId);
    const persistedScope = discoveryStateQuery.data.scope;
    const persistedProfile: ScanProfile =
      persistedScope?.scan_profile === "low_impact" ? "low_impact" : "normal";
    setScopeStates((current) => {
      if (current[networkId]) return current;
      return {
        ...current,
        [networkId]: persistedScope
          ? {
              excludedCidrs: persistedScope.excluded_cidrs.join("\n"),
              scanProfile: persistedProfile,
              scope: persistedScope,
              appliedExcludedCidrs: persistedScope.excluded_cidrs,
              appliedScanProfile: persistedProfile,
              scanRun: discoveryStateQuery.data.scan_run,
            }
          : DEFAULT_SCOPE_STATE,
      };
    });
  }, [discoveryStateQuery.data, selectedNetwork]);

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
      launchKeysByNetwork.current.delete(variables.networkId);
      setScopeStates((current) => ({
        ...current,
        [variables.networkId]: {
          excludedCidrs: variables.excludedCidrs.join("\n"),
          scanProfile: variables.scanProfile,
          scope,
          appliedExcludedCidrs: variables.excludedCidrs,
          appliedScanProfile: variables.scanProfile,
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
      launchKeysByNetwork.current.delete(variables.networkId);
      setScopeStates((current) => {
        return {
          ...current,
          [variables.networkId]: {
            ...(current[variables.networkId] ?? DEFAULT_SCOPE_STATE),
            scanRun,
          },
        };
      });
    },
  });
  const confirmMutation = useMutation({
    mutationFn: ({
      networkId,
      targetCount,
    }: {
      networkId: string;
      targetCount: number;
      launch: boolean;
    }) => confirmDiscoveryScope(networkId, targetCount),
    onSuccess: (scope, variables) => {
      setScopeStates((current) => ({
        ...current,
        [variables.networkId]: {
          ...(current[variables.networkId] ?? DEFAULT_SCOPE_STATE),
          scope,
          scanRun: null,
        },
      }));
      if (variables.launch) {
        const idempotencyKey =
          launchKeysByNetwork.current.get(variables.networkId) ??
          createIdempotencyKey();
        launchKeysByNetwork.current.set(variables.networkId, idempotencyKey);
        scanMutation.mutate({
          networkId: variables.networkId,
          idempotencyKey,
        });
      }
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
    Boolean(selectedNetwork) &&
    !invalidExcludedCidr &&
    !draftMutation.isPending &&
    !confirmMutation.isPending &&
    !scanMutation.isPending;
  const saveScope = (launch: boolean) => {
    if (!selectedNetwork || !canConfirm) return;
    const networkId = selectedNetwork.id;
    const confirmScope = (scope: DiscoveryScope) => {
      confirmMutation.mutate({
        networkId,
        targetCount: scope.target_count,
        launch,
      });
    };

    if (selectedState.scope && !scopeIsDirty) {
      confirmScope(selectedState.scope);
      return;
    }

    draftMutation.mutate(
      {
        networkId,
        excludedCidrs,
        scanProfile: selectedState.scanProfile,
      },
      { onSuccess: confirmScope },
    );
  };
  const scanHistory = scansQuery.data?.items ?? [];
  const activeScan =
    scansQuery.data?.active_scan ??
    scanHistory.find((run) => isActiveScanRun(run.status)) ??
    null;
  const selectedScanRun = activeScan ?? scanHistory[0] ?? selectedState.scanRun;

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
      {actionError ? (
        <CardContent className="pb-0">
          <ScopeError error={actionError} />
        </CardContent>
      ) : null}
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
                  const networkLabel = network.name || "Unnamed network";
                  return (
                    <div className="flex items-center gap-1" key={network.id}>
                      <Button
                        aria-pressed={network.id === selectedNetwork?.id}
                        className="h-auto min-w-0 flex-1 justify-start px-3 py-2 text-left"
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
                            {networkLabel}
                          </span>
                          <span className="block font-mono text-xs text-muted-foreground">
                            {network.cidr}
                          </span>
                        </span>
                        {confirmed ? (
                          <CheckIcon data-icon="inline-end" />
                        ) : null}
                      </Button>
                      {onEditNetwork ? (
                        <Button
                          aria-label={`Edit ${networkLabel}`}
                          onClick={() => onEditNetwork(network)}
                          size="icon-sm"
                          type="button"
                          variant="ghost"
                        >
                          <PencilIcon />
                        </Button>
                      ) : null}
                      {onDeleteNetwork ? (
                        <Button
                          aria-label={`Delete ${networkLabel}`}
                          onClick={() => onDeleteNetwork(network)}
                          size="icon-sm"
                          type="button"
                          variant="ghost"
                        >
                          <Trash2Icon />
                        </Button>
                      ) : null}
                    </div>
                  );
                })}
              </div>
            </div>
            {selectedNetwork ? (
              <ScopeForm
                canConfirm={canConfirm}
                confirmPending={confirmMutation.isPending}
                draftPending={draftMutation.isPending}
                error={
                  discoveryStateQuery.error ??
                  draftMutation.error ??
                  confirmMutation.error
                }
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
                scanHistory={scanHistory}
                scanRun={selectedScanRun}
                stateLoading={discoveryStateQuery.isLoading}
                onExcludedCidrsChange={(excludedCidrs) => {
                  scanMutation.reset();
                  setSelectedState((current) => ({
                    ...current,
                    excludedCidrs,
                  }));
                }}
                onProfileChange={(scanProfile) => {
                  scanMutation.reset();
                  setSelectedState((current) => ({
                    ...current,
                    scanProfile,
                  }));
                }}
                onConfirm={() => {
                  scanMutation.reset();
                  saveScope(false);
                }}
                onConfirmAndLaunch={() => {
                  scanMutation.reset();
                  saveScope(true);
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
  isDirty,
  canConfirm,
  draftPending,
  confirmPending,
  error,
  scanError,
  scanPending,
  scanHistory,
  scanRun,
  stateLoading,
  onExcludedCidrsChange,
  onProfileChange,
  onConfirm,
  onConfirmAndLaunch,
  onLaunch,
}: {
  network: Network;
  scope: DiscoveryScope | null;
  excludedCidrs: string;
  invalidExcludedCidr: string | null;
  profile: ScanProfile;
  isDirty: boolean;
  canConfirm: boolean;
  draftPending: boolean;
  confirmPending: boolean;
  error: unknown;
  scanError: unknown;
  scanPending: boolean;
  scanHistory: ScanRun[];
  scanRun: ScanRun | null;
  stateLoading: boolean;
  onExcludedCidrsChange: (value: string) => void;
  onProfileChange: (value: ScanProfile) => void;
  onConfirm: () => void;
  onConfirmAndLaunch: () => void;
  onLaunch: () => void;
}) {
  const confirmed = isScopeConfirmed(scope, isDirty);
  const status = confirmed
    ? "Confirmed"
    : isDirty
      ? "Changes not saved"
      : scope
        ? "Ready to enable"
        : "Not set up";
  const exclusionCount = parseExcludedCidrs(excludedCidrs).length;
  const optionsSummary = `${profile === "low_impact" ? "Low impact" : "Normal"} · ${
    exclusionCount
      ? `${exclusionCount} exclusion${exclusionCount === 1 ? "" : "s"}`
      : "no exclusions"
  }`;

  return (
    <form
      className="flex flex-col gap-5"
      onSubmit={(event) => {
        event.preventDefault();
        onConfirm();
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

      <details className="rounded-lg border" open={scope !== null}>
        <summary className="flex cursor-pointer list-none items-center justify-between gap-3 px-4 py-3 outline-none focus-visible:ring-3 focus-visible:ring-ring/50">
          <span className="font-medium">Scan options</span>
          <span className="text-right text-sm text-muted-foreground">
            {optionsSummary}
          </span>
        </summary>
        <div className="border-t p-4">
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor={`scope-exclusions-${network.id}`}>
                Excluded addresses or ranges
              </FieldLabel>
              <Textarea
                aria-invalid={Boolean(invalidExcludedCidr)}
                disabled={stateLoading}
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
                disabled={stateLoading}
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
        </div>
      </details>

      {stateLoading ? (
        <p aria-live="polite" className="text-sm text-muted-foreground">
          Loading saved discovery state…
        </p>
      ) : null}

      <ScopeError error={error} />

      {!confirmed ? (
        <div className="flex flex-wrap gap-2">
          <Button
            disabled={stateLoading || !canConfirm}
            onClick={onConfirmAndLaunch}
            type="button"
          >
            {draftPending || confirmPending || scanPending ? (
              <Spinner data-icon="inline-start" />
            ) : (
              <PlayIcon data-icon="inline-start" />
            )}
            {draftPending
              ? "Saving scope…"
              : confirmPending
                ? "Enabling discovery…"
                : scanPending
                  ? "Queueing initial discovery…"
                  : "Save and launch initial discovery"}
          </Button>
          <Button
            disabled={stateLoading || !canConfirm}
            type="submit"
            variant="outline"
          >
            <CheckIcon data-icon="inline-start" />
            Enable discovery
          </Button>
        </div>
      ) : null}
      {confirmed ? (
        <>
          <p className="flex items-center gap-2 text-sm text-muted-foreground">
            <CheckIcon className="text-primary" />
            Discovery scope confirmed.
          </p>
          <ScanLaunch
            error={scanError}
            network={network}
            onLaunch={onLaunch}
            pending={scanPending}
            run={scanRun}
            history={scanHistory}
          />
        </>
      ) : null}
    </form>
  );
}

export function ScanLaunch({
  network,
  run,
  history = [],
  pending,
  error,
  onLaunch,
}: {
  network: Network;
  run: ScanRun | null;
  history?: ScanRun[];
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
          <p className="font-medium">Network scan</p>
          <p className="mt-1 text-sm text-muted-foreground">
            Scan approved addresses in {network.cidr} using the standard
            discovery profile.
          </p>
        </div>
        <Button
          disabled={pending || Boolean(canCancel)}
          onClick={onLaunch}
          type="button"
        >
          {pending ? (
            <Spinner data-icon="inline-start" />
          ) : (
            <PlayIcon data-icon="inline-start" />
          )}
          {pending ? "Queueing scan…" : "Scan standard ports"}
        </Button>
      </div>

      {pending ? (
        <p aria-live="polite" className="text-sm text-muted-foreground">
          Submitting scan request for confirmed scope. The server will queue
          work only for approved targets.
        </p>
      ) : null}
      {error ? <ScanLaunchError error={error} /> : null}
      {currentRun ? <ScanRunStatus run={currentRun} /> : null}
      {currentRun?.job ? <ScanJobDetails run={currentRun} /> : null}
      {history.length > 1 ? <ScanHistory history={history} /> : null}
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
            {getUserFacingError(
              cancelMutation.error,
              "The scan cancellation request failed.",
            )}
          </AlertDescription>
        </Alert>
      ) : null}
      {runQuery.error ? (
        <Alert variant="destructive">
          <CircleAlertIcon />
          <AlertTitle>Scan status update failed</AlertTitle>
          <AlertDescription>
            {getUserFacingError(
              runQuery.error,
              "The scan status could not be refreshed.",
            )}
          </AlertDescription>
        </Alert>
      ) : null}
    </div>
  );
}

function ScanJobDetails({ run }: { run: ScanRun }) {
  const job = run.job;
  if (!job) return null;
  const progress = job.progress;
  const progressTargets = progress.targets_completed;
  const progressPorts = progress.ports_completed;
  return (
    <details aria-label="Scan job details" className="rounded-lg border p-3">
      <summary className="cursor-pointer text-sm font-medium">
        Technical details and activity log
      </summary>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <p className="text-sm font-medium">Job activity</p>
          <p className="font-mono text-xs text-muted-foreground">{job.id}</p>
        </div>
        <Badge variant={job.status === "failed" ? "destructive" : "outline"}>
          {job.status}
        </Badge>
      </div>
      <dl className="mt-3 grid gap-2 text-xs text-muted-foreground sm:grid-cols-3">
        <div>
          <dt>Attempts</dt>
          <dd className="font-medium text-foreground">
            {job.attempts} of {job.max_attempts}
          </dd>
        </div>
        <div>
          <dt>Worker progress</dt>
          <dd className="font-medium text-foreground">
            {typeof progressTargets === "number"
              ? `${formatCount(progressTargets)} targets`
              : typeof progressPorts === "number"
                ? `${formatCount(progressPorts)} ports`
                : "No progress reported"}
          </dd>
        </div>
        <div>
          <dt>Last updated</dt>
          <dd className="font-medium text-foreground">
            {formatScanTime(job.updated_at)}
          </dd>
        </div>
      </dl>
      {job.last_error ? (
        <p className="mt-3 text-xs text-destructive">
          {getUserFacingError(
            job.last_error,
            "The scan job reported an error.",
          )}
        </p>
      ) : null}
      {run.logs?.length ? (
        <div className="mt-3 border-t pt-3">
          <p className="text-sm font-medium">Activity ({run.logs.length})</p>
          <ol className="mt-2 flex flex-col gap-2 text-xs text-muted-foreground">
            {run.logs.map((log) => (
              <li className="flex flex-wrap gap-x-2 gap-y-1" key={log.id}>
                <time dateTime={log.occurred_at}>
                  {formatScanTime(log.occurred_at)}
                </time>
                <span className="font-medium text-foreground">
                  {scanLogLabel(log.action)}
                </span>
                <span>{log.result}</span>
              </li>
            ))}
          </ol>
        </div>
      ) : (
        <p className="mt-3 border-t pt-3 text-xs text-muted-foreground">
          No activity events have been recorded for this job yet.
        </p>
      )}
    </details>
  );
}

function scanLogLabel(action: string): string {
  switch (action) {
    case "scan_run.create":
      return "Scan queued";
    case "scan_run.cancel":
      return "Cancellation requested";
    default:
      return action.replaceAll("_", " ").replaceAll(".", ": ");
  }
}

function ScanHistory({ history }: { history: ScanRun[] }) {
  return (
    <div aria-label="Recent scan jobs" className="rounded-lg border p-3">
      <div className="flex items-center justify-between gap-2">
        <p className="text-sm font-medium">Recent scan jobs</p>
        <Badge variant="outline">{history.length}</Badge>
      </div>
      <div className="mt-3 divide-y text-sm">
        {history.map((run) => (
          <div
            className="flex flex-wrap items-center justify-between gap-2 py-2 first:pt-0 last:pb-0"
            key={run.id}
          >
            <div>
              <p className="font-medium">
                {scanKindLabel(run.kind)}
                {run.target_address ? ` · ${run.target_address}` : ""}
              </p>
              <p className="text-xs text-muted-foreground">
                {formatScanTime(run.created_at)} ·{" "}
                {formatCount(run.targets_completed)} of{" "}
                {formatCount(run.targets_planned)} targets
              </p>
            </div>
            <Badge
              variant={run.status === "failed" ? "destructive" : "outline"}
            >
              {run.status}
            </Badge>
          </div>
        ))}
      </div>
    </div>
  );
}

function ScanRunStatus({ run }: { run: ScanRun }) {
  const completedTargets = formatCount(run.targets_completed);
  const plannedTargets = formatCount(run.targets_planned);
  const portProgress = `${formatCount(run.ports_completed)} of ${formatCount(run.ports_planned)} ports completed.`;
  const scanLabel = run.kind === "full_tcp" ? "Full TCP scan" : "Standard scan";
  const targetDescription = run.target_address
    ? `Device address ${run.target_address}. `
    : "";

  switch (run.status) {
    case "pending":
      return (
        <Alert aria-live="polite">
          <NetworkIcon />
          <AlertTitle>{scanLabel} queued</AlertTitle>
          <AlertDescription>
            {targetDescription}Server accepted scan for {plannedTargets}{" "}
            confirmed targets. Work has not started yet. {portProgress}
          </AlertDescription>
        </Alert>
      );
    case "running":
      return (
        <Alert aria-live="polite">
          <NetworkIcon />
          <AlertTitle>{scanLabel} running</AlertTitle>
          <AlertDescription>
            {targetDescription}
            {completedTargets} of {plannedTargets} confirmed targets completed.{" "}
            {portProgress}
          </AlertDescription>
        </Alert>
      );
    case "succeeded":
      return (
        <Alert aria-live="polite">
          <CheckIcon />
          <AlertTitle>{scanLabel} completed</AlertTitle>
          <AlertDescription>
            {run.authoritative && run.complete
              ? `${targetDescription}${plannedTargets} confirmed targets scanned. Results cover scanned ports.`
              : "Server reported success without a complete authoritative result."}
          </AlertDescription>
        </Alert>
      );
    case "failed":
      return (
        <Alert aria-live="polite" variant="destructive">
          <CircleAlertIcon />
          <AlertTitle>{scanLabel} failed</AlertTitle>
          <AlertDescription>
            {getUserFacingError(
              run.error,
              "The server marked this scan as failed without more detail.",
            )}{" "}
            {portProgress} Partial results are not authoritative.
          </AlertDescription>
        </Alert>
      );
    case "cancelled":
      return (
        <Alert aria-live="polite" variant="destructive">
          <CircleAlertIcon />
          <AlertTitle>{scanLabel} cancelled</AlertTitle>
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
        {getUserFacingError(
          error,
          "The server rejected the initial discovery launch.",
        )}
      </AlertDescription>
    </Alert>
  );
}

function formatScanTime(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.valueOf())
    ? value
    : date.toLocaleString(undefined, {
        dateStyle: "medium",
        timeStyle: "short",
      });
}

function scanKindLabel(kind: ScanRun["kind"]): string {
  return kind
    .split("_")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function ScopeError({ error }: { error: unknown }) {
  if (!error) return null;
  return (
    <Alert variant="destructive">
      <CircleAlertIcon />
      <AlertTitle>Scope action failed</AlertTitle>
      <AlertDescription>
        {getUserFacingError(error, "The discovery scope action failed.")}
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
