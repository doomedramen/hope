"use client";
import { Popover } from "@base-ui/react/popover";
import { ChevronDown, Radar, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ScanProgress } from "@/components/scan-progress";
import type { Network, ScanJob } from "@/lib/protocol";

export function DiscoveryMenu({
  networks,
  scans,
  busy,
  onScan,
  onCancel,
  onAddNetwork,
}: {
  networks: Network[];
  scans: ScanJob[];
  busy: boolean;
  onScan: (network?: Network) => void;
  onCancel: (scan: ScanJob) => void;
  onAddNetwork: () => void;
}) {
  const active = scans.filter(
    (s) => s.status === "running" || s.status === "queued",
  );
  const failed = scans.filter(
    (s) => s.status === "failed" || s.status === "interrupted",
  );
  const isScanning = (id: string) => active.some((s) => s.network_id === id);
  const available = networks.filter((n) => n.enabled && !isScanning(n.id));
  const label = active.length
    ? `Scanning ${active.length} ${active.length === 1 ? "network" : "networks"}`
    : "Discovery";
  return (
    <Popover.Root>
      <Popover.Trigger
        render={<Button variant="outline" className="discovery-trigger" />}
        aria-label={`${label}${failed.length ? `, ${failed.length} failed` : ""}`}
      >
        <Radar aria-hidden="true" />
        <span>{label}</span>
        {failed.length > 0 && (
          <span className="tone-critical">{failed.length} failed</span>
        )}
        <ChevronDown aria-hidden="true" />
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Positioner sideOffset={8} align="end" className="z-50">
          <Popover.Popup className="discovery-popup rounded-xl border bg-popover p-4 text-popover-foreground shadow-lg outline-none">
            <div className="flex items-center justify-between gap-3">
              <Popover.Title className="font-semibold">Discovery</Popover.Title>
              <Popover.Close
                render={<Button variant="ghost" size="icon-sm" />}
                aria-label="Close discovery"
              >
                <X />
              </Popover.Close>
            </div>
            <Popover.Description className="mb-4 text-sm text-muted-foreground">
              Scan networks and review recent results.
            </Popover.Description>
            <Button
              className="w-full"
              disabled={busy || !available.length}
              onClick={() => onScan()}
            >
              Scan all enabled networks
            </Button>
            {!networks.length && (
              <Button
                variant="outline"
                className="mt-2 w-full"
                onClick={onAddNetwork}
              >
                Add a network
              </Button>
            )}
            {networks.length > 0 && (
              <div
                className="mt-4 flex flex-col gap-2"
                aria-label="Scan a network"
              >
                {networks.map((n) => (
                  <div
                    key={n.id}
                    className="flex items-center justify-between gap-3"
                  >
                    <div className="min-w-0">
                      <p className="truncate text-sm font-medium">{n.name}</p>
                      <p className="text-xs text-muted-foreground">
                        {n.cidr}
                        {!n.enabled && " · Scheduled scans paused"}
                      </p>
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      aria-label={`Scan ${n.name} now`}
                      disabled={busy || isScanning(n.id)}
                      onClick={() => onScan(n)}
                    >
                      {isScanning(n.id) ? "Scanning" : "Scan now"}
                    </Button>
                  </div>
                ))}
              </div>
            )}
            {scans.length > 0 ? (
              <div className="mt-4">
                <h3 className="mb-2 text-sm font-semibold">Latest scans</h3>
                <ScanProgress
                  scans={[
                    ...active,
                    ...scans.filter((s) => !active.includes(s)),
                  ]}
                  networks={networks}
                  onCancel={onCancel}
                />
              </div>
            ) : (
              <p className="mt-4 text-sm text-muted-foreground">
                No scans yet.
              </p>
            )}
          </Popover.Popup>
        </Popover.Positioner>
      </Popover.Portal>
    </Popover.Root>
  );
}
