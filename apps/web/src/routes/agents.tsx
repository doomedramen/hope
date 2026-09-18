import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/agents")({
  component: AgentsPage,
});

function AgentsPage() {
  return (
    <div>
      <h1 className="text-xl font-semibold">Agents</h1>
      <p className="mt-2 text-sm text-slate-500">Coming in a later milestone.</p>
    </div>
  );
}
