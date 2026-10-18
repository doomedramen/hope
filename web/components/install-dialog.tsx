"use client";
import { useCallback, useEffect, useState } from "react";
import { Copy, Check, KeyRound } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { Badge } from "@/components/ui/badge";
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
import { hostDisplayName } from "@/lib/devices";
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
  InstallStage,
  InstallStatus,
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

const STAGE_LABELS: Record<InstallStage, string> = {
  connecting: "Connecting to the verified host key",
  authenticating: "Authenticating",
  preflight: "Checking the host",
  recover: "Recovering an interrupted install",
  uninstall: "Removing the agent",
  stage: "Downloading the agent",
  verify: "Verifying the signature",
  stop: "Stopping the previous service",
  place_binary: "Installing the agent binary",
  configure: "Configuring and enrolling",
  write_unit: "Writing the service definition",
  activate: "Starting the service",
  wait_report: "Installer waiting for the first report",
  rollback: "Restoring the previous installation",
  complete: "Installer finished",
  confirming_report: "Waiting for the server to accept a report",
};
const STATUS_LABELS: Record<InstallStatus, string> = {
  running: "Running",
  completed: "Completed",
  failed: "Failed",
  interrupted: "Interrupted",
  uncertain: "Remote state unknown",
};
function statusVariant(status: InstallStatus) {
  if (status === "completed") return "secondary" as const;
  if (status === "running") return "outline" as const;
  return "destructive" as const;
}

