import { describe, expect, it } from "vitest";
import { PAGE_SIZE, nextIndex, pagesFor, scrollToReveal, visibleRange } from "./virtual";

describe("visibleRange", () => {
  it("covers the viewport plus overscan, clamped to the list", () => {
    expect(visibleRange(0, 320, 32, 1000, 4)).toEqual({ start: 0, end: 15 });
    expect(visibleRange(3200, 320, 32, 1000, 4)).toEqual({ start: 96, end: 115 });
    expect(visibleRange(31_900, 320, 32, 1000, 4)).toEqual({ start: 992, end: 1000 });
  });

  it("handles empty lists", () => {
    expect(visibleRange(0, 320, 32, 0)).toEqual({ start: 0, end: 0 });
  });
});

describe("pagesFor", () => {
  it("never asks for more than 200 rows per page", () => {
    expect(PAGE_SIZE).toBeLessThanOrEqual(200);
    expect(pagesFor({ start: 0, end: 15 })).toEqual([0]);
    expect(pagesFor({ start: 190, end: 215 })).toEqual([0, 1]);
    expect(pagesFor({ start: 5, end: 5 })).toEqual([]);
  });
});

describe("scrollToReveal", () => {
  it("scrolls only when the row is outside the viewport", () => {
    expect(scrollToReveal(5, 0, 320, 32)).toBeNull();
    expect(scrollToReveal(20, 0, 320, 32)).toBe(21 * 32 - 320);
    expect(scrollToReveal(2, 320, 320, 32)).toBe(64);
  });
});

describe("nextIndex", () => {
  it("moves and clamps", () => {
    expect(nextIndex("ArrowDown", 0, 10, 5)).toBe(1);
    expect(nextIndex("ArrowUp", 0, 10, 5)).toBe(0);
    expect(nextIndex("PageDown", 0, 10, 5)).toBe(4);
    expect(nextIndex("End", 0, 10, 5)).toBe(9);
    expect(nextIndex("Home", 7, 10, 5)).toBe(0);
    expect(nextIndex("x", 3, 10, 5)).toBeNull();
    expect(nextIndex("ArrowDown", 0, 0, 5)).toBeNull();
  });
});
