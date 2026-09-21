import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Link,
  Outlet,
  createRootRoute,
  useRouterState,
} from "@tanstack/react-router";
import {
  ActivityIcon,
  CalendarDaysIcon,
  GitCompareArrowsIcon,
  SearchIcon,
  ServerIcon,
  Settings2Icon,
  ShieldCheckIcon,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { AuthPanel } from "@/components/AuthPanel";
import { ApiError, fetchDevices, fetchSetupStatus } from "@/lib/api";

const PRIMARY_NAV = [
  { to: "/devices", label: "Devices", icon: ServerIcon },
  { to: "/monitoring", label: "Monitoring", icon: ActivityIcon },
  { to: "/maintenance", label: "Maintenance", icon: CalendarDaysIcon },
] as const;

const SECONDARY_NAV = [
  { to: "/changes", label: "Activity", icon: GitCompareArrowsIcon },
  { to: "/settings", label: "Settings", icon: Settings2Icon },
] as const;

export const Route = createRootRoute({
  component: RootLayout,
});

function RootLayout() {
  const queryClient = useQueryClient();
  const pathname = useRouterState({
    select: (state) => state.location.pathname,
  });
  const mainContentRef = useRef<HTMLDivElement>(null);
  const previousPathname = useRef(pathname);
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

  useEffect(() => {
    if (previousPathname.current === pathname) return;
    previousPathname.current = pathname;
    mainContentRef.current?.focus();
  }, [pathname]);

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
    <div className="min-h-screen bg-background">
      <a
        className="fixed top-2 left-4 z-50 -translate-y-16 rounded-lg bg-primary px-3 py-2 text-sm text-primary-foreground transition-transform focus:translate-y-0 focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
        href="#main-content"
      >
        Skip to content
      </a>
      <header className="sticky top-0 z-30 border-b bg-background/95 backdrop-blur supports-backdrop-filter:bg-background/85">
        <div className="mx-auto flex min-h-16 max-w-[1440px] flex-wrap items-center gap-x-6 gap-y-2 px-4 py-2 sm:px-6 lg:px-8">
          <Link
            aria-label="Hope devices"
            className="mr-1 flex shrink-0 items-center gap-2 rounded-lg outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
            to="/devices"
          >
            <span className="grid size-8 place-items-center rounded-lg bg-primary text-primary-foreground">
              <ShieldCheckIcon aria-hidden="true" className="size-4" />
            </span>
            <span className="font-semibold tracking-tight">Hope</span>
          </Link>

          <nav
            aria-label="Primary navigation"
            className="order-3 flex basis-full min-w-0 flex-none items-center gap-1 overflow-x-auto pb-0.5 md:order-none md:w-auto md:basis-auto md:flex-none"
          >
            {PRIMARY_NAV.map((item) => (
              <NavLink item={item} key={item.to} pathname={pathname} />
            ))}
          </nav>

          <nav
            aria-label="Secondary navigation"
            className="ml-auto flex items-center gap-1"
          >
            {SECONDARY_NAV.map((item) => (
              <NavLink item={item} key={item.to} pathname={pathname} quiet />
            ))}
          </nav>

          <Link
            aria-label="Search devices"
            className="hidden size-9 items-center justify-center rounded-lg text-muted-foreground outline-none transition-colors hover:bg-muted hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50 sm:inline-flex"
            search={{ focus: "search" }}
            title="Search devices"
            to="/devices"
          >
            <SearchIcon aria-hidden="true" className="size-4" />
          </Link>
        </div>
      </header>
      <main
        className="mx-auto min-w-0 max-w-[1440px] scroll-mt-20 p-4 outline-none sm:p-6 lg:p-8"
        id="main-content"
        ref={mainContentRef}
        tabIndex={-1}
      >
        <Outlet />
      </main>
    </div>
  );
}

function NavLink({
  item,
  pathname,
  quiet = false,
}: {
  item: { to: string; label: string; icon: typeof ServerIcon };
  pathname: string;
  quiet?: boolean;
}) {
  const Icon = item.icon;
  const active = pathname === item.to || pathname.startsWith(`${item.to}/`);
  return (
    <Link
      activeProps={{ "aria-current": "page" }}
      className={`inline-flex min-h-9 shrink-0 items-center gap-2 rounded-lg px-3 text-sm outline-none transition-colors focus-visible:ring-3 focus-visible:ring-ring/50 ${
        active
          ? "bg-accent font-medium text-accent-foreground"
          : quiet
            ? "text-muted-foreground hover:bg-muted hover:text-foreground"
            : "text-foreground/75 hover:bg-muted hover:text-foreground"
      }`}
      to={item.to}
    >
      <Icon aria-hidden="true" className="size-4" />
      <span>{item.label}</span>
    </Link>
  );
}
