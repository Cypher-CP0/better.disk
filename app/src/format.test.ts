import { describe, expect, it } from "vitest";
import { formatBytes } from "./format";

// The same cases as bd_core::bytes tests, so both sides agree.
describe("formatBytes", () => {
  it("matches the Rust formatter", () => {
    const cases: [number, string][] = [
      [0, "0 B"],
      [999, "999 B"],
      [1_000, "1.0 KB"],
      [1_500, "1.5 KB"],
      [1_049, "1.0 KB"],
      [1_050, "1.1 KB"],
      [9_949, "9.9 KB"],
      [9_950, "10 KB"],
      [88_000_000_000, "88 GB"],
      [512_400_000, "512 MB"],
      [999_500, "1.0 MB"],
      [999_499, "999 KB"],
    ];
    for (const [input, expected] of cases) {
      expect(formatBytes(input)).toBe(expected);
    }
  });
});
