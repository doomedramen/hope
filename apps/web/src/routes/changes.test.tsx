import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { AnchorHTMLAttributes } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ChangesPage } from "./changes";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ children, ...props }: AnchorHTMLAttributes<HTMLAnchorElement>) => (
    <a {...props}>{children}</a>
  ),
  createFileRoute: () => () => ({}),
}));

function jsonResponse(value: unknown): Response {
  return new Response(JSON.stringify(value), {
    headers: { "content-type": "application/json" },
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("ChangesPage", () => {
  it("humanizes the selected category in the trigger", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const path = String(input);
        if (path.includes("/api/v1/changes")) {
          return Promise.resolve(
            jsonResponse({
              items: [
                {
                  id: "change-1",
                  entity_kind: "monitor",
                  entity_id: "monitor-1",
                  category: "monitor.created",
                  severity: "notice",
                  before: null,
                  after: { state: "up" },
                  evidence_source: "agent",
                  occurred_at: "2026-09-20T10:00:00Z",
                  acknowledged: false,
                },
              ],
            }),
          );
        }
        return Promise.resolve(jsonResponse({ items: [] }));
      }),
    );

    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <QueryClientProvider client={queryClient}>
        <ChangesPage />
      </QueryClientProvider>,
    );

    const trigger = await screen.findByRole("combobox", {
      name: "Filter changes by category",
    });
    fireEvent.click(trigger);
    const option = await screen.findByRole("option", {
      name: "Monitor.Created",
    });
    fireEvent.pointerDown(option, { pointerType: "mouse" });
    fireEvent.click(option, { detail: 1 });

    await waitFor(() => expect(trigger).toHaveTextContent("Monitor.Created"));
  });
});
