import { describe, expect, it } from "vitest";
import { BUCKETS, contrast, dominantBucket, luminance, textColorFor } from "./colors";

describe("age shades", () => {
  it("has six buckets from darkest to lightest, then unknown", () => {
    expect(BUCKETS.map((b) => b.id)).toEqual([
      "within7Days",
      "days8To30",
      "days31To90",
      "days91To365",
      "overOneYear",
      "unknown",
    ]);
    const known = BUCKETS.slice(0, 5).map((b) => luminance(b.fill));
    for (let i = 1; i < known.length; i++) {
      expect(known[i]!).toBeGreaterThan(known[i - 1]!);
    }
  });

  it("chooses label text meeting WCAG AA (4.5:1) on every shade", () => {
    for (const b of BUCKETS) {
      expect(contrast(b.fill, textColorFor(b.fill))).toBeGreaterThanOrEqual(4.5);
      if (b.pattern) {
        expect(contrast(b.pattern, textColorFor(b.fill))).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it("computes known contrast ratios", () => {
    expect(contrast("#000000", "#ffffff")).toBeCloseTo(21, 5);
    expect(contrast("#777777", "#777777")).toBeCloseTo(1, 5);
  });
});

describe("dominantBucket", () => {
  it("picks the bucket with the most bytes", () => {
    expect(
      dominantBucket([
        { bucket: "overOneYear", bytes: 10 },
        { bucket: "within7Days", bytes: 5 },
      ]),
    ).toBe("overOneYear");
  });

  it("breaks known ties toward the more recent bucket", () => {
    expect(
      dominantBucket([
        { bucket: "overOneYear", bytes: 5 },
        { bucket: "days8To30", bytes: 5 },
      ]),
    ).toBe("days8To30");
  });

  it("lets Unknown win ties and empty input", () => {
    expect(
      dominantBucket([
        { bucket: "unknown", bytes: 5 },
        { bucket: "days8To30", bytes: 5 },
      ]),
    ).toBe("unknown");
    expect(dominantBucket([])).toBe("unknown");
  });
});
