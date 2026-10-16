"use client";
import { useState } from "react";
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
import { post } from "@/lib/api";
import type { Network as HopeNetwork } from "@/lib/protocol";

const SUGGESTIONS = ["192.168.1.0/24", "192.168.0.0/24", "10.0.0.0/24"];

export function NetworkDialog({
  open,
  onOpenChange,
  onAdded,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  onAdded: (network: HopeNetwork | null) => void;
}) {
  const [name, setName] = useState("");
  const [cidr, setCidr] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState<HopeNetwork | null>(null);
  function reset() {
    setCreated(null);
    setName("");
    setCidr("");
    setError("");
  }
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      const network =
        created ??
        (await post<HopeNetwork>("/networks", {
          name,
          cidr,
          enabled: true,
        }));
      setCreated(network);
      onAdded(null);
      try {
        await post(`/networks/${network.id}/scan`);
      } catch (e) {
        setError(
          `Network saved, but scanning could not start: ${(e as Error).message}`,
        );
        return;
      }
      onOpenChange(false);
      reset();
      onAdded(network);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Dialog
      open={open}
      onOpenChange={(value) => {
        onOpenChange(value);
        if (!value) reset();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add a network</DialogTitle>
          <DialogDescription>
            H.O.P.E scans the range now, then keeps watching it every five
            minutes for new and changed devices.
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={submit} className="flex flex-col gap-5">
          {error && (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="network-name">Network name</FieldLabel>
              <Input
                id="network-name"
                placeholder="Home network"
                required
                disabled={created != null}
                maxLength={120}
                value={name}
                onChange={(e) => setName(e.target.value)}
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="network-cidr">IPv4 CIDR</FieldLabel>
              <Input
                id="network-cidr"
                placeholder="192.168.1.0/24"
                required
                disabled={created != null}
                className="font-mono"
                value={cidr}
                onChange={(e) => setCidr(e.target.value)}
              />
              <FieldDescription>
                The server must have a route to this network. Use a /20 or
                smaller range.
              </FieldDescription>
              {!created && (
                <div className="flex flex-wrap gap-1.5 pt-1">
                  {SUGGESTIONS.map((suggestion) => (
                    <button
                      key={suggestion}
                      type="button"
                      className="chip font-mono"
                      aria-pressed={cidr === suggestion}
                      onClick={() => setCidr(suggestion)}
                    >
                      {suggestion}
                    </button>
                  ))}
                </div>
              )}
            </Field>
          </FieldGroup>
          <Button disabled={busy} type="submit">
            {busy
              ? "Starting discovery…"
              : created
                ? "Retry discovery"
                : "Add and discover"}
          </Button>
        </form>
      </DialogContent>
    </Dialog>
  );
}
