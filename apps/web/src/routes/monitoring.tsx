import { createFileRoute } from "@tanstack/react-router";
import { ServiceReviewQueue } from "@/components/ServiceReviewQueue";

export const Route = createFileRoute("/monitoring")({
  component: MonitoringPage,
});

function MonitoringPage() {
  return (
    <div className="mx-auto flex max-w-7xl flex-col gap-6">
      <div>
        <p className="text-sm text-muted-foreground">
          Monitoring / Service review
        </p>
        <h1 className="mt-1 text-3xl font-semibold tracking-tight">
          Service fingerprints
        </h1>
      </div>
      <ServiceReviewQueue />
    </div>
  );
}
