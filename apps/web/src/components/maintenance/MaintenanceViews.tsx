import { CalendarDaysIcon, Clock3Icon, Repeat2Icon } from "lucide-react";
import { useMemo, useState } from "react";
import type { MaintenanceEvent, MaintenanceOccurrence } from "@/lib/api";
import { formatDate, labelize, shortId } from "@/lib/format";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Calendar } from "@/components/ui/calendar";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

export type MaintenanceView = "calendar" | "timeline" | "list";

export function maintenanceStateVariant(
  state: string,
): "healthy" | "attention" | "critical" | "info" | "neutral" {
  switch (state) {
    case "active":
      return "attention";
    case "overrunning":
      return "critical";
    case "completed":
      return "healthy";
    case "scheduled":
    case "upcoming":
      return "info";
    default:
      return "neutral";
  }
}

function safeDate(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

function dateKey(date: Date) {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

function formatTime(value: string) {
  const date = safeDate(value);
  return date
    ? new Intl.DateTimeFormat(undefined, {
        hour: "numeric",
        minute: "2-digit",
      }).format(date)
    : "Unknown time";
}

function formatDay(value: string) {
  const date = safeDate(value);
  return date
    ? new Intl.DateTimeFormat(undefined, {
        weekday: "short",
        month: "short",
        day: "numeric",
      }).format(date)
    : "Unknown date";
}

function formatDuration(start: string, end: string) {
  const startDate = safeDate(start);
  const endDate = safeDate(end);
  if (!startDate || !endDate) return "Unknown duration";
  const minutes = Math.max(
    0,
    Math.round((endDate.getTime() - startDate.getTime()) / 60_000),
  );
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  const remainder = minutes % 60;
  return remainder ? `${hours}h ${remainder}m` : `${hours}h`;
}

function occurrencesFor(event: MaintenanceEvent): MaintenanceOccurrence[] {
  if (event.occurrences.length > 0) return event.occurrences;
  return [
    {
      id: `${event.id}-planned`,
      occurrence_key: event.start_at,
      occurrence_index: 0,
      start_at: event.start_at,
      end_at: event.end_at,
      reservation_start: new Date(
        new Date(event.start_at).getTime() - event.lead_in_seconds * 1000,
      ).toISOString(),
      reservation_end: new Date(
        new Date(event.end_at).getTime() + event.cooldown_seconds * 1000,
      ).toISOString(),
      timezone: event.timezone,
    },
  ];
}

export interface MaintenanceOccurrenceItem {
  event: MaintenanceEvent;
  occurrence: MaintenanceOccurrence;
}

export function occurrenceItems(events: MaintenanceEvent[]) {
  return events
    .flatMap((event) =>
      occurrencesFor(event).map((occurrence) => ({ event, occurrence })),
    )
    .sort((left, right) => {
      const leftTime =
        safeDate(left.occurrence.start_at)?.getTime() ??
        Number.MAX_SAFE_INTEGER;
      const rightTime =
        safeDate(right.occurrence.start_at)?.getTime() ??
        Number.MAX_SAFE_INTEGER;
      return leftTime - rightTime;
    });
}

function resourceLabel(event: MaintenanceEvent) {
  const labels = event.resources.map((resource) => {
    if (resource.key) return resource.key;
    if (resource.kind && resource.id)
      return `${resource.kind}/${shortId(resource.id)}`;
    return "Unidentified resource";
  });
  return labels.length > 0 ? labels : ["No resources"];
}

function EventBadges({ event }: { event: MaintenanceEvent }) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Badge variant={maintenanceStateVariant(event.state)}>
        {labelize(event.state)}
      </Badge>
      {event.disruptive ? <Badge variant="outline">Disruptive</Badge> : null}
      {event.recurrence_rule ? (
        <Badge variant="outline">
          <Repeat2Icon />
          Recurring
        </Badge>
      ) : null}
    </div>
  );
}

function OccurrenceMeta({ item }: { item: MaintenanceOccurrenceItem }) {
  const { event, occurrence } = item;
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
      <span className="inline-flex items-center gap-1">
        <Clock3Icon className="size-3.5" />
        {formatTime(occurrence.start_at)}–{formatTime(occurrence.end_at)}
      </span>
      <span>{formatDuration(occurrence.start_at, occurrence.end_at)}</span>
      <span>{event.timezone}</span>
    </div>
  );
}

