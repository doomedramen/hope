import { createFileRoute } from "@tanstack/react-router";
import { MonitorProposalQueue } from "@/components/MonitorProposalQueue";
import { ServiceReviewQueue } from "@/components/ServiceReviewQueue";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

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
      <Tabs className="gap-4" defaultValue="services">
        <TabsList variant="line">
          <TabsTrigger value="services">Service reviews</TabsTrigger>
          <TabsTrigger value="proposals">Monitor proposals</TabsTrigger>
        </TabsList>
        <TabsContent value="services">
          <ServiceReviewQueue />
        </TabsContent>
        <TabsContent value="proposals">
          <MonitorProposalQueue />
        </TabsContent>
      </Tabs>
    </div>
  );
}
