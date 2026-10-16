"use client";
import { useEffect, useState } from "react";
import { BellRing, Send } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import {
  Field,
  FieldGroup,
  FieldLabel,
  FieldDescription,
} from "@/components/ui/field";
import { age, api, post, put } from "@/lib/api";
import type { NotificationSettings, OkResponse } from "@/lib/protocol";

const EVENTS: [keyof NotificationSettings, string, string][] = [
  [
    "notify_new_devices",
    "New devices",
    "A device the server has never seen joins a monitored range.",
  ],
  [
    "notify_offline",
    "Offline",
    "An agent stops reporting, or a router, switch, access point, server, NAS or hypervisor stops answering.",
  ],
  [
    "notify_resources",
    "Problems",
    "Full disks or pools, degraded pools, hot sensors, failed services and failing monitors.",
  ],
  ["notify_recovery", "Recoveries", "A follow-up when a problem clears."],
];

export function NotificationsDialog({
  open,
  onOpenChange,
  notify,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  notify: (message: string, tone?: "good" | "warning") => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Notification settings</DialogTitle>
          <DialogDescription>
            Push alerts to your phone or desktop through{" "}
            <a
              href="https://ntfy.sh"
              target="_blank"
              rel="noreferrer"
              className="underline underline-offset-2"
            >
              ntfy
            </a>
            , hosted or self-hosted.
          </DialogDescription>
        </DialogHeader>
        {/* Remounted on every open so the form always starts from the saved settings. */}
        {open && <NotificationsForm notify={notify} />}
      </DialogContent>
    </Dialog>
  );
}

function NotificationsForm({
  notify,
}: {
  notify: (message: string, tone?: "good" | "warning") => void;
}) {
  const [settings, setSettings] = useState<NotificationSettings | null>(null);
  const [token, setToken] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let live = true;
    api<NotificationSettings>("/notifications")
      .then((value) => live && setSettings(value))
      .catch((e: Error) => live && setError(e.message));
    return () => {
      live = false;
    };
  }, []);
  if (!settings)
    return error ? (
      <Alert variant="destructive">
        <AlertDescription>{error}</AlertDescription>
      </Alert>
    ) : (
      <p className="text-sm text-muted-foreground">Loading…</p>
    );
  const change = (patch: Partial<NotificationSettings>) =>
    setSettings({ ...settings, ...patch });
  async function save(tokenValue: string | null = token || null) {
    if (!settings) return null;
    setBusy(true);
    setError("");
    try {
      const saved = await put<NotificationSettings>("/notifications", {
        ...settings,
        ntfy_token: tokenValue,
      });
      setSettings(saved);
      setToken("");
      return saved;
    } catch (e) {
      setError((e as Error).message);
      return null;
    } finally {
      setBusy(false);
    }
  }
  async function test() {
    if (!(await save())) return;
    setBusy(true);
    try {
      await post<OkResponse>("/notifications/test");
      notify("Test notification sent. Check your ntfy app.", "good");
      setSettings(await api<NotificationSettings>("/notifications"));
    } catch (e) {
      setError((e as Error).message);
      setSettings(await api<NotificationSettings>("/notifications"));
    } finally {
      setBusy(false);
    }
  }
  const subscribe =
    settings.ntfy_topic &&
    `${settings.ntfy_server.replace(/\/$/, "")}/${settings.ntfy_topic}`;
  return (
    <form
      className="flex flex-col gap-5"
      aria-label="Notification settings"
      onSubmit={async (event) => {
        event.preventDefault();
        if (await save()) notify("Notification settings saved.", "good");
      }}
    >
      {error && (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      <label className="flex items-center gap-3 rounded-xl border p-3">
        <input
          type="checkbox"
          className="size-4 accent-[var(--primary)]"
          checked={settings.ntfy_enabled}
          onChange={(e) => change({ ntfy_enabled: e.target.checked })}
        />
        <BellRing className="size-4 text-muted-foreground" />
        <span className="text-sm font-medium">Send notifications</span>
      </label>
      <FieldGroup>
        <Field>
          <FieldLabel htmlFor="ntfy-server">ntfy server</FieldLabel>
          <Input
            id="ntfy-server"
            inputMode="url"
            value={settings.ntfy_server}
            onChange={(e) => change({ ntfy_server: e.target.value })}
          />
        </Field>
        <Field>
          <FieldLabel htmlFor="ntfy-topic">Topic</FieldLabel>
          <Input
            id="ntfy-topic"
            placeholder="hope-alerts-7f3k9"
            maxLength={64}
            value={settings.ntfy_topic}
            onChange={(e) => change({ ntfy_topic: e.target.value })}
          />
          <FieldDescription>
            Subscribe to the same topic in the ntfy app
            {subscribe ? (
              <>
                {" "}
                (<span className="mono">{subscribe}</span>)
              </>
            ) : null}
            . On a public server anyone who knows the topic can read it, so pick
            one that is hard to guess or use an access token.
          </FieldDescription>
        </Field>
        <Field>
          <FieldLabel htmlFor="ntfy-token">Access token (optional)</FieldLabel>
          <div className="flex gap-2">
            <Input
              id="ntfy-token"
              type="password"
              autoComplete="off"
              placeholder={
                settings.ntfy_token_set ? "Stored; type to replace" : "tk_…"
              }
              value={token}
              onChange={(e) => setToken(e.target.value)}
            />
            {settings.ntfy_token_set && (
              <Button
                type="button"
                variant="outline"
                disabled={busy}
                onClick={() => void save("")}
              >
                Remove
              </Button>
            )}
          </div>
        </Field>
      </FieldGroup>
      <fieldset className="flex flex-col gap-2.5">
        <legend className="mb-1 text-sm font-semibold">Notify me about</legend>
        {EVENTS.map(([key, label, hint]) => (
          <label key={key} className="flex items-start gap-3 text-sm">
            <input
              type="checkbox"
              className="mt-0.5 size-4 accent-[var(--primary)]"
              checked={Boolean(settings[key])}
              onChange={(e) => change({ [key]: e.target.checked })}
            />
            <span>
              <span className="font-medium">{label}</span>
              <span className="block text-xs text-muted-foreground">
                {hint}
              </span>
            </span>
          </label>
        ))}
      </fieldset>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-xs text-muted-foreground" data-testid="ntfy-status">
          {settings.last_error
            ? `Last attempt failed: ${settings.last_error}`
            : settings.last_sent_at
              ? `Last notification sent ${age(settings.last_sent_at).toLowerCase()}`
              : "Nothing sent yet."}
        </p>
        <div className="flex gap-2">
          <Button
            type="button"
            variant="outline"
            disabled={busy || !settings.ntfy_topic}
            onClick={() => void test()}
          >
            <Send data-icon="inline-start" />
            Send test
          </Button>
          <Button type="submit" disabled={busy}>
            Save
          </Button>
        </div>
      </div>
    </form>
  );
}
