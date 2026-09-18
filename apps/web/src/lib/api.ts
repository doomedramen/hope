export interface ReadyResponse {
  status: "ready" | "not_ready";
  error?: string;
}

export async function fetchHealthReady(): Promise<ReadyResponse> {
  const res = await fetch("/health/ready");
  const body = (await res.json()) as ReadyResponse;
  if (!res.ok && !body.status) {
    throw new Error(`health check failed: ${res.status}`);
  }
  return body;
}
