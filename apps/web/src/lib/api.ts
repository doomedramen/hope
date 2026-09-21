export interface ReadyResponse {
  status: "ready" | "not_ready";
  error?: string;
}

export interface ApiPage<T> {
  items: T[];
  next_cursor: string | null;
}

export interface Network {
  id: string;
  site_id: string | null;
  cidr: string;
  vlan: number | null;
  gateway: string | null;
  scan_policy: Record<string, unknown>;
  name: string | null;
  version: number;
  created_at: string;
  updated_at: string;
}

export interface DiscoveryScope {
  network_id: string;
  excluded_cidrs: string[];
  scan_profile: "normal" | "low_impact" | string;
  target_count: number;
  confirmed_target_count: number | null;
  confirmed_at: string | null;
  enabled: boolean;
  tcp_concurrency: number;
  per_host_concurrency: number;
  connect_timeout_ms: number;
  discovery_interval_seconds: number;
  full_tcp_interval_seconds: number;
  version: number;
  created_at: string;
  updated_at: string;
}

export interface DiscoveryState {
  scope: DiscoveryScope | null;
  scan_run: ScanRun | null;
}

export interface AgentEnrollmentBootstrap {
  code: string;
  expires_in_minutes: number;
}

export type ScanRunKind = "initial_discovery" | "change_scan" | "full_tcp";

export type ScanRunStatus =
  "pending" | "running" | "succeeded" | "failed" | "cancelled";

export interface ScanJobSnapshot {
  id: string;
  job_type: string;
  status: string;
  progress: Record<string, unknown>;
  attempts: number;
  max_attempts: number;
  run_at: string;
  last_error: string | null;
  created_at: string;
  updated_at: string;
}

export interface ScanLogEntry {
  id: string;
  actor_kind: string;
  action: string;
  result: string;
  detail: Record<string, unknown> | null;
  occurred_at: string;
}

export interface ScanRun {
  id: string;
  network_id: string;
  job_id: string;
  kind: ScanRunKind;
  status: ScanRunStatus;
  scope_version: number;
  targets_planned: number;
  targets_completed: number;
  ports_planned: number;
  ports_completed: number;
  complete: boolean;
  authoritative: boolean;
  cancellation_requested: boolean;
  requested_by: string | null;
  source: "operator" | "scheduler" | "system";
  error: string | null;
  started_at: string | null;
  finished_at: string | null;
  created_at: string;
  updated_at: string;
  job?: ScanJobSnapshot | null;
  logs?: ScanLogEntry[];
}

export interface NetworkScansResponse {
  items: ScanRun[];
  active_scan: ScanRun | null;
}

export interface Device {
  id: string;
  device_type: string;
  name: string | null;
  status: "active" | "stale" | "archived" | "merged" | string;
  identity_confidence: number;
  canonical_of: string | null;
  version: number;
  created_at: string;
  updated_at: string;
}

export interface InventoryInterface {
  id: string;
  device_id: string;
  mac: string | null;
  description: string | null;
  first_seen: string;
  last_seen: string;
  version: number;
  created_at: string;
  updated_at: string;
}

export interface Address {
  id: string;
  interface_id: string;
  ip: string;
  address_type: "static" | "dhcp" | "unknown" | string;
  first_seen: string;
  last_seen: string;
  is_current: boolean;
  version: number;
  created_at: string;
  updated_at: string;
}

export interface Evidence {
  id: string;
  subject_table: string;
  subject_id: string;
  source_type: string;
  source_instance: string | null;
  attribute: string;
  value: unknown;
  confidence: number;
  first_seen: string;
  last_seen: string;
  expires_at: string | null;
  absent: boolean;
  confirmed_by: string | null;
  overridden: boolean;
}

export interface DeviceDetail extends Device {
  interfaces: InventoryInterface[];
  evidence: Evidence[];
  identity_confidence: number;
  merged_member_ids: string[];
}

export type AgentStatus = "online" | "offline" | "stale" | string;

export interface AgentInventorySummary {
  interfaces: number;
  filesystems: number;
  processes: number;
  sockets: number;
  containers: number;
}

