import "@testing-library/jest-dom/vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  RouterProvider,
  createMemoryHistory,
  createRouter,
} from "@tanstack/react-router";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { routeTree } from "@/routeTree.gen";
import { currentSectionLabel } from "./__root";

function jsonResponse(value: unknown): Response {
  return new Response(JSON.stringify(value), {
    headers: { "content-type": "application/json" },
  });
}

function renderRoot(initialEntry = "/devices") {
  vi.stubGlobal("scrollTo", vi.fn());
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const path = String(input);
      if (path.endsWith("/setup")) {
        return jsonResponse({ setup_required: false });
      }
      return jsonResponse({ items: [], next_cursor: null });
    }),
  );

  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const router = createRouter({
    history: createMemoryHistory({ initialEntries: [initialEntry] }),
    routeTree,
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("Hope navigation", () => {
  it.each([390, 609])(
    "keeps the narrow header as one row at %ipx",
    async (width) => {
      Object.defineProperty(window, "innerWidth", {
        configurable: true,
        value: width,
      });
      renderRoot();

      const header = await screen.findByRole("banner");
      const headerContent = header.firstElementChild;
      expect(headerContent).not.toHaveClass("flex-wrap");
      expect(
        screen.getByRole("button", { name: "Open navigation menu" }),
      ).toHaveTextContent("Menu");
      expect(screen.getByTestId("mobile-current-section")).toHaveTextContent(
        "Devices",
      );
    },
  );

  it("groups mobile links, marks the active item, and restores focus after Escape", async () => {
    renderRoot();
    const trigger = await screen.findByRole("button", {
      name: "Open navigation menu",
    });

    fireEvent.click(trigger);
    const menu = await screen.findByRole("menu");
    expect(menu).toHaveTextContent("Primary");
    expect(menu).toHaveTextContent("Secondary");
    expect(screen.getByRole("menuitem", { name: "Devices" })).toHaveAttribute(
      "aria-current",
      "page",
    );

    fireEvent.keyDown(menu, { key: "Escape" });
    await waitFor(() =>
      expect(screen.queryByRole("menu")).not.toBeInTheDocument(),
    );
    expect(trigger).toHaveFocus();
  });

  it("closes the menu when a destination is chosen", async () => {
    renderRoot();
    const trigger = await screen.findByRole("button", {
      name: "Open navigation menu",
    });
    fireEvent.click(trigger);
    const monitoring = await screen.findByRole("menuitem", {
      name: "Monitoring",
    });

    fireEvent.click(monitoring);
    await waitFor(() =>
      expect(screen.queryByRole("menu")).not.toBeInTheDocument(),
    );
  });

  it.each([
    ["/networks", "Networks"],
    ["/agents", "Agents"],
    ["/changes", "Activity"],
    ["/settings", "Settings"],
    ["/maintenance", "Maintenance"],
  ])("names the current section for %s", (pathname, label) => {
    expect(currentSectionLabel(pathname)).toBe(label);
  });
});
