import {
  useEffect,
  useState,
  type Dispatch,
  type FormEvent,
  type SetStateAction,
} from "react";
import {
  CalendarDaysIcon,
  CircleAlertIcon,
  PlusIcon,
  Trash2Icon,
} from "lucide-react";
import {
  MaintenanceApiError,
  type MaintenanceEvent,
  type MaintenanceEventInput,
  type MaintenanceResource,
  type MaintenanceResourceInput,
} from "@/lib/api";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Calendar } from "@/components/ui/calendar";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { MaintenanceConflictList } from "./MaintenanceConflictList";

type ResourceMode = "key" | "entity";

interface FormResource {
  role: "target" | "required" | "affected" | "exclusive";
  mode: ResourceMode;
  key: string;
  kind: string;
  id: string;
  expected_failure: boolean;
}

interface FormState {
  name: string;
  description: string;
  timezone: string;
  start: string;
  end: string;
  recurrence_rule: string;
  lead_in_minutes: string;
  cooldown_minutes: string;
  disruptive: boolean;
  owner: string;
  source: string;
  notes: string;
  links: string;
  state: "draft" | "scheduled";
  resources: FormResource[];
}

const RESOURCE_ROLES = ["target", "required", "affected", "exclusive"] as const;
const RECURRENCE_FREQUENCIES = [
  "DAILY",
  "WEEKLY",
  "MONTHLY",
  "YEARLY",
] as const;
const RECURRENCE_WEEKDAYS = [
  ["MO", "Monday"],
  ["TU", "Tuesday"],
  ["WE", "Wednesday"],
  ["TH", "Thursday"],
  ["FR", "Friday"],
  ["SA", "Saturday"],
  ["SU", "Sunday"],
] as const;

type RecurrenceFrequency = (typeof RECURRENCE_FREQUENCIES)[number];

interface GuidedRecurrence {
  frequency: RecurrenceFrequency | "";
  interval: string;
  weekdays: string[];
  end: "never" | "count";
  count: string;
}

function emptyGuidedRecurrence(): GuidedRecurrence {
  return {
    frequency: "",
    interval: "1",
    weekdays: [],
    end: "never",
    count: "3",
  };
}

function pad(value: number) {
  return String(value).padStart(2, "0");
}

function localDateTime(value?: string | null) {
  const date = value ? new Date(value) : new Date(Date.now() + 60 * 60 * 1000);
  if (Number.isNaN(date.getTime())) return "";
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

interface LocalDateTimeParts {
  date: string;
  time: string;
}

function splitLocalDateTime(value: string): LocalDateTimeParts | null {
  const match = /^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2})$/.exec(value);
  if (!match) return null;
  const [, date, time] = match;
  const [year, month, day] = date.split("-").map(Number);
  const [hour, minute] = time.split(":").map(Number);
  const parsed = new Date(year, month - 1, day, hour, minute);
  if (
    parsed.getFullYear() !== year ||
    parsed.getMonth() !== month - 1 ||
    parsed.getDate() !== day ||
    parsed.getHours() !== hour ||
    parsed.getMinutes() !== minute
  ) {
    return null;
  }
  return { date, time };
}

function dateFromLocalDate(value: string) {
  const [year, month, day] = value.split("-").map(Number);
  if (![year, month, day].every(Number.isFinite)) return undefined;
  const date = new Date(year, month - 1, day);
  return date.getFullYear() === year &&
    date.getMonth() === month - 1 &&
    date.getDate() === day
    ? date
    : undefined;
}

