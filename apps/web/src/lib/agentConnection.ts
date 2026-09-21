const STORAGE_KEY = "hope.agent.connection-address";

export function agentConnectionAddress(): string {
  if (typeof window === "undefined") return "https://hope.example";
  try {
    return window.localStorage.getItem(STORAGE_KEY) || window.location.origin;
  } catch {
    return window.location.origin;
  }
}

export function saveAgentConnectionAddress(address: string): string {
  const url = new URL(address.trim());
  if (
    !["http:", "https:"].includes(url.protocol) ||
    url.username ||
    url.password ||
    url.pathname !== "/" ||
    url.search ||
    url.hash
  ) {
    throw new Error("Use a Hope HTTP or HTTPS address without a path.");
  }
  const origin = url.origin;
  window.localStorage.setItem(STORAGE_KEY, origin);
  return origin;
}
