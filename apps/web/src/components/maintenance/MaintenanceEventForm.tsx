import {
  useEffect,
  useState,
  type Dispatch,
  type FormEvent,
  type SetStateAction,
} from "react";
import { CircleAlertIcon, PlusIcon, Trash2Icon } from "lucide-react";
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

function pad(value: number) {
  return String(value).padStart(2, "0");
}

function localDateTime(value?: string | null) {
  const date = value ? new Date(value) : new Date(Date.now() + 60 * 60 * 1000);
  if (Number.isNaN(date.getTime())) return "";
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
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

  useEffect(() => {
    if (open) {
      setForm(formStateFor(event));
      setValidationError(null);
    }
  }, [event, open]);

  function update<K extends keyof FormState>(field: K, value: FormState[K]) {
    setForm((current) => ({ ...current, [field]: value }));
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
    if (
      !form.start ||
      !form.end ||
      new Date(form.end) <= new Date(form.start)
    ) {
      setValidationError("End time must be later than start time.");
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
      <DialogContent className="max-h-[min(90vh,60rem)] max-w-3xl overflow-y-auto">
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
                <Input
                  id="maintenance-start"
                  onChange={(eventObject) =>
                    update("start", eventObject.target.value)
                  }
                  required
                  type="datetime-local"
                  value={form.start}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor="maintenance-end">End</FieldLabel>
                <Input
                  id="maintenance-end"
                  onChange={(eventObject) =>
                    update("end", eventObject.target.value)
                  }
                  required
                  type="datetime-local"
                  value={form.end}
                />
              </Field>
            </div>

            <div className="grid gap-4 sm:grid-cols-2">
              <Field>
                <FieldLabel htmlFor="maintenance-recurrence">
                  Recurrence rule
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
                  Optional RRULE content. Expansion horizon is 90 days.
                </FieldDescription>
              </Field>
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
                        <Trash2Icon />
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
              <PlusIcon />
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
