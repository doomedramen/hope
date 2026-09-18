import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/infrastructure")({
  component: InfrastructurePage,
});

function InfrastructurePage() {
  return (
    <div>
      <h1 className="text-xl font-semibold">Infrastructure</h1>
      <p className="mt-2 text-sm text-slate-500">Coming in a later milestone.</p>
    </div>
  );
}
