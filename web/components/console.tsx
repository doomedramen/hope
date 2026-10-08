"use client";
import { useCallback, useEffect, useRef, useState } from "react";
import dynamic from "next/dynamic";
import {
  Activity,
  ArrowDownToLine,
  CircleHelp,
  LayoutList,
  Map,
  Network,
  Plus,
  Radar,
  Search,
  Settings2,
  ShieldCheck,
  LogOut,
  Play,
  Square,
  Server,
  Cpu,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import {
  Field,
  FieldGroup,
  FieldLabel,
  FieldDescription,
} from "@/components/ui/field";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  Empty,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
  EmptyDescription,
  EmptyContent,
} from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import { Progress } from "@/components/ui/progress";
import {
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectGroup,
  SelectItem,
} from "@/components/ui/select";
import { InstallDialog } from "@/components/install-dialog";
import { HostDetails } from "@/components/host-details";
import { api, post, put, bytes, age, percentage, status } from "@/lib/api";
import type {
  AuthStatus as Auth,
  Bootstrap,
  Host,
  MapState,
  Network as HopeNetwork,
} from "@/lib/protocol";

const NetworkMap = dynamic(
  () => import("@/components/network-map").then((m) => m.NetworkMap),
  { ssr: false, loading: () => <Skeleton className="h-[65vh] w-full" /> },
);

