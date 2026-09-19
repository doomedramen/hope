import { useQuery } from "@tanstack/react-query";
import { fetchHealthReady } from "../lib/api";
import { Badge } from "./ui/badge";
import { StatusDot, type StatusTone } from "./ui/status-dot";

export function HealthIndicator() {
  const { data, isLoading, isError } = useQuery({
    queryKey: ["health", "ready"],
    queryFn: fetchHealthReady,
    refetchInterval: 15_000,
  });

  const state = isLoading
    ? "checking"
    : isError
      ? "down"
      : data?.status === "ready"
        ? "ready"
        : "down";

  const label =
    state === "ready"
      ? "Server ready"
      : state === "checking"
        ? "Checking..."
        : "Server unreachable";

  const tone: StatusTone =
    state === "ready"
      ? "healthy"
      : state === "checking"
        ? "attention"
        : "critical";

  return (
    <Badge data-testid="health-indicator" variant={tone}>
      <StatusDot tone={tone} />
      {label}
    </Badge>
  );
}
