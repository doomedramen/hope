"use client";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { Bootstrap } from "@/lib/protocol";

/** Agent report intervals the server accepts, in seconds. */
const REPORT_INTERVALS: [number, string][] = [
  [5, "Every 5 seconds"],
  [10, "Every 10 seconds"],
  [15, "Every 15 seconds"],
  [30, "Every 30 seconds"],
  [60, "Every minute"],
];
export const reportIntervalLabel = (seconds: number) =>
  REPORT_INTERVALS.find(([s]) => s === seconds)?.[1] ??
  `Every ${seconds} seconds`;
export function SettingsDialog({
  open,
  onOpenChange,
  data,
  onSetReportInterval,
  busy,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  data: Bootstrap | null;
  onSetReportInterval: (seconds: number) => void;
  busy: boolean;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>
            Server capabilities and agent reporting frequency.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-6">
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
                One sample every 30 seconds is kept for 24 hours; five-minute
                averages are kept through day seven.
              </dd>
            </dl>
            {data && (
              <div className="flex flex-col gap-1.5">
                <div className="flex items-center gap-2 text-xs text-muted-foreground">
                  <span className="shrink-0">Agents report</span>
                  <Select
                    value={String(data.report_interval_seconds)}
                    onValueChange={(value) =>
                      value && onSetReportInterval(Number(value))
                    }
                  >
                    <SelectTrigger
                      size="sm"
                      aria-label="Agent report interval"
                      className="min-w-0 flex-1"
                      disabled={busy}
                    >
                      <SelectValue>
                        {(value: string) => reportIntervalLabel(Number(value))}
                      </SelectValue>
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {REPORT_INTERVALS.map(([seconds, label]) => (
                          <SelectItem key={seconds} value={String(seconds)}>
                            {label}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </div>
                <p className="text-xs text-muted-foreground">
                  Live figures on this page update as each report arrives.
                  Shorter intervals feel more immediate but cost agents a little
                  more work; stored history is unaffected.
                </p>
              </div>
            )}
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
