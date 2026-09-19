import { useMutation } from "@tanstack/react-query";
import { CheckIcon, CircleAlertIcon, NetworkIcon } from "lucide-react";
import { useMemo, useState } from "react";
import {
  confirmDiscoveryScope,
  draftDiscoveryScope,
  type DiscoveryScope,
  type Network,
} from "@/lib/api";
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
}

const DEFAULT_SCOPE_STATE: ScopeDraftState = {
  excludedCidrs: "",
  scanProfile: "normal",
  scope: null,
  appliedExcludedCidrs: null,
  appliedScanProfile: null,
  acknowledged: false,
};

export function DiscoveryScopeSetup({
  networks,
  loading,
  error,
}: {
  networks: Network[];
  loading: boolean;
  error: unknown;
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
          Approve exactly which private network addresses discovery may scan.
        </CardDescription>
        <CardAction>
          <Badge variant="outline">{networks.length} configured</Badge>
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
                  const confirmed =
                    state?.scope?.confirmed_at &&
                    !isScopeDirty(
                      state,
                      parseExcludedCidrs(state.excludedCidrs),
                    );
                  return (
                    <Button
                      aria-pressed={network.id === selectedNetwork?.id}
                      className="h-auto justify-start px-3 py-2 text-left"
                      key={network.id}
                      onClick={() => {
                        setSelectedNetworkId(network.id);
                        draftMutation.reset();
                        confirmMutation.reset();
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
                isDirty={scopeIsDirty}
                network={selectedNetwork}
                onAcknowledge={(acknowledged) =>
                  setSelectedState((current) => ({
                    ...current,
                    acknowledged,
                  }))
                }
                onExcludedCidrsChange={(excludedCidrs) =>
                  setSelectedState((current) => ({
                    ...current,
                    excludedCidrs,
                    acknowledged: false,
                  }))
                }
                onProfileChange={(scanProfile) =>
                  setSelectedState((current) => ({
                    ...current,
                    scanProfile,
                    acknowledged: false,
                  }))
                }
                onConfirm={() => {
                  if (selectedState.scope) {
                    confirmMutation.mutate({
                      networkId: selectedNetwork.id,
                      targetCount: selectedState.scope.target_count,
                    });
                  }
                }}
                onDraft={() => {
                  draftMutation.mutate({
                    networkId: selectedNetwork.id,
                    excludedCidrs,
                    scanProfile: selectedState.scanProfile,
                  });
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
  profile,
  acknowledged,
  isDirty,
  canConfirm,
  draftPending,
  confirmPending,
  error,
  onExcludedCidrsChange,
  onProfileChange,
  onAcknowledge,
  onDraft,
  onConfirm,
}: {
  network: Network;
  scope: DiscoveryScope | null;
  excludedCidrs: string;
  profile: ScanProfile;
  acknowledged: boolean;
  isDirty: boolean;
  canConfirm: boolean;
  draftPending: boolean;
  confirmPending: boolean;
  error: unknown;
  onExcludedCidrsChange: (value: string) => void;
  onProfileChange: (value: ScanProfile) => void;
  onAcknowledge: (value: boolean) => void;
  onDraft: () => void;
  onConfirm: () => void;
}) {
  const confirmed = Boolean(scope?.confirmed_at) && !isDirty;
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
            id={`scope-exclusions-${network.id}`}
            onChange={(event) => onExcludedCidrsChange(event.target.value)}
            placeholder="One CIDR per line, for example 192.168.1.10/32"
            rows={3}
            value={excludedCidrs}
          />
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
        <Button disabled={draftPending} type="submit" variant="outline">
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
              Discovery stays blocked until you confirm this scope.
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
        <p className="flex items-center gap-2 text-sm text-muted-foreground">
          <CheckIcon className="text-primary" />
          Discovery scope confirmed for {targetCount} targets.
        </p>
      ) : null}
    </form>
  );
}

function ScopeError({ error }: { error: unknown }) {
  if (!error) return null;
  return (
    <Alert variant="destructive">
      <CircleAlertIcon />
      <AlertTitle>Scope action failed</AlertTitle>
      <AlertDescription>
        {error instanceof Error ? error.message : "Try again."}
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

function formatCount(value: number): string {
  return new Intl.NumberFormat().format(value);
}
