"use client";
import { Age } from "@/components/age";
import { Square } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import type { Network, ScanJob } from "@/lib/protocol";

const STATUS: Record<ScanJob["status"], string> = {
  queued: "Queued",
  running: "Scanning",
  completed: "Discovery complete",
  cancelled: "Cancelled",
  failed: "Failed",
  interrupted: "Interrupted",
};

export function ScanProgress({
  scans,
  networks,
  onCancel,
}: {
  scans: ScanJob[];
  networks: Network[];
  onCancel: (scan: ScanJob) => void;
}) {
  if (!scans.length) return null;
  return (
    <section className="scan-strip" aria-label="Scan progress">
      {scans.map((scan) => (
        <div key={scan.id} className="scan-item">
          <div className="flex items-center justify-between gap-3">
            <span className="truncate text-sm font-semibold">
              {networks.find((n) => n.id === scan.network_id)?.name ??
                scan.cidr}
            </span>
            <Badge
              variant={
                scan.status === "running"
                  ? "default"
                  : scan.status === "completed"
                    ? "secondary"
                    : "outline"
              }
            >
              {STATUS[scan.status]}
            </Badge>
          </div>
          <Progress
            value={scan.total ? (scan.scanned / scan.total) * 100 : 0}
            aria-label={`${scan.cidr} scan progress`}
          />
          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span className="tabular">
              {scan.scanned} / {scan.total} addresses · {scan.discovered}{" "}
              responsive
            </span>
            {scan.status === "running" || scan.status === "queued" ? (
              <Button
                variant="ghost"
                size="xs"
                onClick={() => onCancel(scan)}
                aria-label={`Cancel ${scan.cidr} scan`}
              >
                <Square data-icon="inline-start" />
                Cancel
              </Button>
            ) : (
              <span>
                <Age value={scan.finished_at} />
              </span>
            )}
          </div>
          {scan.error && (
            <p className="text-xs text-destructive">{scan.error}</p>
          )}
        </div>
      ))}
    </section>
  );
}
