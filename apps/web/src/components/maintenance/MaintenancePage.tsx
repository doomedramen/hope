import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  CalendarClockIcon,
  CalendarDaysIcon,
  CheckCircle2Icon,
  CircleAlertIcon,
  Clock3Icon,
  ExternalLinkIcon,
  Layers3Icon,
  PencilIcon,
  PlusIcon,
  ShieldAlertIcon,
} from "lucide-react";
import { useMemo, useState, type ReactNode } from "react";
import {
  cancelMaintenanceEvent,
  completeMaintenanceEvent,
  createMaintenanceEvent,
  fetchMaintenanceConflicts,
  fetchMaintenanceEvents,
  getUserFacingError,
  patchMaintenanceEvent,
  startMaintenanceEvent,
  type MaintenanceConflict,
  type MaintenanceEvent,
  type MaintenanceEventInput,
} from "@/lib/api";
import { formatDate, formatRelative, labelize, shortId } from "@/lib/format";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { MaintenanceConflictList } from "./MaintenanceConflictList";
import { MaintenanceEventForm } from "./MaintenanceEventForm";
import {
  MaintenanceCalendarView,
  MaintenanceListView,
  MaintenanceTimelineView,
  maintenanceStateVariant,
  occurrenceItems,
  type MaintenanceView,
} from "./MaintenanceViews";

type LifecycleAction = "schedule" | "start" | "complete" | "cancel";

const EMPTY_EVENTS: MaintenanceEvent[] = [];

function formatSeconds(seconds: number) {
  if (seconds <= 0) return "None";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  const remainder = minutes % 60;
  return remainder ? `${hours}h ${remainder}m` : `${hours}h`;
}

function resourceName(resource: MaintenanceEvent["resources"][number]) {
  if (resource.key) return resource.key;
  if (resource.kind && resource.id)
    return `${resource.kind}/${shortId(resource.id)}`;
  return "Unidentified resource";
}

function SummaryCard({
  icon,
  label,
  value,
  detail,
}: {
  icon: ReactNode;
  label: string;
  value: string | number;
  detail: string;
}) {
  return (
    <Card size="sm">
      <CardContent className="flex items-center gap-3">
        <span className="grid size-9 shrink-0 place-items-center rounded-lg bg-muted text-muted-foreground">
          {icon}
        </span>
        <div className="min-w-0">
          <p className="text-xs text-muted-foreground">{label}</p>
          <p className="text-2xl font-semibold tracking-tight">{value}</p>
          <p className="truncate text-xs text-muted-foreground">{detail}</p>
        </div>
      </CardContent>
    </Card>
  );
}

