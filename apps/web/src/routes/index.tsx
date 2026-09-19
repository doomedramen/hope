import { createFileRoute } from "@tanstack/react-router";
import { OverviewDashboard } from "@/components/overview/OverviewDashboard";

export const Route = createFileRoute("/")({
  component: OverviewDashboard,
});
