import { useQuery } from "@tanstack/react-query";
import { fetchHealthReady } from "../lib/api";

export function HealthIndicator() {
  const { data, isLoading, isError } = useQuery({
    queryKey: ["health", "ready"],
    queryFn: fetchHealthReady,
    refetchInterval: 15_000,
  });

  const state = isLoading ? "checking" : isError ? "down" : data?.status === "ready" ? "ready" : "down";

  const color =
    state === "ready"
      ? "bg-emerald-500"
      : state === "checking"
        ? "bg-amber-400"
        : "bg-red-500";

  const label =
    state === "ready" ? "Server ready" : state === "checking" ? "Checking..." : "Server unreachable";

  return (
    <div className="flex items-center gap-2 text-sm text-slate-600" data-testid="health-indicator">
      <span className={`inline-block h-2.5 w-2.5 rounded-full ${color}`} aria-hidden />
      <span>{label}</span>
    </div>
  );
}
