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
          className="grid w-full grid-cols-2 gap-1 sm:inline-flex sm:w-fit sm:grid-cols-none"
          variant="line"
        >
          <TabsTrigger className="min-w-0 whitespace-nowrap" value="monitors">
            Monitors
          </TabsTrigger>
          <TabsTrigger className="min-w-0 whitespace-nowrap" value="incidents">
            Incidents
          </TabsTrigger>
          <TabsTrigger className="min-w-0 whitespace-nowrap" value="services">
            Service reviews
          </TabsTrigger>
          <TabsTrigger className="min-w-0 whitespace-nowrap" value="proposals">
            Monitor proposals
          </TabsTrigger>
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
