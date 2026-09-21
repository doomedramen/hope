import { describe, expect, it } from "vitest";
import { labelize } from "./format";

describe("labelize", () => {
  it("keeps formatting safe for missing API values", () => {
    expect(labelize(undefined)).toBe("Unknown");
    expect(labelize(null)).toBe("Unknown");
    expect(labelize("agent_metric.sample")).toBe("Agent Metric Sample");
  });
});
