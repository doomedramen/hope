import { createFileRoute } from "@tanstack/react-router";
import { AgentsPage } from "@/components/AgentsPage";

export const Route = createFileRoute("/agents")({
  component: AgentsPage,
});
