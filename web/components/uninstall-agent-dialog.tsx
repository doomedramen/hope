"use client";
import { useEffect, useState } from "react";
import { Check, Copy, ShieldOff } from "lucide-react";
import { api, post } from "@/lib/api";
import { hostDisplayName } from "@/lib/devices";
import type { AgentUninstallInstructions, Host } from "@/lib/protocol";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { SshInstall } from "@/components/install-dialog";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

export function UninstallAgentDialog({
  host,
  onOpenChange,
  onDisabled,
}: {
  host: Host;
  onOpenChange: (open: boolean) => void;
  onDisabled: () => void;
}) {
  const [instructions, setInstructions] =
    useState<AgentUninstallInstructions | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [revoked, setRevoked] = useState(host.agent_revoked);
  const [copied, setCopied] = useState(false);
  const agentId = host.agent_id;
  useEffect(() => {
    let alive = true;
    api<AgentUninstallInstructions>(`/agents/${agentId}/uninstall-instructions`)
      .then((result) => {
        if (alive) setInstructions(result);
      })
      .catch((failure) => {
        if (alive) setError(failure.message);
      });
    return () => {
      alive = false;
    };
  }, [agentId]);

  async function disable() {
    setBusy(true);
    setError("");
    try {
      await post(`/agents/${agentId}/revoke`);
      setRevoked(true);
      onDisabled();
    } catch (failure) {
      setError((failure as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Uninstall agent</DialogTitle>
          <DialogDescription>
            Stop reporting and remove the agent from {hostDisplayName(host)}.
            Device history stays in H.O.P.E.
          </DialogDescription>
        </DialogHeader>
        <Tabs defaultValue="ssh">
          <TabsList>
            <TabsTrigger value="ssh">Uninstall over SSH</TabsTrigger>
            <TabsTrigger value="manual">Uninstall manually</TabsTrigger>
          </TabsList>
          <TabsContent value="ssh" className="pt-4">
            <SshInstall host={host} uninstall />
          </TabsContent>
          <TabsContent value="manual" className="flex flex-col gap-5 pt-4">
            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
            <section className="flex flex-col gap-3">
              <h3 className="font-medium">1. Disable reporting</h3>
              <p className="text-sm text-muted-foreground">
                Revoke access immediately. Updated agents stop at their next
                server contact and stay disabled after reboot until re-enrolled.
                Older agents may keep retrying until you uninstall them.
              </p>
              {revoked || host.agent_revoked ? (
                <p className="text-sm">Access revoked. Reports are blocked.</p>
              ) : (
                <Button variant="destructive" disabled={busy} onClick={disable}>
                  <ShieldOff data-icon="inline-start" />
                  {busy ? "Disabling…" : "Disable agent"}
                </Button>
              )}
            </section>
            <section className="flex flex-col gap-3">
              <h3 className="font-medium">2. Remove the installation</h3>
              <p className="text-sm text-muted-foreground">
                Run this command on {hostDisplayName(host)} as the user who
                installed the agent. System services require root or
                passwordless sudo. It stops the service and removes the service
                definition, updater and binary. The enrollment identity is kept
                for reinstallation.
              </p>
              {instructions ? (
                <>
                  <div className="install-command">
                    <code>{instructions.command}</code>
                  </div>
                  <Button
                    variant="outline"
                    onClick={async () => {
                      try {
                        await navigator.clipboard.writeText(
                          instructions.command,
                        );
                        setCopied(true);
                      } catch {
                        setError(
                          "Clipboard access failed. Select and copy the command above.",
                        );
                      }
                    }}
                  >
                    {copied ? <Check /> : <Copy />}
                    {copied ? "Copied" : "Copy uninstall command"}
                  </Button>
                  {instructions.custom_ca_required && (
                    <Alert>
                      <AlertDescription>
                        The CA certificate must exist at the path in the command
                        on this device. Adjust that path if needed.
                      </AlertDescription>
                    </Alert>
                  )}
                </>
              ) : (
                !error && <p className="text-sm">Loading uninstall command…</p>
              )}
              <p className="text-xs text-muted-foreground">
                For a custom state directory, set HOPE_AGENT_DIR before running
                the command. Check the command output on the device to confirm
                removal; this dialog cannot confirm it remotely.
              </p>
            </section>
          </TabsContent>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}
