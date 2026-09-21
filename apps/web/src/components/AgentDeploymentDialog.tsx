import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangleIcon,
  CheckCircle2Icon,
  KeyRoundIcon,
  ShieldCheckIcon,
} from "lucide-react";
import { useEffect, useMemo, useState, type FormEvent } from "react";
import {
  createCredential,
  fetchCredentials,
  fetchJob,
  fetchSshHostKeys,
  getUserFacingError,
  installAgent,
  trustSshHostKey,
  type Credential,
  type DeviceDetail,
} from "@/lib/api";
import { labelize } from "@/lib/format";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { agentConnectionAddress, saveAgentConnectionAddress } from "@/lib/agentConnection";

const TERMINAL_JOB_STATES = new Set(["succeeded", "failed", "cancelled"]);

export function AgentDeploymentDialog({
  device,
  onOpenChange,
  open,
}: {
  device: DeviceDetail | undefined;
  onOpenChange: (open: boolean) => void;
  open: boolean;
}) {
  const queryClient = useQueryClient();
  const [host, setHost] = useState("");
  const [port, setPort] = useState("22");
  const [credentialId, setCredentialId] = useState("");
  const [showCreateCredential, setShowCreateCredential] = useState(false);
  const [credentialName, setCredentialName] = useState("");
  const [username, setUsername] = useState("root");
  const [password, setPassword] = useState("");
  const [privateKey, setPrivateKey] = useState("");
  const [sshMethod, setSshMethod] = useState<"password" | "key">("password");
  const [saveCredential, setSaveCredential] = useState(false);
  const [connectionAddress, setConnectionAddress] = useState(agentConnectionAddress);
  const [jobId, setJobId] = useState<string | null>(null);
  const [idempotencyKey, setIdempotencyKey] = useState<string | null>(null);

  const credentialsQuery = useQuery({
    queryKey: ["credentials", device?.id],
    queryFn: fetchCredentials,
    enabled: open,
  });
  const jobQuery = useQuery({
    queryKey: ["job", jobId],
    queryFn: () => fetchJob(jobId!),
    enabled: Boolean(jobId),
    refetchInterval: (query) => {
      const status = query.state.data?.status;
      return status && TERMINAL_JOB_STATES.has(status) ? false : 1_500;
    },
  });
  const hostKeysQuery = useQuery({
    queryKey: ["ssh-host-keys", device?.id],
    queryFn: () => fetchSshHostKeys(device!.id),
    enabled: open && Boolean(device),
    refetchInterval: jobId ? 1_500 : false,
  });

  const credentials = credentialsQuery.data?.items ?? [];
  const pendingHostKeys = useMemo(
    () =>
      (hostKeysQuery.data?.items ?? []).filter(
        (key) => key.state === "pending" || key.state === "changed",
      ),
    [hostKeysQuery.data],
  );

  const createCredentialMutation = useMutation({
    mutationFn: () => {
      if (!device)
        throw new Error("Select a device before creating a credential.");
      return createCredential({
        name: credentialName.trim(),
        scope: { kind: "device", device_id: device.id },
        secret: { type: "ssh_password", username, password },
      });
    },
    onSuccess: async (credential) => {
      await queryClient.invalidateQueries({
        queryKey: ["credentials", device?.id],
      });
      setCredentialId(credential.id);
      setCredentialName("");
      setPassword("");
      setShowCreateCredential(false);
    },
  });

  const installMutation = useMutation({
    mutationFn: async (forceNewJob: boolean) => {
      if (!device) throw new Error("Select a device before deploying the agent.");
      const address = saveAgentConnectionAddress(connectionAddress);
      let selectedCredentialId = credentialId;
      if (!selectedCredentialId) {
        const credential = await createCredential({
          name: credentialName.trim() || `Agent deployment: ${device.name}`,
          scope: { kind: "device", device_id: device.id },
          secret: sshMethod === "password"
            ? { type: "ssh_password", username, password }
            : { type: "ssh_private_key", username, private_key_pem: privateKey },
        });
        selectedCredentialId = credential.id;
      }
      const nextKey = forceNewJob ? crypto.randomUUID() : (idempotencyKey ?? crypto.randomUUID());
      setIdempotencyKey(nextKey);
      return installAgent(device.id, {
        host: host.trim(),
        port: Number(port),
        credential_id: selectedCredentialId,
        connection_url: address,
        disassociate_after_enrollment: !saveCredential && !credentialId,
        delete_credential_after: !saveCredential && !credentialId,
        idempotencyKey: nextKey,
      });
    },
    onSuccess: ({ job_id }) => setJobId(job_id),
  });

  const trustMutation = useMutation({
    mutationFn: trustSshHostKey,
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ["ssh-host-keys", device?.id],
      });
      if (jobId)
        await queryClient.invalidateQueries({ queryKey: ["job", jobId] });
      if (jobQuery.data?.status === "failed") installMutation.mutate(true);
    },
  });

  useEffect(() => {
    if (!open) return;
    setHost("");
    setPort("22");
    setCredentialId("");
    setShowCreateCredential(false);
    setCredentialName("");
    setUsername("root");
    setPassword("");
    setPrivateKey("");
    setSaveCredential(false);
    setConnectionAddress(agentConnectionAddress());
    setJobId(null);
    setIdempotencyKey(null);
  }, [open, device?.id]);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    installMutation.mutate(job?.status === "failed");
  }

  const job = jobQuery.data;
  const actionError =
    credentialsQuery.error ??
    createCredentialMutation.error ??
    installMutation.error ??
    jobQuery.error ??
    hostKeysQuery.error ??
    trustMutation.error;
  const isBusy =
    createCredentialMutation.isPending ||
    installMutation.isPending ||
    trustMutation.isPending;

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent className="max-h-[min(90vh,52rem)] max-w-2xl overflow-y-auto">
        <DialogHeader>
          <DialogTitle>Deploy agent</DialogTitle>
          <DialogDescription>
            Connect to {device?.name || "this device"} over SSH, install the
            signed agent, and wait for its first check-in.
          </DialogDescription>
        </DialogHeader>

        <form className="grid gap-5" onSubmit={submit}>
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="agent-deploy-connection">Agent connection address</FieldLabel>
              <Input id="agent-deploy-connection" value={connectionAddress} onChange={(event) => setConnectionAddress(event.target.value)} required />
              <FieldDescription>Address this device can reach: Hope’s HTTP LAN address or your HTTPS proxy host.</FieldDescription>
            </Field>
            <div className="grid gap-4 sm:grid-cols-[minmax(0,1fr)_8rem]">
              <Field>
                <FieldLabel htmlFor="agent-install-host">SSH host</FieldLabel>
                <Input
                  id="agent-install-host"
                  onChange={(event) => setHost(event.target.value)}
                  placeholder="192.0.2.10"
                  required
                  value={host}
                />
              </Field>
              <Field>
                <FieldLabel htmlFor="agent-install-port">Port</FieldLabel>
                <Input
                  id="agent-install-port"
                  min="1"
                  max="65535"
                  onChange={(event) => setPort(event.target.value)}
                  required
                  type="number"
                  value={port}
                />
              </Field>
            </div>

            <Field>
              <FieldLabel htmlFor="agent-install-credential">
                SSH credential
              </FieldLabel>
              <select
                aria-label="SSH credential"
                className="h-8 rounded-lg border border-input bg-transparent px-2.5 text-sm outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
                id="agent-install-credential"
                onChange={(event) => setCredentialId(event.target.value)}
                value={credentialId}
              >
                <option value="">Enter SSH details below</option>
                {credentials.map((credential) => (
                  <option key={credential.id} value={credential.id}>
                    {credential.name} ({labelize(credential.kind)})
                  </option>
                ))}
              </select>
              <FieldDescription>
                Secrets stay in the vault; the browser only chooses a
                credential.
              </FieldDescription>
            </Field>
          </FieldGroup>

          <div className="rounded-lg border bg-muted/30 p-4">
            <div className="flex items-start justify-between gap-3">
              <div>
                <p className="font-medium">
                  {showCreateCredential
                    ? "Create SSH credential"
                    : "Need a credential?"}
                </p>
                {!showCreateCredential ? (
                  <p className="text-xs text-muted-foreground">
                    Add a short-lived password credential for this device.
                  </p>
                ) : null}
              </div>
              <Button
                onClick={() => {
                  setShowCreateCredential((current) => !current);
                  setCredentialId("");
                }}
                size="sm"
                type="button"
                variant="outline"
              >
                <KeyRoundIcon data-icon="inline-start" />
                {showCreateCredential ? "Use existing" : "Create credential"}
              </Button>
            </div>
            {showCreateCredential || !credentialId ? (
              <div className="mt-4 grid gap-4">
                <div className="grid gap-4 sm:grid-cols-2">
                  <Field>
                    <FieldLabel htmlFor="agent-credential-name">
                      Credential name
                    </FieldLabel>
                    <Input
                      id="agent-credential-name"
                      onChange={(event) =>
                        setCredentialName(event.target.value)
                      }
                      value={credentialName}
                    />
                  </Field>
                  <Field>
                    <FieldLabel htmlFor="agent-credential-username">
                      Username
                    </FieldLabel>
                    <Input
                      id="agent-credential-username"
                      onChange={(event) => setUsername(event.target.value)}
                      required
                      value={username}
                    />
                  </Field>
                </div>
                <Field>
                  <FieldLabel htmlFor="agent-ssh-method">Authentication</FieldLabel>
                  <select id="agent-ssh-method" className="h-8 rounded-lg border border-input bg-transparent px-2.5 text-sm" value={sshMethod} onChange={(event) => setSshMethod(event.target.value as "password" | "key")}>
                    <option value="password">Password</option><option value="key">Private key</option>
                  </select>
                </Field>
                {sshMethod === "password" ? <Field><FieldLabel htmlFor="agent-credential-password">Password</FieldLabel><Input id="agent-credential-password" onChange={(event) => setPassword(event.target.value)} required type="password" value={password} /></Field> : <Field><FieldLabel htmlFor="agent-credential-key">Private key (PEM)</FieldLabel><textarea id="agent-credential-key" className="min-h-28 w-full rounded-lg border border-input bg-transparent p-2.5 font-mono text-xs" value={privateKey} onChange={(event) => setPrivateKey(event.target.value)} required /></Field>}
                <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={saveCredential} onChange={(event) => setSaveCredential(event.target.checked)} />Save SSH credential for later</label>
                <Button
                  disabled={isBusy || !credentialName.trim() || !password}
                  onClick={() => createCredentialMutation.mutate()}
                  type="button"
                  variant="secondary"
                >
                  {createCredentialMutation.isPending ? (
                    <Spinner data-icon="inline-start" />
                  ) : null}
                  Save credential now
                </Button>
              </div>
            ) : null}
          </div>

          {actionError ? (
            <Alert aria-live="assertive" variant="destructive">
              <AlertTriangleIcon />
              <AlertTitle>Agent install needs attention</AlertTitle>
              <AlertDescription>
                {getUserFacingError(
                  actionError,
                  "The agent install request failed.",
                )}
              </AlertDescription>
            </Alert>
          ) : null}

          {pendingHostKeys.length > 0 ? (
            <Alert>
              <ShieldCheckIcon />
              <AlertTitle>Review the SSH host key</AlertTitle>
              <AlertDescription className="grid gap-3">
                <p>
                  The first connection is paused until you explicitly trust the
                  target fingerprint.
                </p>
                {pendingHostKeys.map((key) => (
                  <div
                    className="flex flex-wrap items-center justify-between gap-3 rounded-md border bg-background p-3"
                    key={key.id}
                  >
                    <div className="min-w-0 text-xs">
                      <p className="font-medium">
                        {key.host}:{key.port} · {key.key_type}
                      </p>
                      <p className="break-all font-mono text-muted-foreground">
                        {key.fingerprint_sha256}
                      </p>
                    </div>
                    <Button
                      disabled={isBusy}
                      onClick={() => trustMutation.mutate(key.id)}
                      size="sm"
                      type="button"
                    >
                      <ShieldCheckIcon data-icon="inline-start" />
                      Trust host key
                    </Button>
                  </div>
                ))}
              </AlertDescription>
            </Alert>
          ) : null}

          {job ? (
            <div className="rounded-lg border p-4" role="status">
              <div className="flex items-center gap-2">
                {job.status === "succeeded" ? (
                  <CheckCircle2Icon className="text-emerald-600" />
                ) : (
                  <Spinner />
                )}
                <p className="font-medium">Agent install {job.status}</p>
              </div>
              {job.last_error ? (
                <p className="mt-2 text-sm text-destructive">
                  {getUserFacingError(
                    job.last_error,
                    "The deployment job reported an error.",
                  )}
                </p>
              ) : null}
              <p className="mt-1 text-xs text-muted-foreground">
                Attempt {job.attempts} of {job.max_attempts} · job {job.id}
              </p>
            </div>
          ) : null}

          <DialogFooter>
            <Button
              onClick={() => onOpenChange(false)}
              type="button"
              variant="outline"
            >
              Done
            </Button>
            <Button
              disabled={
                isBusy ||
                (!credentialId && !(sshMethod === "password" ? password : privateKey)) ||
                !host.trim() ||
                Boolean(job && job.status === "succeeded")
              }
              type="submit"
            >
              {installMutation.isPending ? (
                <Spinner data-icon="inline-start" />
              ) : null}
              Deploy agent
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function credentialLabel(credential: Credential): string {
  return `${credential.name} (${labelize(credential.kind)})`;
}
