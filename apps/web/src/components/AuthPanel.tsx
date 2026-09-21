import { useState, type FormEvent } from "react";
import { ArrowRightIcon, KeyRoundIcon, ShieldCheckIcon } from "lucide-react";
import { getUserFacingError, login, setupAdmin } from "@/lib/api";
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
  setupRequired,
}: {
  onAuthenticated: () => void;
  setupRequired: boolean;
}) {
  const [mode, setMode] = useState<"login" | "setup">(
    setupRequired ? "setup" : "login",
  );
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
      setError(getUserFacingError(caught, "Authentication failed."));
    } finally {
      setIsBusy(false);
    }
  }

  return (
    <main className="grid min-h-screen place-items-center bg-background px-6 py-12 sm:px-10">
      <section className="w-full max-w-md">
        <Card className="w-full shadow-sm">
          <CardHeader>
            <div className="mb-4 flex size-10 items-center justify-center rounded-xl bg-primary text-primary-foreground">
              <KeyRoundIcon />
            </div>
            <CardTitle render={<h1 />}>
              {mode === "login" ? "Sign in" : "Create operator account"}
            </CardTitle>
            <CardDescription>
              {mode === "login"
                ? "Sign in to manage devices and network records."
                : "Create the first admin account for this Hope server."}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form
              aria-busy={isBusy}
              className="flex flex-col gap-5"
              onSubmit={submit}
            >
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
                <Alert aria-live="assertive" variant="destructive">
                  <ShieldCheckIcon aria-hidden="true" />
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
                ? "Create the admin account"
                : "Sign in to an existing account"}
            </Button>
          </CardContent>
        </Card>
      </section>
    </main>
  );
}
