"use client";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { put } from "@/lib/api";
import type { Host } from "@/lib/protocol";
import type { Tone } from "@/lib/fleet";

export function UptimeSettings({
  host,
  refresh,
  notify,
}: {
  host: Host;
  refresh: () => void;
  notify: (message: string, tone?: Tone) => void;
}) {
  const [enabled, setEnabled] = useState(host.uptime?.enabled ?? false);
  const [device, setDevice] = useState(host.uptime?.device ?? false);
  const [ports, setPorts] = useState(host.uptime?.ports.join(", ") ?? "");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  async function save() {
    setError("");
    const values = ports.trim() ? ports.trim().split(/[\s,]+/) : [];
    if (
      values.some(
        (value) =>
          !/^\d+$/.test(value) || Number(value) < 1 || Number(value) > 65535,
      ) ||
      values.length > 32
    ) {
      setError(
        "Enter up to 32 TCP ports between 1 and 65535, separated by commas.",
      );
      return;
    }
    setBusy(true);
    try {
      await put(`/hosts/${host.id}/uptime`, {
        enabled,
        device,
        ports: values.map(Number),
      });
      notify("Uptime monitoring saved.", "good");
      refresh();
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="flex flex-col gap-3" aria-label="Uptime monitoring">
      <h3 className="detail-heading">Uptime monitoring</h3>
      <label className="flex items-center gap-2 text-sm">
        <input
          type="checkbox"
          checked={enabled}
          onChange={(event) => setEnabled(event.target.checked)}
        />
        Enable uptime monitoring
      </label>
      <fieldset
        disabled={!enabled || busy}
        className="flex flex-col gap-3 disabled:opacity-50"
      >
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={device}
            onChange={(event) => setDevice(event.target.checked)}
          />
          Monitor device downtime
        </label>
        <p className="text-xs text-muted-foreground">
          Uses agent reports (silent for 2 minutes) or completed discovery scans
          (unseen for at least 15 minutes and two scan intervals). Devices
          without an active agent need an enabled discovery range.
        </p>
        <label className="flex flex-col gap-1.5 text-sm">
          TCP ports to monitor
          <Input
            value={ports}
            onChange={(event) => setPorts(event.target.value)}
            placeholder="22, 443, 8080"
          />
        </label>
        <p className="text-xs text-muted-foreground">
          Checks connections from the H.O.P.E server to any known IP address.
          Leave ports empty to monitor only device downtime.
        </p>
      </fieldset>
      <p className="text-xs text-muted-foreground">
        Checks run about once a minute. Two failed checks trigger an alert.
        Enable the ntfy connector in Notifications to receive alerts; its
        recovery setting controls follow-up messages.
      </p>
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
      <Button
        size="sm"
        className="self-start"
        disabled={busy}
        onClick={() => void save()}
      >
        {busy ? "Saving…" : "Save uptime monitoring"}
      </Button>
    </section>
  );
}