export function Console() {
  const [auth, setAuth] = useState<Auth | null>(null);
  const [error, setError] = useState("");
  const check = useCallback(
    () =>
      api<Auth>("/auth/status")
        .then(setAuth)
        .catch((e) => setError(e.message)),
    [],
  );
  useEffect(() => {
    void check();
  }, [check]);
  if (error)
    return (
      <main className="auth-shell">
        <Alert variant="destructive">
          <AlertTitle>Cannot reach H.O.P.E</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
        <Button
          onClick={() => {
            setError("");
            void check();
          }}
        >
          Retry connection
        </Button>
      </main>
    );
  if (!auth)
    return (
      <main className="auth-shell">
        <Brand />
        <Skeleton className="h-48 w-full" />
      </main>
    );
  if (!auth.authenticated)
    return <SignIn setup={auth.setup_required} onComplete={check} />;
  return (
    <Dashboard
      onLogout={async () => {
        await post("/logout");
        await check();
      }}
    />
  );
}
function Brand() {
  return (
    <div className="brand">
      <div className="brand-mark">
        <Network size={22} />
      </div>
      <span>
        H.O.P.E<span className="brand-subtitle">Network observatory</span>
      </span>
    </div>
  );
}
function SignIn({
  setup,
  onComplete,
}: {
  setup: boolean;
  onComplete: () => Promise<void>;
}) {
  const [password, setPassword] = useState("");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      await post(setup ? "/auth/setup" : "/auth/login", {
        password,
        setup_token: token,
      });
      await onComplete();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <main className="auth-shell">
      <Brand />
      <div className="auth-panel">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">
            {setup ? "Meet your network." : "Welcome back."}
          </h1>
          <p className="mt-2 text-sm text-muted-foreground">
            {setup
              ? "Create your administrator account to start discovering devices."
              : "Sign in to your H.O.P.E observatory."}
          </p>
        </div>
        <form onSubmit={submit} className="flex flex-col gap-6">
          {error && (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          <FieldGroup>
            {setup && (
              <Field>
                <FieldLabel htmlFor="setup-token">Setup token</FieldLabel>
                <Input
                  id="setup-token"
                  type="password"
                  autoComplete="off"
                  required
                  value={token}
                  onChange={(e) => setToken(e.target.value)}
                />
                <FieldDescription>
                  Find this token in your H.O.P.E server startup logs.
                </FieldDescription>
              </Field>
            )}
            <Field>
              <FieldLabel htmlFor="password">Administrator password</FieldLabel>
              <Input
                id="password"
                type="password"
                required
                minLength={setup ? 12 : undefined}
                maxLength={256}
                autoComplete={setup ? "new-password" : "current-password"}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
              {setup && (
                <FieldDescription>Use at least 12 characters.</FieldDescription>
              )}
            </Field>
          </FieldGroup>
          <Button type="submit" disabled={busy}>
            {busy ? "Connecting…" : setup ? "Create account" : "Sign in"}
          </Button>
        </form>
      </div>
      <p className="text-center text-xs text-muted-foreground">
        Your devices. Your data. Your server.
      </p>
    </main>
  );
}

function Dashboard({ onLogout }: { onLogout: () => Promise<void> }) {
  const [data, setData] = useState<Bootstrap | null>(null);
  const [error, setError] = useState("");
  const [connected, setConnected] = useState(false);
  const [view, setView] = useState("map");
  const [mode, setMode] = useState<"topology" | "subnets" | "health">(
    "topology",
  );
  const [filter, setFilter] = useState("");
  const [network, setNetwork] = useState<string | null>("all");
  const [selected, setSelected] = useState<string | null>(() =>
    typeof window === "undefined"
      ? null
      : new URLSearchParams(window.location.search).get("host"),
  );
  const [networkOpen, setNetworkOpen] = useState(false);
  const [installOpen, setInstallOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const refreshPending = useRef(false);
  const refresh = useCallback(async () => {
    if (refreshPending.current) return;
    refreshPending.current = true;
    try {
      setData(await api<Bootstrap>("/bootstrap"));
      setError("");
    } catch (e) {
      setError((e as Error).message);
    } finally {
      refreshPending.current = false;
    }
  }, []);
  useEffect(() => {
    const initialRefresh = setTimeout(() => {
      void refresh();
    }, 0);
    const source = new EventSource("/api/v1/events");
    let timer: ReturnType<typeof setTimeout> | undefined;
    source.onopen = () => setConnected(true);
    source.onerror = () => setConnected(false);
    source.addEventListener("refresh", () => {
      if (!timer)
        timer = setTimeout(() => {
          timer = undefined;
          void refresh();
        }, 500);
    });
    const fallback = setInterval(() => {
      void refresh();
    }, 15000);
    const navigate = () =>
      setSelected(new URLSearchParams(window.location.search).get("host"));
    window.addEventListener("popstate", navigate);
    return () => {
      clearTimeout(initialRefresh);
      source.close();
      clearTimeout(timer);
      clearInterval(fallback);
      window.removeEventListener("popstate", navigate);
    };
  }, [refresh]);
  const select = (id: string | null) => {
    setSelected(id);
    const url = new URL(window.location.href);
    if (id) url.searchParams.set("host", id);
    else url.searchParams.delete("host");
    window.history.replaceState({}, "", url);
  };
  async function action(work: () => Promise<unknown>) {
    setBusy(true);
    setError("");
    try {
      await work();
      await refresh();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function saveMap(map: MapState) {
    try {
      const saved = await put<MapState>("/map", map);
      setData((previous) =>
        previous ? { ...previous, map: saved } : previous,
      );
    } catch (e) {
      setError((e as Error).message);
    }
  }
  const hosts = data?.hosts ?? [];
  const networks = data?.networks ?? [];
  const visible = hosts.filter(
    (h) =>
      (network === "all" || h.network_ids.includes(network ?? "")) &&
      `${h.name} ${h.addresses.join(" ")} ${h.macs.join(" ")}`
        .toLowerCase()
        .includes(filter.toLowerCase()),
  );
  const host = hosts.find((h) => h.id === selected) ?? null;
  const reporting = hosts.filter((h) => status(h) === "Reporting").length;
  const scans =
    data?.scans.filter(
      (job, i, all) =>
        all.findIndex((j) => j.network_id === job.network_id) === i,
    ) ?? [];
  return (
    <div className="app-shell">
      <aside className="sidebar">
        <Brand />
        <nav className="flex flex-col gap-1" aria-label="Main navigation">
          <Button
            variant={view === "map" ? "secondary" : "ghost"}
            className="justify-start"
            onClick={() => setView("map")}
          >
            <Map data-icon="inline-start" />
            Network map
          </Button>
          <Button
            variant={view === "hosts" ? "secondary" : "ghost"}
            className="justify-start"
            onClick={() => setView("hosts")}
          >
            <LayoutList data-icon="inline-start" />
            Hosts
            <Badge variant="outline" className="ml-auto">
              {hosts.length}
            </Badge>
          </Button>
          <Button
            variant="ghost"
            className="justify-start"
            onClick={() => setSettingsOpen(true)}
          >
            <Settings2 data-icon="inline-start" />
            Networks & settings
          </Button>
        </nav>
        <div className="sidebar-networks">
          <h2>Monitored networks</h2>
          {networks.length ? (
            networks.map((n) => (
              <button
                key={n.id}
                className="network-shortcut"
                onClick={() => setNetwork(n.id)}
              >
                <span
                  className={`status-dot ${n.enabled ? "live" : "stale"}`}
                />
                <span>
                  {n.name}
                  <small>{n.cidr}</small>
                </span>
              </button>
            ))
          ) : (
            <p>Add your first network to begin.</p>
          )}
          <Button
            variant="ghost"
            size="sm"
            className="mt-3 justify-start"
            onClick={() => setNetworkOpen(true)}
          >
            <Plus data-icon="inline-start" />
            Add network
          </Button>
        </div>
        <div className="sidebar-bottom">
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <ShieldCheck size={15} />
            Self-hosted. Locally observed.
          </div>
          <Button
            variant="ghost"
            className="justify-start"
            onClick={() => {
              void action(onLogout);
            }}
          >
            <LogOut data-icon="inline-start" />
            Sign out
          </Button>
        </div>
      </aside>
      <main className="workspace">
        <header className="topbar">
          <div className="flex items-center gap-2 text-sm">
            <span className={`status-dot ${connected ? "live" : "stale"}`} />
            {connected ? "Live connection" : "Reconnecting"}
            <span className="hidden text-muted-foreground sm:inline">
              / Network observatory
            </span>
          </div>
          <Button variant="outline" onClick={() => setInstallOpen(true)}>
            <ArrowDownToLine data-icon="inline-start" />
            Add agent
          </Button>
        </header>
        <div className="page-heading">
          <div>
            <h1>
              {view === "map"
                ? "Your network, connected."
                : "Every host, in view."}
            </h1>
            <p>
              {view === "map"
                ? "Explore the devices and relationships behind your network."
                : "Discovery, hardware, and live health in one place."}
            </p>
          </div>
          <Button
            onClick={() => {
              if (!networks.length) {
                setNetworkOpen(true);
                return;
              }
              void action(async () => {
                const targets = networks.filter(
                  (n) => n.enabled && (network === "all" || n.id === network),
                );
                const results = await Promise.allSettled(
                  targets.map((n) => post(`/networks/${n.id}/scan`)),
                );
                const failed = results.filter((r) => r.status === "rejected");
                if (failed.length)
                  throw new Error(
                    `${failed.length} scan(s) could not start. A range may already be scanning.`,
                  );
              });
            }}
            disabled={busy}
          >
            <Radar data-icon="inline-start" />
            Discover devices
          </Button>
        </div>
        {error && (
          <Alert variant="destructive" className="mb-5">
            <AlertTitle>Action needs attention</AlertTitle>
            <AlertDescription>
              {error}
              <Button
                variant="link"
                onClick={() => {
                  void refresh();
                }}
              >
                Refresh
              </Button>
            </AlertDescription>
          </Alert>
        )}
        <div className="overview-strip">
          <div>
            <Server size={19} />
            <span>
              <strong>{hosts.length}</strong>Discovered hosts
            </span>
          </div>
          <div>
            <Activity size={19} />
            <span>
              <strong>
                {reporting}
                <small> / {hosts.filter((h) => h.agent_id).length}</small>
              </strong>
              Agents reporting
            </span>
          </div>
          <div>
            <Network size={19} />
            <span>
              <strong>{networks.length}</strong>Network ranges
            </span>
          </div>
          <div>
            <Cpu size={19} />
            <span>
              <strong>
                7 <small>days</small>
              </strong>
              Metric history
            </span>
          </div>
        </div>
        {scans.length > 0 && (
          <section className="scan-strip" aria-label="Scan progress">
            {scans.map((scan) => (
              <div key={scan.id} className="scan-item">
                <div className="flex items-center justify-between gap-3">
                  <span className="text-sm font-medium">
                    {networks.find((n) => n.id === scan.network_id)?.name ??
                      scan.cidr}
                  </span>
                  <Badge variant="secondary">
                    {scan.status === "completed"
                      ? "Discovery complete"
                      : scan.status}
                  </Badge>
                </div>
                <Progress
                  value={scan.total ? (scan.scanned / scan.total) * 100 : 0}
                  aria-label={`${scan.cidr} scan progress`}
                />
                <div className="flex items-center justify-between text-xs text-muted-foreground">
                  <span>
                    {scan.scanned} / {scan.total} addresses · {scan.discovered}{" "}
                    observed
                  </span>
                  {scan.status === "running" ? (
                    <Button
                      variant="ghost"
                      size="xs"
                      onClick={() => {
                        void action(() => post(`/scans/${scan.id}/cancel`));
                      }}
                    >
                      <Square data-icon="inline-start" />
                      Cancel
                    </Button>
                  ) : (
                    <span>{age(scan.finished_at)}</span>
                  )}
                </div>
                {scan.error && (
                  <p className="text-xs text-destructive">{scan.error}</p>
                )}
              </div>
            ))}
          </section>
        )}
        {data?.capabilities.limitations.length ? (
          <div className="capability-notice">
            <CircleHelp size={16} />
            <span>{data.capabilities.limitations.join(" ")}</span>
          </div>
        ) : null}
        <div className="inventory-layout">
          <section className="inventory-main">
            <div className="view-toolbar">
              <div className="flex flex-wrap items-center gap-3">
                <ToggleGroup
                  value={[view]}
                  onValueChange={(v) => {
                    if (v[0]) setView(v[0]);
                  }}
                  variant="outline"
                  size="sm"
                  aria-label="Inventory view"
                >
                  <ToggleGroupItem value="map">
                    <Map data-icon="inline-start" />
                    Map
                  </ToggleGroupItem>
                  <ToggleGroupItem value="hosts">
                    <LayoutList data-icon="inline-start" />
                    Hosts
                  </ToggleGroupItem>
                </ToggleGroup>
                {view === "map" && (
                  <ToggleGroup
                    value={[mode]}
                    onValueChange={(v) => {
                      if (["topology", "subnets", "health"].includes(v[0]))
                        setMode(v[0] as typeof mode);
                    }}
                    size="sm"
                    aria-label="Map mode"
                  >
                    <ToggleGroupItem value="topology">Topology</ToggleGroupItem>
                    <ToggleGroupItem value="subnets">Subnets</ToggleGroupItem>
                    <ToggleGroupItem value="health">Health</ToggleGroupItem>
                  </ToggleGroup>
                )}
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <Select value={network} onValueChange={setNetwork}>
                  <SelectTrigger aria-label="Filter by network">
                    <SelectValue>
                      {network === "all"
                        ? "All networks"
                        : networks.find((n) => n.id === network)?.name}
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectItem value="all">All networks</SelectItem>
                      {networks.map((n) => (
                        <SelectItem value={n.id} key={n.id}>
                          {n.name}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
                <div className="search-box">
                  <Search size={15} />
                  <Input
                    aria-label="Search hosts"
                    placeholder="Search hosts…"
                    value={filter}
                    onChange={(e) => setFilter(e.target.value)}
                  />
                </div>
              </div>
            </div>
            {!data ? (
              <Skeleton className="h-[60vh] w-full" />
            ) : !hosts.length ? (
              <Empty className="min-h-[480px]">
                <EmptyHeader>
                  <EmptyMedia variant="icon">
                    <Radar />
                  </EmptyMedia>
                  <EmptyTitle>Your network starts here</EmptyTitle>
                  <EmptyDescription>
                    Add a CIDR range to find devices, or install an agent to
                    start reporting hardware and health.
                  </EmptyDescription>
                </EmptyHeader>
                <EmptyContent>
                  <Button onClick={() => setNetworkOpen(true)}>
                    <Plus data-icon="inline-start" />
                    Add a network
                  </Button>
                  <Button
                    variant="outline"
                    onClick={() => setInstallOpen(true)}
                  >
                    Install an agent
                  </Button>
                </EmptyContent>
              </Empty>
            ) : !visible.length ? (
              <Empty className="min-h-[400px]">
                <EmptyHeader>
                  <EmptyTitle>No matching hosts</EmptyTitle>
                  <EmptyDescription>
                    Try another network or search term.
                  </EmptyDescription>
                </EmptyHeader>
                <Button
                  variant="outline"
                  onClick={() => {
                    setFilter("");
                    setNetwork("all");
                  }}
                >
                  Clear filters
                </Button>
              </Empty>
            ) : view === "map" ? (
              <NetworkMap
                hosts={visible}
                networks={networks.filter(
                  (n) => network === "all" || n.id === network,
                )}
                layout={data.map}
                mode={mode}
                selected={selected}
                onSelect={select}
                onSave={saveMap}
              />
            ) : (
              <HostTable hosts={visible} selected={selected} select={select} />
            )}
            <footer className="inventory-footer">
              <span>{visible.length} hosts in view</span>
              <span>Discovery runs every five minutes for enabled ranges.</span>
            </footer>
          </section>
          {host && (
            <HostDetails
              key={host.id}
              host={host}
              hosts={hosts}
              close={() => select(null)}
              install={() => setInstallOpen(true)}
              refresh={() => {
                void refresh();
              }}
            />
          )}
        </div>
      </main>
      <NetworkDialog
        open={networkOpen}
        onOpenChange={setNetworkOpen}
        onAdded={() => {
          void refresh();
        }}
      />
      <InstallDialog
        open={installOpen}
        onOpenChange={setInstallOpen}
        host={host}
      />
      <Dialog open={settingsOpen} onOpenChange={setSettingsOpen}>
        <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl">
          <DialogHeader>
            <DialogTitle>Networks & settings</DialogTitle>
            <DialogDescription>
              Control continuous discovery and review server capabilities.
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-4">
            {networks.map((n) => (
              <div
                key={n.id}
                className="flex items-center justify-between gap-3 rounded-lg border p-3"
              >
                <div>
                  <strong className="text-sm">{n.name}</strong>
                  <p className="text-xs text-muted-foreground">{n.cidr}</p>
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    void action(() =>
                      put(`/networks/${n.id}`, { enabled: !n.enabled }),
                    );
                  }}
                >
                  {n.enabled ? (
                    <Square data-icon="inline-start" />
                  ) : (
                    <Play data-icon="inline-start" />
                  )}
                  {n.enabled ? "Pause monitoring" : "Enable monitoring"}
                </Button>
              </div>
            ))}
            <Button
              variant="outline"
              onClick={() => {
                setSettingsOpen(false);
                setNetworkOpen(true);
              }}
            >
              <Plus data-icon="inline-start" />
              Add network
            </Button>
            <dl className="detail-list">
              <dt>Server</dt>
              <dd>{data?.server_url}</dd>
              <dt>Platform</dt>
              <dd>{data?.capabilities.platform}</dd>
              <dt>Discovery</dt>
              <dd>{data?.capabilities.discovery_mode}</dd>
              <dt>ICMP</dt>
              <dd>{data?.capabilities.icmp ? "Available" : "Unavailable"}</dd>
              <dt>ARP</dt>
              <dd>
                {data?.capabilities.arp
                  ? "Available on local link"
                  : "Unavailable"}
              </dd>
            </dl>
            <p className="text-xs text-muted-foreground">
              Agent reports use 30-second intervals. Full-resolution samples
              last 24 hours, with five-minute averages retained through day
              seven.
            </p>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
function HostTable({
  hosts,
  selected,
  select,
}: {
  hosts: Host[];
  selected: string | null;
  select: (id: string) => void;
}) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Host</TableHead>
          <TableHead>Address</TableHead>
          <TableHead>Status</TableHead>
          <TableHead>CPU</TableHead>
          <TableHead>Memory</TableHead>
          <TableHead>Last seen</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {hosts.map((h) => (
          <TableRow
            key={h.id}
            data-state={h.id === selected ? "selected" : undefined}
          >
            <TableCell>
              <Button
                variant="link"
                className="px-0"
                onClick={() => select(h.id)}
              >
                {h.name}
              </Button>
              <div className="text-xs text-muted-foreground">
                {h.inventory?.os ?? "Discovered device"}
              </div>
            </TableCell>
            <TableCell>{h.addresses[0] ?? "Unknown"}</TableCell>
            <TableCell>
              <Badge variant="secondary">{status(h)}</Badge>
            </TableCell>
            <TableCell>{percentage(h.metrics?.cpu_percent)}</TableCell>
            <TableCell>
              {h.inventory ? bytes(h.inventory.memory_total_bytes) : "—"}
            </TableCell>
            <TableCell>{age(h.last_seen)}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
function NetworkDialog({
  open,
  onOpenChange,
  onAdded,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  onAdded: () => void;
}) {
  const [name, setName] = useState("");
  const [cidr, setCidr] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      const network = await post<HopeNetwork>("/networks", {
        name,
        cidr,
        enabled: true,
      });
      onAdded();
      try {
        await post(`/networks/${network.id}/scan`);
      } catch (e) {
        setError(
          `Network saved, but scanning could not start: ${(e as Error).message}`,
        );
        return;
      }
      onOpenChange(false);
      setName("");
      setCidr("");
      onAdded();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add a network</DialogTitle>
          <DialogDescription>
            H.O.P.E discovers reachable devices and keeps watching for changes.
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={submit} className="flex flex-col gap-5">
          {error && (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="network-name">Network name</FieldLabel>
              <Input
                id="network-name"
                placeholder="Home network"
                required
                maxLength={120}
                value={name}
                onChange={(e) => setName(e.target.value)}
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="network-cidr">IPv4 CIDR</FieldLabel>
              <Input
                id="network-cidr"
                placeholder="192.168.1.0/24"
                required
                value={cidr}
                onChange={(e) => setCidr(e.target.value)}
              />
              <FieldDescription>
                The server must have a route to this network. Use a /20 or
                smaller range.
              </FieldDescription>
            </Field>
          </FieldGroup>
          <Button disabled={busy} type="submit">
            {busy ? "Adding network…" : "Add and discover"}
          </Button>
        </form>
      </DialogContent>
    </Dialog>
  );
}
