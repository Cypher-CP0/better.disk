import { describe, expect, it } from "vitest";
import {
  type Rect,
  MAX_RECTS,
  SMALLER_ITEMS_KEY,
  hitTest,
  layoutLevel,
  ordinalCompare,
  squarify,
} from "./squarify";

interface Item {
  name: string;
  bytes: number;
}

const bounds: Rect = { x: 0, y: 0, w: 800, h: 500 };
const area = (r: Rect) => r.w * r.h;
const overlap = (a: Rect, b: Rect) =>
  Math.max(0, Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x)) *
  Math.max(0, Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y));

function items(values: number[]): Item[] {
  return values.map((bytes, i) => ({ name: `item-${String(i).padStart(5, "0")}`, bytes }));
}

function layout(list: Item[], b: Rect = bounds, opts = {}) {
  return layoutLevel(
    list,
    (t) => t.bytes,
    (t) => t.name,
    b,
    opts,
  );
}

describe("squarify", () => {
  it("areas are proportional and tile the bounds", () => {
    const values = [500, 300, 120, 80, 60, 30, 10, 5, 3, 1];
    const rects = squarify(values, bounds);
    const total = values.reduce((a, b) => a + b, 0);
    rects.forEach((r, i) => {
      expect(area(r)).toBeCloseTo((values[i]! / total) * area(bounds), 6);
      expect(r.x).toBeGreaterThanOrEqual(-1e-9);
      expect(r.y).toBeGreaterThanOrEqual(-1e-9);
      expect(r.x + r.w).toBeLessThanOrEqual(bounds.w + 1e-9);
      expect(r.y + r.h).toBeLessThanOrEqual(bounds.h + 1e-9);
    });
    const sum = rects.reduce((a, r) => a + area(r), 0);
    expect(sum).toBeCloseTo(area(bounds), 6);
    for (let i = 0; i < rects.length; i++) {
      for (let j = i + 1; j < rects.length; j++) {
        expect(overlap(rects[i]!, rects[j]!)).toBeLessThan(1e-6);
      }
    }
  });

  it("equal values in a square give near-square blocks", () => {
    const rects = squarify(new Array(16).fill(1), { x: 0, y: 0, w: 400, h: 400 });
    for (const r of rects) {
      expect(Math.max(r.w / r.h, r.h / r.w)).toBeLessThan(2);
    }
  });

  it("handles empty input and empty bounds", () => {
    expect(squarify([], bounds)).toEqual([]);
    expect(squarify([1, 2], { x: 0, y: 0, w: 0, h: 10 })).toHaveLength(2);
  });
});

describe("layoutLevel", () => {
  it("orders by size, then name, independent of input order", () => {
    const a = items([10, 10, 5, 20]);
    const b = [...a].reverse();
    const la = layout(a);
    const lb = layout(b);
    expect(la.blocks.map((x) => x.key)).toEqual(lb.blocks.map((x) => x.key));
    expect(la.blocks.map((x) => x.key)).toEqual([
      "item-00003",
      "item-00000",
      "item-00001",
      "item-00002",
    ]);
    expect(la.blocks.map((x) => x.rect)).toEqual(lb.blocks.map((x) => x.rect));
  });

  it("caps at 1,500 rectangles with one Smaller items block", () => {
    // Equal values, so only the cap (not block size) forces aggregation.
    const many = items(new Array(5000).fill(1));
    const l = layout(many, { x: 0, y: 0, w: 4000, h: 4000 });
    expect(l.blocks.length).toBe(MAX_RECTS);
    const smaller = l.blocks.filter((b) => b.item === null);
    expect(smaller).toHaveLength(1);
    expect(smaller[0]!.count).toBe(5000 - (MAX_RECTS - 1));
    expect(smaller[0]!.value).toBe(5000 - (MAX_RECTS - 1));
    expect(l.smaller).toHaveLength(5000 - (MAX_RECTS - 1));
    expect(smaller[0]!.key).toBe(SMALLER_ITEMS_KEY);
  });

  it("aggregates blocks too small to select even below the cap", () => {
    const list = items([1_000_000, 1_000_000, 1, 1, 1]);
    const l = layout(list, { x: 0, y: 0, w: 200, h: 100 });
    expect(l.blocks.filter((b) => b.item !== null)).toHaveLength(2);
    expect(l.smaller.map((s) => s.name)).toEqual(["item-00002", "item-00003", "item-00004"]);
  });

  it("excludes zero-byte and invalid values from the map", () => {
    const list = items([0, 5, -1, Number.NaN, 7]);
    const l = layout(list);
    expect(l.blocks).toHaveLength(2);
    expect(l.excluded).toHaveLength(3);
    expect(l.total).toBe(12);
  });

  it("insets drawn rectangles by the gutter without changing exact areas", () => {
    const l = layout(items([60, 40]));
    for (const b of l.blocks) {
      expect(b.inner.w).toBeCloseTo(b.rect.w - 2, 9);
      expect(b.inner.x).toBeCloseTo(b.rect.x + 1, 9);
    }
  });

  it("hit-tests the exact rectangles", () => {
    const l = layout(items([60, 40]));
    const first = l.blocks[0]!;
    const hit = hitTest(l.blocks, first.rect.x + 1, first.rect.y + 1);
    expect(hit?.key).toBe(first.key);
    expect(hitTest(l.blocks, -5, -5)).toBeNull();
  });
});

describe("ordinalCompare", () => {
  it("compares UTF-16 code units, not locale order", () => {
    expect(ordinalCompare("B", "a")).toBeLessThan(0);
    // U+1F600 (surrogate pair D83D DE00) sorts before U+FF5E in UTF-16.
    expect(ordinalCompare("\u{1F600}", "～")).toBeLessThan(0);
  });
});
