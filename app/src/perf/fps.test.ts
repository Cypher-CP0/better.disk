import { describe, expect, it } from "vitest";
import { summarizeFrames } from "./fps";

describe("summarizeFrames", () => {
  it("computes fps and percentiles", () => {
    const deltas = new Array(60).fill(1000 / 60);
    deltas[59] = 50;
    const s = summarizeFrames(deltas, new Array(60).fill(2), 1500);
    expect(s.frames).toBe(60);
    expect(s.fps).toBeGreaterThan(55);
    expect(s.fps).toBeLessThan(60);
    expect(s.longFrames).toBe(1);
    expect(s.p50FrameMs).toBeCloseTo(16.67, 1);
    expect(s.drawP95Ms).toBe(2);
  });

  it("handles no frames", () => {
    expect(summarizeFrames([], [], 0).fps).toBe(0);
  });
});
