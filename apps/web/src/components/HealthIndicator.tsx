import { useQuery } from "@tanstack/react-query";
import { fetchHealthReady } from "../lib/api";
import { Badge } from "./ui/badge";

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

  return (
    <Badge
      data-testid="health-indicator"
      variant={state === "down" ? "destructive" : "outline"}
    >
      <span
        className={`size-1.5 rounded-full ${state === "ready" ? "bg-emerald-500" : state === "checking" ? "bg-amber-500" : "bg-destructive"}`}
      />
      {label}
    </Badge>
  );
}