function localDatePart(date: Date) {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

function LocalDateTimePicker({
  id,
  label,
  value,
  onChange,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  const parts = splitLocalDateTime(value);
  const [dateDraft, setDateDraft] = useState(parts?.date ?? "");
  const [timeDraft, setTimeDraft] = useState(parts?.time ?? "");

  useEffect(() => {
    const next = splitLocalDateTime(value);
    if (next) {
      setDateDraft(next.date);
      setTimeDraft(next.time);
    }
  }, [value]);

  function emit(nextDate: string, nextTime: string) {
    onChange(`${nextDate}T${nextTime}`);
  }

  const selectedDate = dateFromLocalDate(dateDraft);

  return (
    <div className="grid gap-2">
      <div className="flex gap-2">
        <Input
          aria-label={label}
          id={id}
          inputMode="numeric"
          onChange={(eventObject) => {
            const nextDate = eventObject.target.value;
            setDateDraft(nextDate);
            emit(nextDate, timeDraft);
          }}
          placeholder="YYYY-MM-DD"
          required
          type="text"
          value={dateDraft}
        />
        <Input
          aria-label={`${label} time`}
          className="w-28"
          inputMode="numeric"
          onChange={(eventObject) => {
            const nextTime = eventObject.target.value;
            setTimeDraft(nextTime);
            emit(dateDraft, nextTime);
          }}
          placeholder="HH:MM"
          required
          type="text"
          value={timeDraft}
        />
        <Popover>
          <PopoverTrigger
            aria-label="Show local date and time picker"
            render={<Button size="icon" type="button" variant="outline" />}
          >
            <CalendarDaysIcon />
          </PopoverTrigger>
          <PopoverContent align="start" className="w-auto p-0">
            <Calendar
              defaultMonth={selectedDate}
              mode="single"
              onSelect={(date) => {
                if (!date) return;
                const nextDate = localDatePart(date);
                setDateDraft(nextDate);
                emit(nextDate, timeDraft);
              }}
              selected={selectedDate}
            />
            <div className="border-t p-3">
              <Label htmlFor={`${id}-picker-time`}>Time (24-hour)</Label>
              <Input
                className="mt-2"
                id={`${id}-picker-time`}
                inputMode="numeric"
                onChange={(eventObject) => {
                  const nextTime = eventObject.target.value;
                  setTimeDraft(nextTime);
                  emit(dateDraft, nextTime);
                }}
                placeholder="HH:MM"
                type="text"
                value={timeDraft}
              />
              <p className="mt-1 text-xs text-muted-foreground">
                Enter local time as HH:MM.
              </p>
            </div>
          </PopoverContent>
        </Popover>
      </div>
      <p className="text-xs text-muted-foreground">
        Use local time in YYYY-MM-DD and HH:MM format.
      </p>
    </div>
  );
}

function zonedDateTime(value: string, timezone: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  try {
    const parts = new Intl.DateTimeFormat("en-CA", {
      timeZone: timezone,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      hourCycle: "h23",
    })
      .formatToParts(date)
      .reduce<Record<string, string>>((values, part) => {
        if (part.type !== "literal") values[part.type] = part.value;
        return values;
      }, {});
    return `${parts.year}-${parts.month}-${parts.day}T${parts.hour}:${parts.minute}`;
  } catch {
    return localDateTime(value);
  }
}

function defaultFormState(): FormState {
  const start = localDateTime();
  const endDate = new Date(`${start}:00`);
  endDate.setHours(endDate.getHours() + 1);
  return {
    name: "",
    description: "",
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
    start,
    end: localDateTime(endDate.toISOString()),
    recurrence_rule: "",
    lead_in_minutes: "0",
    cooldown_minutes: "0",
    disruptive: false,
    owner: "",
    source: "",
    notes: "",
    links: "",
    state: "scheduled",
    resources: [],
  };
}

function resourceToForm(resource: MaintenanceResource): FormResource {
  if (resource.key) {
    return {
      role: toResourceRole(resource.role),
      mode: "key",
      key: resource.key,
      kind: "",
      id: "",
      expected_failure: resource.expected_failure,
    };
  }
  return {
    role: toResourceRole(resource.role),
    mode: "entity",
    key: "",
    kind: resource.kind ?? "",
    id: resource.id ?? "",
    expected_failure: resource.expected_failure,
  };
}

function toResourceRole(role: string): FormResource["role"] {
  return RESOURCE_ROLES.includes(role as FormResource["role"])
    ? (role as FormResource["role"])
    : "affected";
}

function formStateFor(event: MaintenanceEvent | null) {
  if (!event) return defaultFormState();
  return {
    name: event.name,
    description: event.description ?? "",
    timezone: event.timezone,
    start: zonedDateTime(event.start_at, event.timezone),
    end: zonedDateTime(event.end_at, event.timezone),
    recurrence_rule: event.recurrence_rule ?? "",
    lead_in_minutes: String(Math.round(event.lead_in_seconds / 60)),
    cooldown_minutes: String(Math.round(event.cooldown_seconds / 60)),
    disruptive: event.disruptive,
    owner: event.owner ?? "",
    source: event.source ?? "",
    notes: event.notes ?? "",
    links: event.links.join("\n"),
    state: event.state === "draft" ? "draft" : "scheduled",
    resources: event.resources.map(resourceToForm),
  } satisfies FormState;
}

function toSeconds(value: string) {
  const minutes = Number(value);
  return Number.isFinite(minutes) && minutes >= 0
    ? Math.round(minutes * 60)
    : 0;
}

function toInput(form: FormState): MaintenanceEventInput {
  const resources: MaintenanceResourceInput[] = form.resources
    .filter((resource) =>
      resource.mode === "key"
        ? resource.key.trim().length > 0
        : resource.kind.trim().length > 0 || resource.id.trim().length > 0,
    )
    .map((resource) =>
      resource.mode === "key"
        ? {
            role: resource.role,
            key: resource.key.trim(),
            expected_failure: resource.expected_failure,
          }
        : {
            role: resource.role,
            kind: resource.kind.trim(),
            id: resource.id.trim(),
            expected_failure: resource.expected_failure,
          },
    );

  const links = form.links
    .split(/[\n,]/)
    .map((link) => link.trim())
    .filter(Boolean);

  return {
    name: form.name.trim(),
    description: form.description.trim() || null,
    timezone: form.timezone.trim(),
    start: form.start,
    end: form.end,
    recurrence_rule: form.recurrence_rule.trim() || null,
    lead_in_seconds: toSeconds(form.lead_in_minutes),
    cooldown_seconds: toSeconds(form.cooldown_minutes),
    disruptive: form.disruptive,
    owner: form.owner.trim() || null,
    source: form.source.trim() || null,
    notes: form.notes || null,
    links,
    resources,
    state: form.state,
  };
}

function isSupportedTimezone(value: string) {
  try {
    new Intl.DateTimeFormat("en-US", { timeZone: value }).format();
    return true;
  } catch {
    return false;
  }
}

function recurrenceValidationError(value: string) {
  const rule = value.trim().replace(/^RRULE:/i, "");
  if (!rule) return null;
  const fields = rule.split(";");
  const frequency = fields.find((field) => /^FREQ=/i.test(field));
  const supportedFrequencies = new Set([
    "SECONDLY",
    "MINUTELY",
    "HOURLY",
    "DAILY",
    "WEEKLY",
    "MONTHLY",
    "YEARLY",
  ]);
  if (
    !frequency ||
    !supportedFrequencies.has(frequency.slice(5).toUpperCase())
  ) {
    return "Recurrence rule must include a supported FREQ value.";
  }
  if (
    /[\r\n]/.test(rule) ||
    fields.some((field) => {
      const [key, fieldValue, ...rest] = field.split("=");
      return (
        !key ||
        !fieldValue ||
        rest.length > 0 ||
        !/^[A-Z][A-Z0-9-]*$/i.test(key)
      );
    })
  ) {
    return "Recurrence rule must use KEY=VALUE pairs separated by semicolons.";
  }
  if (fields.some((field) => /^(DTSTART|RDATE|EXDATE)=/i.test(field))) {
    return "Recurrence rule must contain only RRULE properties.";
  }
  return null;
}

function parseGuidedRecurrence(value: string): GuidedRecurrence | null {
  const rule = value.trim().replace(/^RRULE:/i, "");
  if (!rule) return emptyGuidedRecurrence();
  const properties = new Map<string, string>();
  for (const field of rule.split(";")) {
    const [key, fieldValue, ...rest] = field.split("=");
    if (!key || !fieldValue || rest.length > 0) return null;
    properties.set(key.toUpperCase(), fieldValue);
  }
  const allowedProperties = new Set(["FREQ", "INTERVAL", "BYDAY", "COUNT"]);
  if ([...properties.keys()].some((key) => !allowedProperties.has(key))) {
    return null;
  }
  const frequency = properties.get("FREQ")?.toUpperCase();
  if (
    !frequency ||
    !RECURRENCE_FREQUENCIES.includes(frequency as RecurrenceFrequency)
  ) {
    return null;
  }
  const intervalValue = Number.parseInt(properties.get("INTERVAL") ?? "1", 10);
  if (
    !Number.isFinite(intervalValue) ||
    intervalValue < 1 ||
    String(intervalValue) !== (properties.get("INTERVAL") ?? "1")
  ) {
    return null;
  }
  const rawWeekdays = properties.get("BYDAY") ?? "";
  const weekdays = rawWeekdays ? rawWeekdays.split(",") : [];
  if (
    weekdays.some((day) => !RECURRENCE_WEEKDAYS.some(([code]) => code === day))
  ) {
    return null;
  }
  const count = Number.parseInt(properties.get("COUNT") ?? "", 10);
  if (
    properties.has("COUNT") &&
    (!Number.isFinite(count) ||
      count < 1 ||
      String(count) !== properties.get("COUNT"))
  ) {
    return null;
  }
  return {
    frequency: frequency as RecurrenceFrequency,
    interval: String(intervalValue),
    weekdays,
    end: Number.isFinite(count) && count > 0 ? "count" : "never",
    count: Number.isFinite(count) && count > 0 ? String(count) : "3",
  };
}

function buildGuidedRecurrence(recurrence: GuidedRecurrence) {
  if (!recurrence.frequency) return "";
  const fields = [`FREQ=${recurrence.frequency}`];
  const interval = Number.parseInt(recurrence.interval, 10);
  if (Number.isFinite(interval) && interval > 1) {
    fields.push(`INTERVAL=${interval}`);
  }
  if (recurrence.frequency === "WEEKLY" && recurrence.weekdays.length > 0) {
    const weekdays = RECURRENCE_WEEKDAYS.map(([code]) => code).filter((code) =>
      recurrence.weekdays.includes(code),
    );
    if (weekdays.length > 0) fields.push(`BYDAY=${weekdays.join(",")}`);
  }
  if (recurrence.end === "count") {
    const count = Number.parseInt(recurrence.count, 10);
    if (Number.isFinite(count) && count > 0) fields.push(`COUNT=${count}`);
  }
  return fields.join(";");
}

export function MaintenanceEventForm({
  open,
  event,
  pending,
  error,
  onOpenChange,
  onSubmit,
}: {
  open: boolean;
  event: MaintenanceEvent | null;
  pending: boolean;
  error: unknown;
  onOpenChange: (open: boolean) => void;
  onSubmit: (input: MaintenanceEventInput) => void;
}) {
  const [form, setForm] = useState<FormState>(() => formStateFor(event));
  const [validationError, setValidationError] = useState<string | null>(null);
  const [recurrenceMode, setRecurrenceMode] = useState<"guided" | "advanced">(
    "guided",
  );
  const [guidedRecurrence, setGuidedRecurrence] = useState<GuidedRecurrence>(
    emptyGuidedRecurrence,
  );

  useEffect(() => {
    if (open) {
      const nextForm = formStateFor(event);
      const parsedRecurrence = parseGuidedRecurrence(nextForm.recurrence_rule);
      setForm(nextForm);
      setGuidedRecurrence(parsedRecurrence ?? emptyGuidedRecurrence());
      setRecurrenceMode(
        parsedRecurrence || !nextForm.recurrence_rule ? "guided" : "advanced",
      );
      setValidationError(null);
    }
  }, [event, open]);

  function update<K extends keyof FormState>(field: K, value: FormState[K]) {
    setForm((current) => ({ ...current, [field]: value }));
  }

  function updateGuidedRecurrence(patch: Partial<GuidedRecurrence>) {
    const next = { ...guidedRecurrence, ...patch };
    setGuidedRecurrence(next);
    update("recurrence_rule", buildGuidedRecurrence(next));
  }

  function switchRecurrenceMode(value: string | null) {
    if (value === "advanced") {
      setRecurrenceMode("advanced");
      return;
    }
    if (value !== "guided") return;
    const parsed =
      parseGuidedRecurrence(form.recurrence_rule) ?? emptyGuidedRecurrence();
    setGuidedRecurrence(parsed);
    update("recurrence_rule", buildGuidedRecurrence(parsed));
    setRecurrenceMode("guided");
  }

  function submit(eventObject: FormEvent<HTMLFormElement>) {
    eventObject.preventDefault();
    setValidationError(null);
    if (!form.name.trim()) {
      setValidationError("Event name is required.");
      return;
    }
    if (!form.timezone.trim()) {
      setValidationError("Timezone is required.");
      return;
    }
    if (!isSupportedTimezone(form.timezone.trim())) {
      setValidationError("Timezone must be a supported IANA timezone.");
      return;
    }
    const recurrenceError = recurrenceValidationError(form.recurrence_rule);
    if (recurrenceError) {
      setValidationError(recurrenceError);
      return;
    }
    const start = splitLocalDateTime(form.start);
    const end = splitLocalDateTime(form.end);
    if (!start || !end || new Date(form.end) <= new Date(form.start)) {
      setValidationError(
        start && end
          ? "End time must be later than start time."
          : "Start and end must use YYYY-MM-DD and HH:MM format.",
      );
      return;
    }
    const invalidResource = form.resources.find((resource) =>
      resource.mode === "key"
        ? !resource.key.trim()
        : !resource.kind.trim() || !resource.id.trim(),
    );
    if (invalidResource) {
      setValidationError(
        "Complete each resource identifier or remove the row.",
      );
      return;
    }
    onSubmit(toInput(form));
  }

  const conflicts = error instanceof MaintenanceApiError ? error.conflicts : [];
  const errorMessage = error instanceof Error ? error.message : null;

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent className="max-h-[min(90vh,60rem)] overflow-y-auto sm:max-w-5xl">
        <DialogHeader>
          <DialogTitle>
            {event ? "Edit maintenance event" : "Create maintenance event"}
          </DialogTitle>
          <DialogDescription>
            Reserve targets and affected resources. The server expands recurring
            events for 90 days and rejects conflicts.
          </DialogDescription>
        </DialogHeader>

        <form className="flex flex-col gap-6" onSubmit={submit}>
          {validationError || errorMessage ? (
            <Alert variant="destructive">
              <CircleAlertIcon />
              <AlertTitle>
                {conflicts.length > 0
                  ? "Reservation conflict"
                  : "Event not saved"}
              </AlertTitle>
              <AlertDescription>
                {validationError ?? errorMessage}
                {conflicts.length > 0 ? (
                  <div className="mt-3">
                    <MaintenanceConflictList conflicts={conflicts} />
                  </div>
                ) : null}
              </AlertDescription>
            </Alert>
          ) : null}

          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="maintenance-name">Event name</FieldLabel>
              <Input
                autoComplete="off"
                id="maintenance-name"
                onChange={(eventObject) =>
                  update("name", eventObject.target.value)
                }
                placeholder="Router firmware window"
                required
                value={form.name}
              />
            </Field>

            <Field>
              <FieldLabel htmlFor="maintenance-description">
                Description
              </FieldLabel>
              <Textarea
                id="maintenance-description"
                onChange={(eventObject) =>
                  update("description", eventObject.target.value)
                }
                placeholder="What changes during this window?"
                value={form.description}
              />
            </Field>

            <div className="grid gap-4 sm:grid-cols-2">
              <Field>
                <FieldLabel htmlFor="maintenance-timezone">Timezone</FieldLabel>
                <Input
                  id="maintenance-timezone"
                  onChange={(eventObject) =>
                    update("timezone", eventObject.target.value)
                  }
                  placeholder="Europe/London"
                  value={form.timezone}
                />
                <FieldDescription>
                  Used for recurrence and daylight-saving expansion.
                </FieldDescription>
              </Field>
              {!event ? (
                <Field>
                  <FieldLabel htmlFor="maintenance-state">
                    Initial state
                  </FieldLabel>
                  <Select
                    onValueChange={(value) => {
                      if (value === "draft" || value === "scheduled")
                        update("state", value);
                    }}
                    value={form.state}
                  >
                    <SelectTrigger id="maintenance-state">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="scheduled">Scheduled</SelectItem>
                      <SelectItem value="draft">Draft</SelectItem>
                    </SelectContent>
                  </Select>
                </Field>
              ) : null}
            </div>

            <div className="grid gap-4 sm:grid-cols-2">
              <Field>
                <FieldLabel htmlFor="maintenance-start">Start</FieldLabel>
                <LocalDateTimePicker
                  id="maintenance-start"
                  label="Start"
                  onChange={(value) => update("start", value)}
                  value={form.start}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor="maintenance-end">End</FieldLabel>
                <LocalDateTimePicker
                  id="maintenance-end"
                  label="End"
                  onChange={(value) => update("end", value)}
                  value={form.end}
                />
              </Field>
            </div>

            <section
              aria-labelledby="maintenance-recurrence-title"
              className="flex flex-col gap-3 rounded-lg border p-4"
            >
              <div>
                <h3
                  className="text-sm font-medium"
                  id="maintenance-recurrence-title"
                >
                  Recurrence
                </h3>
                <p className="mt-1 text-xs text-muted-foreground">
                  Build a common schedule, or enter an advanced RRULE. Expansion
                  horizon is 90 days.
                </p>
              </div>
              <div className="grid gap-4 sm:grid-cols-2">
                <Field>
                  <FieldLabel htmlFor="maintenance-recurrence-mode">
                    Recurrence input mode
                  </FieldLabel>
                  <Select
                    onValueChange={switchRecurrenceMode}
                    value={recurrenceMode}
                  >
                    <SelectTrigger id="maintenance-recurrence-mode">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="guided">Guided builder</SelectItem>
                      <SelectItem value="advanced">Advanced RRULE</SelectItem>
                    </SelectContent>
                  </Select>
                </Field>
              </div>
              {recurrenceMode === "guided" ? (
                <div className="flex flex-col gap-4">
                  <div className="grid gap-4 sm:grid-cols-2">
                    <Field>
                      <FieldLabel htmlFor="maintenance-recurrence-frequency">
                        Frequency
                      </FieldLabel>
                      <Select
                        onValueChange={(value) => {
                          if (value === "none") {
                            updateGuidedRecurrence({ frequency: "" });
                          } else if (
                            RECURRENCE_FREQUENCIES.includes(
                              value as RecurrenceFrequency,
                            )
                          ) {
                            updateGuidedRecurrence({
                              frequency: value as RecurrenceFrequency,
                            });
                          }
                        }}
                        value={guidedRecurrence.frequency || "none"}
                      >
                        <SelectTrigger
                          aria-label="Recurrence frequency"
                          id="maintenance-recurrence-frequency"
                        >
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          <SelectItem value="none">Does not repeat</SelectItem>
                          {RECURRENCE_FREQUENCIES.map((frequency) => (
                            <SelectItem key={frequency} value={frequency}>
                              {frequency[0] + frequency.slice(1).toLowerCase()}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </Field>
                    {guidedRecurrence.frequency ? (
                      <Field>
                        <FieldLabel htmlFor="maintenance-recurrence-interval">
                          Repeat every
                        </FieldLabel>
                        <div className="flex items-center gap-2">
                          <Input
                            aria-label="Repeat every"
                            id="maintenance-recurrence-interval"
                            min="1"
                            onChange={(eventObject) =>
                              updateGuidedRecurrence({
                                interval: eventObject.target.value,
                              })
                            }
                            type="number"
                            value={guidedRecurrence.interval}
                          />
                          <span className="text-sm text-muted-foreground">
                            {guidedRecurrence.frequency.toLowerCase()}
                          </span>
                        </div>
                      </Field>
                    ) : null}
                  </div>
                  {guidedRecurrence.frequency === "WEEKLY" ? (
                    <fieldset className="grid gap-2">
                      <legend className="text-sm font-medium">
                        Days of week
                      </legend>
                      <div className="flex flex-wrap gap-3">
                        {RECURRENCE_WEEKDAYS.map(([code, label]) => (
                          <label
                            className="flex items-center gap-2 text-sm"
                            key={code}
                          >
                            <Checkbox
                              aria-label={label}
                              checked={guidedRecurrence.weekdays.includes(code)}
                              onCheckedChange={(checked) =>
                                updateGuidedRecurrence({
                                  weekdays:
                                    checked === true
                                      ? [...guidedRecurrence.weekdays, code]
                                      : guidedRecurrence.weekdays.filter(
                                          (day) => day !== code,
                                        ),
                                })
                              }
                            />
                            {label}
                          </label>
                        ))}
                      </div>
                    </fieldset>
                  ) : null}
                  {guidedRecurrence.frequency ? (
                    <div className="grid gap-4 sm:grid-cols-2">
                      <Field>
                        <FieldLabel htmlFor="maintenance-recurrence-end">
                          Recurrence end
                        </FieldLabel>
                        <Select
                          onValueChange={(value) => {
                            if (value === "never" || value === "count") {
                              updateGuidedRecurrence({
                                end: value,
                              });
                            }
                          }}
                          value={guidedRecurrence.end}
                        >
                          <SelectTrigger
                            aria-label="Recurrence end"
                            id="maintenance-recurrence-end"
                          >
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value="never">Never ends</SelectItem>
                            <SelectItem value="count">
                              After a fixed number of occurrences
                            </SelectItem>
                          </SelectContent>
                        </Select>
                      </Field>
                      {guidedRecurrence.end === "count" ? (
                        <Field>
                          <FieldLabel htmlFor="maintenance-recurrence-count">
                            Occurrences
                          </FieldLabel>
                          <Input
                            aria-label="Occurrences"
                            id="maintenance-recurrence-count"
                            min="1"
                            onChange={(eventObject) =>
                              updateGuidedRecurrence({
                                count: eventObject.target.value,
                              })
                            }
                            type="number"
                            value={guidedRecurrence.count}
                          />
                        </Field>
                      ) : null}
                    </div>
                  ) : null}
                  <p className="text-xs text-muted-foreground">
                    Generated rule: {form.recurrence_rule || "One-time event"}
                  </p>
                </div>
              ) : (
                <Field>
                  <FieldLabel htmlFor="maintenance-recurrence">
                    Advanced recurrence rule
                  </FieldLabel>
                  <Input
                    id="maintenance-recurrence"
                    onChange={(eventObject) =>
                      update("recurrence_rule", eventObject.target.value)
                    }
                    placeholder="FREQ=WEEKLY;BYDAY=SA"
                    value={form.recurrence_rule}
                  />
                  <FieldDescription>
                    Optional RRULE content for advanced schedules.
                  </FieldDescription>
                </Field>
              )}
            </section>
            <div className="grid gap-4 sm:grid-cols-2">
              <div className="flex flex-col justify-end gap-3 rounded-lg border p-3">
                <div className="flex items-center justify-between gap-3">
                  <Label htmlFor="maintenance-disruptive">
                    Disruptive event
                  </Label>
                  <Switch
                    checked={form.disruptive}
                    id="maintenance-disruptive"
                    onCheckedChange={(checked) => update("disruptive", checked)}
                  />
                </div>
                <p className="text-xs text-muted-foreground">
                  Disruptive events use global one-at-a-time conflict policy
                  when enabled.
                </p>
              </div>
            </div>

            <div className="grid gap-4 sm:grid-cols-2">
              <Field>
                <FieldLabel htmlFor="maintenance-lead-in">
                  Lead-in reservation (minutes)
                </FieldLabel>
                <Input
                  id="maintenance-lead-in"
                  min="0"
                  onChange={(eventObject) =>
                    update("lead_in_minutes", eventObject.target.value)
                  }
                  type="number"
                  value={form.lead_in_minutes}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor="maintenance-cooldown">
                  Cooldown reservation (minutes)
                </FieldLabel>
                <Input
                  id="maintenance-cooldown"
                  min="0"
                  onChange={(eventObject) =>
                    update("cooldown_minutes", eventObject.target.value)
                  }
                  type="number"
                  value={form.cooldown_minutes}
                />
              </Field>
            </div>
          </FieldGroup>

          <section
            aria-labelledby="maintenance-resources-title"
            className="flex flex-col gap-3 rounded-lg border p-4"
          >
            <div>
              <h3
                className="text-sm font-medium"
                id="maintenance-resources-title"
              >
                Reserved resources
              </h3>
              <p className="mt-1 text-xs text-muted-foreground">
                Use a resource key such as network-core, or an entity kind and
                UUID.
              </p>
            </div>
            {form.resources.length === 0 ? (
              <p className="rounded-md border border-dashed p-3 text-sm text-muted-foreground">
                No resources reserved.
              </p>
            ) : (
              <div className="flex flex-col gap-3">
                {form.resources.map((resource, index) => (
                  <div
                    className="rounded-lg border bg-muted/20 p-3"
                    key={`${resource.role}-${index}`}
                  >
                    <div className="grid gap-3 sm:grid-cols-[10rem_9rem_minmax(0,1fr)_auto] sm:items-end">
                      <Field>
                        <FieldLabel
                          htmlFor={`maintenance-resource-role-${index}`}
                        >
                          Role
                        </FieldLabel>
                        <Select
                          onValueChange={(value) => {
                            if (
                              RESOURCE_ROLES.includes(
                                value as FormResource["role"],
                              )
                            ) {
                              setForm((current) => ({
                                ...current,
                                resources: current.resources.map(
                                  (item, itemIndex) =>
                                    itemIndex === index
                                      ? {
                                          ...item,
                                          role: value as FormResource["role"],
                                        }
                                      : item,
                                ),
                              }));
                            }
                          }}
                          value={resource.role}
                        >
                          <SelectTrigger
                            id={`maintenance-resource-role-${index}`}
                          >
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            {RESOURCE_ROLES.map((role) => (
                              <SelectItem key={role} value={role}>
                                {role}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </Field>
                      <Field>
                        <FieldLabel
                          htmlFor={`maintenance-resource-mode-${index}`}
                        >
                          Identifier
                        </FieldLabel>
                        <Select
                          onValueChange={(value) => {
                            if (value === "key" || value === "entity") {
                              setForm((current) => ({
                                ...current,
                                resources: current.resources.map(
                                  (item, itemIndex) =>
                                    itemIndex === index
                                      ? { ...item, mode: value }
                                      : item,
                                ),
                              }));
                            }
                          }}
                          value={resource.mode}
                        >
                          <SelectTrigger
                            id={`maintenance-resource-mode-${index}`}
                          >
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value="key">Resource key</SelectItem>
                            <SelectItem value="entity">Entity</SelectItem>
                          </SelectContent>
                        </Select>
                      </Field>
                      {resource.mode === "key" ? (
                        <Field>
                          <FieldLabel
                            htmlFor={`maintenance-resource-key-${index}`}
                          >
                            Resource key
                          </FieldLabel>
                          <Input
                            id={`maintenance-resource-key-${index}`}
                            onChange={(eventObject) =>
                              updateResource(setForm, index, {
                                key: eventObject.target.value,
                              })
                            }
                            placeholder="network-core"
                            value={resource.key}
                          />
                        </Field>
                      ) : (
                        <div className="grid gap-3 sm:grid-cols-2">
                          <Field>
                            <FieldLabel
                              htmlFor={`maintenance-resource-kind-${index}`}
                            >
                              Kind
                            </FieldLabel>
                            <Input
                              id={`maintenance-resource-kind-${index}`}
                              onChange={(eventObject) =>
                                updateResource(setForm, index, {
                                  kind: eventObject.target.value,
                                })
                              }
                              placeholder="devices"
                              value={resource.kind}
                            />
                          </Field>
                          <Field>
                            <FieldLabel
                              htmlFor={`maintenance-resource-id-${index}`}
                            >
                              UUID
                            </FieldLabel>
                            <Input
                              id={`maintenance-resource-id-${index}`}
                              onChange={(eventObject) =>
                                updateResource(setForm, index, {
                                  id: eventObject.target.value,
                                })
                              }
                              placeholder="00000000-0000-0000-0000-000000000000"
                              value={resource.id}
                            />
                          </Field>
                        </div>
                      )}
                      <Button
                        aria-label={`Remove resource ${index + 1}`}
                        onClick={() =>
                          setForm((current) => ({
                            ...current,
                            resources: current.resources.filter(
                              (_, itemIndex) => itemIndex !== index,
                            ),
                          }))
                        }
                        size="icon-sm"
                        type="button"
                        variant="ghost"
                      >
                        <Trash2Icon data-icon="inline-start" />
                      </Button>
                    </div>
                    <div className="mt-3 flex items-center gap-2 text-xs text-muted-foreground">
                      <Checkbox
                        checked={resource.expected_failure}
                        id={`maintenance-resource-expected-failure-${index}`}
                        onCheckedChange={(checked) =>
                          updateResource(setForm, index, {
                            expected_failure: checked === true,
                          })
                        }
                      />
                      <Label
                        htmlFor={`maintenance-resource-expected-failure-${index}`}
                      >
                        Expected monitor failure during reservation
                      </Label>
                    </div>
                  </div>
                ))}
              </div>
            )}
            <Button
              className="self-start"
              onClick={() =>
                setForm((current) => ({
                  ...current,
                  resources: [
                    ...current.resources,
                    {
                      role: "affected",
                      mode: "key",
                      key: "",
                      kind: "",
                      id: "",
                      expected_failure: true,
                    },
                  ],
                }))
              }
              size="sm"
              type="button"
              variant="outline"
            >
              <PlusIcon data-icon="inline-start" />
              Add resource
            </Button>
          </section>

          <FieldGroup>
            <div className="grid gap-4 sm:grid-cols-2">
              <Field>
                <FieldLabel htmlFor="maintenance-owner">Owner</FieldLabel>
                <Input
                  id="maintenance-owner"
                  onChange={(eventObject) =>
                    update("owner", eventObject.target.value)
                  }
                  value={form.owner}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor="maintenance-source">Source</FieldLabel>
                <Input
                  id="maintenance-source"
                  onChange={(eventObject) =>
                    update("source", eventObject.target.value)
                  }
                  placeholder="operator"
                  value={form.source}
                />
              </Field>
            </div>
            <Field>
              <FieldLabel htmlFor="maintenance-notes">Notes</FieldLabel>
              <Textarea
                id="maintenance-notes"
                onChange={(eventObject) =>
                  update("notes", eventObject.target.value)
                }
                value={form.notes}
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="maintenance-links">Links</FieldLabel>
              <Textarea
                id="maintenance-links"
                onChange={(eventObject) =>
                  update("links", eventObject.target.value)
                }
                placeholder="One URL per line"
                value={form.links}
              />
            </Field>
          </FieldGroup>

          <DialogFooter>
            <Button
              onClick={() => onOpenChange(false)}
              type="button"
              variant="outline"
            >
              Cancel
            </Button>
            <Button disabled={pending} type="submit">
              {pending ? "Saving…" : event ? "Save changes" : "Create event"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function updateResource(
  setForm: Dispatch<SetStateAction<FormState>>,
  index: number,
  update: Partial<FormResource>,
) {
  setForm((current) => ({
    ...current,
    resources: current.resources.map((resource, resourceIndex) =>
      resourceIndex === index ? { ...resource, ...update } : resource,
    ),
  }));
}
