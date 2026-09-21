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
  LayoutDashboardIcon,
  NetworkIcon,
  SearchIcon,
  RadioTowerIcon,
  ServerIcon,
  Settings2Icon,
  ShieldCheckIcon,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { AuthPanel } from "@/components/AuthPanel";
import { HealthIndicator } from "@/components/HealthIndicator";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarInset,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
  SidebarTrigger,
  useSidebar,
} from "@/components/ui/sidebar";
import { ApiError, fetchDevices, fetchSetupStatus } from "@/lib/api";

const NAV_ITEMS = [
  { to: "/", label: "Overview", icon: LayoutDashboardIcon },
  { to: "/devices", label: "Devices", icon: ServerIcon },
  { to: "/networks", label: "Networks", icon: NetworkIcon },
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
    <SidebarProvider>
      <a
        className="fixed top-2 left-4 z-50 -translate-y-16 rounded-lg bg-primary px-3 py-2 text-sm text-primary-foreground transition-transform focus:translate-y-0 focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
        href="#main-content"
      >
        Skip to content
      </a>
      <AppSidebar />
      <SidebarInset>
        <header className="flex h-14 items-center gap-2 border-b bg-card/70 px-4 md:px-8">
          <SidebarTrigger />
          <Link
            aria-label="Hope overview"
            className="rounded-lg font-medium outline-none focus-visible:ring-3 focus-visible:ring-ring/50 md:hidden"
            to="/"
          >
            Hope
          </Link>
          <div className="ml-auto flex items-center gap-3">
            <div className="md:hidden">
              <HealthIndicator />
            </div>
            <Link
              aria-label="Search devices"
              className="hidden h-8 w-full max-w-sm items-center gap-2 rounded-lg border bg-background px-3 text-sm text-muted-foreground transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/50 md:flex"
              search={{ focus: "search" }}
              to="/devices"
            >
              <SearchIcon aria-hidden="true" className="size-4" />
              Search devices
            </Link>
          </div>
        </header>
        <div
          className="min-w-0 flex-1 scroll-mt-4 p-4 outline-none sm:p-5 md:p-8"
          id="main-content"
          ref={mainContentRef}
          tabIndex={-1}
        >
          <Outlet />
        </div>
      </SidebarInset>
    </SidebarProvider>
  );
}

function AppSidebar() {
  const { isMobile, setOpenMobile } = useSidebar();
  const pathname = useRouterState({
    select: (state) => state.location.pathname,
  });

  return (
    <Sidebar collapsible="icon">
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton
              render={<Link aria-label="Hope overview" to="/" />}
              size="lg"
              tooltip="Hope overview"
            >
              <span className="grid size-8 shrink-0 place-items-center rounded-lg bg-sidebar-primary text-sidebar-primary-foreground">
                <ShieldCheckIcon aria-hidden="true" className="size-4" />
              </span>
              <span className="font-semibold tracking-tight group-data-[collapsible=icon]/menu-button:hidden">
                Hope
              </span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent aria-label="Primary navigation" role="navigation">
        <SidebarGroup>
          <SidebarGroupLabel>Workspace</SidebarGroupLabel>
          <SidebarGroupContent>
            <SidebarMenu>
              {NAV_ITEMS.map((item) => {
                const Icon = item.icon;
                const isActive =
                  item.to === "/"
                    ? pathname === "/"
                    : pathname === item.to ||
                      pathname.startsWith(`${item.to}/`);
                return (
                  <SidebarMenuItem key={item.to}>
                    <SidebarMenuButton
                      isActive={isActive}
                      onClick={() => {
                        if (isMobile) setOpenMobile(false);
                      }}
                      render={
                        <Link
                          activeProps={{ "aria-current": "page" }}
                          to={item.to}
                        />
                      }
                      tooltip={item.label}
                    >
                      <Icon aria-hidden="true" />
                      <span>{item.label}</span>
                    </SidebarMenuButton>
                  </SidebarMenuItem>
                );
              })}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>
      <SidebarFooter>
        <HealthIndicator />
      </SidebarFooter>
    </Sidebar>
  );
}
