import { createFileRoute } from "@tanstack/react-router";
import { IncidentList } from "@/components/IncidentList";
import { MonitorList } from "@/components/MonitorList";
import { MonitorProposalQueue } from "@/components/MonitorProposalQueue";
import { ServiceReviewQueue } from "@/components/ServiceReviewQueue";
import { useRef, useState } from "react";

export const Route = createFileRoute("/monitoring")({
  validateSearch: (search: Record<string, unknown>): { monitor?: string } => {
    const monitor =
      typeof search.monitor === "string" ? search.monitor.trim() : "";
    return monitor ? { monitor } : {};
  },
  component: MonitoringPage,
});

function MonitoringPage() {
  const incidentHistoryRef = useRef<HTMLDetailsElement>(null);
  const [incidentHistoryOpen, setIncidentHistoryOpen] = useState(false);
  const revealIncidentHistory = () => {
    setIncidentHistoryOpen(true);
    if (incidentHistoryRef.current) {
      incidentHistoryRef.current.open = true;
      incidentHistoryRef.current.scrollIntoView?.({ block: "start" });
      incidentHistoryRef.current.querySelector<HTMLElement>("summary")?.focus();
    }
  };

  return (
    <div className="mx-auto flex max-w-7xl flex-col gap-6">
      <h1 className="text-3xl font-semibold tracking-tight">Monitoring</h1>
      <MonitorList onOpenIncidentHistory={revealIncidentHistory} />
      <section aria-label="Monitoring follow-up" className="space-y-3">
        <details className="rounded-lg border bg-card" id="suggested-checks">
          <summary className="cursor-pointer px-4 py-3 font-medium outline-none focus-visible:ring-3 focus-visible:ring-ring/50">
            Suggested checks
          </summary>
          <div className="border-t p-4 sm:p-6">
            <MonitorProposalQueue />
          </div>
        </details>
        <details className="rounded-lg border bg-card" id="service-reviews">
          <summary className="cursor-pointer px-4 py-3 font-medium outline-none focus-visible:ring-3 focus-visible:ring-ring/50">
            Service classification reviews
          </summary>
          <div className="border-t p-4 sm:p-6">
            <ServiceReviewQueue />
          </div>
        </details>
        <details
          className="rounded-lg border bg-card"
          id="incident-history"
          onToggle={(event) => setIncidentHistoryOpen(event.currentTarget.open)}
          open={incidentHistoryOpen}
          ref={incidentHistoryRef}
        >
          <summary className="cursor-pointer px-4 py-3 font-medium outline-none focus-visible:ring-3 focus-visible:ring-ring/50">
            Incident history
          </summary>
          <div className="border-t p-4 sm:p-6">
            <IncidentList />
          </div>
        </details>
      </section>
    </div>
  );
}
