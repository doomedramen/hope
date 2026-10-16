"use client";
import { useState } from "react";
import { Play, Plus, Radar, Square, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { Bootstrap, Network } from "@/lib/protocol";

/** Scheduled scan intervals the server accepts, in minutes. */
export const INTERVALS: [number, string][] = [
  [5, "Every 5 minutes"],
  [15, "Every 15 minutes"],
  [30, "Every 30 minutes"],
  [60, "Every hour"],
  [360, "Every 6 hours"],
  [1440, "Once a day"],
];
const intervalLabel = (minutes: number) =>
  INTERVALS.find(([m]) => m === minutes)?.[1] ?? `Every ${minutes} minutes`;

export function NetworksDialog({
  open,
  onOpenChange,
  data,
  onToggleNetwork,
  onScanNetwork,
  onDeleteNetwork,
  onAddNetwork,
  onSetInterval,
  busy,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  data: Bootstrap | null;
  onToggleNetwork: (network: Network) => void;
  onScanNetwork: (network: Network) => void;
  onDeleteNetwork: (network: Network) => Promise<void>;
  onAddNetwork: () => void;
  onSetInterval: (network: Network, minutes: number) => void;
  busy: boolean;
}) {
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const networks = data?.networks ?? [];
  return (
    <Dialog
      open={open}
      onOpenChange={(value) => {
        onOpenChange(value);
        if (!value) setConfirmDelete(null);
      }}
    >
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Networks</DialogTitle>
          <DialogDescription>
            Control continuous discovery and review what this server can
            observe.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-6">
          <section className="flex flex-col gap-3">
            <h3 className="text-sm font-semibold">Monitored ranges</h3>
            {networks.length === 0 && (
              <p className="text-sm text-muted-foreground">
                No ranges yet. Add one to start discovering devices.
              </p>
            )}
            {networks.map((n) => (
              <div
                key={n.id}
                className="flex flex-col gap-3 rounded-xl border p-3"
              >
                <div className="flex items-center justify-between gap-3">
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <span
                        className="status-dot"
                        data-tone={n.enabled ? "good" : "neutral"}
                      />
                      <strong className="truncate text-sm">{n.name}</strong>
                    </div>
                    <p className="mono mt-0.5 text-muted-foreground">
                      {n.cidr} ·{" "}
                      {n.enabled
                        ? intervalLabel(n.interval_minutes).toLowerCase()
                        : "paused"}
                    </p>
                  </div>
                  <div className="flex shrink-0 gap-1">
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={busy}
                      onClick={() => onScanNetwork(n)}
                      aria-label={`Scan ${n.name} now`}
                    >
                      <Radar data-icon="inline-start" />
                      Scan now
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={busy}
                      onClick={() => onToggleNetwork(n)}
                    >
                      {n.enabled ? (
                        <Square data-icon="inline-start" />
                      ) : (
                        <Play data-icon="inline-start" />
                      )}
                      {n.enabled ? "Pause" : "Resume"}
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      disabled={busy}
                      aria-label={`Remove ${n.name}`}
                      onClick={() =>
                        setConfirmDelete(confirmDelete === n.id ? null : n.id)
                      }
                    >
                      <Trash2 />
                    </Button>
                  </div>
                </div>
                <div className="flex items-center gap-2 text-xs text-muted-foreground">
                  <span className="shrink-0">Background scan</span>
                  <Select
                    value={String(n.interval_minutes)}
                    onValueChange={(value) =>
                      value && onSetInterval(n, Number(value))
                    }
                  >
                    <SelectTrigger
                      size="sm"
                      aria-label={`Scan interval for ${n.name}`}
                      className="min-w-0 flex-1"
                    >
                      <SelectValue>
                        {(value: string) => intervalLabel(Number(value))}
                      </SelectValue>
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {INTERVALS.map(([minutes, label]) => (
                          <SelectItem key={minutes} value={String(minutes)}>
                            {label}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </div>
                {confirmDelete === n.id && (
                  <Alert>
                    <AlertDescription>
                      Remove {n.name}? Discovered hosts stay in the inventory;
                      they are only no longer grouped under this range.
                      <div className="mt-3 flex gap-2">
                        <Button
                          variant="destructive"
                          size="sm"
                          disabled={busy}
                          onClick={async () => {
                            await onDeleteNetwork(n);
                            setConfirmDelete(null);
                          }}
                        >
                          Remove range
                        </Button>
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => setConfirmDelete(null)}
                        >
                          Keep
                        </Button>
                      </div>
                    </AlertDescription>
                  </Alert>
                )}
              </div>
            ))}
            <Button
              variant="outline"
              onClick={() => {
                onOpenChange(false);
                onAddNetwork();
              }}
            >
              <Plus data-icon="inline-start" />
              Add network
            </Button>
          </section>
          <section className="flex flex-col gap-3">
            <h3 className="text-sm font-semibold">This server</h3>
            <dl className="detail-list">
              <dt>Server URL</dt>
              <dd className="mono">{data?.server_url}</dd>
              <dt>Platform</dt>
              <dd>{data?.capabilities.platform}</dd>
              <dt>Discovery</dt>
              <dd>{data?.capabilities.discovery_mode}</dd>
              <dt>ICMP</dt>
              <dd>{data?.capabilities.icmp ? "Available" : "Unavailable"}</dd>
              <dt>ARP</dt>
              <dd>
                {data?.capabilities.arp
                  ? "Available on the local link"
                  : "Unavailable"}
              </dd>
              <dt>History</dt>
              <dd>
                Agents report every 30 seconds. Full-resolution samples last 24
                hours; five-minute averages are kept through day seven.
              </dd>
            </dl>
            {data?.capabilities.limitations.map((limitation) => (
              <p key={limitation} className="text-xs text-muted-foreground">
                {limitation}
              </p>
            ))}
          </section>
        </div>
      </DialogContent>
    </Dialog>
  );
}
