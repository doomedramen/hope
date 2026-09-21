import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import {
  CheckIcon,
  CircleAlertIcon,
  GitBranchIcon,
  GitMergeIcon,
  NetworkIcon,
  PencilIcon,
  PlusIcon,
  SearchIcon,
  ServerIcon,
  ShieldCheckIcon,
  Undo2Icon,
  XIcon,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { AgentDeploymentDialog } from "@/components/AgentDeploymentDialog";
import {
  createDevice,
  fetchAddresses,
  fetchDevice,
  fetchDevices,
  fetchIdentitySuggestions,
  mergeDevice,
  patchDevice,
  resolveIdentitySuggestion,
  splitDevice,
  undoMerge,
  ApiError,
  type Address,
  type Device,
  type DeviceDetail,
  type IdentitySuggestion,
  type Network,
} from "@/lib/api";
import { displayValue, labelize, shortId } from "@/lib/format";
import {
  validateNetworkFields,
  type NetworkField,
} from "@/lib/network-validation";
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
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import {
  Field,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Progress,
  ProgressLabel,
  ProgressValue,
} from "@/components/ui/progress";
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

export const Route = createFileRoute("/infrastructure")({
  validateSearch: (
    search: Record<string, unknown>,
  ): { device?: string; focus?: "search"; q?: string } => {
    const device = typeof search.device === "string" ? search.device : null;
    const focus = search.focus === "search" ? "search" : undefined;
    const q = typeof search.q === "string" ? search.q : null;
    return {
      ...(device ? { device } : {}),
      ...(focus ? { focus } : {}),
      ...(q ? { q } : {}),
    };
  },
  component: InfrastructurePage,
});

const EMPTY_DEVICES: Device[] = [];
const EMPTY_SUGGESTIONS: IdentitySuggestion[] = [];
const EMPTY_ADDRESSES: Address[] = [];

export function InfrastructurePage() {
  const queryClient = useQueryClient();
  const [requestedDeviceId, setRequestedDeviceId] = useState<string | null>(
    () =>
      typeof window === "undefined"
        ? null
        : new URLSearchParams(window.location.search).get("device"),
  );
  const [search, setSearch] = useState(() =>
    typeof window === "undefined"
      ? ""
      : (new URLSearchParams(window.location.search).get("q") ?? ""),
  );
  const searchInputRef = useRef<HTMLInputElement>(null);
  const shouldFocusSearch =
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).get("focus") === "search";
  const [dialog, setDialog] = useState<
    "create" | "edit" | "merge" | "split" | "agent-install" | null
  >(null);
  const devicesQuery = useQuery({
    queryKey: ["devices"],
    queryFn: fetchDevices,
  });
  const suggestionsQuery = useQuery({
    queryKey: ["identity-suggestions"],
    queryFn: fetchIdentitySuggestions,
  });
  const devices = devicesQuery.data?.items ?? EMPTY_DEVICES;
  const visibleDevices = useMemo(() => {
    const needle = search.trim().toLowerCase();
    if (!needle) return devices;
    return devices.filter((device) =>
      [device.name, device.device_type, device.status, device.id].some(
        (value) => value?.toLowerCase().includes(needle),
      ),
    );
  }, [devices, search]);

  useEffect(() => {
    if (shouldFocusSearch) searchInputRef.current?.focus();
  }, [shouldFocusSearch]);

  const selectedId = visibleDevices.some(
    (device) => device.id === requestedDeviceId,
  )
    ? requestedDeviceId
    : (visibleDevices[0]?.id ?? null);

  const selected = devices.find((device) => device.id === selectedId) ?? null;
  const detailQuery = useQuery({
    queryKey: ["device", selectedId],
    queryFn: () => fetchDevice(selectedId!),
    enabled: Boolean(selectedId),
  });
  const addressesQuery = useQuery({
    queryKey: ["addresses"],
    queryFn: fetchAddresses,
    enabled: Boolean(detailQuery.data),
  });

  const invalidateInventory = () =>
    Promise.all([
      queryClient.invalidateQueries({ queryKey: ["devices"] }),
      queryClient.invalidateQueries({ queryKey: ["device", selectedId] }),
      queryClient.invalidateQueries({ queryKey: ["addresses"] }),
    ]);

  const createMutation = useMutation({
    mutationFn: createDevice,
    onSuccess: async (device) => {
      await invalidateInventory();
      setRequestedDeviceId(device.id);
      setDialog(null);
    },
  });
  const editMutation = useMutation({
    mutationFn: ({
      id,
      version,
      name,
      status,
    }: {
      id: string;
      version: number;
      name: string;
      status: string;
    }) => patchDevice(id, { version, name, status }),
    onSuccess: async () => {
      await invalidateInventory();
      setDialog(null);
    },
  });
  const mergeMutation = useMutation({
    mutationFn: ({
      absorbedId,
      into,
      reason,
    }: {
      absorbedId: string;
      into: string;
      reason: string;
    }) => mergeDevice(absorbedId, { into, reason }),
    onSuccess: async () => {
      await invalidateInventory();
      setDialog(null);
    },
  });
  const splitMutation = useMutation({
    mutationFn: ({
      sourceId,
      interfaceIds,
      name,
      deviceType,
    }: {
      sourceId: string;
      interfaceIds: string[];
      name: string;
      deviceType: string;
    }) =>
      splitDevice(sourceId, {
        interface_ids: interfaceIds,
        name: name || undefined,
        device_type: deviceType,
      }),
    onSuccess: async () => {
      await invalidateInventory();
      setDialog(null);
    },
  });
  const undoMutation = useMutation({
    mutationFn: undoMerge,
    onSuccess: invalidateInventory,
  });
  const resolveSuggestionMutation = useMutation({
    mutationFn: ({
      id,
      decision,
    }: {
      id: string;
      decision: "confirm" | "reject";
    }) => resolveIdentitySuggestion(id, decision),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: ["identity-suggestions"] }),
  });

  const currentAddresses =
    addressesQuery.data?.items.filter((address) => address.is_current).length ??
    0;
  const staleDevices = devices.filter(
    (device) => device.status === "stale",
  ).length;
  const suggestions = suggestionsQuery.data?.items ?? EMPTY_SUGGESTIONS;

  return (
    <div className="mx-auto flex max-w-7xl flex-col gap-6">
      <div className="flex flex-col justify-between gap-4 md:flex-row md:items-end">
        <div>
          <h1 className="text-3xl font-semibold tracking-tight">Devices</h1>
          <p className="mt-2 max-w-2xl text-sm text-muted-foreground">
            Browse device identity, interfaces, address history, and review work
            in one focused inventory view.
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button onClick={() => setDialog("create")}>
            <PlusIcon data-icon="inline-start" />
            Add device
          </Button>
        </div>
      </div>

      <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <MetricCard
          icon={<ServerIcon />}
          label="Devices"
          value={devices.length}
          detail="Device records"
        />
        <MetricCard
          icon={<NetworkIcon />}
          label="Current IPs"
          value={currentAddresses}
          detail="Current IP addresses"
        />
        <MetricCard
          icon={<CircleAlertIcon />}
          label="Stale devices"
          value={staleDevices}
          detail={staleDevices ? "Needs review" : "No stale devices"}
        />
        <MetricCard
          icon={<ShieldCheckIcon />}
          label="Review queue"
          value={suggestions.length}
          detail="Ambiguous identity matches"
        />
      </div>

      <div className="grid gap-6 xl:grid-cols-[minmax(0,1fr)_25rem]">
        <Card aria-busy={devicesQuery.isLoading}>
          <CardHeader className="border-b max-sm:grid-cols-1">
            <CardTitle>Devices</CardTitle>
            <CardDescription>{visibleDevices.length} shown</CardDescription>
            <CardAction className="max-sm:col-start-1 max-sm:row-start-2 max-sm:justify-self-stretch">
              <div className="relative">
                <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  aria-label="Search devices"
                  className="w-full pl-8 sm:w-56"
                  onChange={(event) => setSearch(event.target.value)}
                  placeholder="Search inventory"
                  ref={searchInputRef}
                  value={search}
                />
              </div>
            </CardAction>
          </CardHeader>
          <CardContent>
            {devicesQuery.isLoading ? (
              <TableSkeleton />
            ) : devicesQuery.isError ? (
              <LoadError
                error={devicesQuery.error}
                retry={() => devicesQuery.refetch()}
                title="Devices unavailable"
              />
            ) : visibleDevices.length === 0 ? (
              <NoDevices onCreate={() => setDialog("create")} />
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Type</TableHead>
                    <TableHead>State</TableHead>
                    <TableHead className="text-right">Record</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {visibleDevices.map((device) => (
                    <TableRow
                      aria-selected={device.id === selectedId}
                      className="cursor-pointer"
                      data-state={
                        device.id === selectedId ? "selected" : undefined
                      }
                      key={device.id}
                      onClick={() => setRequestedDeviceId(device.id)}
                    >
                      <TableCell>
                        <button
                          aria-pressed={device.id === selectedId}
                          className="flex min-w-0 w-full items-center gap-2 rounded-md text-left outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
                          onClick={() => setRequestedDeviceId(device.id)}
                          type="button"
                        >
                          <span className="grid size-7 place-items-center rounded-md bg-muted">
                            <ServerIcon
                              aria-hidden="true"
                              className="size-3.5"
                            />
                          </span>
                          <div className="min-w-0">
                            <p className="truncate font-medium">
                              {device.name || "Unnamed device"}
                            </p>
                            <p className="truncate font-mono text-xs text-muted-foreground">
                              {shortId(device.id)}
                            </p>
                          </div>
                        </button>
                      </TableCell>
                      <TableCell>{labelize(device.device_type)}</TableCell>
                      <TableCell>
                        <StatusBadge value={device.status} />
                      </TableCell>
                      <TableCell className="text-right font-mono text-xs text-muted-foreground">
                        v{device.version}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </CardContent>
        </Card>

        <DeviceDetail
          addressError={addressesQuery.error}
          addresses={addressesQuery.data?.items ?? EMPTY_ADDRESSES}
          detail={detailQuery.data}
          error={detailQuery.error}
          loading={detailQuery.isLoading}
          onEdit={() => setDialog("edit")}
          onInstall={() => setDialog("agent-install")}
          onMerge={() => setDialog("merge")}
          onSplit={() => setDialog("split")}
          onUndo={(id) => undoMutation.mutate(id)}
          undoPending={undoMutation.isPending}
        />
      </div>

      <ReviewQueue
        devices={devices}
        error={suggestionsQuery.error}
        loading={suggestionsQuery.isLoading}
        pendingId={
          resolveSuggestionMutation.isPending
            ? resolveSuggestionMutation.variables?.id
            : null
        }
        mutationError={resolveSuggestionMutation.error}
        resolve={(id, decision) =>
          resolveSuggestionMutation.mutate({ id, decision })
        }
        suggestions={suggestions}
      />

      <CreateDeviceDialog
        error={createMutation.error}
        onOpenChange={(open) => !open && setDialog(null)}
        open={dialog === "create"}
        pending={createMutation.isPending}
        submit={(deviceType, name) =>
          createMutation.mutate({
            device_type: deviceType,
            name: name || undefined,
          })
        }
      />
      {detailQuery.data ? (
        <EditDeviceDialog
          detail={detailQuery.data}
          error={editMutation.error}
          onOpenChange={(open) => !open && setDialog(null)}
          open={dialog === "edit"}
          pending={editMutation.isPending}
          submit={(name, status) =>
            editMutation.mutate({
              id: detailQuery.data!.id,
              version: detailQuery.data!.version,
              name,
              status,
            })
          }
        />
      ) : null}
      {selected ? (
        <MergeDialog
          devices={devices}
          error={mergeMutation.error}
          onOpenChange={(open) => !open && setDialog(null)}
          open={dialog === "merge"}
          pending={mergeMutation.isPending}
          source={selected}
          submit={(into, reason) =>
            mergeMutation.mutate({ absorbedId: selected.id, into, reason })
          }
        />
      ) : null}
      {detailQuery.data ? (
        <SplitDialog
          detail={detailQuery.data}
          error={splitMutation.error}
          onOpenChange={(open) => !open && setDialog(null)}
          open={dialog === "split"}
          pending={splitMutation.isPending}
          submit={(interfaceIds, name) =>
            splitMutation.mutate({
              sourceId: detailQuery.data!.id,
              interfaceIds,
              name,
              deviceType: detailQuery.data!.device_type,
            })
          }
        />
      ) : null}
      <AgentDeploymentDialog
        device={detailQuery.data}
        onOpenChange={(open) => !open && setDialog(null)}
        open={dialog === "agent-install"}
      />
    </div>
  );
}

function MetricCard({
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

function StatusBadge({ value }: { value: string }) {
  const variant =
    value === "critical"
      ? "destructive"
      : value === "stale" || value === "warning"
        ? "outline"
        : "secondary";
  return <Badge variant={variant}>{labelize(value)}</Badge>;
}

function DeviceDetail({
  addressError,
  addresses,
  detail,
  loading,
  error,
  onEdit,
  onInstall,
  onMerge,
  onSplit,
  onUndo,
  undoPending,
}: {
  addressError: unknown;
  addresses: Address[];
  detail: DeviceDetail | undefined;
  loading: boolean;
  error: unknown;
  onEdit: () => void;
  onInstall: () => void;
  onMerge: () => void;
  onSplit: () => void;
  onUndo: (id: string) => void;
  undoPending: boolean;
}) {
  if (loading)
    return (
      <Card>
        <CardContent className="flex flex-col gap-4">
          <Skeleton className="h-8 w-2/3" />
          <Skeleton className="h-20 w-full" />
          <Skeleton className="h-40 w-full" />
        </CardContent>
      </Card>
    );
  if (error)
    return (
      <Card>
        <CardContent>
          <LoadError error={error} title="Device detail unavailable" />
        </CardContent>
      </Card>
    );
  if (!detail)
    return (
      <Card>
        <CardContent>
          <Empty>
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <ServerIcon />
              </EmptyMedia>
              <EmptyTitle>Select a device</EmptyTitle>
              <EmptyDescription>
                View identity, interfaces, addresses, and evidence.
              </EmptyDescription>
            </EmptyHeader>
          </Empty>
        </CardContent>
      </Card>
    );
  const merged = detail.merged_member_ids.filter((id) => id !== detail.id);
  return (
    <Card>
      <CardHeader className="border-b">
        <div>
          <CardTitle>{detail.name || "Unnamed device"}</CardTitle>
          <CardDescription>
            {labelize(detail.device_type)} ·{" "}
            <span className="font-mono">{shortId(detail.id)}</span>
          </CardDescription>
        </div>
        <CardAction>
          <StatusBadge value={detail.status} />
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-5">
        <div className="flex flex-wrap gap-2">
          <Button onClick={onEdit} size="sm" variant="outline">
            <PencilIcon data-icon="inline-start" />
            Edit
          </Button>
          <Button onClick={onInstall} size="sm" variant="outline">
            <ShieldCheckIcon data-icon="inline-start" />
            Install agent
          </Button>
          <Button onClick={onMerge} size="sm" variant="outline">
            <GitMergeIcon data-icon="inline-start" />
            Merge
          </Button>
          <Button onClick={onSplit} size="sm" variant="outline">
            <GitBranchIcon data-icon="inline-start" />
            Split
          </Button>
        </div>
        <section>
          <h3 className="text-sm font-medium">Identity confidence</h3>
          <ConfidenceMeter value={detail.identity_confidence} />
          <p className="mt-2 text-xs leading-5 text-muted-foreground">
            Manual facts outrank automatic evidence. Scores are calculated when
            this record is read.
          </p>
        </section>
        <section>
          <h3 className="mb-2 text-sm font-medium">Interfaces & IP history</h3>
          {addressError ? (
            <div className="mb-3">
              <LoadError
                error={addressError}
                title="Address history unavailable"
              />
            </div>
          ) : null}
          {detail.interfaces.length ? (
            <div className="flex flex-col gap-2">
              {detail.interfaces.map((networkInterface) => (
                <InterfaceCard
                  addresses={addresses.filter(
                    (address) => address.interface_id === networkInterface.id,
                  )}
                  description={networkInterface.description}
                  id={networkInterface.id}
                  key={networkInterface.id}
                  mac={networkInterface.mac}
                />
              ))}
            </div>
          ) : (
            <p className="text-sm text-muted-foreground">
              No interfaces attached.
            </p>
          )}
        </section>
        <section>
          <h3 className="mb-2 text-sm font-medium">Evidence</h3>
          {detail.evidence.length ? (
            <div className="flex flex-col divide-y">
              {detail.evidence.slice(0, 6).map((evidence) => (
                <div
                  className="flex items-start justify-between gap-3 py-2"
                  key={evidence.id}
                >
                  <div className="min-w-0">
                    <p className="font-medium">
                      {labelize(evidence.attribute)}
                    </p>
                    <p className="truncate text-xs text-muted-foreground">
                      {displayValue(evidence.value)}
                    </p>
                  </div>
                  <div className="shrink-0 text-right text-xs">
                    <p>{Math.round(evidence.confidence * 100)}%</p>
                    <p className="text-muted-foreground">
                      {labelize(evidence.source_type)}
                    </p>
                  </div>
                </div>
              ))}
            </div>
          ) : (
            <p className="text-sm text-muted-foreground">
              No evidence collected.
            </p>
          )}
        </section>
        {merged.length ? (
          <section>
            <h3 className="mb-2 text-sm font-medium">Merged records</h3>
            <div className="flex flex-col gap-2">
              {merged.map((id) => (
                <div
                  className="flex items-center justify-between rounded-lg border p-2"
                  key={id}
                >
                  <span className="font-mono text-xs">{shortId(id)}</span>
                  <Button
                    disabled={undoPending}
                    onClick={() => onUndo(id)}
                    size="sm"
                    variant="ghost"
                  >
                    <Undo2Icon data-icon="inline-start" />
                    Restore
                  </Button>
                </div>
              ))}
            </div>
          </section>
        ) : null}
      </CardContent>
    </Card>
  );
}

function InterfaceCard({
  id,
  description,
  mac,
  addresses,
}: {
  id: string;
  description: string | null;
  mac: string | null;
  addresses: Address[];
}) {
  const current = addresses.find((address) => address.is_current);
  const previous = addresses.filter((address) => !address.is_current);
  return (
    <div className="rounded-lg border p-3">
      <div className="flex items-center justify-between gap-2">
        <p className="font-medium">{description || "Unnamed interface"}</p>
        <span className="font-mono text-xs text-muted-foreground">
          {mac || shortId(id)}
        </span>
      </div>
      <p className="mt-1 font-mono text-sm">{current?.ip || "No current IP"}</p>
      {previous.length ? (
        <p className="mt-1 text-xs text-muted-foreground">
          Previous: {previous.map((address) => address.ip).join(", ")}
        </p>
      ) : null}
    </div>
  );
}

function ReviewQueue({
  suggestions,
  devices,
  loading,
  error,
  mutationError,
  pendingId,
  resolve,
}: {
  suggestions: IdentitySuggestion[];
  devices: Device[];
  loading: boolean;
  error: unknown;
  mutationError: unknown;
  pendingId: string | null;
  resolve: (id: string, decision: "confirm" | "reject") => void;
}) {
  const names = new Map(
    devices.map((device) => [device.id, device.name || "Unnamed device"]),
  );
  return (
    <Card>
      <CardHeader>
        <CardTitle>Identity review queue</CardTitle>
        <CardDescription>
          Approve or reject ambiguous identity matches.
        </CardDescription>
        <CardAction>
          <Badge variant="outline">{suggestions.length} pending</Badge>
        </CardAction>
      </CardHeader>
      <CardContent>
        {mutationError ? (
          <div className="mb-4">
            <MutationError error={mutationError} />
          </div>
        ) : null}
        {loading ? (
          <TableSkeleton rows={3} />
        ) : error ? (
          <LoadError error={error} title="Identity review queue unavailable" />
        ) : suggestions.length === 0 ? (
          <Empty>
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <ShieldCheckIcon />
              </EmptyMedia>
              <EmptyTitle>No pending matches</EmptyTitle>
              <EmptyDescription>
                No identity matches need review.
              </EmptyDescription>
            </EmptyHeader>
          </Empty>
        ) : (
          <div className="flex flex-col divide-y">
            {suggestions.map((suggestion) => (
              <div
                className="flex flex-col gap-3 py-4 md:flex-row md:items-center md:justify-between"
                key={suggestion.id}
              >
                <div>
                  <p className="font-medium">
                    {names.get(suggestion.candidate_device_id) ||
                      `Device ${shortId(suggestion.candidate_device_id)}`}
                  </p>
                  <p className="text-sm text-muted-foreground">
                    {suggestion.explanation.matched
                      .map(
                        (match) =>
                          `${labelize(match.rule_type)} ${Math.round(match.weight * 100)}%`,
                      )
                      .join(" · ") || "No explanation attached"}
                  </p>
                </div>
                <div className="flex items-center gap-2">
                  <Badge variant="outline">
                    {Math.round(suggestion.score * 100)}%
                  </Badge>
                  <Button
                    disabled={pendingId === suggestion.id}
                    onClick={() => resolve(suggestion.id, "confirm")}
                    size="sm"
                  >
                    <CheckIcon data-icon="inline-start" />
                    Confirm
                  </Button>
                  <Button
                    disabled={pendingId === suggestion.id}
                    onClick={() => resolve(suggestion.id, "reject")}
                    size="sm"
                    variant="ghost"
                  >
                    <XIcon data-icon="inline-start" />
                    Reject
                  </Button>
                </div>
              </div>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

export function CreateDeviceDialog({
  open,
  onOpenChange,
  submit,
  pending,
  error,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  submit: (deviceType: string, name: string) => void;
  pending: boolean;
  error: unknown;
}) {
  const [name, setName] = useState("");
  const [deviceType, setDeviceType] = useState("physical_host");
  useEffect(() => {
    if (open) {
      setName("");
      setDeviceType("physical_host");
    }
  }, [open]);

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add device</DialogTitle>
          <DialogDescription>
            Create a manually managed inventory record.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-5"
          onSubmit={(event) => {
            event.preventDefault();
            const trimmedName = name.trim();
            if (trimmedName) submit(deviceType, trimmedName);
          }}
        >
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="device-name">Device name</FieldLabel>
              <Input
                id="device-name"
                onChange={(event) => setName(event.target.value)}
                placeholder="e.g. kitchen-host"
                required
                value={name}
              />
            </Field>
            <DeviceTypeField value={deviceType} onChange={setDeviceType} />
          </FieldGroup>
          <MutationError error={error} />
          <DialogFooter>
            <Button disabled={pending || !name.trim()} type="submit">
              {pending ? (
                <Spinner data-icon="inline-start" />
              ) : (
                <PlusIcon data-icon="inline-start" />
              )}
              Create device
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function AddNetworkDialog({
  open,
  network = null,
  onOpenChange,
  submit,
  pending,
  error,
}: {
  open: boolean;
  network?: Network | null;
  onOpenChange: (open: boolean) => void;
  submit: (input: {
    cidr: string;
    name?: string;
    gateway?: string;
    vlan?: number;
  }) => void;
  pending: boolean;
  error: unknown;
}) {
  const [name, setName] = useState("");
  const [cidr, setCidr] = useState("");
  const [gateway, setGateway] = useState("");
  const [vlan, setVlan] = useState("");
  const [fieldErrors, setFieldErrors] = useState<
    Partial<Record<NetworkField, string>>
  >({});
  useEffect(() => {
    if (open) {
      setName(network?.name ?? "");
      setCidr(network?.cidr ?? "");
      setGateway(network?.gateway ?? "");
      setVlan(network?.vlan == null ? "" : String(network.vlan));
      setFieldErrors({});
    }
  }, [network, open]);
  const serverFieldError = (field: NetworkField) =>
    error instanceof ApiError ? error.fieldErrors[field] : undefined;
  const cidrError = fieldErrors.cidr ?? serverFieldError("cidr");
  const gatewayError = fieldErrors.gateway ?? serverFieldError("gateway");
  const vlanError = fieldErrors.vlan ?? serverFieldError("vlan");

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{network ? "Edit network" : "Add network"}</DialogTitle>
          <DialogDescription>
            Define the CIDR boundary that discovery can scan.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-5"
          noValidate
          onSubmit={(event) => {
            event.preventDefault();
            const nextErrors = validateNetworkFields({ cidr, gateway, vlan });
            setFieldErrors(nextErrors);
            if (Object.keys(nextErrors).length) return;
            submit({
              cidr: cidr.trim(),
              gateway: gateway.trim() || undefined,
              name: name.trim() || undefined,
              vlan: vlan ? Number(vlan) : undefined,
            });
          }}
        >
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="network-name">Name</FieldLabel>
              <Input
                id="network-name"
                onChange={(event) => setName(event.target.value)}
                placeholder="e.g. Home LAN"
                value={name}
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="network-cidr">CIDR</FieldLabel>
              <Input
                id="network-cidr"
                aria-invalid={Boolean(cidrError)}
                onChange={(event) => {
                  setCidr(event.target.value);
                  setFieldErrors((current) => ({
                    ...current,
                    cidr: undefined,
                  }));
                }}
                placeholder="e.g. 192.168.1.0/24"
                required
                value={cidr}
              />
              <FieldError>{cidrError}</FieldError>
            </Field>
            <div className="grid gap-5 sm:grid-cols-2">
              <Field>
                <FieldLabel htmlFor="network-gateway">Gateway</FieldLabel>
                <Input
                  id="network-gateway"
                  aria-invalid={Boolean(gatewayError)}
                  onChange={(event) => {
                    setGateway(event.target.value);
                    setFieldErrors((current) => ({
                      ...current,
                      gateway: undefined,
                    }));
                  }}
                  placeholder="e.g. 192.168.1.1"
                  value={gateway}
                />
                <FieldError>{gatewayError}</FieldError>
              </Field>
              <Field>
                <FieldLabel htmlFor="network-vlan">VLAN</FieldLabel>
                <Input
                  id="network-vlan"
                  aria-invalid={Boolean(vlanError)}
                  inputMode="numeric"
                  min="1"
                  max="4094"
                  onChange={(event) => {
                    setVlan(event.target.value);
                    setFieldErrors((current) => ({
                      ...current,
                      vlan: undefined,
                    }));
                  }}
                  placeholder="Optional"
                  type="number"
                  value={vlan}
                />
                <FieldError>{vlanError}</FieldError>
              </Field>
            </div>
          </FieldGroup>
          <MutationError error={error} />
          <DialogFooter>
            <Button disabled={pending} type="submit">
              {pending ? (
                <Spinner data-icon="inline-start" />
              ) : (
                <PlusIcon data-icon="inline-start" />
              )}
              {network ? "Save network" : "Add network"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function EditDeviceDialog({
  open,
  onOpenChange,
  detail,
  submit,
  pending,
  error,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  detail: DeviceDetail;
  submit: (name: string, status: string) => void;
  pending: boolean;
  error: unknown;
}) {
  const [name, setName] = useState(detail.name ?? "");
  const [status, setStatus] = useState(detail.status);
  useEffect(() => {
    setName(detail.name ?? "");
    setStatus(detail.status);
  }, [detail.id, detail.name, detail.status]);

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Edit device</DialogTitle>
          <DialogDescription>
            This change checks the record version before saving.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-5"
          onSubmit={(event) => {
            event.preventDefault();
            submit(name, status);
          }}
        >
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="edit-device-name">Device name</FieldLabel>
              <Input
                id="edit-device-name"
                onChange={(event) => setName(event.target.value)}
                value={name}
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="edit-device-status">
                Lifecycle state
              </FieldLabel>
              <Select
                onValueChange={(value) => {
                  if (value) setStatus(value);
                }}
                value={status}
              >
                <SelectTrigger id="edit-device-status">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    <SelectItem value="active">Active</SelectItem>
                    <SelectItem value="stale">Stale</SelectItem>
                    <SelectItem value="archived">Archived</SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
          </FieldGroup>
          <MutationError error={error} />
          <DialogFooter>
            <Button disabled={pending} type="submit">
              {pending ? (
                <Spinner data-icon="inline-start" />
              ) : (
                <CheckIcon data-icon="inline-start" />
              )}
              Save changes
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function MergeDialog({
  open,
  onOpenChange,
  source,
  devices,
  submit,
  pending,
  error,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  source: Device;
  devices: Device[];
  submit: (target: string, reason: string) => void;
  pending: boolean;
  error: unknown;
}) {
  const targets = devices.filter((device) => device.id !== source.id);
  const [target, setTarget] = useState(targets[0]?.id ?? "");
  const [reason, setReason] = useState("Operator confirmed duplicate");
  const selectedTarget = targets.find((device) => device.id === target);
  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Merge device</DialogTitle>
          <DialogDescription>
            Merge the source into the target. You can undo the merge later.
          </DialogDescription>
        </DialogHeader>
        {targets.length === 0 ? (
          <NoDevices />
        ) : (
          <form
            className="flex flex-col gap-5"
            onSubmit={(event) => {
              event.preventDefault();
              submit(target, reason);
            }}
          >
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="merge-source">Absorb</FieldLabel>
                <Input
                  disabled
                  id="merge-source"
                  value={source.name || shortId(source.id)}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor="merge-target">Into survivor</FieldLabel>
                <Select
                  onValueChange={(value) => {
                    if (value) setTarget(value);
                  }}
                  value={target}
                >
                  <SelectTrigger id="merge-target">
                    <SelectValue>
                      {selectedTarget
                        ? `${selectedTarget.name || "Unnamed device"} · ${shortId(selectedTarget.id)}`
                        : "Select survivor"}
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      {targets.map((device) => (
                        <SelectItem key={device.id} value={device.id}>
                          {device.name || "Unnamed device"} ·{" "}
                          {shortId(device.id)}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </Field>
              <Field>
                <FieldLabel htmlFor="merge-reason">Reason</FieldLabel>
                <Input
                  id="merge-reason"
                  onChange={(event) => setReason(event.target.value)}
                  value={reason}
                />
              </Field>
            </FieldGroup>
            <MutationError error={error} />
            <DialogFooter>
              <Button disabled={pending || !target} type="submit">
                {pending ? (
                  <Spinner data-icon="inline-start" />
                ) : (
                  <GitMergeIcon data-icon="inline-start" />
                )}
                Merge devices
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

function SplitDialog({
  open,
  onOpenChange,
  detail,
  submit,
  pending,
  error,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  detail: DeviceDetail;
  submit: (interfaces: string[], name: string) => void;
  pending: boolean;
  error: unknown;
}) {
  const [name, setName] = useState("");
  const [interfaces, setInterfaces] = useState<string[]>([]);
  const toggle = (id: string, checked: boolean) =>
    setInterfaces((current) =>
      checked ? [...current, id] : current.filter((value) => value !== id),
    );
  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Split interfaces</DialogTitle>
          <DialogDescription>
            Move selected interfaces and their IP history into a new device.
          </DialogDescription>
        </DialogHeader>
        {detail.interfaces.length === 0 ? (
          <Empty>
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <GitBranchIcon />
              </EmptyMedia>
              <EmptyTitle>No interfaces to split</EmptyTitle>
            </EmptyHeader>
          </Empty>
        ) : (
          <form
            className="flex flex-col gap-5"
            onSubmit={(event) => {
              event.preventDefault();
              submit(interfaces, name);
            }}
          >
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="split-name">New device name</FieldLabel>
                <Input
                  id="split-name"
                  onChange={(event) => setName(event.target.value)}
                  placeholder="e.g. recovered-host"
                  value={name}
                />
              </Field>
              <FieldSet>
                <FieldLegend>Interfaces to move</FieldLegend>
                {detail.interfaces.map((networkInterface) => (
                  <Field key={networkInterface.id} orientation="horizontal">
                    <Checkbox
                      checked={interfaces.includes(networkInterface.id)}
                      id={networkInterface.id}
                      onCheckedChange={(checked) =>
                        toggle(networkInterface.id, checked)
                      }
                    />
                    <FieldLabel htmlFor={networkInterface.id}>
                      {networkInterface.description || "Unnamed interface"}{" "}
                      <span className="font-mono text-xs text-muted-foreground">
                        {networkInterface.mac || shortId(networkInterface.id)}
                      </span>
                    </FieldLabel>
                  </Field>
                ))}
              </FieldSet>
            </FieldGroup>
            <MutationError error={error} />
            <DialogFooter>
              <Button
                disabled={pending || interfaces.length === 0}
                type="submit"
              >
                {pending ? (
                  <Spinner data-icon="inline-start" />
                ) : (
                  <GitBranchIcon data-icon="inline-start" />
                )}
                Create split device
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

function DeviceTypeField({
  value,
  onChange,
}: {
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <Field>
      <FieldLabel htmlFor="device-type">Device type</FieldLabel>
      <Select
        onValueChange={(next) => {
          if (next) onChange(next);
        }}
        value={value}
      >
        <SelectTrigger id="device-type">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectGroup>
            {[
              "physical_host",
              "vm",
              "lxc",
              "container_host",
              "appliance",
              "unknown",
            ].map((type) => (
              <SelectItem key={type} value={type}>
                {labelize(type)}
              </SelectItem>
            ))}
          </SelectGroup>
        </SelectContent>
      </Select>
    </Field>
  );
}

function ConfidenceMeter({ value }: { value: number }) {
  const percentage = Math.round(Math.max(0, Math.min(1, value)) * 100);
  return (
    <div className="mt-3">
      <Progress value={percentage}>
        <div className="flex w-full items-center gap-2">
          <ProgressLabel>Evidence strength</ProgressLabel>
          <ProgressValue />
        </div>
      </Progress>
      <div className="mt-1 flex justify-between text-[11px] text-muted-foreground">
        <span>New</span>
        <span>Review 40%</span>
        <span>Auto 85%</span>
      </div>
    </div>
  );
}

function MutationError({ error }: { error: unknown }) {
  if (!error) return null;
  return (
    <Alert variant="destructive">
      <CircleAlertIcon />
      <AlertTitle>Action failed</AlertTitle>
      <AlertDescription>
        {error instanceof Error ? error.message : "Try again."}
      </AlertDescription>
    </Alert>
  );
}

function LoadError({
  error,
  retry,
  title = "Inventory unavailable",
}: {
  error: unknown;
  retry?: () => void;
  title?: string;
}) {
  return (
    <Alert variant="destructive">
      <CircleAlertIcon />
      <AlertTitle>{title}</AlertTitle>
      <AlertDescription>
        {error instanceof Error ? error.message : "Try again."}
      </AlertDescription>
      {retry ? (
        <Button onClick={retry} size="sm" variant="outline">
          Retry
        </Button>
      ) : null}
    </Alert>
  );
}

function NoDevices({ onCreate }: { onCreate?: () => void }) {
  return (
    <Empty>
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <ServerIcon />
        </EmptyMedia>
        <EmptyTitle>No devices yet</EmptyTitle>
        <EmptyDescription>
          Create the first record or run the seed command.
        </EmptyDescription>
      </EmptyHeader>
      {onCreate ? (
        <Button onClick={onCreate}>
          <PlusIcon data-icon="inline-start" />
          Add device
        </Button>
      ) : null}
    </Empty>
  );
}

function TableSkeleton({ rows = 5 }: { rows?: number }) {
  return (
    <div className="flex flex-col gap-3">
      {Array.from({ length: rows }, (_, index) => (
        <Skeleton className="h-11 w-full" key={index} />
      ))}
    </div>
  );
}
