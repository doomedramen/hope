"use client";
import { useEffect, useState } from "react";
import { Copy, Check, KeyRound } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import {
  Field,
  FieldGroup,
  FieldLabel,
  FieldDescription,
} from "@/components/ui/field";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectGroup,
  SelectItem,
} from "@/components/ui/select";
import type {
  EnrollmentTokenResponse as Enrollment,
  EnrollmentTokenRequest,
  ReleaseAvailability,
  FingerprintResponse,
  Host,
  InstallJob,
} from "@/lib/protocol";
import { api, post } from "@/lib/api";

export function InstallDialog({
  open,
  onOpenChange,
  host,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  host: Host | null;
}) {
  const [enrollment, setEnrollment] = useState<Enrollment | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const [releases, setReleases] = useState<ReleaseAvailability | null>(null);
  useEffect(() => {
    if (!open) return;
    let alive = true;
    api<ReleaseAvailability>("/releases")
      .then((result) => {
        if (alive) setReleases(result);
      })
      .catch((failure) => {
        if (alive) setError(failure.message);
      });
    return () => {
      alive = false;
    };
  }, [open]);
  async function generate() {
    setBusy(true);
    setError("");
    try {
      if (!releases?.packaged_platforms.length)
        throw new Error(
          "No signed agent packages are available on this server.",
        );
      setEnrollment(
        await post<Enrollment>("/enrollment-tokens", {
          reset_agent_id: host?.agent_revoked ? host.agent_id : null,
        } satisfies EnrollmentTokenRequest),
      );
      setCopied(false);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Dialog
      open={open}
      onOpenChange={(v) => {
        onOpenChange(v);
        if (!v) {
          setEnrollment(null);
          setError("");
        }
      }}
    >
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Add a reporting agent</DialogTitle>
          <DialogDescription>
            Hardware details and live metrics, sent directly to your H.O.P.E
            server.
          </DialogDescription>
        </DialogHeader>
        <Tabs defaultValue="manual">
          <TabsList>
            <TabsTrigger value="manual">Install manually</TabsTrigger>
            <TabsTrigger
              value="ssh"
              disabled={!host || !releases?.packaged_platforms.length}
            >
              Install over SSH
            </TabsTrigger>
          </TabsList>
          <TabsContent value="manual" className="flex flex-col gap-5 pt-4">
            <p className="text-sm text-muted-foreground">
              Run the generated command on a Linux or macOS host. The installer
              detects architecture, permissions, and the service manager.
            </p>
            {releases && (
              <Alert>
                <AlertDescription>
                  {releases.packaged_platforms.length
                    ? `Packaged targets: ${releases.packaged_platforms.join(", ")}. The installer verifies the signature before execution.`
                    : "No signed agent packages are available. The server administrator must package releases before agents can be installed."}
                </AlertDescription>
              </Alert>
            )}
            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
            {!enrollment ? (
              <Button
                onClick={generate}
                disabled={busy || !releases?.packaged_platforms.length}
              >
                <KeyRound data-icon="inline-start" />
                {busy ? "Generating…" : "Generate installation command"}
              </Button>
            ) : (
              <>
                <div className="install-command">
                  <code>{enrollment.command}</code>
                </div>
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <p className="text-xs text-muted-foreground">
                    One use. Expires at{" "}
                    {new Date(
                      enrollment.expires_at * 1000,
                    ).toLocaleTimeString()}
                    .
                  </p>
                  <Button
                    variant="outline"
                    onClick={async () => {
                      try {
                        await navigator.clipboard.writeText(enrollment.command);
                        setCopied(true);
                      } catch {
                        setError(
                          "Clipboard access failed. Select and copy the command above.",
                        );
                      }
                    }}
                  >
                    {copied ? (
                      <Check data-icon="inline-start" />
                    ) : (
                      <Copy data-icon="inline-start" />
                    )}
                    {copied ? "Copied" : "Copy command"}
                  </Button>
                </div>
                {enrollment.custom_ca_required && (
                  <Alert>
                    <AlertDescription>
                      Replace the CA certificate path with a trusted certificate
                      already present on the target host.
                    </AlertDescription>
                  </Alert>
                )}
                <Button variant="ghost" onClick={generate} disabled={busy}>
                  Generate another command
                </Button>
              </>
            )}
            <p className="text-xs text-muted-foreground">
              Without root or usable sudo, H.O.P.E installs a user service when
              supported. Reporting may depend on your login session.
            </p>
          </TabsContent>
          <TabsContent value="ssh" className="pt-4">
            {open && host && <SshInstall key={host.id} host={host} />}
          </TabsContent>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}

function SshInstall({ host }: { host: Host }) {
  const [address, setAddress] = useState(
    host.addresses.find((a) => !a.includes(":")) ?? host.addresses[0] ?? "",
  );
  const [port, setPort] = useState("22");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [key, setKey] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [fingerprint, setFingerprint] = useState("");
  const [verified, setVerified] = useState(false);
  const [job, setJob] = useState<InstallJob | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const target = { host_id: host.id, address, port: Number(port) };
  useEffect(() => {
    if (!job || job.status !== "running") return;
    const timer = setInterval(() => {
      api<InstallJob[]>("/ssh/jobs")
        .then((jobs) => {
          const current = jobs.find((j) => j.id === job.id);
          if (current) setJob(current);
        })
        .catch((e) => setError(e.message));
    }, 2000);
    return () => clearInterval(timer);
  }, [job]);
  async function inspect() {
    setBusy(true);
    setError("");
    setVerified(false);
    try {
      const result = await post<FingerprintResponse>(
        "/ssh/fingerprint",
        target,
      );
      setFingerprint(result.fingerprint);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function install() {
    setBusy(true);
    setError("");
    try {
      setJob(
        await post<InstallJob>("/ssh/install", {
          ...target,
          username,
          fingerprint,
          password: password || null,
          private_key: key || null,
          passphrase: passphrase || null,
        }),
      );
      setPassword("");
      setKey("");
      setPassphrase("");
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="flex flex-col gap-4">
      {error && (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      {job ? (
        <div className="flex flex-col gap-3">
          <h3 className="font-medium">Installation {job.status}</h3>
          <ol className="flex list-decimal flex-col gap-2 pl-5 text-sm">
            {job.messages.map((m, i) => (
              <li key={i}>{m}</li>
            ))}
          </ol>
          {job.status === "failed" && (
            <Button variant="outline" onClick={() => setJob(null)}>
              Retry installation
            </Button>
          )}
        </div>
      ) : (
        <>
          <FieldGroup>
            <Field>
              <FieldLabel>Host address</FieldLabel>
              <Select
                value={address}
                onValueChange={(v) => {
                  if (v) {
                    setAddress(v);
                    setFingerprint("");
                    setVerified(false);
                  }
                }}
              >
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup>
                    {host.addresses.map((a) => (
                      <SelectItem key={a} value={a}>
                        {a}
                      </SelectItem>
                    ))}
                  </SelectGroup>
                </SelectContent>
              </Select>
            </Field>
            <Field>
              <FieldLabel htmlFor="ssh-port">SSH port</FieldLabel>
              <Input
                id="ssh-port"
                type="number"
                min={1}
                max={65535}
                value={port}
                onChange={(e) => {
                  setPort(e.target.value);
                  setFingerprint("");
                  setVerified(false);
                }}
              />
            </Field>
          </FieldGroup>
          <Button
            variant="outline"
            disabled={busy || !address}
            onClick={inspect}
          >
            {busy ? "Connecting…" : "Inspect host fingerprint"}
          </Button>
          {fingerprint && (
            <Alert>
              <AlertDescription>
                <code className="break-all">{fingerprint}</code>
                <p className="mt-2">
                  Compare this with the host’s SSH fingerprint through a trusted
                  channel.
                </p>
                <Button
                  className="mt-3"
                  size="sm"
                  variant={verified ? "secondary" : "outline"}
                  onClick={() => setVerified((v) => !v)}
                >
                  {verified
                    ? "Fingerprint verified"
                    : "I verified this fingerprint"}
                </Button>
              </AlertDescription>
            </Alert>
          )}
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="ssh-user">Username</FieldLabel>
              <Input
                id="ssh-user"
                autoComplete="username"
                value={username}
                onChange={(e) => setUsername(e.target.value)}
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="ssh-password">Password</FieldLabel>
              <Input
                id="ssh-password"
                type="password"
                autoComplete="off"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="ssh-key">Or private key</FieldLabel>
              <Textarea
                id="ssh-key"
                rows={3}
                autoComplete="off"
                value={key}
                onChange={(e) => setKey(e.target.value)}
              />
              <FieldDescription>
                Credentials are used only for this installation job.
              </FieldDescription>
            </Field>
            {key && (
              <Field>
                <FieldLabel htmlFor="ssh-passphrase">Key passphrase</FieldLabel>
                <Input
                  id="ssh-passphrase"
                  type="password"
                  autoComplete="off"
                  value={passphrase}
                  onChange={(e) => setPassphrase(e.target.value)}
                />
              </Field>
            )}
          </FieldGroup>
          <p className="text-xs text-muted-foreground">
            System installation requires root or passwordless sudo. Otherwise
            the installer attempts a user service. Custom CA deployments require
            the certificate at the configured path on the remote host.
          </p>
          <Button
            disabled={!verified || !username || (!password && !key) || busy}
            onClick={install}
          >
            Install on {host.name}
          </Button>
        </>
      )}
    </div>
  );
}