export interface Agent {
  id: string;
  hostname: string | null;
  status: AgentStatus;
  agent_version: string | null;
  os: string | null;
  arch: string | null;
  capabilities: string[];
  last_seen: string | null;
  inventory_summary: AgentInventorySummary;
  device_id: string | null;
  protocol_version: number | null;
  revoked_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface AgentHostInventory {
  hostname: string | null;
  os: string | null;
  distribution: string | null;
  kernel: string | null;
  arch: string | null;
  boot_id: string | null;
  uptime_seconds: number | null;
  cpu_model: string | null;
  cpu_count: number | null;
  load_1m: number | null;
  memory_total_bytes: number | null;
  memory_used_bytes: number | null;
  machine_id_hash: string | null;
}

export interface AgentNetworkInterface {
  name: string;
  mac: string | null;
  addresses: string[];
  state: string | null;
  mtu: number | null;
}

export interface AgentNetworkRoute {
  destination: string;
  gateway: string | null;
  interface_name: string | null;
}

export interface AgentNetworkInventory {
  interfaces: AgentNetworkInterface[];
  routes: AgentNetworkRoute[];
}

export interface AgentFilesystemInventory {
  mount_point: string;
  device: string | null;
  filesystem: string | null;
  total_bytes: number | null;
  used_bytes: number | null;
  available_bytes: number | null;
  inode_total: number | null;
  inode_used: number | null;
  read_only: boolean | null;
}

export interface AgentProcessInventory {
  pid: number;
  name: string;
  command: string | null;
  user: string | null;
  state: string | null;
  cpu_percent: number | null;
  memory_bytes: number | null;
  started_at: string | null;
}

export type AgentReachabilityState =
  "reachable" | "unreachable" | "not_checked" | "unknown" | string;

export interface AgentSocketReachability {
  state: AgentReachabilityState;
  checked_at: string | null;
  worker_id: string | null;
  endpoint: string | null;
}

export interface AgentSocketInventory {
  protocol: string;
  local_address: string;
  local_port: number;
  state: string | null;
  listening: boolean;
  process_name: string | null;
  process_id: number | null;
  reachability: AgentSocketReachability;
}

export interface AgentContainerInventory {
  id: string;
  name: string;
  image: string | null;
  status: string | null;
  health: string | null;
  networks: string[];
  mounts: string[];
  published_ports: string[];
}

export interface AgentEvidenceItem {
  id: string;
  source: string;
  source_instance: string | null;
  attribute: string;
  value: unknown;
  confidence: number;
  observed_at: string;
  expires_at: string | null;
  absent: boolean;
  confirmed: boolean;
}

export interface AgentReconciliation {
  status: "matched" | "pending" | "conflict" | "unmatched" | string;
  device_id: string | null;
  confidence: number | null;
  matched_identifiers: string[];
  conflicts: string[];
  last_reconciled_at: string | null;
  explanation: string | null;
}

export interface AgentDetail extends Agent {
  host: AgentHostInventory | null;
  network: AgentNetworkInventory | null;
  filesystems: AgentFilesystemInventory[];
  processes: AgentProcessInventory[];
  sockets: AgentSocketInventory[];
  containers: AgentContainerInventory[];
  evidence: AgentEvidenceItem[];
  reconciliation: AgentReconciliation;
}

export type CredentialKind = "ssh_private_key" | "ssh_password" | string;

export interface Credential {
  id: string;
  name: string;
  kind: CredentialKind;
  scope: { kind: string; device_id?: string; [key: string]: unknown };
  version: number;
  created_by: string | null;
  created_at: string;
  updated_at: string;
  last_used_at: string | null;
  revoked_at: string | null;
  deleted_at: string | null;
}

export type JobStatus =
  "pending" | "running" | "succeeded" | "failed" | "cancelled" | string;

export interface DeploymentJob {
  id: string;
  job_type: string;
  status: JobStatus;
  progress: Record<string, unknown>;
  attempts: number;
  max_attempts: number;
  run_at: string | null;
  last_error: string | null;
  created_at: string | null;
  updated_at: string | null;
}

export type SshHostKeyState =
  "pending" | "trusted" | "changed" | "revoked" | string;

export interface SshHostKey {
  id: string;
  device_id: string | null;
  host: string;
  port: number;
  key_type: string;
  fingerprint_sha256: string;
  previous_fingerprint_sha256: string | null;
  state: SshHostKeyState;
  first_seen_at: string | null;
  last_seen_at: string | null;
  trusted_at: string | null;
  changed_at: string | null;
  revoked_at: string | null;
  version: number;
}

export interface MatchedIdentifier {
  rule_type: string;
  weight: number;
  value: string;
}

export interface IdentityExplanation {
  score: number;
  matched: MatchedIdentifier[];
  threshold: number;
  decision: "auto_match" | "suggested" | "new_device" | string;
}

export interface IdentityIdentifier {
  rule_type: string;
  value: string;
  pinned: boolean;
}

export interface IdentitySuggestion {
  id: string;
  candidate_device_id: string;
  observed: IdentityIdentifier[];
  explanation: IdentityExplanation;
  score: number;
  status: "pending" | "confirmed" | "rejected" | string;
  resolved_by: string | null;
  resolved_at: string | null;
  created_at: string;
  updated_at: string;
}

export type ServiceReviewStatus = "pending" | "confirmed" | "rejected";

export interface FingerprintEvidenceField {
  path: string;
  value: string;
}

export interface FingerprintCandidate {
  rule_id: string;
  fixture_version: number;
  protocol: string;
  product: string | null;
  version: string | null;
  product_version?: string | null;
  confidence: number;
  evidence_fields: FingerprintEvidenceField[];
  reasons: string[];
}

export interface ServiceReviewItem {
  id: string;
  service_id: string;
  fingerprint_evidence_id: string;
  source_instance: string | null;
  candidate_key: string;
  candidate: FingerprintCandidate;
  candidates: FingerprintCandidate[];
  evidence: {
    m2: Record<string, unknown>;
    fingerprint: Record<string, unknown>;
  };
  rule_id: string;
  fixture_version: number;
  confidence: number;
  reason: "low_confidence" | "conflict" | string;
  policy_threshold: number;
  status: ServiceReviewStatus;
  resolved_by: string | null;
  resolved_at: string | null;
  created_at: string;
  updated_at: string;
}

export type MonitorProposalStatus = "pending" | "approved" | "rejected";

export interface MonitorProposal {
  id: string;
  service_id: string;
  endpoint_id: string;
  rule_id: string;
  rule_version: number;
  target_identity: string;
  target: Record<string, unknown>;
  protocol: string;
  product: string | null;
  product_version: string | null;
  check_type: "http" | "tcp" | string;
  check_config: Record<string, unknown>;
  confidence: number;
  auto_create_allowed: boolean;
  resolved_from: Record<string, unknown>;
  source_evidence_id: string | null;
  source_rule_id: string | null;
  status: MonitorProposalStatus;
  decision_source: "automatic" | "manual" | string;
  decided_by: string | null;
  decided_at: string | null;
  user_overrides: Record<string, unknown>;
  override_by: string | null;
  override_at: string | null;
  version: number;
  created_at: string;
  updated_at: string;
}

export type MonitorState =
  "unknown" | "up" | "degraded" | "down" | "stale" | string;

export interface Monitor {
  id: string;
  proposal_id: string | null;
  service_id: string;
  endpoint_id: string;
  monitor_type: "icmp" | "tcp" | "http" | "https" | "dns" | "tls" | string;
  config: Record<string, unknown>;
  interval_seconds: number;
  timeout_ms: number;
  failure_threshold: number;
  recovery_threshold: number;
  enabled: boolean;
  state: MonitorState;
  underlying_state: Exclude<MonitorState, "stale">;
  consecutive_failures: number;
  consecutive_successes: number;
  last_result_at: string | null;
  last_success_at: string | null;
  last_failure_at: string | null;
  next_run_at: string | null;
  lease_owner: string | null;
  lease_expires_at: string | null;
  version: number;
  created_by: string | null;
  created_at: string;
  updated_at: string;
  endpoint_address: string | null;
  endpoint_port: number | null;
  endpoint_url: string | null;
  endpoint_dns_name: string | null;
  service_name: string | null;
  service_product: string | null;
  service_product_version: string | null;
}

export type MonitorResultStatus = "success" | "failure" | "timeout" | "error";

export interface MonitorResult {
  id: string;
  monitor_id: string;
  status: MonitorResultStatus | string;
  observed_at: string;
  latency_ms: number | null;
  error: string | null;
  details: Record<string, unknown>;
  created_at: string;
}

export type IncidentState = "open" | "recovered" | string;

export interface Incident {
  id: string;
  monitor_id: string;
  state: IncidentState;
  severity: "warning" | "critical" | string;
  opened_at: string;
  recovered_at: string | null;
  last_event_at: string;
  failure_count: number;
  last_result_id: string | null;
  summary: string | null;
  created_at: string;
  updated_at: string;
  service_id: string;
  endpoint_id: string;
  monitor_type: string;
  monitor_state: MonitorState;
  endpoint_address: string | null;
  endpoint_port: number | null;
  endpoint_url: string | null;
  endpoint_dns_name: string | null;
  service_name: string | null;
  service_product: string | null;
}

export type NotificationProvider = "webhook" | "ntfy";

export interface NotificationChannel {
  id: string;
  name: string;
  provider: NotificationProvider | string;
  config: Record<string, unknown>;
  enabled: boolean;
  created_at: string;
  updated_at: string;
}

export interface NotificationRoute {
  id: string;
  channel_id: string;
  min_severity: string;
  event_types: string[];
  delay_seconds: number;
  enabled: boolean;
  created_at: string;
  updated_at: string;
  channel_name: string;
  provider: NotificationProvider | string;
}

export interface ChangeEvent {
  id: string;
  entity_kind: string;
  entity_id: string;
  category: string;
  severity: "info" | "notice" | "warning" | "critical" | string;
  before: Record<string, unknown> | null;
  after: Record<string, unknown> | null;
  evidence_source: string | null;
  occurred_at: string;
  acknowledged: boolean;
}

export type MaintenanceState =
  | "draft"
  | "scheduled"
  | "upcoming"
  | "active"
  | "overrunning"
  | "completed"
  | "cancelled"
  | string;

export type MaintenanceResourceRole =
  "target" | "required" | "affected" | "exclusive" | string;

export interface MaintenanceResource {
  role: MaintenanceResourceRole;
  kind: string | null;
  id: string | null;
  key: string | null;
  expected_failure: boolean;
}

export interface MaintenanceResourceInput {
  role: "target" | "required" | "affected" | "exclusive";
  kind?: string;
  id?: string;
  key?: string;
  expected_failure?: boolean;
}

export interface MaintenanceOccurrence {
  id: string;
  occurrence_key: string;
  occurrence_index: number;
  start_at: string;
  end_at: string;
  reservation_start: string;
  reservation_end: string;
  timezone: string;
}

export interface MaintenanceEvent {
  id: string;
  name: string;
  description: string | null;
  timezone: string;
  start_at: string;
  end_at: string;
  recurrence_rule: string | null;
  lead_in_seconds: number;
  cooldown_seconds: number;
  disruptive: boolean;
  notification_policy: Record<string, unknown>;
  owner: string | null;
  source: string | null;
  notes: string | null;
  links: string[];
  state: MaintenanceState;
  version: number;
  created_at: string;
  updated_at: string;
  resources: MaintenanceResource[];
  occurrences: MaintenanceOccurrence[];
}

export interface MaintenanceEventInput {
  name: string;
  description?: string | null;
  timezone: string;
  start: string;
  end?: string;
  duration_seconds?: number;
  recurrence_rule?: string | null;
  lead_in_seconds?: number;
  cooldown_seconds?: number;
  disruptive?: boolean;
  notification_policy?: Record<string, unknown>;
  owner?: string | null;
  source?: string | null;
  notes?: string | null;
  links?: string[];
  resources: MaintenanceResourceInput[];
  state?: "draft" | "scheduled" | "upcoming";
}

export interface MaintenanceEventPatchInput extends Omit<
  Partial<MaintenanceEventInput>,
  "state"
> {
  version: number;
  state?: MaintenanceState;
}

export interface MaintenanceSuggestedMove {
  event_id: string;
  occurrence_id: string;
  start_after: string;
}

export interface MaintenanceConflict {
  event_id: string;
  occurrence_id: string;
  conflicting_event_id: string;
  conflicting_occurrence_id: string;
  resource_role: MaintenanceResourceRole;
  resource_kind: string | null;
  resource_id: string | null;
  resource_key: string | null;
  related_resource_role: MaintenanceResourceRole | null;
  related_resource_kind: string | null;
  related_resource_id: string | null;
  related_resource_key: string | null;
  relationship: string;
  overlap_start: string;
  overlap_end: string;
  reason: string;
  suggestion: string;
  suggested_move: MaintenanceSuggestedMove;
}

interface MaintenanceErrorBody {
  error?: string;
  conflicts?: MaintenanceConflict[];
  current_version?: number | null;
}

export interface LoginResponse {
  status: "ok";
}

export interface SetupResponse {
  email: string;
}

export interface SetupStatusResponse {
  setup_required: boolean;
}

export class ApiError extends Error {
  readonly status: number;
  readonly fieldErrors: Record<string, string>;

