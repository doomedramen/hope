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
