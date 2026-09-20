import { createFileRoute } from "@tanstack/react-router";
import { InfrastructurePage } from "./infrastructure";

export const Route = createFileRoute("/devices")({
  validateSearch: (
    search: Record<string, unknown>,
  ): { device?: string; focus?: "search"; q?: string } => {
    const device = typeof search.device === "string" ? search.device : null;
    const focus = search.focus === "search" ? "search" : undefined;
    const q = typeof search.q === "string" ? search.q : null;
    return {
      ...(device ? { device } : {}),
      ...(focus ? { focus } : {}),
      ...(q ? { q } : {}),
    };
  },
  component: InfrastructurePage,
});
