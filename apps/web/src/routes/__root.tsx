import { Link, Outlet, createRootRoute } from "@tanstack/react-router";
import { HealthIndicator } from "../components/HealthIndicator";

const NAV_ITEMS = [
  { to: "/", label: "Overview" },
  { to: "/infrastructure", label: "Infrastructure" },
  { to: "/monitoring", label: "Monitoring" },
  { to: "/maintenance", label: "Maintenance" },
  { to: "/changes", label: "Changes" },
  { to: "/agents", label: "Agents" },
  { to: "/settings", label: "Settings" },
] as const;

export const Route = createRootRoute({
  component: RootLayout,
});

function RootLayout() {
  return (
    <div className="min-h-screen bg-slate-50 text-slate-900">
      <div className="flex min-h-screen">
        <aside className="w-56 shrink-0 border-r border-slate-200 bg-white px-4 py-6">
          <div className="mb-6 px-2 text-lg font-semibold">Hope</div>
          <nav className="flex flex-col gap-1">
            {NAV_ITEMS.map((item) => (
              <Link
                key={item.to}
                to={item.to}
                className="rounded-md px-2 py-1.5 text-sm text-slate-700 hover:bg-slate-100"
                activeProps={{ className: "bg-slate-100 font-medium text-slate-900" }}
              >
                {item.label}
              </Link>
            ))}
          </nav>
        </aside>
        <div className="flex flex-1 flex-col">
          <header className="flex items-center justify-between border-b border-slate-200 bg-white px-6 py-3">
            <div className="text-sm text-slate-500">Homelab Operations Platform</div>
            <HealthIndicator />
          </header>
          <main className="flex-1 p-6">
            <Outlet />
          </main>
        </div>
      </div>
    </div>
  );
}
