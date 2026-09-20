import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, Outlet, createRootRoute } from "@tanstack/react-router";
import {
  ActivityIcon,
  CalendarDaysIcon,
  GitCompareArrowsIcon,
  LayoutDashboardIcon,
  SearchIcon,
  RadioTowerIcon,
  ServerIcon,
  Settings2Icon,
  ShieldCheckIcon,
} from "lucide-react";
import { useState } from "react";
import { AuthPanel } from "@/components/AuthPanel";
import { HealthIndicator } from "@/components/HealthIndicator";
import { ApiError, fetchDevices, fetchSetupStatus } from "@/lib/api";

const NAV_ITEMS = [
  { to: "/", label: "Overview", icon: LayoutDashboardIcon },
  { to: "/infrastructure", label: "Infrastructure", icon: ServerIcon },
  { to: "/monitoring", label: "Monitoring", icon: ActivityIcon },
  { to: "/maintenance", label: "Maintenance", icon: CalendarDaysIcon },
  { to: "/changes", label: "Changes", icon: GitCompareArrowsIcon },
  { to: "/agents", label: "Agents", icon: RadioTowerIcon },
  { to: "/settings", label: "Settings", icon: Settings2Icon },
] as const;

export const Route = createRootRoute({
  component: RootLayout,
});

function RootLayout() {
  const queryClient = useQueryClient();
  const [authenticatedThisVisit, setAuthenticatedThisVisit] = useState(false);
  const sessionQuery = useQuery({
    queryKey: ["session-probe"],
    queryFn: fetchDevices,
    retry: false,
    staleTime: 60_000,
  });
  const setupQuery = useQuery({
    queryKey: ["setup-status"],
    queryFn: fetchSetupStatus,
    retry: false,
    staleTime: 60_000,
  });
  const requiresLogin =
    sessionQuery.error instanceof ApiError && sessionQuery.error.status === 401;

  if (
    (sessionQuery.isLoading || setupQuery.isLoading) &&
    !authenticatedThisVisit
  ) {
    return (
      <main
        aria-busy="true"
        aria-live="polite"
        className="grid min-h-screen place-items-center bg-background"
      >
        <div
          className="flex items-center gap-2 text-sm text-muted-foreground"
          role="status"
        >
          <ShieldCheckIcon
            aria-hidden="true"
            className="size-4 animate-pulse"
          />
          Connecting to Hope
        </div>
      </main>
    );
  }
  if (requiresLogin && !authenticatedThisVisit) {
    return (
      <AuthPanel
        setupRequired={setupQuery.data?.setup_required === true}
        onAuthenticated={() => {
          setAuthenticatedThisVisit(true);
          void queryClient.invalidateQueries({ queryKey: ["session-probe"] });
        }}
      />
    );
  }

  return (
    <div className="min-h-screen bg-background text-foreground">
      <a
        className="fixed top-2 left-4 z-50 -translate-y-16 rounded-lg bg-primary px-3 py-2 text-sm text-primary-foreground transition-transform focus:translate-y-0 focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
        href="#main-content"
      >
        Skip to content
      </a>
      <div className="flex min-h-screen">
        <aside className="hidden w-60 shrink-0 border-r bg-sidebar p-4 text-sidebar-foreground md:flex md:flex-col">
          <Link
            aria-label="Hope overview"
            className="mb-8 flex items-center gap-2 rounded-lg px-2 text-lg font-semibold tracking-tight outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
            to="/"
          >
            <span className="grid size-8 place-items-center rounded-lg bg-sidebar-primary text-sidebar-primary-foreground">
              <ShieldCheckIcon aria-hidden="true" className="size-4" />
            </span>
            Hope
          </Link>
          {renderPrimaryNavigation()}
          <div className="border-t pt-4">
            <HealthIndicator />
          </div>
        </aside>
        <div className="flex min-w-0 flex-1 flex-col">
          <header className="flex h-14 items-center justify-between border-b px-5 md:hidden">
            <Link
              aria-label="Hope overview"
              className="rounded-lg font-medium outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
              to="/"
            >
              Hope
            </Link>
            <HealthIndicator />
          </header>
          {renderPrimaryNavigation({ mobile: true })}
          <header className="hidden h-14 items-center justify-end border-b bg-card/70 px-8 md:flex">
            <Link
              aria-label="Search devices"
              className="flex h-8 w-full max-w-sm items-center gap-2 rounded-lg border bg-background px-3 text-sm text-muted-foreground transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
              search={{ focus: "search" }}
              to="/infrastructure"
            >
              <SearchIcon aria-hidden="true" className="size-4" />
              Search devices
            </Link>
          </header>
          <main
            className="min-w-0 flex-1 scroll-mt-4 p-4 sm:p-5 md:p-8"
            id="main-content"
            tabIndex={-1}
          >
            <Outlet />
          </main>
        </div>
      </div>
    </div>
  );
}

function renderPrimaryNavigation({
  mobile = false,
}: { mobile?: boolean } = {}) {
  return (
    <nav
      aria-label="Primary navigation"
      className={
        mobile
          ? "grid grid-cols-2 gap-1 border-b px-3 py-2 md:hidden"
          : "flex flex-1 flex-col gap-1"
      }
    >
      {NAV_ITEMS.map((item) => {
        const Icon = item.icon;
        return (
          <Link
            activeProps={{
              "aria-current": "page",
              className: "bg-sidebar-accent text-sidebar-accent-foreground",
            }}
            className={
              mobile
                ? "flex min-h-10 items-center gap-2 rounded-lg px-2.5 py-2 text-sm text-muted-foreground outline-none hover:bg-muted hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50"
                : "flex min-h-10 items-center gap-2 rounded-lg px-2.5 py-2 text-sm text-sidebar-foreground/70 outline-none hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-3 focus-visible:ring-ring/50"
            }
            key={item.to}
            to={item.to}
          >
            <Icon aria-hidden="true" className="size-4 shrink-0" />
            {item.label}
          </Link>
        );
      })}
    </nav>
  );
}