export function MaintenancePage() {
  const queryClient = useQueryClient();
  const [view, setView] = useState<MaintenanceView>("timeline");
  const [selectedIdState, setSelectedIdState] = useState<string | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [editingEvent, setEditingEvent] = useState<MaintenanceEvent | null>(
    null,
  );
  const eventsQuery = useQuery({
    queryKey: ["maintenance-events"],
    queryFn: () => fetchMaintenanceEvents({ limit: 200 }),
  });
  const events = eventsQuery.data?.items ?? EMPTY_EVENTS;
  const selectedId =
    selectedIdState && events.some((event) => event.id === selectedIdState)
      ? selectedIdState
      : (events[0]?.id ?? null);
  const selectedEvent = events.find((event) => event.id === selectedId) ?? null;
  const conflictsQuery = useQuery({
    enabled: selectedEvent !== null,
    queryKey: ["maintenance-conflicts", selectedEvent?.id],
    queryFn: () => fetchMaintenanceConflicts(selectedEvent!.id),
    retry: false,
  });
  const saveMutation = useMutation({
    mutationFn: ({
      event,
      input,
    }: {
      event: MaintenanceEvent | null;
      input: MaintenanceEventInput;
    }) =>
      event
        ? patchMaintenanceEvent(event.id, {
            ...input,
            state: undefined,
            version: event.version,
          })
        : createMaintenanceEvent(input),
    onSuccess: async (event) => {
      await queryClient.invalidateQueries({ queryKey: ["maintenance-events"] });
      setSelectedIdState(event.id);
      setEditorOpen(false);
      setEditingEvent(null);
    },
  });
  const lifecycleMutation = useMutation({
    mutationFn: ({
      event,
      action,
    }: {
      event: MaintenanceEvent;
      action: LifecycleAction;
    }) => {
      switch (action) {
        case "schedule":
          return patchMaintenanceEvent(event.id, {
            version: event.version,
            state: "scheduled",
          });
        case "start":
          return startMaintenanceEvent(event.id, event.version);
        case "complete":
          return completeMaintenanceEvent(event.id, event.version);
        case "cancel":
          return cancelMaintenanceEvent(event.id, event.version);
      }
    },
    onSuccess: async (event) => {
      await queryClient.invalidateQueries({ queryKey: ["maintenance-events"] });
      setSelectedIdState(event.id);
    },
  });

  const occurrenceCount = useMemo(
    () => occurrenceItems(events).length,
    [events],
  );
  const upcomingCount = events.filter((event) =>
    ["draft", "scheduled", "upcoming"].includes(event.state),
  ).length;
  const activeCount = events.filter((event) =>
    ["active", "overrunning"].includes(event.state),
  ).length;
  const overrunCount = events.filter(
    (event) => event.state === "overrunning",
  ).length;
  const resourceCount = new Set(
    events.flatMap((event) => event.resources.map(resourceName)),
  ).size;

  function openCreate() {
    saveMutation.reset();
    setEditingEvent(null);
    setEditorOpen(true);
  }

  function openEdit(event: MaintenanceEvent) {
    saveMutation.reset();
    setEditingEvent(event);
    setEditorOpen(true);
  }

  function selectEvent(event: MaintenanceEvent) {
    setSelectedIdState(event.id);
  }

  return (
    <div className="mx-auto flex max-w-7xl flex-col gap-6">
      <div className="flex flex-col gap-4 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <h1 className="text-3xl font-semibold tracking-tight">Maintenance</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Plan reservations, inspect conflicts, and track lifecycle state.
          </p>
        </div>
        <Button onClick={openCreate}>
          <PlusIcon data-icon="inline-start" />
          New event
        </Button>
      </div>

      <div className="order-2 grid gap-4 sm:order-none sm:grid-cols-2 xl:grid-cols-4">
        <SummaryCard
          detail={`${occurrenceCount} expanded occurrence${occurrenceCount === 1 ? "" : "s"}`}
          icon={<CalendarClockIcon />}
          label="Planned"
          value={upcomingCount}
        />
        <SummaryCard
          detail={
            overrunCount
              ? `${overrunCount} overrunning`
              : "No overrunning events"
          }
          icon={<ShieldAlertIcon />}
          label="Active"
          value={activeCount}
        />
        <SummaryCard
          detail="Distinct reserved identifiers"
          icon={<Layers3Icon />}
          label="Resources"
          value={resourceCount}
        />
        <SummaryCard
          detail="90-day recurrence horizon"
          icon={<Clock3Icon />}
          label="Expansion"
          value={events.length ? "Ready" : "—"}
        />
      </div>

      <Card className="order-1 sm:order-none">
        <CardHeader className="border-b">
          <div>
            <CardTitle>Schedule</CardTitle>
            <CardDescription>
              Events include lead-in and cooldown reservation windows.
            </CardDescription>
          </div>
          <CardAction>
            <Badge variant="outline">
              {events.length} event{events.length === 1 ? "" : "s"}
            </Badge>
          </CardAction>
        </CardHeader>
        {eventsQuery.isLoading ? (
          <MaintenanceLoading />
        ) : eventsQuery.isError ? (
          <CardContent className="pt-6">
            <Alert variant="destructive">
              <CircleAlertIcon />
              <AlertTitle>Maintenance unavailable</AlertTitle>
              <AlertDescription>
                {getUserFacingError(
                  eventsQuery.error,
                  "Maintenance events could not be loaded.",
                )}
              </AlertDescription>
              <Button
                onClick={() => eventsQuery.refetch()}
                size="sm"
                variant="outline"
              >
                Retry
              </Button>
            </Alert>
          </CardContent>
        ) : events.length === 0 ? (
          <CardContent className="pt-6">
            <Empty>
              <EmptyHeader>
                <EmptyMedia variant="icon">
                  <CalendarDaysIcon />
                </EmptyMedia>
                <EmptyTitle>No maintenance events</EmptyTitle>
                <EmptyDescription>
                  Create one-time or recurring reservations before planned work
                  begins.
                </EmptyDescription>
              </EmptyHeader>
              <Button onClick={openCreate}>
                <PlusIcon data-icon="inline-start" />
                Create event
              </Button>
            </Empty>
          </CardContent>
        ) : (
          <CardContent className="pt-5">
            <Tabs
              onValueChange={(value) => {
                if (
                  value === "calendar" ||
                  value === "timeline" ||
                  value === "list"
                )
                  setView(value);
              }}
              value={view}
            >
              <TabsList className="max-w-full overflow-x-auto" variant="line">
                <TabsTrigger value="timeline">Timeline</TabsTrigger>
                <TabsTrigger value="calendar">Calendar</TabsTrigger>
                <TabsTrigger value="list">List</TabsTrigger>
              </TabsList>
              <TabsContent className="pt-5" value="timeline">
                <MaintenanceTimelineView
                  events={events}
                  onSelect={selectEvent}
                  selectedId={selectedId}
                />
              </TabsContent>
              <TabsContent className="pt-5" value="calendar">
                <MaintenanceCalendarView
                  events={events}
                  onSelect={selectEvent}
                  selectedId={selectedId}
                />
              </TabsContent>
              <TabsContent className="pt-5" value="list">
                <MaintenanceListView
                  events={events}
                  onSelect={selectEvent}
                  selectedId={selectedId}
                />
              </TabsContent>
            </Tabs>
          </CardContent>
        )}
      </Card>

      {selectedEvent ? (
        <MaintenanceDetail
          conflicts={conflictsQuery.data?.items ?? []}
          conflictsError={conflictsQuery.error}
          conflictsLoading={conflictsQuery.isLoading}
          event={selectedEvent}
          lifecycleError={lifecycleMutation.error}
          lifecyclePending={lifecycleMutation.isPending}
          onEdit={() => openEdit(selectedEvent)}
          onLifecycle={(action) =>
            lifecycleMutation.mutate({ action, event: selectedEvent })
          }
        />
      ) : null}

      <MaintenanceEventForm
        error={saveMutation.error}
        event={editingEvent}
        onOpenChange={(open) => {
          setEditorOpen(open);
          if (!open) setEditingEvent(null);
        }}
        onSubmit={(input) =>
          saveMutation.mutate({ event: editingEvent, input })
        }
        open={editorOpen}
        pending={saveMutation.isPending}
      />
    </div>
  );
}

