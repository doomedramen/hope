import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, Outlet, createRootRoute } from "@tanstack/react-router";
import {
  ActivityIcon,
  CalendarDaysIcon,
  GitCompareArrowsIcon,
  LayoutDashboardIcon,
  RadioTowerIcon,
  ServerIcon,
  Settings2Icon,
  ShieldCheckIcon,
} from "lucide-react";
import { useState } from "react";
import { AuthPanel } from "@/components/AuthPanel";
import { HealthIndicator } from "@/components/HealthIndicator";
import { ApiError, fetchDevices } from "@/lib/api";

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
  const requiresLogin =
    sessionQuery.error instanceof ApiError && sessionQuery.error.status === 401;

  if (sessionQuery.isLoading && !authenticatedThisVisit) {
    return (
      <main className="grid min-h-screen place-items-center bg-background">
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <ShieldCheckIcon className="size-4 animate-pulse" />
          Connecting to Hope
        </div>
      </main>
    );
  }
  if (requiresLogin && !authenticatedThisVisit) {
    return (
      <AuthPanel
        onAuthenticated={() => {
          setAuthenticatedThisVisit(true);
          void queryClient.invalidateQueries({ queryKey: ["session-probe"] });
        }}
      />
    );
  }

  return (
    <div className="min-h-screen bg-background text-foreground">
      <div className="flex min-h-screen">
        <aside className="hidden w-60 shrink-0 border-r bg-sidebar p-4 text-sidebar-foreground md:flex md:flex-col">
          <Link
            className="mb-8 flex items-center gap-2 px-2 text-lg font-semibold tracking-tight"
            to="/"
          >
            <span className="grid size-8 place-items-center rounded-lg bg-sidebar-primary text-sidebar-primary-foreground">
              <ShieldCheckIcon className="size-4" />
            </span>
            Hope
          </Link>
          <nav className="flex flex-1 flex-col gap-1">
            {NAV_ITEMS.map((item) => {
              const Icon = item.icon;
              return (
                <Link
                  activeProps={{
                    className:
                      "bg-sidebar-accent text-sidebar-accent-foreground",
                  }}
                  className="flex items-center gap-2 rounded-lg px-2.5 py-2 text-sm text-sidebar-foreground/70 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
                  key={item.to}
                  to={item.to}
                >
                  <Icon className="size-4" />
                  {item.label}
                </Link>
              );
            })}
          </nav>
          <div className="border-t pt-4">
            <HealthIndicator />
          </div>
        </aside>
        <div className="flex min-w-0 flex-1 flex-col">
          <header className="flex h-14 items-center justify-between border-b px-5">
            <div>
              <span className="font-medium md:hidden">Hope</span>
              <span className="hidden text-sm text-muted-foreground md:inline">
                Homelab operations platform
              </span>
            </div>
            <HealthIndicator />
          </header>
          <main className="flex-1 p-5 md:p-8">
            <Outlet />
          </main>
        </div>
      </div>
    </div>
  );
}
