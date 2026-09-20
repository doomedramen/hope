import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import type { AnchorHTMLAttributes, ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { OverviewDashboard } from "./OverviewDashboard";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ children, ...props }: AnchorHTMLAttributes<HTMLAnchorElement>) => (
    <a {...props}>{children}</a>
  ),
}));

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    headers: { "content-type": "application/json" },
    status,
  });
}

function renderDashboard(children: ReactNode = <OverviewDashboard />) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });

  return render(
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("OverviewDashboard", () => {
  it("shows an unavailable state for needs-attention data when a dashboard query fails", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        if (String(input) === "/api/v1/monitors?limit=100") {
          return Promise.reject(new Error("monitor store unavailable"));
        }
        return Promise.resolve(jsonResponse({ items: [] }));
      }),
    );

    renderDashboard();

    expect(
      await screen.findByText("Needs-attention data unavailable."),
    ).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Retry" })).toHaveLength(2);
  });

  it("renders live inventory, monitor, review, and change data in the overview", async () => {
    const now = Date.now();
    const recent = (minutesAgo: number) =>
      new Date(now - minutesAgo * 60_000).toISOString();

    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input);

        if (path === "/api/v1/devices?limit=100") {
          return Promise.resolve(
            jsonResponse({ items: [{ id: "device-1" }, { id: "device-2" }] }),
          );
        }
        if (path === "/api/v1/networks?limit=100") {
          return Promise.resolve(
            jsonResponse({ items: [{ id: "network-1" }] }),
          );
        }
        if (path === "/api/v1/agents?limit=100") {
          return Promise.resolve(jsonResponse({ items: [{ id: "agent-1" }] }));
        }
        if (path === "/api/v1/identity-suggestions?limit=100") {
          return Promise.resolve(
            jsonResponse({
              items: [
                {
                  id: "suggestion-1",
                  candidate_device_id: "device-1",
                  score: 0.74,
                  explanation: { matched: [{ rule_type: "mac" }] },
                },
              ],
            }),
          );
        }
        if (path === "/api/v1/service-reviews?limit=100&status=pending") {
          return Promise.resolve(
            jsonResponse({
              items: [
                {
                  id: "service-review-1",
                  candidate: { product: "Grafana", confidence: 0.86 },
                },
              ],
            }),
          );
        }
        if (path === "/api/v1/monitor-proposals?limit=100&status=pending") {
          return Promise.resolve(
            jsonResponse({
              items: [
                {
                  id: "proposal-1",
                  target_identity: "grafana.internal",
                  check_type: "http",
                  product: "Grafana",
                },
              ],
            }),
          );
        }
        if (path === "/api/v1/monitors?limit=100") {
          return Promise.resolve(
            jsonResponse({
              items: [
                { id: "monitor-up", state: "up" },
                { id: "monitor-degraded", state: "degraded" },
                { id: "monitor-down", state: "down" },
              ],
            }),
          );
        }
        if (
          path.startsWith("/api/v1/monitors/") &&
          path.endsWith("/results?limit=100")
        ) {
          return Promise.resolve(
            jsonResponse({
              items: [
                { id: `${path}-1`, status: "success", observed_at: recent(20) },
                { id: `${path}-2`, status: "success", observed_at: recent(10) },
              ],
            }),
          );
        }
        if (path === "/api/v1/changes?limit=100") {
          return Promise.resolve(
            jsonResponse({
              items: [
                {
                  id: "change-1",
                  category: "service",
                  entity_kind: "device",
                  entity_id: "device-1",
                  severity: "info",
                  occurred_at: recent(8),
                  evidence_source: "agent",
                },
                {
                  id: "change-2",
                  category: "port",
                  entity_kind: "device",
                  entity_id: "device-2",
                  severity: "warning",
                  occurred_at: recent(5),
                  evidence_source: "network_scan",
                },
              ],
            }),
          );
        }

        return Promise.resolve(jsonResponse({ items: [] }));
      }),
    );

    renderDashboard();

    expect(
      await screen.findByRole("heading", { name: "Infrastructure overview" }),
    ).toBeInTheDocument();
    expect((await screen.findAllByText("2")).length).toBeGreaterThan(0);
    expect(await screen.findByText("Healthy now")).toBeInTheDocument();
    expect((await screen.findAllByText("3")).length).toBeGreaterThan(0);
    expect(
      screen.getByRole("heading", { name: "Needs attention" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Candidate device-1")).toBeInTheDocument();
    expect(screen.getByText("Grafana")).toBeInTheDocument();
    expect(screen.getByText("grafana.internal")).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "Service health" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "Recent changes" }),
    ).toBeInTheDocument();
  });
});
