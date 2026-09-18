import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/")({
  component: OverviewPage,
});

function OverviewPage() {
  return (
    <div>
      <h1 className="text-xl font-semibold">Overview</h1>
      <p className="mt-2 text-sm text-slate-500">
        Incidents, maintenance, coverage, changes, and agent status will land here (spec §13.1).
      </p>
    </div>
  );
}
