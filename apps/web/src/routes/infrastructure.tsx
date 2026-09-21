import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useRouter } from "@tanstack/react-router";
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
import { useEffect, useMemo, useRef, useState } from "react";
import { AgentDeploymentDialog } from "@/components/AgentDeploymentDialog";
import {
  createDevice,
  cancelScanRun,
  fetchAddresses,
  fetchDevice,
  fetchDeviceFullScan,
  fetchDevices,
  fetchInterfaces,
  fetchIdentitySuggestions,
  fetchMonitors,
  fetchServices,
  getUserFacingError,
  launchDeviceFullScan,
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
  type InventoryInterface,
  type Monitor,
  type Network,
  type Service,
} from "@/lib/api";
import {
  formatEvidenceValue,
  formatRelative,
  labelize,
  shortId,
} from "@/lib/format";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
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
const EMPTY_SERVICES: Service[] = [];
const EMPTY_MONITORS: Monitor[] = [];
const EMPTY_INTERFACES: InventoryInterface[] = [];
const MONITOR_PAGE_SIZE = 100;

export function InfrastructurePage() {
  const queryClient = useQueryClient();
  const router = useRouter({ warn: false });
  type DeviceSearch = { device?: string; focus?: "search"; q?: string };
  const navigate = router
    ? (search: (previous: DeviceSearch) => DeviceSearch) =>
        router.navigate({
          to: "/devices",
          search,
        } as Parameters<typeof router.navigate>[0])
    : null;
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
  const servicesQuery = useQuery({
    queryKey: ["services"],
    queryFn: fetchServices,
    staleTime: 30_000,
  });
  const interfacesQuery = useQuery({
    queryKey: ["interfaces"],
    queryFn: fetchInterfaces,
    staleTime: 30_000,
  });
  const monitorsQuery = useQuery({
    queryKey: ["monitors"],
    queryFn: () => fetchMonitors(),
    staleTime: 15_000,
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
    : null;

  const selected = devices.find((device) => device.id === selectedId) ?? null;
  const detailQuery = useQuery({
    queryKey: ["device", selectedId],
    queryFn: () => fetchDevice(selectedId!),
    enabled: Boolean(selectedId),
  });
  const addressesQuery = useQuery({
    queryKey: ["addresses"],
    queryFn: fetchAddresses,
  });

  const selectDevice = (id: string | null) => {
    setRequestedDeviceId(id);
    if (navigate) {
      void navigate((previous) => ({
        ...previous,
        device: id ?? undefined,
      }));
    }
  };
  const updateSearch = (value: string) => {
    setSearch(value);
    if (navigate) {
      void navigate((previous) => ({
        ...previous,
        q: value.trim() || undefined,
      }));
    }
  };

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
      selectDevice(device.id);
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

  const suggestions = suggestionsQuery.data?.items ?? EMPTY_SUGGESTIONS;
  const services = servicesQuery.data?.items ?? EMPTY_SERVICES;
  const monitors = monitorsQuery.data?.items ?? EMPTY_MONITORS;
  const monitorsPossiblyTruncated = monitors.length >= MONITOR_PAGE_SIZE;
  const interfaces = interfacesQuery.data?.items ?? EMPTY_INTERFACES;
  const deviceServices = new Map<string, Service[]>();
  for (const service of services) {
    if (service.owner_kind !== "device" || !service.owner_id) continue;
    const current = deviceServices.get(service.owner_id) ?? [];
    current.push(service);
    deviceServices.set(service.owner_id, current);
  }
  const monitorsByService = new Map<string, Monitor[]>();
  for (const monitor of monitors) {
    const current = monitorsByService.get(monitor.service_id) ?? [];
    current.push(monitor);
    monitorsByService.set(monitor.service_id, current);
  }
  const addressByInterface = new Map(
    (addressesQuery.data?.items ?? [])
      .filter((address) => address.is_current)
      .map((address) => [address.interface_id, address.ip]),
  );
  const currentIpsByDevice = new Map<string, string[]>();
  for (const networkInterface of interfaces) {
    const ip = addressByInterface.get(networkInterface.id);
    if (!ip) continue;
    const current = currentIpsByDevice.get(networkInterface.device_id) ?? [];
    current.push(ip);
    currentIpsByDevice.set(networkInterface.device_id, current);
  }

  return (
    <div className="mx-auto flex max-w-7xl flex-col gap-6">
      <div className="flex flex-col justify-between gap-4 md:flex-row md:items-end">
        <div>
          <h1 className="text-3xl font-semibold tracking-tight">Devices</h1>
          <p className="mt-2 max-w-2xl text-sm text-muted-foreground">
            See each device, the services running on it, and the latest useful
            condition. Select a row when you need the record behind it.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <a
            className="inline-flex min-h-9 items-center gap-2 rounded-lg border px-3 text-sm text-muted-foreground outline-none transition-colors hover:bg-muted hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50"
            href="/networks"
          >
            <NetworkIcon aria-hidden="true" className="size-4" />
            Networks
          </a>
          <a
            className="inline-flex min-h-9 items-center gap-2 rounded-lg border px-3 text-sm text-muted-foreground outline-none transition-colors hover:bg-muted hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50"
            href="/agents"
          >
            <ShieldCheckIcon aria-hidden="true" className="size-4" />
            Agents
          </a>
          <Button onClick={() => setDialog("create")}>
            <PlusIcon data-icon="inline-start" />
            Add device
          </Button>
        </div>
      </div>

      <div
        className={
          selectedId
            ? "grid gap-6 lg:grid-cols-[minmax(18rem,0.8fr)_minmax(0,1.5fr)]"
            : ""
        }
      >
        <Card
          aria-busy={devicesQuery.isLoading}
          className={selectedId ? "max-lg:hidden" : undefined}
        >
          <CardHeader className="border-b max-sm:grid-cols-1">
            <div>
              <CardTitle>Device list</CardTitle>
              <CardDescription>
                {visibleDevices.length} of {devices.length} records
              </CardDescription>
            </div>
            <CardAction className="max-sm:col-start-1 max-sm:row-start-2 max-sm:justify-self-stretch">
              <div className="relative">
                <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  aria-label="Search devices"
                  className="w-full pl-8 sm:w-56"
                  onChange={(event) => updateSearch(event.target.value)}
                  placeholder="Search devices"
                  ref={searchInputRef}
                  value={search}
                />
              </div>
            </CardAction>
          </CardHeader>
          <CardContent className="p-0">
            {devicesQuery.isLoading ? (
              <div className="p-6">
                <TableSkeleton />
              </div>
            ) : devicesQuery.isError ? (
              <div className="p-6">
                <LoadError
                  error={devicesQuery.error}
                  retry={() => devicesQuery.refetch()}
                  title="Devices unavailable"
                />
              </div>
            ) : visibleDevices.length === 0 ? (
              <div className="p-6">
                <NoDevices onCreate={() => setDialog("create")} />
              </div>
            ) : (
              <>
                <div className="divide-y sm:hidden">
                  {visibleDevices.map((device) => {
                    const deviceServiceRows =
                      deviceServices.get(device.id) ?? [];
                    const checks = deviceServiceRows.flatMap(
                      (service) => monitorsByService.get(service.id) ?? [],
                    );
                    const states = checks
                      .map((monitor) => monitor.state)
                      .filter(Boolean);
                    const failing = states.filter((state) =>
                      ["down", "degraded"].includes(state),
                    ).length;
                    return (
                      <button
                        aria-pressed={device.id === selectedId}
                        className="flex min-h-20 w-full items-center justify-between gap-3 p-4 text-left outline-none transition-colors hover:bg-muted/50 focus-visible:ring-3 focus-visible:ring-ring/50"
                        key={device.id}
                        onClick={() => selectDevice(device.id)}
                        type="button"
                      >
                        <span className="flex min-w-0 items-center gap-3">
                          <span className="grid size-8 shrink-0 place-items-center rounded-lg bg-muted text-muted-foreground">
                            <ServerIcon aria-hidden="true" className="size-4" />
                          </span>
                          <span className="min-w-0">
                            <span className="block truncate font-medium">
                              {device.name || "Unnamed device"}
                            </span>
                            <span className="block truncate text-xs text-muted-foreground">
                              {currentIpsByDevice.get(device.id)?.[0] ??
                                `Record updated ${formatRelative(device.updated_at)}`}
                            </span>
                            <span className="mt-1 block text-xs text-muted-foreground">
                              {servicesQuery.isLoading ||
                              monitorsQuery.isLoading
                                ? "Loading checks…"
                                : servicesQuery.isError || monitorsQuery.isError
                                  ? "Condition unavailable"
                                  : deviceServiceRows.length
                                    ? `${failing ? `${failing} needs attention · ` : ""}${deviceServiceRows.length} service${deviceServiceRows.length === 1 ? "" : "s"}` +
                                      (monitorsPossiblyTruncated
                                        ? " · check list limited"
                                        : "")
                                    : "No service observations"}
                            </span>
                          </span>
                        </span>
                        <StatusBadge
                          value={
                            failing
                              ? states.includes("down")
                                ? "critical"
                                : "warning"
                              : device.status
                          }
                        />
                      </button>
                    );
                  })}
                </div>
                <div className="hidden overflow-x-auto sm:block">
                  <Table className="min-w-[42rem]">
                    <TableHeader>
                      <TableRow>
                        <TableHead>Device</TableHead>
                        <TableHead>Address / record</TableHead>
                        <TableHead>Services</TableHead>
                        <TableHead className="text-right">Condition</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {visibleDevices.map((device) => {
                        const deviceServiceRows =
                          deviceServices.get(device.id) ?? [];
                        const checks = deviceServiceRows.flatMap(
                          (service) => monitorsByService.get(service.id) ?? [],
                        );
                        const states = checks
                          .map((monitor) => monitor.state)
                          .filter(Boolean);
                        const failing = states.filter((state) =>
                          ["down", "degraded"].includes(state),
                        ).length;
                        return (
                          <TableRow
                            aria-selected={device.id === selectedId}
                            className="cursor-pointer"
                            data-state={
                              device.id === selectedId ? "selected" : undefined
                            }
                            key={device.id}
                            onClick={() => selectDevice(device.id)}
                          >
                            <TableCell>
                              <button
                                aria-label={`Open ${device.name || "unnamed device"}`}
                                aria-pressed={device.id === selectedId}
                                className="flex min-h-11 min-w-0 w-full items-center gap-3 rounded-md text-left outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
                                onClick={() => selectDevice(device.id)}
                                type="button"
                              >
                                <span className="grid size-8 shrink-0 place-items-center rounded-lg bg-muted text-muted-foreground">
                                  <ServerIcon
                                    aria-hidden="true"
                                    className="size-4"
                                  />
                                </span>
                                <span className="min-w-0">
                                  <span className="block truncate font-medium">
                                    {device.name || "Unnamed device"}
                                  </span>
                                  <span className="block truncate text-xs text-muted-foreground">
                                    {labelize(device.device_type)}
                                  </span>
                                </span>
                              </button>
                            </TableCell>
                            <TableCell className="whitespace-nowrap text-sm text-muted-foreground">
                              {interfacesQuery.isLoading ||
                              addressesQuery.isLoading
                                ? "Loading address…"
                                : (currentIpsByDevice.get(device.id)?.[0] ??
                                  `Record updated ${formatRelative(device.updated_at)}`)}
                            </TableCell>
                            <TableCell>
                              {servicesQuery.isLoading ||
                              monitorsQuery.isLoading ? (
                                <span className="text-sm text-muted-foreground">
                                  Loading checks…
                                </span>
                              ) : servicesQuery.isError ||
                                monitorsQuery.isError ? (
                                <span className="text-sm text-muted-foreground">
                                  Condition unavailable
                                </span>
                              ) : deviceServiceRows.length ? (
                                <span className="text-sm">
                                  {failing
                                    ? `${failing} needs attention · `
                                    : ""}
                                  {deviceServiceRows.length} service
                                  {deviceServiceRows.length === 1 ? "" : "s"}
                                  {monitorsPossiblyTruncated
                                    ? " · check list limited"
                                    : ""}
                                </span>
                              ) : (
                                <span className="text-sm text-muted-foreground">
                                  No service observations
                                </span>
                              )}
                            </TableCell>
                            <TableCell className="text-right">
                              <StatusBadge
                                value={
                                  failing
                                    ? states.includes("down")
                                      ? "critical"
                                      : "warning"
                                    : device.status
                                }
                              />
                            </TableCell>
                          </TableRow>
                        );
                      })}
                    </TableBody>
                  </Table>
                </div>
              </>
            )}
          </CardContent>
        </Card>

        {selectedId ? (
          <DeviceDetail
            addressError={addressesQuery.error}
            addresses={addressesQuery.data?.items ?? EMPTY_ADDRESSES}
            detail={detailQuery.data}
            error={detailQuery.error}
            loading={detailQuery.isLoading}
            monitors={monitors}
            monitorsError={monitorsQuery.error}
            monitorsLoading={monitorsQuery.isLoading}
            monitorsPossiblyTruncated={monitorsPossiblyTruncated}
            services={services}
            servicesError={servicesQuery.error}
            servicesLoading={servicesQuery.isLoading}
            onBack={() => selectDevice(null)}
            onEdit={() => setDialog("edit")}
            onInstall={() => setDialog("agent-install")}
            onMerge={() => setDialog("merge")}
            onSplit={() => setDialog("split")}
            onUndo={(id) => undoMutation.mutate(id)}
            undoPending={undoMutation.isPending}
          />
        ) : null}
      </div>

      <details className="rounded-xl border bg-card">
        <summary className="cursor-pointer list-none px-5 py-4 outline-none focus-visible:ring-3 focus-visible:ring-ring/50">
          <span className="flex items-center justify-between gap-3">
            <span>
              <span className="block font-medium">Identity review</span>
              <span className="block text-sm text-muted-foreground">
                Resolve ambiguous matches when you are ready.
              </span>
            </span>
            <Badge variant={suggestions.length ? "outline" : "secondary"}>
              {suggestions.length} pending
            </Badge>
          </span>
        </summary>
        <div className="border-t p-4 sm:p-5">
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
        </div>
      </details>

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

function StatusBadge({ value }: { value: string }) {
  const variant =
    value === "critical" || value === "down"
      ? "critical"
      : value === "stale" || value === "warning" || value === "degraded"
        ? "attention"
        : value === "up" ||
            value === "active" ||
            value === "healthy" ||
            value === "recovered"
          ? "healthy"
          : value === "discovered"
            ? "info"
            : "neutral";
  return <Badge variant={variant}>{labelize(value)}</Badge>;
}

function DeviceFullScan({ device }: { device: DeviceDetail }) {
  const [open, setOpen] = useState(false);
  const queryClient = useQueryClient();
  const scanQuery = useQuery({
    queryKey: ["device-full-scan", device.id],
    queryFn: () => fetchDeviceFullScan(device.id),
    refetchInterval: (query) => {
      const run = query.state.data;
      return run?.status === "pending" || run?.status === "running"
        ? 2_000
        : false;
    },
  });
  const launchKey = useRef<string | null>(null);
  const launch = useMutation({
    mutationFn: () => {
      launchKey.current ??=
        globalThis.crypto?.randomUUID?.() ??
        `device-scan-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      return launchDeviceFullScan(device.id, launchKey.current);
    },
    onSuccess: (created) => {
      launchKey.current = null;
      queryClient.setQueryData(["device-full-scan", device.id], created);
      setOpen(false);
    },
  });
  const cancel = useMutation({
    mutationFn: cancelScanRun,
    onSuccess: (updated) =>
      queryClient.setQueryData(["device-full-scan", device.id], updated),
  });
  const current = scanQuery.data;
  const active = current?.status === "pending" || current?.status === "running";
  return (
    <>
      <Button
        disabled={launch.isPending || active}
        onClick={() => setOpen(true)}
        size="sm"
        variant="outline"
      >
        Full port scan
      </Button>
      <Dialog onOpenChange={setOpen} open={open}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              Full port scan for {device.name || "device"}
            </DialogTitle>
            <DialogDescription>
              Check all 65,535 TCP ports on one current device address in a
              confirmed network scope. This can take a long time.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button onClick={() => setOpen(false)} variant="outline">
              Cancel
            </Button>
            <Button disabled={launch.isPending} onClick={() => launch.mutate()}>
              {launch.isPending ? "Queueing…" : "Start full scan"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      {launch.error ? (
        <p className="text-xs text-destructive">
          {getUserFacingError(launch.error, "Full scan could not start.")}
        </p>
      ) : null}
      {current ? (
        <div
          aria-live="polite"
          className="w-full text-xs text-muted-foreground"
        >
          Full scan of {current.target_address || "device"}: {current.status}.{" "}
          {current.ports_completed.toLocaleString()} of{" "}
          {current.ports_planned.toLocaleString()} ports checked.
          {active ? (
            <Button
              disabled={cancel.isPending || current.cancellation_requested}
              onClick={() => cancel.mutate(current.id)}
              size="sm"
              variant="ghost"
            >
              {current.cancellation_requested
                ? "Cancellation requested"
                : "Cancel scan"}
            </Button>
          ) : null}
        </div>
      ) : null}
    </>
  );
}

function DeviceDetail({
  addressError,
  addresses,
  detail,
  loading,
  error,
  monitors,
  monitorsError,
  monitorsLoading,
  monitorsPossiblyTruncated,
  services,
  servicesError,
  servicesLoading,
  onBack,
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
  monitors: Monitor[];
  monitorsError: unknown;
  monitorsLoading: boolean;
  monitorsPossiblyTruncated: boolean;
  services: Service[];
  servicesError: unknown;
  servicesLoading: boolean;
  onBack: () => void;
  onEdit: () => void;
  onInstall: () => void;
  onMerge: () => void;
  onSplit: () => void;
  onUndo: (id: string) => void;
  undoPending: boolean;
}) {
  const [tab, setTab] = useState<"overview" | "activity" | "details">(
    "overview",
  );
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
  if (!detail) return null;
  const merged = detail.merged_member_ids.filter((id) => id !== detail.id);
  const deviceServices = services.filter(
    (service) =>
      service.owner_kind === "device" && service.owner_id === detail.id,
  );
  const serviceMonitors = monitors.filter((monitor) =>
    deviceServices.some((service) => service.id === monitor.service_id),
  );
  const currentAddresses = detail.interfaces.flatMap((networkInterface) =>
    addresses.filter(
      (address) =>
        address.interface_id === networkInterface.id && address.is_current,
    ),
  );
  const currentCondition = serviceMonitors.some(
    (monitor) => monitor.state === "down",
  )
    ? "critical"
    : serviceMonitors.some((monitor) =>
          ["degraded", "stale"].includes(monitor.state),
        )
      ? "warning"
      : detail.status;

  return (
    <Card className="min-w-0 overflow-hidden">
      <CardHeader className="border-b">
        <div className="min-w-0">
          <button
            className="mb-2 inline-flex min-h-9 items-center rounded-lg px-2 text-sm text-muted-foreground outline-none hover:bg-muted hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50 lg:hidden"
            onClick={onBack}
            type="button"
          >
            ← Devices
          </button>
          <CardTitle className="truncate">
            {detail.name || "Unnamed device"}
          </CardTitle>
          <CardDescription className="flex flex-wrap gap-x-2">
            <span>{labelize(detail.device_type)}</span>
            {currentAddresses[0] ? (
              <span className="font-mono">{currentAddresses[0].ip}</span>
            ) : null}
            <span>Record updated {formatRelative(detail.updated_at)}</span>
          </CardDescription>
        </div>
        <CardAction>
          <StatusBadge value={currentCondition} />
        </CardAction>
      </CardHeader>
      <CardContent className="p-0">
        <div className="flex flex-wrap gap-2 border-b px-5 py-3">
          <Button onClick={onEdit} size="sm" variant="outline">
            <PencilIcon data-icon="inline-start" />
            Edit
          </Button>
          <details className="relative">
            <summary className="inline-flex min-h-9 cursor-pointer list-none items-center rounded-lg border px-3 text-sm outline-none transition-colors hover:bg-muted focus-visible:ring-3 focus-visible:ring-ring/50">
              More actions
            </summary>
            <div className="absolute top-11 left-0 z-10 grid min-w-48 gap-1 rounded-lg border bg-popover p-1 shadow-lg">
              <Button
                className="justify-start"
                onClick={onInstall}
                size="sm"
                variant="ghost"
              >
                <ShieldCheckIcon data-icon="inline-start" />
                Deploy agent
              </Button>
              <DeviceFullScan key={detail.id} device={detail} />
            </div>
          </details>
        </div>
        <Tabs
          onValueChange={(value) => setTab(value as typeof tab)}
          value={tab}
        >
          <div className="border-b px-5 pt-2">
            <TabsList aria-label="Device details" variant="line">
              <TabsTrigger value="overview">Overview</TabsTrigger>
              <TabsTrigger value="activity">Activity</TabsTrigger>
              <TabsTrigger value="details">Details</TabsTrigger>
            </TabsList>
          </div>
          <TabsContent className="m-0 space-y-5 p-5" value="overview">
            {currentCondition === "critical" ||
            currentCondition === "warning" ? (
              <section className="rounded-lg border border-status-critical-border bg-status-critical-bg/40 p-4">
                <p className="font-medium">A service needs attention</p>
                <p className="mt-1 text-sm text-muted-foreground">
                  {serviceMonitors
                    .filter((monitor) =>
                      ["down", "degraded", "stale"].includes(monitor.state),
                    )
                    .map(
                      (monitor) =>
                        monitor.service_name ||
                        monitor.service_product ||
                        `Service ${shortId(monitor.service_id)}`,
                    )
                    .join(", ") || "The latest device condition needs review."}
                </p>
              </section>
            ) : null}
            <section>
              <div className="mb-3 flex items-center justify-between gap-3">
                <div>
                  <h3 className="font-medium">Services</h3>
                  <p className="text-sm text-muted-foreground">
                    Checks and observations attached to this device.
                  </p>
                </div>
                <Badge variant="secondary">{deviceServices.length}</Badge>
              </div>
              {servicesLoading ? (
                <p className="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
                  Loading service inventory…
                </p>
              ) : servicesError ? (
                <LoadError
                  error={servicesError}
                  title="Service inventory unavailable"
                />
              ) : deviceServices.length ? (
                <div className="divide-y rounded-lg border">
                  {deviceServices.map((service) => {
                    const checks = serviceMonitors.filter(
                      (monitor) => monitor.service_id === service.id,
                    );
                    return (
                      <div
                        className="flex flex-wrap items-center justify-between gap-3 p-3"
                        key={service.id}
                      >
                        <div className="min-w-0">
                          <p className="truncate font-medium">
                            {service.name ||
                              service.product ||
                              "Unnamed service"}
                          </p>
                          <p className="text-sm text-muted-foreground">
                            {[service.protocol, service.product_version]
                              .filter(Boolean)
                              .join(" · ") || "Observed service"}
                          </p>
                        </div>
                        <div className="flex flex-wrap items-center justify-end gap-2">
                          {monitorsLoading ? (
                            <Badge variant="secondary">Loading checks…</Badge>
                          ) : monitorsError ? (
                            <Badge variant="secondary">
                              Check data unavailable
                            </Badge>
                          ) : checks.length ? (
                            checks.map((monitor) => (
                              <a
                                className="rounded-md outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
                                href={`/monitoring?monitor=${encodeURIComponent(monitor.id)}`}
                                key={monitor.id}
                              >
                                <StatusBadge value={monitor.state} />
                              </a>
                            ))
                          ) : monitorsPossiblyTruncated ? (
                            <Badge variant="secondary">
                              Check list limited
                            </Badge>
                          ) : (
                            <Badge variant="secondary">No active check</Badge>
                          )}
                        </div>
                      </div>
                    );
                  })}
                </div>
              ) : (
                <p className="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
                  No service observations are associated with this device yet.
                </p>
              )}
            </section>
          </TabsContent>
          <TabsContent className="m-0 space-y-4 p-5" value="activity">
            <section>
              <h3 className="font-medium">Recent service activity</h3>
              <p className="mt-1 text-sm text-muted-foreground">
                Results are scoped to checks Hope knows about for this device.
              </p>
            </section>
            {servicesError ? (
              <LoadError
                error={servicesError}
                title="Service inventory unavailable"
              />
            ) : monitorsLoading ? (
              <p className="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
                Loading monitor activity…
              </p>
            ) : monitorsError ? (
              <LoadError
                error={monitorsError}
                title="Monitor activity unavailable"
              />
            ) : monitorsPossiblyTruncated && !serviceMonitors.length ? (
              <p className="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
                The check list is limited to the latest 100 checks. Open
                Monitoring to search the wider estate.
              </p>
            ) : serviceMonitors.length ? (
              <div className="divide-y rounded-lg border">
                {serviceMonitors.map((monitor) => (
                  <div
                    className="flex flex-wrap items-center justify-between gap-3 p-3"
                    key={monitor.id}
                  >
                    <div>
                      <p className="font-medium">
                        {monitor.service_name ||
                          monitor.service_product ||
                          `Service ${shortId(monitor.service_id)}`}
                      </p>
                      <p className="text-sm text-muted-foreground">
                        Last result {formatRelative(monitor.last_result_at)} ·{" "}
                        {labelize(monitor.monitor_type)}
                      </p>
                    </div>
                    <a
                      className="rounded-md outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
                      href={`/monitoring?monitor=${encodeURIComponent(monitor.id)}`}
                    >
                      <StatusBadge value={monitor.state} />
                    </a>
                  </div>
                ))}
              </div>
            ) : (
              <p className="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
                No monitor results are available for this device.
              </p>
            )}
          </TabsContent>
          <TabsContent className="m-0 space-y-5 p-5" value="details">
            <section>
              <div className="flex items-center justify-between gap-3">
                <div>
                  <h3 className="font-medium">Identity history</h3>
                  <p className="text-sm text-muted-foreground">
                    Evidence explains why this record represents this device.
                  </p>
                </div>
                <Badge variant="outline">
                  {Math.round(detail.identity_confidence * 100)}% confidence
                </Badge>
              </div>
              <div className="mt-3">
                <ConfidenceMeter value={detail.identity_confidence} />
              </div>
              {detail.evidence.length ? (
                <div className="mt-3 divide-y rounded-lg border">
                  {detail.evidence.slice(0, 8).map((evidence) => (
                    <div
                      className="flex items-start justify-between gap-3 p-3"
                      key={evidence.id}
                    >
                      <div className="min-w-0">
                        <p className="font-medium">
                          {labelize(evidence.attribute)}
                        </p>
                        <p className="break-words text-sm text-muted-foreground">
                          {formatEvidenceValue(evidence.value)}
                        </p>
                      </div>
                      <span className="shrink-0 text-right text-xs text-muted-foreground">
                        {Math.round(evidence.confidence * 100)}% ·{" "}
                        {labelize(evidence.source_type)}
                      </span>
                    </div>
                  ))}
                </div>
              ) : (
                <p className="mt-3 rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
                  No evidence collected.
                </p>
              )}
            </section>
            <section>
              <h3 className="mb-3 font-medium">Network interfaces</h3>
              {addressError ? (
                <LoadError
                  error={addressError}
                  title="Address history unavailable"
                />
              ) : null}
              {detail.interfaces.length ? (
                <div className="flex flex-col gap-2">
                  {detail.interfaces.map((networkInterface) => (
                    <InterfaceCard
                      addresses={addresses.filter(
                        (address) =>
                          address.interface_id === networkInterface.id,
                      )}
                      description={networkInterface.description}
                      id={networkInterface.id}
                      key={networkInterface.id}
                      mac={networkInterface.mac}
                    />
                  ))}
                </div>
              ) : (
                <p className="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
                  No interfaces attached.
                </p>
              )}
            </section>
            <section>
              <h3 className="mb-3 font-medium">Record actions</h3>
              <div className="flex flex-wrap gap-2">
                <Button onClick={onMerge} size="sm" variant="outline">
                  <GitMergeIcon data-icon="inline-start" />
                  Merge record
                </Button>
                <Button onClick={onSplit} size="sm" variant="outline">
                  <GitBranchIcon data-icon="inline-start" />
                  Split interfaces
                </Button>
              </div>
              {merged.length ? (
                <div className="mt-3 divide-y rounded-lg border">
                  {merged.map((id) => (
                    <div
                      className="flex items-center justify-between gap-3 p-3"
                      key={id}
                    >
                      <span className="text-sm">
                        Merged record{" "}
                        <span className="font-mono">{shortId(id)}</span>
                      </span>
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
              ) : null}
            </section>
          </TabsContent>
        </Tabs>
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
        {getUserFacingError(error, "Try again.")}
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
        {getUserFacingError(error, "Try again.")}
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
