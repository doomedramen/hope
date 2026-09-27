import { createFileRoute } from "@tanstack/react-router";
import { AgentsPage } from "@/components/AgentsPage";
import type { AgentLocation } from "@/components/agents/AgentOperations";

export const Route = createFileRoute("/agents")({
  validateSearch: (search: Record<string, unknown>): AgentLocation => {
    const result: AgentLocation = {};
    for (const key of [
      "agent",
      "tab",
      "q",
      "status",
      "cursor",
      "range",
      "logq",
      "source",
      "severity",
      "from",
      "to",
    ] as const) {
      if (typeof search[key] === "string" && search[key])
        result[key] = search[key];
    }
    return result;
  },
  component: AgentsRoute,
});
function AgentsRoute() {
  const location = Route.useSearch();
  const navigate = Route.useNavigate();
  return (
    <AgentsPage
      location={location}
      onNavigate={(patch) =>
        void navigate({ search: (old) => ({ ...old, ...patch }) })
      }
    />
  );
}
