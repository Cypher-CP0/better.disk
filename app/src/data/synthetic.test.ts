import { describe, expect, it } from "vitest";
import { bucketForDays, generateLevel, mulberry32, sortItems } from "./synthetic";

describe("synthetic levels", () => {
  it("mulberry32 matches its reference sequence", () => {
    // The Rust shell has the same assertion (synthetic.rs).
    const r = mulberry32(42);
    const first = [r(), r(), r()].map((v) => Math.floor(v * 4294967296));
    expect(first).toEqual([2581720956, 1925393290, 3661312704]);
  });

  it("is deterministic and well-formed", () => {
    const a = generateLevel(7, 500);
    const b = generateLevel(7, 500);
    expect(a).toEqual(b);
    expect(a).toHaveLength(500);
    expect(a.some((i) => i.bytes === 0)).toBe(true);
    expect(a.some((i) => i.bucket === "unknown")).toBe(true);
    expect(new Set(a.map((i) => i.id)).size).toBe(500);
  });

  it("uses inclusive bucket boundaries", () => {
    expect(bucketForDays(7)).toBe("within7Days");
    expect(bucketForDays(8)).toBe("days8To30");
    expect(bucketForDays(365)).toBe("days91To365");
    expect(bucketForDays(366)).toBe("overOneYear");
    expect(bucketForDays(null)).toBe("unknown");
  });

  it("sorts stably by the requested key", () => {
    const items = generateLevel(3, 200);
    const bySize = sortItems(items, "sizeDesc");
    for (let i = 1; i < bySize.length; i++) {
      expect(bySize[i - 1]!.bytes).toBeGreaterThanOrEqual(bySize[i]!.bytes);
    }
    const byDate = sortItems(items, "modifiedDesc");
    expect(byDate[byDate.length - 1]!.modifiedDaysAgo).toBeNull();
  });
});
