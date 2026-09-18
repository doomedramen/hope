import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/maintenance")({
  component: MaintenancePage,
});

function MaintenancePage() {
  return (
    <div>
      <h1 className="text-xl font-semibold">Maintenance</h1>
      <p className="mt-2 text-sm text-slate-500">Coming in a later milestone.</p>
    </div>
  );
}