export function MaintenanceTimelineView({
  events,
  selectedId,
  onSelect,
}: {
  events: MaintenanceEvent[];
  selectedId: string | null;
  onSelect: (event: MaintenanceEvent) => void;
}) {
  const items = occurrenceItems(events).slice(0, 80);
  if (items.length === 0) {
    return (
      <Empty>
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <CalendarDaysIcon />
          </EmptyMedia>
          <EmptyTitle>No scheduled occurrences</EmptyTitle>
          <EmptyDescription>
            Events appear here after they have a planned window.
          </EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }

  return (
    <ol
      aria-label="Maintenance timeline"
      className="relative flex flex-col gap-3 before:absolute before:top-4 before:bottom-4 before:left-3 before:w-px before:bg-border"
    >
      {items.map((item) => (
        <li
          className="relative pl-8"
          key={`${item.event.id}-${item.occurrence.id}`}
        >
          <span
            aria-hidden="true"
            className="absolute top-4 left-1.5 size-3 rounded-full border-2 border-background bg-primary ring-1 ring-primary/30"
          />
          <button
            aria-current={item.event.id === selectedId ? "true" : undefined}
            className="w-full rounded-xl border bg-card p-4 text-left transition-colors hover:bg-muted/40 focus-visible:ring-3 focus-visible:ring-ring/50"
            onClick={() => onSelect(item.event)}
            type="button"
          >
            <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-xs font-medium text-muted-foreground">
                    {formatDay(item.occurrence.start_at)}
                  </span>
                  <EventBadges event={item.event} />
                </div>
                <h3 className="mt-2 truncate font-medium">{item.event.name}</h3>
                <div className="mt-1">
                  <OccurrenceMeta item={item} />
                </div>
              </div>
              <div className="flex shrink-0 flex-wrap gap-1.5 sm:max-w-[16rem] sm:justify-end">
                {resourceLabel(item.event)
                  .slice(0, 3)
                  .map((resource) => (
                    <Badge key={resource} variant="secondary">
                      {resource}
                    </Badge>
                  ))}
                {item.event.resources.length > 3 ? (
                  <Badge variant="secondary">
                    +{item.event.resources.length - 3}
                  </Badge>
                ) : null}
              </div>
            </div>
            <div className="mt-3 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
              <span>
                Reservation {formatTime(item.occurrence.reservation_start)}–
                {formatTime(item.occurrence.reservation_end)}
              </span>
              {item.event.lead_in_seconds || item.event.cooldown_seconds ? (
                <span>Lead/cooldown configured</span>
              ) : null}
            </div>
          </button>
        </li>
      ))}
    </ol>
  );
}

export function MaintenanceListView({
  events,
  selectedId,
  onSelect,
}: {
  events: MaintenanceEvent[];
  selectedId: string | null;
  onSelect: (event: MaintenanceEvent) => void;
}) {
  if (events.length === 0) {
    return (
      <Empty>
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <CalendarDaysIcon />
          </EmptyMedia>
          <EmptyTitle>No maintenance events</EmptyTitle>
          <EmptyDescription>
            Create an event to reserve a maintenance window.
          </EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }

  return (
    <Table>
      <caption className="sr-only">Maintenance events</caption>
      <TableHeader>
        <TableRow>
          <TableHead scope="col">Event</TableHead>
          <TableHead scope="col">Planned window</TableHead>
          <TableHead scope="col">State</TableHead>
          <TableHead scope="col">Reservation</TableHead>
          <TableHead scope="col">Resources</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {events.map((event) => {
          const occurrence = occurrencesFor(event)[0];
          return (
            <TableRow
              key={event.id}
              data-state={event.id === selectedId ? "selected" : undefined}
            >
              <TableCell className="min-w-52">
                <Button
                  className="h-auto max-w-full justify-start px-0 text-left hover:bg-transparent"
                  onClick={() => onSelect(event)}
                  variant="ghost"
                >
                  <span className="min-w-0">
                    <span className="block truncate font-medium">
                      {event.name}
                    </span>
                    <span className="mt-1 block text-xs text-muted-foreground">
                      {event.timezone}
                    </span>
                  </span>
                </Button>
              </TableCell>
              <TableCell>
                <span className="block">{formatDate(event.start_at)}</span>
                <span className="text-xs text-muted-foreground">
                  {formatDuration(event.start_at, event.end_at)}
                </span>
              </TableCell>
              <TableCell>
                <EventBadges event={event} />
              </TableCell>
              <TableCell>
                <span className="block">
                  {formatDate(occurrence.reservation_start)}
                </span>
                <span className="text-xs text-muted-foreground">
                  to {formatDate(occurrence.reservation_end)}
                </span>
              </TableCell>
              <TableCell>
                <div className="flex max-w-56 flex-wrap gap-1.5">
                  {resourceLabel(event)
                    .slice(0, 3)
                    .map((resource) => (
                      <Badge key={resource} variant="secondary">
                        {resource}
                      </Badge>
                    ))}
                  {event.resources.length > 3 ? (
                    <Badge variant="secondary">
                      +{event.resources.length - 3}
                    </Badge>
                  ) : null}
                </div>
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}

export function MaintenanceCalendarView({
  events,
  selectedId,
  onSelect,
}: {
  events: MaintenanceEvent[];
  selectedId: string | null;
  onSelect: (event: MaintenanceEvent) => void;
}) {
  const [selectedDate, setSelectedDate] = useStateDate();
  const items = useMemo(() => occurrenceItems(events), [events]);
  const eventDates = items
    .map((item) => safeDate(item.occurrence.start_at))
    .filter((date): date is Date => date !== null);
  const selectedItems = items.filter((item) => {
    const date = safeDate(item.occurrence.start_at);
    return date ? dateKey(date) === dateKey(selectedDate) : false;
  });

  return (
    <div className="grid gap-5 lg:grid-cols-[minmax(18rem,24rem)_minmax(0,1fr)]">
      <Card className="h-fit">
        <CardHeader>
          <CardTitle>Maintenance calendar</CardTitle>
        </CardHeader>
        <CardContent className="flex justify-center px-2 pb-4 sm:px-4">
          <Calendar
            className="w-full"
            mode="single"
            modifiers={{ maintenance: eventDates }}
            modifiersClassNames={{ maintenance: "bg-accent font-semibold" }}
            onSelect={(date) => {
              if (date) setSelectedDate(date);
            }}
            selected={selectedDate}
          />
        </CardContent>
      </Card>
      <Card className="min-w-0">
        <CardHeader>
          <CardTitle>
            {new Intl.DateTimeFormat(undefined, { dateStyle: "full" }).format(
              selectedDate,
            )}
          </CardTitle>
        </CardHeader>
        <CardContent>
          {selectedItems.length === 0 ? (
            <Empty className="border-0 px-2 py-8">
              <EmptyHeader>
                <EmptyMedia variant="icon">
                  <CalendarDaysIcon />
                </EmptyMedia>
                <EmptyTitle>No events on this date</EmptyTitle>
                <EmptyDescription>
                  Select a highlighted day to inspect its reservations.
                </EmptyDescription>
              </EmptyHeader>
            </Empty>
          ) : (
            <div className="flex flex-col gap-3">
              {selectedItems.map((item) => (
                <button
                  aria-current={
                    item.event.id === selectedId ? "true" : undefined
                  }
                  className="rounded-xl border p-4 text-left transition-colors hover:bg-muted/40 focus-visible:ring-3 focus-visible:ring-ring/50"
                  key={`${item.event.id}-${item.occurrence.id}`}
                  onClick={() => onSelect(item.event)}
                  type="button"
                >
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <h3 className="font-medium">{item.event.name}</h3>
                    <EventBadges event={item.event} />
                  </div>
                  <div className="mt-2">
                    <OccurrenceMeta item={item} />
                  </div>
                  <p className="mt-2 text-xs text-muted-foreground">
                    Reserved {formatTime(item.occurrence.reservation_start)}–
                    {formatTime(item.occurrence.reservation_end)}
                  </p>
                </button>
              ))}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

function useStateDate() {
  const [selectedDate, setSelectedDate] = useState(new Date());
  return [selectedDate, setSelectedDate] as const;
}