function JobView({
  job,
  onRetry,
  onBack,
}: {
  job: InstallJob;
  onRetry: () => void;
  onBack: () => void;
}) {
  const needsGuidance = job.status !== "running" && job.status !== "completed";
  return (
    <div className="flex flex-col gap-3" data-testid="ssh-job">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="font-medium">
          {job.mode === "uninstall"
            ? "Uninstall"
            : job.mode === "reinstall"
              ? "Reinstall"
              : "Install"}{" "}
          on {job.host}:{job.port}
        </h3>
        <Badge variant={statusVariant(job.status)}>
          {STATUS_LABELS[job.status]}
        </Badge>
      </div>
      <p className="text-xs text-muted-foreground">
        {job.username}@{job.host} · started{" "}
        {new Date(job.created_at * 1000).toLocaleString()}
        {job.error_code && <> · code {job.error_code}</>}
      </p>
      <ol
        className="flex list-decimal flex-col gap-1 pl-5 text-sm"
        aria-label={
          job.mode === "uninstall" ? "Uninstall stages" : "Installation stages"
        }
      >
        {job.stages.map((stage) => (
          <li
            key={stage}
            aria-current={
              job.status === "running" && stage === job.stage
                ? "step"
                : undefined
            }
            className={
              stage === job.stage && job.status === "running"
                ? "font-medium"
                : "text-muted-foreground"
            }
          >
            {STAGE_LABELS[stage]}
          </li>
        ))}
      </ol>
      {job.notice && (
        <Alert>
          <AlertDescription>{job.notice}</AlertDescription>
        </Alert>
      )}
      {job.message && job.status !== "running" && (
        <Alert variant={job.status === "completed" ? "default" : "destructive"}>
          <AlertDescription>{job.message}</AlertDescription>
        </Alert>
      )}
      {needsGuidance && job.guidance && (
        <p className="text-sm">
          <span className="font-medium">What to do: </span>
          {job.guidance}
        </p>
      )}
      {job.status === "running" && (
        <p className="text-xs text-muted-foreground">
          Closing this dialog does not stop the job. Reopen it to follow
          progress.
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        {needsGuidance && (
          <Button variant="outline" onClick={onRetry}>
            Retry with a new job
          </Button>
        )}
        <Button variant="ghost" onClick={onBack}>
          {job.status === "running" ? "Job history" : "Back"}
        </Button>
      </div>
    </div>
  );
}

export function SshInstall({
  host,
  uninstall = false,
}: {
  host: Host;
  uninstall?: boolean;
}) {
  const [address, setAddress] = useState(
    host.addresses.find((a) => !a.includes(":")) ?? host.addresses[0] ?? "",
  );
  const [port, setPort] = useState("22");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [key, setKey] = useState("");
  const [passphrase, setPassphrase] = useState("");
  // A fingerprint is only shown, confirmed or used for the exact address and port it was
  // inspected for; a late response for an earlier target is therefore never applied.
  const [inspected, setInspected] = useState<{
    address: string;
    port: number;
    fingerprint: string;
  } | null>(null);
  const [confirmed, setConfirmed] = useState("");
  const [job, setJob] = useState<InstallJob | null>(null);
  const [history, setHistory] = useState<InstallJob[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const hostId = host.id;
  const target = { host_id: hostId, address, port: Number(port) };
  const fingerprint =
    inspected &&
    inspected.address === target.address &&
    inspected.port === target.port
      ? inspected.fingerprint
      : "";
  const confirmationKey = `${address}|${target.port}|${fingerprint}`;
  const verified = fingerprint !== "" && confirmed === confirmationKey;
  const loadHistory = useCallback(async () => {
    try {
      const jobs = await api<InstallJob[]>("/ssh/jobs");
      const mine = jobs.filter(
        (j) => j.host_id === host.id && (j.mode === "uninstall") === uninstall,
      );
      setHistory(mine);
      return mine;
    } catch (e) {
      setError((e as Error).message);
      return [];
    }
  }, [host.id, uninstall]);
  // Reopen a job that is still running; otherwise show the form with history.
  useEffect(() => {
    let alive = true;
    api<InstallJob[]>("/ssh/jobs")
      .then((jobs) => {
        if (!alive) return;
        const mine = jobs.filter(
          (j) => j.host_id === hostId && (j.mode === "uninstall") === uninstall,
        );
        setHistory(mine);
        const running = mine.find((j) => j.status === "running");
        if (running) setJob(running);
      })
      .catch((e) => {
        if (alive) setError(e.message);
      });
    return () => {
      alive = false;
    };
  }, [hostId, uninstall]);
  const jobId = job?.id;
  const running = job?.status === "running";
  useEffect(() => {
    if (!jobId || !running) return;
    const timer = setInterval(() => {
      api<InstallJob>(`/ssh/jobs/${jobId}`)
        .then((current) => {
          setJob(current);
          if (current.status !== "running") void loadHistory();
        })
        .catch((e) => setError(e.message));
    }, 2000);
    return () => clearInterval(timer);
  }, [jobId, running, loadHistory]);
  function retry(previous: InstallJob) {
    // Credentials are never kept, and the host key must be inspected and confirmed again.
    setAddress(previous.host);
    setPort(String(previous.port));
    setUsername(previous.username);
    setInspected(null);
    setConfirmed("");
    setJob(null);
    setError("");
  }
  async function inspect() {
    setBusy(true);
    setError("");
    setConfirmed("");
    const requested = target;
    try {
      const result = await post<FingerprintResponse>(
        "/ssh/fingerprint",
        requested,
      );
      setInspected({
        address: requested.address,
        port: requested.port,
        fingerprint: result.fingerprint,
      });
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
        await post<InstallJob>(uninstall ? "/ssh/uninstall" : "/ssh/install", {
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
        <JobView
          job={job}
          onRetry={() => retry(job)}
          onBack={() => {
            setJob(null);
            void loadHistory();
          }}
        />
      ) : (
        <>
          <FieldGroup>
            <Field>
              <FieldLabel>Host address</FieldLabel>
              <Select
                disabled={busy}
                value={address}
                onValueChange={(v) => {
                  if (v) setAddress(v);
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
                disabled={busy}
                onChange={(e) => setPort(e.target.value)}
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
                  onClick={() => setConfirmed(verified ? "" : confirmationKey)}
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
                Credentials are used only for this job.
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
            {uninstall
              ? "Connect as the installation owner. Removing a system service requires root or passwordless sudo. The installed agent must match this host. Success also revokes its access; device history and identity are kept."
              : "System installation requires root or passwordless sudo. Otherwise the installer attempts a user service."}{" "}
            Custom CA deployments require the certificate at the configured path
            on the remote host.
          </p>
          <Button
            variant={uninstall ? "destructive" : "default"}
            disabled={!verified || !username || (!password && !key) || busy}
            onClick={install}
          >
            {uninstall ? "Uninstall from" : "Install on"}{" "}
            {hostDisplayName(host)}
          </Button>
          {history.length > 0 && (
            <div className="flex flex-col gap-2" data-testid="ssh-history">
              <h3 className="text-sm font-medium">
                {uninstall ? "Previous uninstalls" : "Previous installations"}
              </h3>
              <ul className="flex flex-col gap-1">
                {history.map((entry) => (
                  <li key={entry.id}>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-auto w-full justify-between gap-2 py-1.5"
                      onClick={() => setJob(entry)}
                    >
                      <span className="text-left">
                        {new Date(entry.created_at * 1000).toLocaleString()} ·{" "}
                        {entry.username}@{entry.host}
                      </span>
                      <Badge variant={statusVariant(entry.status)}>
                        {STATUS_LABELS[entry.status]}
                      </Badge>
                    </Button>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </>
      )}
    </div>
  );
}
