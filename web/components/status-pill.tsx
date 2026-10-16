import type { Host } from "@/lib/protocol";
import { hostState } from "@/lib/fleet";

export function StatusPill({
  host,
  className,
}: {
  host: Pick<Host, "agent_id" | "agent_revoked" | "last_report" | "last_seen">;
  className?: string;
}) {
  const state = hostState(host);
  return (
    <span className={`status-pill ${className ?? ""}`} data-tone={state.tone}>
      <i
        className={`status-dot ${state.key === "reporting" ? "pulse" : ""}`}
        data-tone={state.tone}
        aria-hidden="true"
      />
      {state.label}
    </span>
  );
}
