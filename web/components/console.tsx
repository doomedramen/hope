"use client";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import dynamic from "next/dynamic";
import {
  ArrowDownToLine,
  BellRing,
  CircleHelp,
  LayoutDashboard,
  LayoutList,
  LogOut,
  Map,
  Monitor,
  Moon,
  Plus,
  Radar,
  Search,
  Settings2,
  Sun,
  UserRound,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import {
  Empty,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
  EmptyDescription,
  EmptyContent,
} from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
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
import { HostTable } from "@/components/host-table";
import { Overview } from "@/components/overview";
import { NetworkDialog } from "@/components/network-dialog";
import {
  INTERVALS,
  NetworksDialog,
  reportIntervalLabel,
} from "@/components/networks-dialog";
import { AccountDialog } from "@/components/account-dialog";
import { NotificationsDialog } from "@/components/notifications-dialog";
import { DiscoveryMenu } from "@/components/discovery-menu";
import { Brand, SignIn } from "@/components/sign-in";
import {
  CommandPalette,
  commandIcons,
  type Command,
} from "@/components/command-palette";
import { ToastProvider, useToast } from "@/components/toast";
import { api, del, post, put } from "@/lib/api";
import { useTheme } from "@/lib/theme";
import {
  HOST_FILTERS,
  hostState,
  matchesFilter,
  matchesSearch,
  type HostFilter,
} from "@/lib/fleet";
import type {
  AuthStatus as Auth,
  Bootstrap,
  Host,
  MapState,
  MapEdit,
  Network as HopeNetwork,
  ScanJob,
  UpdateRequested,
} from "@/lib/protocol";

const NetworkMap = dynamic(
  () => import("@/components/network-map").then((m) => m.NetworkMap),
  { ssr: false, loading: () => <Skeleton className="h-[65vh] w-full" /> },
);

type View = "overview" | "map" | "hosts" | "host";
type Mode = "topology" | "subnets" | "health";
const VIEWS: View[] = ["overview", "map", "hosts", "host"];
const TITLES: Record<View, string> = {
  overview: "Overview",
  map: "Devices",
  hosts: "Devices",
  host: "Host",
};

function readLocation(): { view: View; host: string | null } {
  if (typeof window === "undefined") return { view: "overview", host: null };
  const params = new URLSearchParams(window.location.search);
  const view = params.get("view");
  const host = params.get("host");
  // A shared host link without a view opens that host's page.
  return {
    view: VIEWS.includes(view as View)
      ? (view as View)
      : host
        ? "host"
        : "overview",
    host,
  };
}

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
    const expired = () =>
      setAuth((current) =>
        current ? { ...current, authenticated: false } : current,
      );
    window.addEventListener("hope:session-expired", expired);
    return () => window.removeEventListener("hope:session-expired", expired);
  }, [check]);
  if (error)
    return (
      <main className="auth-center">
        <div className="flex flex-col gap-4">
          <Brand />
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
        </div>
      </main>
    );
  if (!auth)
    return (
      <main className="auth-center">
        <div className="flex flex-col gap-6">
          <Brand />
          <Skeleton className="h-48 w-full" />
        </div>
      </main>
    );
  if (!auth.authenticated)
    return <SignIn setup={auth.setup_required} onComplete={check} />;
  return (
    <ToastProvider>
      <Dashboard
        onLogout={async () => {
          await post("/logout");
          await check();
        }}
      />
    </ToastProvider>
  );
}

