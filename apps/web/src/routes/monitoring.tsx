import { createFileRoute } from "@tanstack/react-router";
import { IncidentList } from "@/components/IncidentList";
import { MonitorList } from "@/components/MonitorList";
import { MonitorProposalQueue } from "@/components/MonitorProposalQueue";
import { ServiceReviewQueue } from "@/components/ServiceReviewQueue";

export const Route = createFileRoute("/monitoring")({
  validateSearch: (search: Record<string, unknown>): { monitor?: string } => {
    const monitor =
      typeof search.monitor === "string" ? search.monitor.trim() : "";
    return monitor ? { monitor } : {};
  },
  component: MonitoringPage,
});

function MonitoringPage() {
  return (
    <div className="mx-auto flex max-w-7xl flex-col gap-6">
      <div>
        <h1 className="text-3xl font-semibold tracking-tight">Monitoring</h1>
        <p className="mt-2 max-w-2xl text-muted-foreground">
          See each service in context. Select a monitor to inspect its latest
          result, recent checks, and related incident.
        </p>
      </div>
      <MonitorList />
      <section aria-label="Monitoring follow-up" className="space-y-3">
        <p className="text-sm text-muted-foreground">
          Follow-up views stay available when you need fleet-wide review.
        </p>
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
        <details className="rounded-lg border bg-card" id="incident-history">
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
