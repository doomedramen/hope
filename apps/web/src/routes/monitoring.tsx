import { createFileRoute } from "@tanstack/react-router";
import { IncidentList } from "@/components/IncidentList";
import { MonitorList } from "@/components/MonitorList";
import { MonitorProposalQueue } from "@/components/MonitorProposalQueue";
import { ServiceReviewQueue } from "@/components/ServiceReviewQueue";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

export const Route = createFileRoute("/monitoring")({
  component: MonitoringPage,
});

function MonitoringPage() {
  return (
    <div className="mx-auto flex max-w-7xl flex-col gap-6">
      <h1 className="text-3xl font-semibold tracking-tight">Monitoring</h1>
      <Tabs className="gap-4" defaultValue="monitors">
        <TabsList
          aria-label="Monitoring views"
          className="w-full flex-wrap justify-start"
          variant="line"
        >
          <TabsTrigger value="monitors">Monitors</TabsTrigger>
          <TabsTrigger value="incidents">Incidents</TabsTrigger>
          <TabsTrigger value="services">Service reviews</TabsTrigger>
          <TabsTrigger value="proposals">Monitor proposals</TabsTrigger>
        </TabsList>
        <TabsContent value="monitors">
          <MonitorList />
        </TabsContent>
        <TabsContent value="incidents">
          <IncidentList />
        </TabsContent>
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
