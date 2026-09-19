import { createFileRoute } from "@tanstack/react-router";
import { ActivityIcon, RadarIcon } from "lucide-react";
import { ServiceReviewQueue } from "@/components/ServiceReviewQueue";
import { Card, CardContent } from "@/components/ui/card";

export const Route = createFileRoute("/monitoring")({
  component: MonitoringPage,
});

function MonitoringPage() {
  return (
    <div className="mx-auto flex max-w-7xl flex-col gap-6">
      <div>
        <p className="text-sm text-muted-foreground">
          Monitoring / M3 service review
        </p>
        <h1 className="mt-1 text-3xl font-semibold tracking-tight">
          Service fingerprints
        </h1>
        <p className="mt-2 max-w-2xl text-muted-foreground">
          Review ambiguous service identity before it changes the inventory.
        </p>
      </div>
      <div className="grid gap-4 sm:grid-cols-2">
        <Card size="sm">
          <CardContent className="flex items-center gap-3">
            <span className="grid size-9 place-items-center rounded-lg bg-muted text-muted-foreground">
              <ActivityIcon className="size-4" />
            </span>
            <div>
              <p className="text-sm font-medium">Review queue</p>
              <p className="text-xs text-muted-foreground">
                Low-confidence product matches and conflicts
              </p>
            </div>
          </CardContent>
        </Card>
        <Card size="sm">
          <CardContent className="flex items-center gap-3">
            <span className="grid size-9 place-items-center rounded-lg bg-muted text-muted-foreground">
              <RadarIcon className="size-4" />
            </span>
            <div>
              <p className="text-sm font-medium">Evidence first</p>
              <p className="text-xs text-muted-foreground">
                Every decision keeps its source observations
              </p>
            </div>
          </CardContent>
        </Card>
      </div>
      <ServiceReviewQueue />
    </div>
  );
}