  constructor(
    status: number,
    message: string,
    fieldErrors: Record<string, string> = {},
  ) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.fieldErrors = fieldErrors;
  }
}

export class MaintenanceApiError extends ApiError {
  readonly conflicts: MaintenanceConflict[];
  readonly currentVersion: number | null | undefined;

  constructor(status: number, message: string, body: MaintenanceErrorBody) {
    super(status, message);
    this.name = "MaintenanceApiError";
    this.conflicts = body.conflicts ?? [];
    this.currentVersion = body.current_version;
  }
}

const TECHNICAL_ERROR_PATTERNS = [
  /\bdatabase\b/i,
  /\bsql\b/i,
  /\bsyntax error\b/i,
  /\binternal server error\b/i,
  /\btraceback\b/i,
  /\bstack trace\b/i,
  /\bpanic\b/i,
  /\bat [^\s]+:\d+(?::\d+)?\b/i,
];

/**
 * Convert request failures and server-provided error fields into copy that is
 * safe to show to an operator. Detailed failures stay available in logs and
 * developer tooling, never in the primary UI surface.
 */
export function getUserFacingError(
  error: unknown,
  fallback = "Something went wrong. Please try again.",
): string {
  const message =
    error instanceof ApiError || typeof error === "string"
      ? error instanceof ApiError
        ? error.message
        : error
      : null;

  if (
    !message ||
    TECHNICAL_ERROR_PATTERNS.some((pattern) => pattern.test(message))
  ) {
    return fallback;
  }

  // Do not disclose unexpected server failures, even when the backend sends a
  // technically readable message. Client errors are intentionally allowed to
  // explain validation and permission problems near the affected action.
  if (error instanceof ApiError && error.status >= 500) {
    return fallback;
  }

  return message;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const headers = new Headers(init?.headers);
  if (init?.body) {
    headers.set("content-type", "application/json");
  }
  if (init?.method && init.method !== "GET") {
    headers.set("x-requested-with", "hope");
  }

  const res = await fetch(path, {
    credentials: "same-origin",
    ...init,
    headers,
  });

  const contentType = res.headers.get("content-type") ?? "";
  const body =
    res.status === 204
      ? null
      : contentType.includes("json")
        ? ((await res.json()) as T & {
            error?: string;
            field_errors?: Record<string, string>;
          })
        : null;
  if (!res.ok) {
    throw new ApiError(
      res.status,
      body?.error ?? `Request failed (${res.status})`,
      body?.field_errors,
    );
  }
  return body as T;
}