function Dashboard({ onLogout }: { onLogout: () => Promise<void> }) {
  const toast = useToast();
  const { theme, dark, setTheme } = useTheme();
  const [data, setData] = useState<Bootstrap | null>(null);
  const [error, setError] = useState("");
  const [connected, setConnected] = useState(false);
  const [location, setLocation] = useState(readLocation);
  const view = location.view;
  const selected = location.host;
  const [mode, setMode] = useState<Mode>("topology");
  const [filter, setFilter] = useState("");
  const [statusFilter, setStatusFilter] = useState<HostFilter>("all");
  const [network, setNetwork] = useState<string | null>("all");
  const [networkOpen, setNetworkOpen] = useState(false);
  const [installOpen, setInstallOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [accountOpen, setAccountOpen] = useState(false);
  const [notificationsOpen, setNotificationsOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  const refreshPending = useRef(false);
  const knownHosts = useRef(new Set<string>());
  const refresh = useCallback(async () => {
    if (refreshPending.current) return;
    refreshPending.current = true;
    try {
      const latest = await api<Bootstrap>("/bootstrap");
      knownHosts.current = new Set(latest.hosts.map((host) => host.id));
      setData((previous) => {
        if (!previous) return latest;
        // A live host update may have landed while this snapshot was in flight.
        const live = new globalThis.Map(
          previous.hosts.map((host) => [host.id, host]),
        );
        const hosts = latest.hosts.map((host) => {
          const newer = live.get(host.id);
          return newer && (newer.last_report ?? 0) > (host.last_report ?? 0)
            ? newer
            : host;
        });
        return {
          ...latest,
          hosts,
          map:
            previous.map.revision > latest.map.revision
              ? previous.map
              : latest.map,
        };
      });
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
    const schedule = () => {
      if (!timer)
        timer = setTimeout(() => {
          timer = undefined;
          void refresh();
        }, 500);
    };
    source.addEventListener("refresh", schedule);
    // An agent report carries the host's new state: apply it in place.
    source.addEventListener("host", (event) => {
      let host: Host;
      try {
        host = JSON.parse((event as MessageEvent<string>).data) as Host;
      } catch {
        return schedule();
      }
      if (!knownHosts.current.has(host.id)) return schedule();
      setData((previous) =>
        previous
          ? {
              ...previous,
              hosts: previous.hosts.map((existing) =>
                existing.id === host.id ? host : existing,
              ),
            }
          : previous,
      );
    });
    // The server dropped events for this slow client: reload everything.
    source.addEventListener("resync", schedule);
    const fallback = setInterval(() => {
      void refresh();
    }, 15000);
    const navigate = () => setLocation(readLocation());
    window.addEventListener("popstate", navigate);
    return () => {
      clearTimeout(initialRefresh);
      source.close();
      clearTimeout(timer);
      clearInterval(fallback);
      window.removeEventListener("popstate", navigate);
    };
  }, [refresh]);
  const go = useCallback(
    (next: Partial<{ view: View; host: string | null }>) => {
      setLocation((current) => {
        const target = { ...current, ...next };
        const url = new URL(window.location.href);
        if (target.view === "overview") url.searchParams.delete("view");
        else url.searchParams.set("view", target.view);
        if (target.host) url.searchParams.set("host", target.host);
        else url.searchParams.delete("host");
        if (url.href !== window.location.href)
          window.history.pushState({}, "", url);
        return target;
      });
    },
    [],
  );
  const select = useCallback(
    (id: string | null) => {
      go(
        id && window.matchMedia("(max-width: 720px)").matches
          ? { view: "host", host: id }
          : { host: id },
      );
    },
    [go],
  );
  const setView = useCallback(
    (next: View) => {
      if (next === "hosts" || next === "map") {
        try {
          localStorage.setItem("hope-device-view", next);
        } catch {
          /* Storage may be unavailable. */
        }
      }
      go({ view: next });
    },
    [go],
  );
  function openDevices() {
    let preferred: View = "hosts";
    try {
      if (localStorage.getItem("hope-device-view") === "map") preferred = "map";
    } catch {
      /* Use the list when storage is unavailable. */
    }
    setView(preferred);
  }
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
  const [unsavedMap, setUnsavedMap] = useState<MapEdit[]>([]);
  const mapQueue = useRef<Promise<void>>(Promise.resolve());
  function saveMap(edit: MapEdit): Promise<void> {
    const task = mapQueue.current.then(() => performSaveMap(edit));
    mapQueue.current = task.catch(() => {});
    return task;
  }
  async function performSaveMap(edit: MapEdit) {
    try {
      const saved = await api<MapState>("/map", {
        method: "PATCH",
        body: JSON.stringify(edit),
      });
      if (edit.operation === "set_position")
        setUnsavedMap((pending) =>
          pending.filter(
            (old) =>
              old.operation !== "set_position" ||
              old.position.id !== edit.position.id,
          ),
        );
      setData((previous) =>
        previous && saved.revision >= previous.map.revision
          ? { ...previous, map: saved }
          : previous,
      );
    } catch (e) {
      setUnsavedMap((pending) => [
        ...pending.filter(
          (old) =>
            edit.operation !== "set_position" ||
            old.operation !== "set_position" ||
            old.position.id !== edit.position.id,
        ),
        edit,
      ]);
      setError(`Map change not saved: ${(e as Error).message}`);
    }
  }
  async function retryMap() {
    const pending = unsavedMap;
    setUnsavedMap([]);
    setError("");
    for (const edit of pending) await saveMap(edit);
  }
  const hosts = useMemo(() => data?.hosts ?? [], [data]);
  const networks = useMemo(() => data?.networks ?? [], [data]);
  const visible = hosts.filter(
    (h) =>
      (network === "all" || h.network_ids.includes(network ?? "")) &&
      matchesFilter(h, statusFilter) &&
      matchesSearch(h, filter),
  );
  const host = hosts.find((h) => h.id === selected) ?? null;
  const scans =
    data?.scans.filter(
      (job, i, all) =>
        all.findIndex((j) => j.network_id === job.network_id) === i,
    ) ?? [];
  function discover(targetNetwork: HopeNetwork | null = null) {
    if (!networks.length) {
      setNetworkOpen(true);
      return;
    }
    void action(async () => {
      const targets = targetNetwork
        ? [targetNetwork]
        : networks.filter(
            (n) =>
              n.enabled &&
              !scans.some(
                (s) =>
                  s.network_id === n.id &&
                  (s.status === "running" || s.status === "queued"),
              ),
          );
      if (!targets.length)
        throw new Error(
          "No enabled networks are ready to scan. Check Settings or wait for current scans to finish.",
        );
      const results = await Promise.allSettled(
        targets.map((n) => post(`/networks/${n.id}/scan`)),
      );
      const failed = results.filter((r) => r.status === "rejected");
      const started = targets.length - failed.length;
      if (started)
        toast(
          `Discovery started on ${started} ${started === 1 ? "range" : "ranges"}.`,
          "good",
        );
      if (failed.length)
        throw new Error(
          `${failed.length} scan(s) could not start. A range may already be scanning.`,
        );
    });
  }
  const cancelScan = (scan: ScanJob) =>
    void action(() => post(`/scans/${scan.id}/cancel`));
  const openView = (next: View, which?: "attention") => {
    setStatusFilter(which ?? "all");
    setFilter("");
    setNetwork("all");
    setView(next);
  };
  // The inventory view a host page returns to.
  const lastInventory = useRef<"map" | "hosts">("hosts");
  useEffect(() => {
    if (view === "map" || view === "hosts") lastInventory.current = view;
  }, [view]);
  const selectHost = useCallback(
    (target: Host) => {
      if (view === "overview" || view === "host")
        go({ view: "host", host: target.id });
      else select(target.id);
    },
    [go, select, view],
  );
  const closeHostPage = useCallback(
    () => go({ view: lastInventory.current, host: null }),
    [go],
  );
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const typing =
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable);
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen((open) => !open);
      } else if (e.key === "/" && !typing && !e.metaKey && !e.ctrlKey) {
        if (view === "overview") {
          e.preventDefault();
          setPaletteOpen(true);
        } else if (searchRef.current) {
          e.preventDefault();
          searchRef.current.focus();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [view]);
  const commands: Command[] = [
    {
      id: "overview",
      label: "Go to overview",
      hint: "view",
      icon: commandIcons.overview,
      run: () => setView("overview"),
    },
    {
      id: "map",
      label: "Go to network map",
      hint: "view",
      icon: commandIcons.map,
      run: () => setView("map"),
    },
    {
      id: "hosts",
      label: "Go to hosts",
      hint: "view",
      icon: commandIcons.hosts,
      run: () => setView("hosts"),
    },
    {
      id: "discover",
      label: "Discover devices now",
      hint: "action",
      icon: commandIcons.discover,
      run: () => discover(),
    },
    {
      id: "network",
      label: "Add a network range",
      hint: "action",
      icon: commandIcons.network,
      run: () => setNetworkOpen(true),
    },
    {
      id: "agent",
      label: "Add a reporting agent",
      hint: "action",
      icon: commandIcons.agent,
      run: () => setInstallOpen(true),
    },
    {
      id: "settings",
      label: "Settings",
      hint: "action",
      icon: commandIcons.settings,
      run: () => setSettingsOpen(true),
    },
    {
      id: "notifications",
      label: "Notification settings",
      hint: "action",
      icon: commandIcons.notifications,
      run: () => setNotificationsOpen(true),
    },
    {
      id: "password",
      label: "Change password",
      hint: "account",
      icon: commandIcons.account,
      run: () => setAccountOpen(true),
    },
    {
      id: "theme",
      label: dark ? "Switch to light theme" : "Switch to dark theme",
      hint: "appearance",
      icon: dark ? commandIcons.theme : commandIcons.themeDark,
      run: () => setTheme(dark ? "light" : "dark"),
    },
    {
      id: "logout",
      label: "Sign out",
      hint: "session",
      icon: commandIcons.logout,
      run: () => void action(onLogout),
    },
  ];
  const navItem = (
    target: View,
    Icon: typeof LayoutDashboard,
    label: string,
    count?: number,
  ) => (
    <button
      type="button"
      className="nav-item"
      aria-label={label}
      aria-current={view === target ? "page" : undefined}
      onClick={() => setView(target)}
    >
      <Icon />
      <span>{label}</span>
      {count != null && <span className="count">{count}</span>}
    </button>
  );
  return (
    <div className="app-shell">
      <aside className="sidebar">
        <Brand />
        <nav aria-label="Main navigation">
          {navItem("overview", LayoutDashboard, "Overview")}
          <button
            type="button"
            className="nav-item"
            aria-label="Devices"
            aria-current={
              view === "map" || view === "hosts" || view === "host"
                ? "page"
                : undefined
            }
            onClick={openDevices}
          >
            <LayoutList />
            <span>Devices</span>
            <span className="count">{hosts.length}</span>
          </button>
        </nav>
        <div className="sidebar-bottom">
          <div className="theme-switch" role="group" aria-label="Theme">
            {(
              [
                ["light", Sun, "Light"],
                ["dark", Moon, "Dark"],
                ["system", Monitor, "Auto"],
              ] as const
            ).map(([key, Icon, label]) => (
              <button
                key={key}
                type="button"
                aria-pressed={theme === key}
                onClick={() => setTheme(key)}
              >
                <Icon />
                {label}
              </button>
            ))}
          </div>
          <button
            type="button"
            className="nav-item"
            onClick={() => setSettingsOpen(true)}
          >
            <Settings2 />
            <span>Settings</span>
          </button>
          <button
            type="button"
            className="nav-item"
            onClick={() => setAccountOpen(true)}
          >
            <UserRound />
            <span>Account</span>
          </button>
          <button
            type="button"
            className="nav-item"
            onClick={() => {
              void action(onLogout);
            }}
          >
            <LogOut />
            <span>Sign out</span>
          </button>
        </div>
      </aside>
      <main className="workspace">
        <header className="topbar">
          <div className="topbar-title">
            <h1>{TITLES[view]}</h1>
            <span className="connection">
              <i
                className={`status-dot ${connected ? "live pulse" : "stale"}`}
                aria-hidden="true"
              />
              <span>{connected ? "Live" : "Reconnecting"}</span>
            </span>
          </div>
          <div className="topbar-controls flex items-center gap-2">
            <Button
              variant="ghost"
              size="icon"
              aria-label="Notification settings"
              title="Notification settings"
              onClick={() => setNotificationsOpen(true)}
            >
              <BellRing />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              className="mobile-only"
              aria-label="Settings"
              onClick={() => setSettingsOpen(true)}
            >
              <Settings2 />
            </Button>
            <button
              type="button"
              className="palette-trigger"
              onClick={() => setPaletteOpen(true)}
              aria-label="Open command palette"
            >
              <Search size={14} />
              <span>Search hosts, run commands…</span>
              <kbd className="key">⌘K</kbd>
            </button>
            <Button
              variant="ghost"
              size="icon"
              className="mobile-only"
              aria-label="Account"
              onClick={() => setAccountOpen(true)}
            >
              <UserRound />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              className="mobile-logout"
              aria-label="Sign out"
              onClick={() => {
                void action(onLogout);
              }}
            >
              <LogOut />
            </Button>
            <Button
              variant="outline"
              className="topbar-action"
              aria-label="Add agent"
              onClick={() => setInstallOpen(true)}
            >
              <ArrowDownToLine data-icon="inline-start" />
              <span className="btn-label">Add agent</span>
            </Button>
            <DiscoveryMenu
              networks={networks}
              scans={scans}
              busy={busy}
              onScan={discover}
              onCancel={cancelScan}
              onAddNetwork={() => setNetworkOpen(true)}
            />
          </div>
        </header>
        {(error || unsavedMap.length > 0) && (
          <div className="px-7 pt-5">
            {error && (
              <Alert variant="destructive">
                <AlertTitle>Action needs attention</AlertTitle>
                <AlertDescription>
                  {error}
                  <div className="mt-2 flex gap-2">
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => {
                        void refresh();
                      }}
                    >
                      Refresh
                    </Button>
                    {unsavedMap.length > 0 && (
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => {
                          void retryMap();
                        }}
                      >
                        Retry {unsavedMap.length} unsaved map{" "}
                        {unsavedMap.length === 1 ? "change" : "changes"}
                      </Button>
                    )}
                  </div>
                </AlertDescription>
              </Alert>
            )}
            {!error && unsavedMap.length > 0 && (
              <Button
                variant="outline"
                onClick={() => {
                  void retryMap();
                }}
              >
                Retry {unsavedMap.length} unsaved map{" "}
                {unsavedMap.length === 1 ? "change" : "changes"}
              </Button>
            )}
          </div>
        )}
        {!data ? (
          <div className="page">
            <div className="stat-grid">
              {[0, 1, 2, 3].map((i) => (
                <Skeleton key={i} className="h-28 w-full rounded-2xl" />
              ))}
            </div>
            <Skeleton className="h-[50vh] w-full rounded-2xl" />
          </div>
        ) : view === "host" ? (
          <div className="page">
            {host ? (
              <HostDetails
                key={host.id}
                host={host}
                hosts={hosts}
                wide
                close={closeHostPage}
                agentRelease={data.agent_release}
                upgradeCommand={data.agent_upgrade_command}
                install={() => setInstallOpen(true)}
                refresh={() => {
                  void refresh();
                }}
                notify={toast}
              />
            ) : (
              <Empty className="min-h-[400px]">
                <EmptyHeader>
                  <EmptyTitle>Host not found</EmptyTitle>
                  <EmptyDescription>
                    It may have been merged or is not in the inventory yet.
                  </EmptyDescription>
                </EmptyHeader>
                <Button variant="outline" onClick={closeHostPage}>
                  Back to hosts
                </Button>
              </Empty>
            )}
          </div>
        ) : view === "overview" ? (
          <Overview
            data={data}
            onSelect={selectHost}
            onDiscover={() => discover()}
            onAddNetwork={() => setNetworkOpen(true)}
            onInstall={() => setInstallOpen(true)}
            onView={openView}
            busy={busy}
            onUpdateAll={() => {
              void action(async () => {
                const result = await post<UpdateRequested>("/agents/update");
                toast(
                  result.requested
                    ? `Asked ${result.requested} ${result.requested === 1 ? "agent" : "agents"} to update. Each starts when it next reports.`
                    : "No agent could be asked to update.",
                  result.requested ? "good" : "warning",
                );
              });
            }}
          />
        ) : (
          <div className="page">
            {data.capabilities.limitations.length > 0 && (
              <div className="capability-notice" role="note">
                <CircleHelp size={15} />
                <span>{data.capabilities.limitations.join(" ")}</span>
              </div>
            )}
            <div className="inventory-layout">
              <section className="inventory-main">
                <div className="view-toolbar">
                  <div className="flex flex-wrap items-center gap-3">
                    <ToggleGroup
                      value={[view]}
                      onValueChange={(v) => {
                        if (v[0] === "map" || v[0] === "hosts") setView(v[0]);
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
                        List
                      </ToggleGroupItem>
                    </ToggleGroup>
                    {view === "map" && (
                      <ToggleGroup
                        value={[mode]}
                        onValueChange={(v) => {
                          if (["topology", "subnets", "health"].includes(v[0]))
                            setMode(v[0] as Mode);
                        }}
                        size="sm"
                        aria-label="Map mode"
                      >
                        <ToggleGroupItem value="topology">
                          Topology
                        </ToggleGroupItem>
                        <ToggleGroupItem value="subnets">
                          Subnets
                        </ToggleGroupItem>
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
                      <input
                        ref={searchRef}
                        aria-label="Search hosts"
                        placeholder="Search hosts…  /"
                        value={filter}
                        onChange={(e) => setFilter(e.target.value)}
                      />
                      {filter && (
                        <button
                          type="button"
                          aria-label="Clear search"
                          onClick={() => setFilter("")}
                        >
                          <X size={13} />
                        </button>
                      )}
                    </div>
                  </div>
                </div>
                {hosts.length > 0 && (
                  <div className="chip-row" role="group" aria-label="Status">
                    {HOST_FILTERS.map((option) => {
                      const count = hosts.filter(
                        (h) =>
                          (network === "all" ||
                            h.network_ids.includes(network ?? "")) &&
                          matchesSearch(h, filter) &&
                          matchesFilter(h, option.key),
                      ).length;
                      if (
                        !count &&
                        option.key !== "all" &&
                        option.key !== statusFilter
                      )
                        return null;
                      return (
                        <button
                          key={option.key}
                          type="button"
                          className="chip"
                          aria-pressed={statusFilter === option.key}
                          onClick={() => setStatusFilter(option.key)}
                        >
                          {option.label}
                          <span className="count">{count}</span>
                        </button>
                      );
                    })}
                    {visible.length > 0 &&
                      (filter ||
                        network !== "all" ||
                        statusFilter !== "all") && (
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => {
                            setFilter("");
                            setNetwork("all");
                            setStatusFilter("all");
                          }}
                        >
                          Clear filters
                        </Button>
                      )}
                  </div>
                )}
                {!hosts.length ? (
                  <Empty className="min-h-[480px]">
                    <EmptyHeader>
                      <EmptyMedia variant="icon">
                        <Radar />
                      </EmptyMedia>
                      <EmptyTitle>No devices yet</EmptyTitle>
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
                        Try another network, status or search term.
                      </EmptyDescription>
                    </EmptyHeader>
                    <Button
                      variant="outline"
                      onClick={() => {
                        setFilter("");
                        setNetwork("all");
                        setStatusFilter("all");
                      }}
                    >
                      Clear filters
                    </Button>
                  </Empty>
                ) : view === "map" ? (
                  <NetworkMap
                    key={mode}
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
                  <HostTable
                    hosts={visible}
                    selected={selected}
                    select={select}
                  />
                )}
                <footer className="inventory-footer">
                  <span>
                    {visible.length} of {hosts.length} hosts in view
                    {hosts.length > 0 &&
                      ` · ${hosts.filter((h) => hostState(h).tone === "good").length} responding`}
                  </span>
                  <span>Manage scan schedules in Settings.</span>
                </footer>
              </section>
              {host && (
                <HostDetails
                  key={host.id}
                  host={host}
                  hosts={hosts}
                  close={() => select(null)}
                  expand={() => go({ view: "host" })}
                  agentRelease={data.agent_release}
                  upgradeCommand={data.agent_upgrade_command}
                  install={() => setInstallOpen(true)}
                  refresh={() => {
                    void refresh();
                  }}
                  notify={toast}
                />
              )}
            </div>
          </div>
        )}
      </main>
      <NetworkDialog
        open={networkOpen}
        onOpenChange={setNetworkOpen}
        onAdded={(created) => {
          void refresh();
          if (created) {
            toast(`Discovering ${created.name} (${created.cidr}).`, "good");
            if (view === "overview") setView("map");
          }
        }}
      />
      {installOpen && (
        <InstallDialog
          key={host?.id ?? "manual"}
          open={installOpen}
          onOpenChange={setInstallOpen}
          host={host}
        />
      )}
      <NotificationsDialog
        open={notificationsOpen}
        onOpenChange={setNotificationsOpen}
        notify={toast}
      />
      <AccountDialog
        open={accountOpen}
        onOpenChange={setAccountOpen}
        onPasswordChanged={() =>
          toast("Password changed. Other browsers were signed out.", "good")
        }
      />
      <NetworksDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        data={data}
        busy={busy}
        onAddNetwork={() => setNetworkOpen(true)}
        onSetInterval={(n, minutes) => {
          void action(async () => {
            await put(`/networks/${n.id}`, { interval_minutes: minutes });
            toast(
              `${n.name} is now scanned ${INTERVALS.find(([m]) => m === minutes)?.[1].toLowerCase() ?? `every ${minutes} minutes`}.`,
            );
          });
        }}
        onScanNetwork={(n) => discover(n)}
        onToggleNetwork={(n) => {
          void action(async () => {
            await put(`/networks/${n.id}`, { enabled: !n.enabled });
            toast(
              n.enabled
                ? `Paused discovery on ${n.name}.`
                : `Resumed discovery on ${n.name}.`,
            );
          });
        }}
        onDeleteNetwork={async (n) => {
          await action(async () => {
            await del(`/networks/${n.id}`);
            if (network === n.id) setNetwork("all");
            toast(`Removed ${n.name}.`);
          });
        }}
        onSetReportInterval={(seconds) => {
          void action(async () => {
            await put("/settings/reporting", { interval_seconds: seconds });
            toast(
              `Agents will report ${reportIntervalLabel(seconds).toLowerCase()} from their next report.`,
            );
          });
        }}
      />
      <CommandPalette
        open={paletteOpen}
        onOpenChange={setPaletteOpen}
        hosts={hosts}
        commands={commands}
        onSelectHost={selectHost}
      />
    </div>
  );
}
