"use client";
import { useState } from "react";
import { Eye, EyeOff, Loader2, Network } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Field,
  FieldGroup,
  FieldLabel,
  FieldDescription,
  FieldError,
} from "@/components/ui/field";
import { post } from "@/lib/api";

export function Brand() {
  return (
    <div className="brand">
      <div className="brand-mark" aria-hidden="true">
        H
      </div>
      <span>
        H.O.P.E<span className="brand-subtitle">Home Network Monitor</span>
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
  const [visible, setVisible] = useState(false);
  const [busy, setBusy] = useState(false);
  // A refusal belongs to the field that caused it; anything else is shown above the form.
  const [error, setError] = useState<{
    field?: "token" | "password";
    message: string;
  } | null>(null);
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await post(setup ? "/auth/setup" : "/auth/login", {
        password,
        setup_token: token,
      });
      await onComplete();
    } catch (e) {
      const failure = e as Error & { status?: number };
      if (failure.status === 401)
        setError(
          setup
            ? {
                field: "token",
                message:
                  "This setup token does not match the one in the server log.",
              }
            : {
                field: "password",
                message: "That password is not correct.",
              },
        );
      else setError({ message: failure.message });
    } finally {
      setBusy(false);
    }
  }
  const tokenError = error?.field === "token" ? error.message : "";
  const passwordError = error?.field === "password" ? error.message : "";
  return (
    <main className="auth-shell">
      <div className="auth-backdrop" aria-hidden="true" />
      <div className="auth-column">
        <div className="auth-brand">
          <div className="brand-mark">
            <Network size={22} />
          </div>
          <span>H.O.P.E</span>
        </div>
        <section className="auth-card">
          <header>
            <h1>{setup ? "Set up H.O.P.E" : "Sign in"}</h1>
            <p>
              {setup
                ? "Create the administrator account to start discovering devices."
                : "Enter the administrator password."}
            </p>
          </header>
          <form onSubmit={submit} className="flex flex-col gap-6">
            {error && !error.field && (
              <Alert variant="destructive">
                <AlertDescription>{error.message}</AlertDescription>
              </Alert>
            )}
            <FieldGroup>
              {setup && (
                <Field data-invalid={!!tokenError || undefined}>
                  <FieldLabel htmlFor="setup-token">Setup token</FieldLabel>
                  <Input
                    id="setup-token"
                    type="password"
                    autoComplete="off"
                    required
                    value={token}
                    aria-invalid={!!tokenError || undefined}
                    aria-describedby="setup-token-help"
                    onChange={(e) => setToken(e.target.value)}
                  />
                  {tokenError ? (
                    <FieldError id="setup-token-help">{tokenError}</FieldError>
                  ) : (
                    <FieldDescription id="setup-token-help">
                      Printed in the server log at first start, after
                      &ldquo;setup token:&rdquo;.
                    </FieldDescription>
                  )}
                </Field>
              )}
              <Field data-invalid={!!passwordError || undefined}>
                <FieldLabel htmlFor="password">
                  Administrator password
                </FieldLabel>
                <div className="auth-password">
                  <Input
                    id="password"
                    type={visible ? "text" : "password"}
                    required
                    autoFocus={!setup}
                    minLength={setup ? 8 : undefined}
                    maxLength={256}
                    autoComplete={setup ? "new-password" : "current-password"}
                    value={password}
                    aria-invalid={!!passwordError || undefined}
                    aria-describedby={
                      passwordError || setup ? "password-help" : undefined
                    }
                    onChange={(e) => setPassword(e.target.value)}
                  />
                  <button
                    type="button"
                    className="auth-reveal"
                    aria-label={visible ? "Hide password" : "Show password"}
                    aria-pressed={visible}
                    onClick={() => setVisible((v) => !v)}
                  >
                    {visible ? <EyeOff size={16} /> : <Eye size={16} />}
                  </button>
                </div>
                {passwordError ? (
                  <FieldError id="password-help">{passwordError}</FieldError>
                ) : (
                  setup && (
                    <FieldDescription id="password-help">
                      Use at least 8 characters. There is one account; keep it
                      safe.
                    </FieldDescription>
                  )
                )}
              </Field>
            </FieldGroup>
            <Button
              type="submit"
              size="lg"
              className="auth-submit"
              disabled={busy}
            >
              {busy && <Loader2 className="animate-spin" aria-hidden="true" />}
              {busy ? "Connecting…" : setup ? "Create account" : "Sign in"}
            </Button>
          </form>
        </section>
      </div>
    </main>
  );
}