async function maintenanceRequest<T>(
  path: string,
  init?: RequestInit,
): Promise<T> {
  const headers = new Headers(init?.headers);
  if (init?.body) {
    headers.set("content-type", "application/json");
  }
  if (init?.method && init.method !== "GET") {
    headers.set("x-requested-with", "hope");
  }

  const res = await fetch(path, {
    credentials: "same-origin",
    ...init,
    headers,
  });
  const contentType = res.headers.get("content-type") ?? "";
  const body = contentType.includes("json")
    ? ((await res.json()) as T & MaintenanceErrorBody)
    : null;
  if (!res.ok) {
    throw new MaintenanceApiError(
      res.status,
      body?.error ?? `Request failed (${res.status})`,
      body ?? {},
    );
  }
  return body as T;
}

function queryString(
  params: Record<string, string | number | undefined>,
): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== "") {
      search.set(key, String(value));
    }
  }
  const encoded = search.toString();
  return encoded ? `?${encoded}` : "";
}

export async function fetchHealthReady(): Promise<ReadyResponse> {
  const res = await fetch("/health/ready");
  const body = (await res.json()) as ReadyResponse;
  if (!res.ok && !body.status) {
    throw new Error(`health check failed: ${res.status}`);
  }
  return body;
}

export async function fetchDevices(): Promise<ApiPage<Device>> {
  return request<ApiPage<Device>>("/api/v1/devices?limit=100");
}

