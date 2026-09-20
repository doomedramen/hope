import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SettingsPage } from "./settings";

function jsonResponse(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    headers: { "content-type": "application/json" },
    status,
  });
}

const channel = {
  id: "channel-1",
  name: "Ops webhook",
  provider: "webhook",
  config: { url: "https://hooks.example.test/hope", token: "[redacted]" },
  enabled: true,
  created_at: "2026-09-20T10:00:00Z",
  updated_at: "2026-09-20T10:00:00Z",
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("SettingsPage notification channels", () => {
  it("exposes channel lifecycle actions and prefills the edit form", async () => {
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const path = String(input);
      if (path.startsWith("/api/v1/notification-channels")) {
        return Promise.resolve(jsonResponse({ items: [channel] }));
      }
      return Promise.resolve(jsonResponse({ items: [] }));
    });
    vi.stubGlobal("fetch", fetchMock);
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });

    render(
      <QueryClientProvider client={queryClient}>
        <SettingsPage />
      </QueryClientProvider>,
    );

    expect(await screen.findByText("Ops webhook")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Edit Ops webhook" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Disable Ops webhook" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Test Ops webhook" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Delete Ops webhook" }),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Edit Ops webhook" }));

    expect(
      screen.getByRole("button", { name: "Save changes" }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Name")).toHaveValue("Ops webhook");
    expect(screen.getByLabelText("URL")).toHaveValue(
      "https://hooks.example.test/hope",
    );
    expect(
      screen.getByText("Leave blank to keep the saved token unchanged."),
    ).toBeInTheDocument();
  });
});
