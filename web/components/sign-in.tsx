"use client";
import { useState } from "react";
import {
  Activity,
  HardDrive,
  Network,
  Radar,
  ShieldCheck,
  Waypoints,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Field,
  FieldGroup,
  FieldLabel,
  FieldDescription,
} from "@/components/ui/field";
import { post } from "@/lib/api";

export function Brand() {
  return (
    <div className="brand">
      <div className="brand-mark">
        <Network size={20} />
      </div>
      <span>
        H.O.P.E<span className="brand-subtitle">Network observatory</span>
      </span>
    </div>
  );
}

export function SignIn({
  setup,
  onComplete,
}: {
  setup: boolean;
  onComplete: () => Promise<void>;
}) {
  const [password, setPassword] = useState("");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      await post(setup ? "/auth/setup" : "/auth/login", {
        password,
        setup_token: token,
      });
      await onComplete();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <main className="auth-shell">
      <section className="auth-art" aria-hidden="true">
        <div className="auth-grid" />
        <Brand />
        <div>
          <h2>Every device on your network, understood.</h2>
          <p>
            H.O.P.E discovers what is on your LAN, maps how it connects, and
            watches CPU, memory, storage and traffic on the machines you care
            about. Self-hosted, on one small server.
          </p>
          <div className="auth-features">
            <div>
              <Radar size={16} />
              <span>Fair, bounded discovery across every range you add</span>
            </div>
            <div>
              <Waypoints size={16} />
              <span>Topology, subnet and health maps with provenance</span>
            </div>
            <div>
              <Activity size={16} />
              <span>A 30-second agent with seven days of history</span>
            </div>
            <div>
              <HardDrive size={16} />
              <span>Storage, GPU and interface counters per host</span>
            </div>
          </div>
        </div>
        <div className="flex items-center gap-2 text-xs opacity-70">
          <ShieldCheck size={14} />
          Your devices. Your data. Your server.
        </div>
      </section>
      <section className="auth-form">
        <div className="auth-panel">
          <div>
            <h1>{setup ? "Meet your network." : "Welcome back."}</h1>
            <p className="mt-2 text-sm text-muted-foreground">
              {setup
                ? "Create the administrator account to start discovering devices."
                : "Sign in to your H.O.P.E observatory."}
            </p>
          </div>
          <form onSubmit={submit} className="flex flex-col gap-6">
            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
            <FieldGroup>
              {setup && (
                <Field>
                  <FieldLabel htmlFor="setup-token">Setup token</FieldLabel>
                  <Input
                    id="setup-token"
                    type="password"
                    autoComplete="off"
                    required
                    value={token}
                    onChange={(e) => setToken(e.target.value)}
                  />
                  <FieldDescription>
                    Printed in the server log at first start, after &ldquo;setup
                    token:&rdquo;.
                  </FieldDescription>
                </Field>
              )}
              <Field>
                <FieldLabel htmlFor="password">
                  Administrator password
                </FieldLabel>
                <Input
                  id="password"
                  type="password"
                  required
                  minLength={setup ? 8 : undefined}
                  maxLength={256}
                  autoComplete={setup ? "new-password" : "current-password"}
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                />
                {setup && (
                  <FieldDescription>
                    Use at least 8 characters. There is one account; keep it
                    safe.
                  </FieldDescription>
                )}
              </Field>
            </FieldGroup>
            <Button type="submit" size="lg" disabled={busy}>
              {busy ? "Connecting…" : setup ? "Create account" : "Sign in"}
            </Button>
          </form>
        </div>
      </section>
    </main>
  );
}
