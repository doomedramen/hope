import { test, expect } from "@playwright/test";
import { api } from "../lib/api";

test("API requests preserve every supported header shape without duplicate content types", async () => {
  const original = globalThis.fetch;
  const seen: Headers[] = [];
  globalThis.fetch = async (_input, options) => {
    seen.push(new Headers(options?.headers));
    return new Response(JSON.stringify({ ok: true }));
  };
  try {
    for (const headers of [
      new Headers({
        "X-Fixture": "native",
        "content-type": "application/problem+json",
      }),
      [["X-Fixture", "tuple"]] as [string, string][],
      { "X-Fixture": "object" },
    ])
      await api("/fixture", { headers });
    expect(seen.map((headers) => headers.get("X-Fixture"))).toEqual([
      "native",
      "tuple",
      "object",
    ]);
    expect(seen.map((headers) => headers.get("Content-Type"))).toEqual([
      "application/problem+json",
      "application/json",
      "application/json",
    ]);
  } finally {
    globalThis.fetch = original;
  }
});

test("unexpected proxy error bodies retain the HTTP failure instead of throwing a parser error", async () => {
  const original = globalThis.fetch;
  try {
    for (const body of [
      "null",
      "[]",
      '{"error":{}}',
      '"denied"',
      "upstream failure",
    ]) {
      globalThis.fetch = async () => new Response(body, { status: 502 });
      await expect(api("/fixture")).rejects.toMatchObject({
        message: "Request failed (502)",
        status: 502,
      });
    }
  } finally {
    globalThis.fetch = original;
  }
});

test("API deadlines cover a stalled connection and allow the next request to succeed", async () => {
  const original = globalThis.fetch;
  let aborted = false;
  globalThis.fetch = async (_input, options) =>
    new Promise((_resolve, reject) => {
      options?.signal?.addEventListener("abort", () => {
        aborted = true;
        reject(options.signal?.reason);
      });
    });
  try {
    await expect(api("/fixture", { timeoutMs: 20 })).rejects.toThrow(
      "The server took too long to respond",
    );
    expect(aborted).toBe(true);
    globalThis.fetch = async () => new Response('{"ok":true}');
    await expect(api("/fixture")).resolves.toEqual({ ok: true });
  } finally {
    globalThis.fetch = original;
  }
});

test("API deadlines cover stalled response bodies too", async () => {
  const original = globalThis.fetch;
  globalThis.fetch = async (_input, options) =>
    new Response(
      new ReadableStream({
        start(controller) {
          controller.enqueue(new TextEncoder().encode('{"partial":'));
          options?.signal?.addEventListener("abort", () =>
            controller.error(options.signal?.reason),
          );
        },
      }),
    );
  try {
    await expect(api("/fixture", { timeoutMs: 20 })).rejects.toThrow(
      "The server took too long to respond",
    );
  } finally {
    globalThis.fetch = original;
  }
});

test("API callers can cancel an active request and retain their cancellation reason", async () => {
  const original = globalThis.fetch;
  globalThis.fetch = async (_input, options) =>
    new Promise((_resolve, reject) => {
      if (options?.signal?.aborted) reject(options.signal.reason);
      else
        options?.signal?.addEventListener("abort", () =>
          reject(options.signal?.reason),
        );
    });
  try {
    const controller = new AbortController();
    const pending = api("/fixture", { signal: controller.signal });
    controller.abort(new Error("View was closed"));
    await expect(pending).rejects.toThrow("View was closed");
    await expect(
      api("/fixture", { signal: controller.signal }),
    ).rejects.toThrow("View was closed");
  } finally {
    globalThis.fetch = original;
  }
});
