"use client";
import { useState } from "react";
import { KeyRound } from "lucide-react";
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
import type { OkResponse, PasswordChange } from "@/lib/protocol";

/** Why the new password cannot be submitted yet, or null when it can. */
function problem(current: string, next: string, confirm: string) {
  if (!current || !next) return null;
  if (next.length < 8) return "Use at least 8 characters.";
  if (next === current)
    return "Choose a new password that differs from the current one.";
  if (confirm && confirm !== next) return "The new passwords do not match.";
  return null;
}

export function ChangePassword({ onChanged }: { onChanged: () => void }) {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const hint = problem(current, next, confirm);
  const ready = !!current && !!next && confirm === next && !hint && !busy;

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!ready) return;
    setBusy(true);
    setError("");
    try {
      await post<OkResponse>("/account/password", {
        current_password: current,
        new_password: next,
      } satisfies PasswordChange);
      setCurrent("");
      setNext("");
      setConfirm("");
      onChanged();
    } catch (e) {
      setError(
        e instanceof Error ? e.message : "The password was not changed.",
      );
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      onSubmit={submit}
      className="flex flex-col gap-4"
      aria-label="Change password"
    >
      {error && (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      <FieldGroup>
        <Field>
          <FieldLabel htmlFor="current-password">Current password</FieldLabel>
          <Input
            id="current-password"
            type="password"
            autoComplete="current-password"
            maxLength={256}
            value={current}
            onChange={(e) => setCurrent(e.target.value)}
          />
        </Field>
        <Field>
          <FieldLabel htmlFor="new-password">New password</FieldLabel>
          <Input
            id="new-password"
            type="password"
            autoComplete="new-password"
            maxLength={256}
            value={next}
            onChange={(e) => setNext(e.target.value)}
            aria-describedby="new-password-hint"
          />
          <FieldDescription id="new-password-hint">
            {hint ?? "At least 8 characters."}
          </FieldDescription>
        </Field>
        <Field>
          <FieldLabel htmlFor="confirm-password">
            Confirm new password
          </FieldLabel>
          <Input
            id="confirm-password"
            type="password"
            autoComplete="new-password"
            maxLength={256}
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
          />
        </Field>
      </FieldGroup>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-xs text-muted-foreground">
          Every other signed-in browser is signed out. This one stays signed in.
        </p>
        <Button type="submit" variant="outline" disabled={!ready}>
          <KeyRound data-icon="inline-start" />
          {busy ? "Changing…" : "Change password"}
        </Button>
      </div>
    </form>
  );
}
