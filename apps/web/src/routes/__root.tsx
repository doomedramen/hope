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
  MenuIcon,
  SearchIcon,
  ServerIcon,
  Settings2Icon,
  ShieldCheckIcon,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { AuthPanel } from "@/components/AuthPanel";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
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

type NavigationItem =
  (typeof PRIMARY_NAV)[number] | (typeof SECONDARY_NAV)[number];

function isActiveNavigationItem(
  item: NavigationItem,
  pathname: string,
): boolean {
  return pathname === item.to || pathname.startsWith(`${item.to}/`);
}

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
        <div className="mx-auto flex min-h-14 max-w-[1440px] items-center gap-2 px-3 sm:px-6 lg:px-8">
          <Link
            aria-label="Hope devices"
            className="flex shrink-0 items-center gap-2 rounded-lg outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
            to="/devices"
          >
            <span className="grid size-8 place-items-center rounded-lg bg-primary text-primary-foreground">
              <ShieldCheckIcon aria-hidden="true" className="size-4" />
            </span>
            <span className="font-semibold tracking-tight">Hope</span>
          </Link>

          <nav
            aria-label="Primary navigation"
            className="hidden min-w-0 items-center gap-1 md:flex"
          >
            {PRIMARY_NAV.map((item) => (
              <NavLink item={item} key={item.to} pathname={pathname} />
            ))}
          </nav>

          <nav
            aria-label="Secondary navigation"
            className="ml-auto hidden items-center gap-1 md:flex"
          >
            {SECONDARY_NAV.map((item) => (
              <NavLink item={item} key={item.to} pathname={pathname} quiet />
            ))}
          </nav>

          <Link
            aria-label="Search devices"
            className="hidden size-9 items-center justify-center rounded-lg text-muted-foreground outline-none transition-colors hover:bg-muted hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50 md:inline-flex"
            search={{ focus: "search" }}
            title="Search devices"
            to="/devices"
          >
            <SearchIcon aria-hidden="true" className="size-4" />
          </Link>

          <MobileNavigation pathname={pathname} />
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
  item: NavigationItem;
  pathname: string;
  quiet?: boolean;
}) {
  const Icon = item.icon;
  const active = isActiveNavigationItem(item, pathname);
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

function MobileNavigation({ pathname }: { pathname: string }) {
  const [open, setOpen] = useState(false);
  const closeMenu = () => setOpen(false);

  return (
    <div className="ml-auto flex min-w-0 items-center gap-1 md:hidden">
      <Link
        aria-label="Search devices"
        className="inline-flex size-9 shrink-0 items-center justify-center rounded-lg text-muted-foreground outline-none transition-colors hover:bg-muted hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50"
        search={{ focus: "search" }}
        title="Search devices"
        to="/devices"
      >
        <SearchIcon aria-hidden="true" className="size-4" />
      </Link>
      <DropdownMenu onOpenChange={setOpen} open={open}>
        <DropdownMenuTrigger
          aria-label="Open navigation menu"
          className="inline-flex min-h-9 shrink-0 items-center gap-1.5 rounded-lg border border-border bg-background px-3 text-sm font-medium outline-none transition-colors hover:bg-muted focus-visible:ring-3 focus-visible:ring-ring/50 aria-expanded:bg-muted"
        >
          <MenuIcon aria-hidden="true" className="size-4" />
          <span>Menu</span>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="end"
          className="w-64 max-w-[calc(100vw-1.5rem)]"
        >
          <DropdownMenuGroup>
            <DropdownMenuLabel>Primary</DropdownMenuLabel>
            {PRIMARY_NAV.map((item) => (
              <MobileNavItem
                item={item}
                key={item.to}
                onNavigate={closeMenu}
                pathname={pathname}
              />
            ))}
          </DropdownMenuGroup>
          <DropdownMenuSeparator />
          <DropdownMenuGroup>
            <DropdownMenuLabel>Secondary</DropdownMenuLabel>
            {SECONDARY_NAV.map((item) => (
              <MobileNavItem
                item={item}
                key={item.to}
                onNavigate={closeMenu}
                pathname={pathname}
              />
            ))}
          </DropdownMenuGroup>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

function MobileNavItem({
  item,
  onNavigate,
  pathname,
}: {
  item: NavigationItem;
  onNavigate: () => void;
  pathname: string;
}) {
  const Icon = item.icon;
  const active = isActiveNavigationItem(item, pathname);
  return (
    <DropdownMenuItem
      aria-current={active ? "page" : undefined}
      className={active ? "bg-accent font-medium text-accent-foreground" : ""}
      render={
        <Link
          activeProps={{ "aria-current": "page" }}
          className="flex w-full items-center gap-2"
          onClick={onNavigate}
          to={item.to}
        />
      }
    >
      <Icon aria-hidden="true" className="size-4" />
      <span>{item.label}</span>
    </DropdownMenuItem>
  );
}