export async function fetchNetworks(): Promise<ApiPage<Network>> {
  return request<ApiPage<Network>>("/api/v1/networks?limit=100");
}

export async function createNetwork(input: {
  cidr: string;
  name?: string;
  gateway?: string;
  vlan?: number;
}): Promise<Network> {
  return request<Network>("/api/v1/networks", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function patchNetwork(
  id: string,
  input: {
    version: number;
    cidr?: string;
    name?: string | null;
    gateway?: string | null;
    vlan?: number | null;
  },
): Promise<Network> {
  return request<Network>(`/api/v1/networks/${id}`, {
    method: "PATCH",
    body: JSON.stringify(input),
  });
}

export async function deleteNetwork(id: string): Promise<void> {
  await request<unknown>(`/api/v1/networks/${id}`, { method: "DELETE" });
}

export async function draftDiscoveryScope(
  networkId: string,
  input: {
    excluded_cidrs: string[];
    scan_profile: "normal" | "low_impact";
  },
): Promise<DiscoveryScope> {
  return request<DiscoveryScope>(
    `/api/v1/networks/${networkId}/discovery-scope`,
    {
      method: "POST",
      body: JSON.stringify(input),
    },
  );
}

export async function confirmDiscoveryScope(
  networkId: string,
  targetCount: number,
): Promise<DiscoveryScope> {
  return request<DiscoveryScope>(
    `/api/v1/networks/${networkId}/discovery-scope/confirm`,
    {
      method: "POST",
      body: JSON.stringify({ target_count: targetCount }),
    },
  );
}

export async function fetchDiscoveryState(
  networkId: string,
): Promise<DiscoveryState> {
  return request<DiscoveryState>(
    `/api/v1/networks/${networkId}/discovery-state`,
  );
}

export async function launchNetworkScan(
  networkId: string,
  input: { kind: ScanRunKind; idempotencyKey: string },
): Promise<ScanRun> {
  return request<ScanRun>(`/api/v1/networks/${networkId}/scans`, {
    method: "POST",
    headers: { "Idempotency-Key": input.idempotencyKey },
    body: JSON.stringify({ kind: input.kind }),
  });
}

export async function fetchNetworkScans(
  networkId: string,
): Promise<NetworkScansResponse> {
  return request<NetworkScansResponse>(`/api/v1/networks/${networkId}/scans`);
}

export async function fetchScanRun(id: string): Promise<ScanRun> {
  return request<ScanRun>(`/api/v1/scans/${id}`);
}

export async function cancelScanRun(id: string): Promise<ScanRun> {
  return request<ScanRun>(`/api/v1/scans/${id}/cancel`, {
    method: "POST",
  });
}

export async function fetchDevice(id: string): Promise<DeviceDetail> {
  return request<DeviceDetail>(`/api/v1/devices/${id}`);
}

export async function fetchAgents(): Promise<ApiPage<Agent>> {
  return request<ApiPage<Agent>>("/api/v1/agents?limit=100");
}

export async function createAgentEnrollment(): Promise<AgentEnrollmentBootstrap> {
  return request<AgentEnrollmentBootstrap>("/api/v1/agent-enrollment", {
    method: "POST",
  });
}

export async function fetchAgent(id: string): Promise<AgentDetail> {
  return request<AgentDetail>(`/api/v1/agents/${id}`);
}

export async function fetchCredentials(): Promise<ApiPage<Credential>> {
  return request<ApiPage<Credential>>("/api/v1/credentials?limit=100");
}

export async function createCredential(input: {
  name: string;
  scope: { kind: "device"; device_id: string };
  secret:
    | {
        type: "ssh_password";
        username: string;
        password: string;
      }
    | {
        type: "ssh_private_key";
        username: string;
        private_key_pem: string;
        passphrase?: string | null;
      };
}): Promise<Credential> {
  return request<Credential>("/api/v1/credentials", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function installAgent(
  deviceId: string,
  input: {
    host: string;
    port: number;
    credential_id: string;
    disassociate_after_enrollment?: boolean;
    idempotencyKey: string;
  },
): Promise<{ job_id: string; status: JobStatus }> {
  return request<{ job_id: string; status: JobStatus }>(
    `/api/v1/devices/${deviceId}/agent-install`,
    {
      method: "POST",
      headers: { "Idempotency-Key": input.idempotencyKey },
      body: JSON.stringify({
        host: input.host,
        port: input.port,
        credential_id: input.credential_id,
        disassociate_after_enrollment:
          input.disassociate_after_enrollment ?? false,
      }),
    },
  );
}

export async function repairAgent(
  deviceId: string,
  input: {
    host: string;
    port: number;
    credential_id: string;
    disassociate_after_enrollment?: boolean;
    idempotencyKey: string;
  },
): Promise<{ job_id: string; status: JobStatus }> {
  return request<{ job_id: string; status: JobStatus }>(
    `/api/v1/devices/${deviceId}/agent-repair`,
    {
      method: "POST",
      headers: { "Idempotency-Key": input.idempotencyKey },
      body: JSON.stringify({
        host: input.host,
        port: input.port,
        credential_id: input.credential_id,
        disassociate_after_enrollment:
          input.disassociate_after_enrollment ?? false,
      }),
    },
  );
}

export async function fetchJob(id: string): Promise<DeploymentJob> {
  return request<DeploymentJob>(`/api/v1/jobs/${id}`);
}

export async function fetchSshHostKeys(
  deviceId?: string,
): Promise<ApiPage<SshHostKey>> {
  return request<ApiPage<SshHostKey>>(
    `/api/v1/ssh-host-keys${deviceId ? `?device_id=${encodeURIComponent(deviceId)}` : ""}`,
  );
}

export async function trustSshHostKey(id: string): Promise<SshHostKey> {
  return request<SshHostKey>(`/api/v1/ssh-host-keys/${id}/trust`, {
    method: "POST",
  });
}

export async function fetchAddresses(): Promise<ApiPage<Address>> {
  return request<ApiPage<Address>>("/api/v1/addresses?limit=200");
}

export async function fetchIdentitySuggestions(): Promise<
  ApiPage<IdentitySuggestion>
> {
  return request<ApiPage<IdentitySuggestion>>(
    "/api/v1/identity-suggestions?limit=100",
  );
}

export async function fetchServiceReviews(
  status: ServiceReviewStatus = "pending",
): Promise<ApiPage<ServiceReviewItem>> {
  return request<ApiPage<ServiceReviewItem>>(
    `/api/v1/service-reviews${queryString({ limit: 100, status })}`,
  );
}

export async function resolveServiceReview(
  id: string,
  decision: "confirm" | "reject",
): Promise<{ id: string; service_id: string; status: ServiceReviewStatus }> {
  return request<{
    id: string;
    service_id: string;
    status: ServiceReviewStatus;
  }>(`/api/v1/service-reviews/${id}/${decision}`, { method: "POST" });
}

export async function fetchChanges(filters?: {
  category?: string;
  severity?: string;
}): Promise<ApiPage<ChangeEvent>> {
  return request<ApiPage<ChangeEvent>>(
    `/api/v1/changes${queryString({ ...filters, limit: 100 })}`,
  );
}

export async function fetchMaintenanceEvents(filters?: {
  cursor?: string;
  limit?: number;
  state?: MaintenanceState;
}): Promise<ApiPage<MaintenanceEvent>> {
  return maintenanceRequest<ApiPage<MaintenanceEvent>>(
    `/api/v1/maintenance-events${queryString({
      cursor: filters?.cursor,
      limit: filters?.limit ?? 200,
      state: filters?.state,
    })}`,
  );
}

export async function fetchMaintenanceEvent(
  id: string,
): Promise<MaintenanceEvent> {
  return maintenanceRequest<MaintenanceEvent>(
    `/api/v1/maintenance-events/${id}`,
  );
}

export async function fetchMaintenanceConflicts(
  id: string,
): Promise<ApiPage<MaintenanceConflict>> {
  return maintenanceRequest<ApiPage<MaintenanceConflict>>(
    `/api/v1/maintenance-events/${id}/conflicts`,
  );
}

export async function createMaintenanceEvent(
  input: MaintenanceEventInput,
): Promise<MaintenanceEvent> {
  return maintenanceRequest<MaintenanceEvent>("/api/v1/maintenance-events", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function patchMaintenanceEvent(
  id: string,
  input: MaintenanceEventPatchInput,
): Promise<MaintenanceEvent> {
  return maintenanceRequest<MaintenanceEvent>(
    `/api/v1/maintenance-events/${id}`,
    {
      method: "PATCH",
      body: JSON.stringify(input),
    },
  );
}

export async function startMaintenanceEvent(
  id: string,
  version: number,
): Promise<MaintenanceEvent> {
  return maintenanceRequest<MaintenanceEvent>(
    `/api/v1/maintenance-events/${id}/start`,
    {
      method: "POST",
      body: JSON.stringify({ version }),
    },
  );
}

export async function completeMaintenanceEvent(
  id: string,
  version: number,
): Promise<MaintenanceEvent> {
  return maintenanceRequest<MaintenanceEvent>(
    `/api/v1/maintenance-events/${id}/complete`,
    {
      method: "POST",
      body: JSON.stringify({ version }),
    },
  );
}

export async function cancelMaintenanceEvent(
  id: string,
  version: number,
): Promise<MaintenanceEvent> {
  return maintenanceRequest<MaintenanceEvent>(
    `/api/v1/maintenance-events/${id}/cancel`,
    {
      method: "POST",
      body: JSON.stringify({ version }),
    },
  );
}

export async function fetchMonitorProposals(
  status: MonitorProposalStatus = "pending",
): Promise<ApiPage<MonitorProposal>> {
  return request<ApiPage<MonitorProposal>>(
    `/api/v1/monitor-proposals${queryString({ limit: 100, status })}`,
  );
}

export async function fetchMonitors(
  state?: MonitorState,
): Promise<{ items: Monitor[] }> {
  return request<{ items: Monitor[] }>(
    `/api/v1/monitors${queryString({ limit: 100, state })}`,
  );
}

export async function fetchMonitorResults(
  monitorId: string,
): Promise<{ items: MonitorResult[] }> {
  return request<{ items: MonitorResult[] }>(
    `/api/v1/monitors/${monitorId}/results?limit=100`,
  );
}

export async function fetchIncidents(
  state?: IncidentState,
): Promise<{ items: Incident[] }> {
  return request<{ items: Incident[] }>(
    `/api/v1/incidents${queryString({ limit: 100, state })}`,
  );
}

export async function fetchIncident(id: string): Promise<Incident> {
  return request<Incident>(`/api/v1/incidents/${id}`);
}

export async function fetchNotificationChannels(): Promise<{
  items: NotificationChannel[];
}> {
  return request<{ items: NotificationChannel[] }>(
    "/api/v1/notification-channels?limit=100",
  );
}

export async function createNotificationChannel(input: {
  name: string;
  provider: NotificationProvider;
  config: Record<string, string>;
}): Promise<NotificationChannel> {
  return request<NotificationChannel>("/api/v1/notification-channels", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function patchNotificationChannel(
  id: string,
  input: {
    name?: string;
    provider?: NotificationProvider;
    config?: Record<string, string | null>;
    enabled?: boolean;
  },
): Promise<NotificationChannel> {
  return request<NotificationChannel>(`/api/v1/notification-channels/${id}`, {
    method: "PATCH",
    body: JSON.stringify(input),
  });
}

export async function deleteNotificationChannel(id: string): Promise<void> {
  await request<unknown>(`/api/v1/notification-channels/${id}`, {
    method: "DELETE",
  });
}

export async function testNotificationChannel(
  id: string,
): Promise<{ status: "sent" }> {
  return request<{ status: "sent" }>(
    `/api/v1/notification-channels/${id}/test`,
    { method: "POST" },
  );
}

export async function fetchNotificationRoutes(): Promise<{
  items: NotificationRoute[];
}> {
  return request<{ items: NotificationRoute[] }>(
    "/api/v1/notification-routes?limit=200",
  );
}

export async function createNotificationRoute(input: {
  channel_id: string;
  min_severity: string;
  event_types: string[];
  delay_seconds: number;
}): Promise<NotificationRoute> {
  return request<NotificationRoute>("/api/v1/notification-routes", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function resolveMonitorProposal(
  id: string,
  decision: "approve" | "reject",
): Promise<MonitorProposal> {
  return request<MonitorProposal>(
    `/api/v1/monitor-proposals/${id}/${decision}`,
    { method: "POST" },
  );
}

export async function createDevice(input: {
  device_type: string;
  name?: string;
  status?: string;
}): Promise<Device> {
  return request<Device>("/api/v1/devices", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function patchDevice(
  id: string,
  input: {
    version: number;
    name?: string | null;
    status?: string;
    device_type?: string;
  },
): Promise<Device> {
  return request<Device>(`/api/v1/devices/${id}`, {
    method: "PATCH",
    body: JSON.stringify(input),
  });
}

export async function mergeDevice(
  absorbedId: string,
  input: { into: string; reason?: string },
): Promise<{ survivor: string; absorbed: string }> {
  return request<{ survivor: string; absorbed: string }>(
    `/api/v1/devices/${absorbedId}/merge`,
    {
      method: "POST",
      body: JSON.stringify(input),
    },
  );
}

export async function splitDevice(
  sourceId: string,
  input: { interface_ids: string[]; device_type: string; name?: string },
): Promise<{ new_device_id: string }> {
  return request<{ new_device_id: string }>(
    `/api/v1/devices/${sourceId}/split`,
    {
      method: "POST",
      body: JSON.stringify(input),
    },
  );
}

export async function undoMerge(
  absorbedId: string,
): Promise<{ restored: string }> {
  return request<{ restored: string }>(
    `/api/v1/devices/${absorbedId}/undo-merge`,
    {
      method: "POST",
    },
  );
}

export async function resolveIdentitySuggestion(
  id: string,
  decision: "confirm" | "reject",
): Promise<{ id: string; status: string }> {
  return request<{ id: string; status: string }>(
    `/api/v1/identity-suggestions/${id}/${decision}`,
    { method: "POST" },
  );
}

export async function setupAdmin(input: {
  email: string;
  password: string;
}): Promise<SetupResponse> {
  return request<SetupResponse>("/api/v1/setup", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function fetchSetupStatus(): Promise<SetupStatusResponse> {
  return request<SetupStatusResponse>("/api/v1/setup");
}

export async function login(input: {
  email: string;
  password: string;
}): Promise<LoginResponse> {
  return request<LoginResponse>("/api/v1/login", {
    method: "POST",
    body: JSON.stringify(input),
  });
}
