"use client";
import { useId, useState } from "react";
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
  const helpId = useId();
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
      <label className="monitor-option">
        <input
          type="checkbox"
          disabled={busy}
          checked={enabled}
          onChange={(event) => setEnabled(event.target.checked)}
        />
        Enable uptime monitoring
      </label>
      <fieldset
        disabled={!enabled || busy}
        className="flex flex-col gap-3 disabled:opacity-50"
      >
        <label className="monitor-option">
          <input
            type="checkbox"
            checked={device}
            onChange={(event) => setDevice(event.target.checked)}
          />
          Monitor device downtime
        </label>
        <label className="flex flex-col gap-1.5 text-sm">
          TCP ports to monitor
          <Input
            aria-describedby={helpId}
            value={ports}
            onChange={(event) => setPorts(event.target.value)}
            placeholder="22, 443, 8080"
          />
        </label>
        <p id={helpId} className="monitor-help">
          Separate ports with commas. Leave empty for device downtime only.
        </p>
      </fieldset>
      <p className="monitor-help">
        Alerts use your ntfy connector. Enable Notifications in the top bar to
        receive them.
      </p>
      <details className="monitor-help">
        <summary>How checks and alerts work</summary>
        <p>
          Checks run about once a minute. Two failures open an alert; the
          connector’s recovery setting controls follow-ups.
        </p>
        <p className="mt-2">
          Device downtime uses agent reports (silent for 2 minutes) or completed
          discovery scans (unseen for at least 15 minutes and two scan
          intervals). Devices without an active agent need an enabled discovery
          range.
        </p>
        <p className="mt-2">
          TCP checks connect from the H.O.P.E server to any known device IP. An
          open connection confirms the port is reachable, not that its
          application is healthy.
        </p>
      </details>
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
