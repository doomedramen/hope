import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/monitoring")({
  component: MonitoringPage,
});

function MonitoringPage() {
  return (
    <div>
      <h1 className="text-xl font-semibold">Monitoring</h1>
      <p className="mt-2 text-sm text-slate-500">Coming in a later milestone.</p>
    </div>
  );
}
