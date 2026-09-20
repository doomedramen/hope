import { createFileRoute } from "@tanstack/react-router";
import { MaintenancePage } from "@/components/maintenance/MaintenancePage";

export const Route = createFileRoute("/maintenance")({
  component: MaintenancePage,
});
