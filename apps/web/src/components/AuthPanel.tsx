import { useState, type FormEvent } from "react";
import { ArrowRightIcon, KeyRoundIcon, ShieldCheckIcon } from "lucide-react";
import { login, setupAdmin } from "@/lib/api";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";

export function AuthPanel({
  onAuthenticated,
}: {
  onAuthenticated: () => void;
}) {
  const [mode, setMode] = useState<"login" | "setup">("login");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [isBusy, setIsBusy] = useState(false);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    setIsBusy(true);
    try {
      if (mode === "setup") await setupAdmin({ email, password });
      await login({ email, password });
      onAuthenticated();
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : "Authentication failed.",
      );
    } finally {
      setIsBusy(false);
    }
  }

  return (
    <main className="grid min-h-screen bg-background lg:grid-cols-2">
      <section className="flex items-center justify-center px-6 py-12 sm:px-10">
        <Card className="w-full max-w-md shadow-sm">
          <CardHeader>
            <div className="mb-4 flex size-10 items-center justify-center rounded-xl bg-primary text-primary-foreground">
              <KeyRoundIcon />
            </div>
            <CardTitle>
              {mode === "login" ? "Welcome back" : "Create operator account"}
            </CardTitle>
            <CardDescription>
              {mode === "login"
                ? "Sign in to manage devices and network records."
                : "Create the first admin account for this Hope server."}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form className="flex flex-col gap-5" onSubmit={submit}>
              <FieldGroup>
                <Field>
                  <FieldLabel htmlFor="email">Email</FieldLabel>
                  <Input
                    id="email"
                    autoComplete="email"
                    onChange={(event) => setEmail(event.target.value)}
                    required
                    type="email"
                    value={email}
                  />
                </Field>
                <Field>
                  <FieldLabel htmlFor="password">Password</FieldLabel>
                  <Input
                    id="password"
                    autoComplete={
                      mode === "setup" ? "new-password" : "current-password"
                    }
                    minLength={mode === "setup" ? 8 : undefined}
                    onChange={(event) => setPassword(event.target.value)}
                    required
                    type="password"
                    value={password}
                  />
                  {mode === "setup" ? (
                    <FieldDescription>
                      Use at least 8 characters.
                    </FieldDescription>
                  ) : null}
                </Field>
              </FieldGroup>
              {error ? (
                <Alert variant="destructive">
                  <ShieldCheckIcon />
                  <AlertTitle>Could not continue</AlertTitle>
                  <AlertDescription>{error}</AlertDescription>
                </Alert>
              ) : null}
              <Button className="w-full" disabled={isBusy} type="submit">
                {isBusy ? (
                  <Spinner data-icon="inline-start" />
                ) : (
                  <ArrowRightIcon data-icon="inline-start" />
                )}
                {isBusy
                  ? "Connecting…"
                  : mode === "login"
                    ? "Sign in"
                    : "Create account"}
              </Button>
            </form>
            <Button
              className="mt-4 w-full"
              onClick={() => {
                setMode(mode === "login" ? "setup" : "login");
                setError(null);
              }}
              variant="ghost"
            >
              {mode === "login"
                ? "First run? Create the admin account"
                : "Already set up? Sign in"}
            </Button>
          </CardContent>
        </Card>
      </section>
      <aside className="hidden bg-primary p-12 text-primary-foreground lg:flex lg:flex-col lg:justify-between">
        <div className="flex items-center gap-3 text-lg font-medium">
          <span className="grid size-8 place-items-center rounded-lg bg-primary-foreground text-primary">
            <ShieldCheckIcon />
          </span>
          Hope
        </div>
        <div className="max-w-md">
          <p className="text-sm text-primary-foreground/70">
            Private infrastructure console
          </p>
          <h1 className="mt-3 text-4xl font-semibold tracking-tight">
            Infrastructure inventory
          </h1>
          <p className="mt-4 text-lg leading-8 text-primary-foreground/75">
            Devices, interfaces, addresses, and identity evidence.
          </p>
        </div>
        <p className="text-sm text-primary-foreground/60">M1 / Inventory</p>
      </aside>
    </main>
  );
}