function MaintenanceLoading() {
  return (
    <CardContent className="flex flex-col gap-3 pt-6">
      {Array.from({ length: 4 }, (_, index) => (
        <Skeleton className="h-24 w-full" key={index} />
      ))}
    </CardContent>
  );
}

function MaintenanceDetail({
  event,
  conflicts,
  conflictsLoading,
  conflictsError,
  lifecyclePending,
  lifecycleError,
  onEdit,
  onLifecycle,
}: {
  event: MaintenanceEvent;
  conflicts: MaintenanceConflict[];
  conflictsLoading: boolean;
  conflictsError: unknown;
  lifecyclePending: boolean;
  lifecycleError: unknown;
  onEdit: () => void;
  onLifecycle: (action: LifecycleAction) => void;
}) {
  const firstOccurrence = occurrenceItems([event])[0]?.occurrence;
  const canEdit = ["draft", "scheduled", "upcoming"].includes(event.state);
  const canCancel = [
    "draft",
    "scheduled",
    "upcoming",
    "active",
    "overrunning",
  ].includes(event.state);
  const canStart = ["scheduled", "upcoming"].includes(event.state);
  const canComplete = ["active", "overrunning"].includes(event.state);

  return (
    <Card id={`maintenance-event-${event.id}`}>
      <CardHeader className="border-b">
        <div className="flex min-w-0 flex-col gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <CardTitle className="truncate">{event.name}</CardTitle>
            <Badge variant={maintenanceStateVariant(event.state)}>
              {labelize(event.state)}
            </Badge>
            {event.disruptive ? (
              <Badge variant="outline">Disruptive</Badge>
            ) : null}
          </div>
          <CardDescription>
            {event.description || "No description."} · Updated{" "}
            {formatRelative(event.updated_at)}
          </CardDescription>
        </div>
        <CardAction className="flex flex-wrap justify-end gap-2">
          {canEdit ? (
            <Button onClick={onEdit} size="sm" variant="outline">
              <PencilIcon data-icon="inline-start" />
              Edit
            </Button>
          ) : null}
          {event.state === "draft" ? (
            <Button
              disabled={lifecyclePending}
              onClick={() => onLifecycle("schedule")}
              size="sm"
              variant="outline"
            >
              Schedule
            </Button>
          ) : null}
          {canStart ? (
            <Button
              disabled={lifecyclePending}
              onClick={() => onLifecycle("start")}
              size="sm"
            >
              Start now
            </Button>
          ) : null}
          {canComplete ? (
            <Button
              disabled={lifecyclePending}
              onClick={() => onLifecycle("complete")}
              size="sm"
            >
              Mark complete
            </Button>
          ) : null}
          {canCancel ? (
            <Button
              disabled={lifecyclePending}
              onClick={() => onLifecycle("cancel")}
              size="sm"
              variant="destructive"
            >
              Cancel
            </Button>
          ) : null}
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-6 pt-6">
        {lifecycleError ? (
          <Alert variant="destructive">
            <CircleAlertIcon />
            <AlertTitle>Lifecycle change was not saved</AlertTitle>
            <AlertDescription>
              {getUserFacingError(lifecycleError, "Try again.")}
            </AlertDescription>
          </Alert>
        ) : null}
        {event.state === "overrunning" ? (
          <Alert variant="destructive">
            <CircleAlertIcon />
            <AlertTitle>Maintenance is overrunning</AlertTitle>
            <AlertDescription>
              Planned end passed while the reservation remains active. Complete
              or cancel the event after health recovers.
            </AlertDescription>
          </Alert>
        ) : event.state === "active" ? (
          <Alert>
            <ShieldAlertIcon />
            <AlertTitle>Expected failures are suppressed</AlertTitle>
            <AlertDescription>
              Underlying observations remain visible. Notifications stay
              suppressed for affected resources and downstream services.
            </AlertDescription>
          </Alert>
        ) : null}

        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
          <DetailValue label="Planned window">
            <span>
              {formatDate(event.start_at)}–{formatDate(event.end_at)}
            </span>
            <span className="text-xs text-muted-foreground">
              {event.timezone}
            </span>
          </DetailValue>
          <DetailValue label="Reservation window">
            <span>
              {firstOccurrence
                ? formatDate(firstOccurrence.reservation_start)
                : "—"}
              –
              {firstOccurrence
                ? formatDate(firstOccurrence.reservation_end)
                : "—"}
            </span>
            <span className="text-xs text-muted-foreground">
              {formatSeconds(event.lead_in_seconds)} lead ·{" "}
              {formatSeconds(event.cooldown_seconds)} cooldown
            </span>
          </DetailValue>
          <DetailValue label="Schedule">
            <span>{event.recurrence_rule ? "Recurring" : "One-time"}</span>
            <span className="truncate text-xs text-muted-foreground">
              {event.recurrence_rule || "Single occurrence"}
            </span>
          </DetailValue>
          <DetailValue label="Owner and source">
            <span>{event.owner || "No owner"}</span>
            <span className="text-xs text-muted-foreground">
              {event.source || "No source"} · v{event.version}
            </span>
          </DetailValue>
        </div>

        <section
          aria-labelledby="maintenance-reserved-resources"
          className="flex flex-col gap-3"
        >
          <div className="flex items-center justify-between gap-3">
            <h3 className="font-medium" id="maintenance-reserved-resources">
              Reserved resources
            </h3>
            <span className="text-xs text-muted-foreground">
              {event.resources.length} resource
              {event.resources.length === 1 ? "" : "s"}
            </span>
          </div>
          {event.resources.length === 0 ? (
            <p className="rounded-lg border border-dashed p-4 text-sm text-muted-foreground">
              No targets or affected resources recorded.
            </p>
          ) : (
            <div className="grid gap-2 sm:grid-cols-2">
              {event.resources.map((resource, index) => (
                <div
                  className="flex min-w-0 items-start gap-3 rounded-lg border p-3"
                  key={`${resource.role}-${resource.key ?? resource.id ?? index}`}
                >
                  <span className="mt-0.5 grid size-8 shrink-0 place-items-center rounded-md bg-muted text-muted-foreground">
                    <Layers3Icon className="size-4" />
                  </span>
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-medium">
                        {resourceName(resource)}
                      </span>
                      <Badge variant="outline">{resource.role}</Badge>
                    </div>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {resource.expected_failure
                        ? "Expected failure"
                        : "Failure not expected"}
                    </p>
                  </div>
                </div>
              ))}
            </div>
          )}
        </section>

        <section
          aria-labelledby="maintenance-occurrences"
          className="flex flex-col gap-3"
        >
          <div className="flex items-center justify-between gap-3">
            <h3 className="font-medium" id="maintenance-occurrences">
              Expanded occurrences
            </h3>
            <span className="text-xs text-muted-foreground">
              {event.occurrences.length || 1} shown by server
            </span>
          </div>
          <div className="grid gap-2 md:grid-cols-2 xl:grid-cols-3">
            {occurrenceItems([event])
              .slice(0, 6)
              .map(({ occurrence }) => (
                <div className="rounded-lg border p-3" key={occurrence.id}>
                  <div className="flex items-center gap-2 text-sm font-medium">
                    <CalendarDaysIcon className="size-4 text-muted-foreground" />
                    {formatDate(occurrence.start_at)}
                  </div>
                  <p className="mt-1 text-xs text-muted-foreground">
                    Reserved {formatDate(occurrence.reservation_start)}–
                    {formatDate(occurrence.reservation_end)}
                  </p>
                </div>
              ))}
          </div>
        </section>

        {event.notes || event.links.length > 0 ? (
          <section className="grid gap-4 border-t pt-5 md:grid-cols-2">
            {event.notes ? (
              <DetailValue label="Notes">
                <span className="whitespace-pre-wrap">{event.notes}</span>
              </DetailValue>
            ) : (
              <div />
            )}
            {event.links.length > 0 ? (
              <DetailValue label="Links">
                {event.links.map((link) => (
                  <a
                    className="inline-flex items-center gap-1 text-primary hover:underline"
                    href={link}
                    key={link}
                  >
                    {link}
                    <ExternalLinkIcon className="size-3" />
                  </a>
                ))}
              </DetailValue>
            ) : null}
          </section>
        ) : null}

        <section
          aria-labelledby="maintenance-conflicts"
          className="flex flex-col gap-3 border-t pt-5"
        >
          <div className="flex items-center justify-between gap-3">
            <h3 className="font-medium" id="maintenance-conflicts">
              Conflict check
            </h3>
            {conflicts.length > 0 ? (
              <Badge variant="destructive">
                {conflicts.length} conflict{conflicts.length === 1 ? "" : "s"}
              </Badge>
            ) : null}
          </div>
          {conflictsLoading ? (
            <Skeleton className="h-16 w-full" />
          ) : conflictsError ? (
            <Alert variant="destructive">
              <CircleAlertIcon />
              <AlertTitle>Conflict check unavailable</AlertTitle>
              <AlertDescription>
                {getUserFacingError(conflictsError, "Try again.")}
              </AlertDescription>
            </Alert>
          ) : conflicts.length > 0 ? (
            <MaintenanceConflictList conflicts={conflicts} />
          ) : (
            <p className="flex items-center gap-2 rounded-lg border border-dashed p-3 text-sm text-muted-foreground">
              <CheckCircle2Icon className="size-4 text-status-healthy-fg" />
              No overlapping reservation found.
            </p>
          )}
        </section>
      </CardContent>
    </Card>
  );
}

function DetailValue({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <span className="text-xs text-muted-foreground">{label}</span>
      <div className="flex min-w-0 flex-col gap-0.5 text-sm">{children}</div>
    </div>
  );
}
