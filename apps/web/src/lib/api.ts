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

export type ScanRunKind = "initial_discovery" | "change_scan" | "full_tcp";

export type ScanRunStatus =
  "pending" | "running" | "succeeded" | "failed" | "cancelled";

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

export interface LoginResponse {
  status: "ok";
}

export interface SetupResponse {
  email: string;
}

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
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
  const body = contentType.includes("json")
    ? ((await res.json()) as T & { error?: string })
    : null;
  if (!res.ok) {
    throw new ApiError(
      res.status,
      body?.error ?? `Request failed (${res.status})`,
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

export async function login(input: {
  email: string;
  password: string;
}): Promise<LoginResponse> {
  return request<LoginResponse>("/api/v1/login", {
    method: "POST",
    body: JSON.stringify(input),
  });
}
