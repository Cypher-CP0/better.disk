// Squarified treemap layout (Bruls, Huizing and van Wijk) with the
// specification's display rules:
//
// * Stable order: size descending, then key in ordinal (code unit) order,
//   so the same data always produces the same picture.
// * At most 1,500 rectangles. Items beyond that, and items whose area would
//   be too small to select, are aggregated into one "Smaller items" block
//   that opens the corresponding list.
// * Zero-byte items are not drawn (they have no area); the list shows them.
// * Subtle 2 px gutters are applied after layout, so the areas computed
//   here stay exactly proportional to the values.

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export const MAX_RECTS = 1500;
/** Smallest block, in CSS px², that can still be pointed at and selected. */
export const MIN_BLOCK_AREA = 36;
export const GUTTER = 2;

export const SMALLER_ITEMS_KEY = "\u{10FFFF}smaller-items";

export interface Block<T> {
  /** `null` for the Smaller items block. */
  item: T | null;
  key: string;
  value: number;
  /** For the Smaller items block: how many items it stands for. */
  count: number;
  /** Exact, proportional rectangle. */
  rect: Rect;
  /** Rectangle to draw: `rect` inset by half the gutter on each side. */
  inner: Rect;
}

export interface Layout<T> {
  blocks: Block<T>[];
  /** Items aggregated into the Smaller items block, in display order. */
  smaller: T[];
  /** Items with no positive value (listed, never drawn). */
  excluded: T[];
  total: number;
}

export interface LayoutOptions {
  maxRects?: number;
  minBlockArea?: number;
  gutter?: number;
}

/** Ordinal comparison of UTF-16 code units, like the Rust side on Windows. */
export function ordinalCompare(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Size descending, then key ascending. */
export function compareEntries(
  a: { value: number; key: string },
  b: { value: number; key: string },
): number {
  return b.value - a.value || ordinalCompare(a.key, b.key);
}

/** Worst aspect ratio in a row of areas laid along a side of length `side`. */
function worst(sum: number, min: number, max: number, side: number): number {
  const s2 = sum * sum;
  const w2 = side * side;
  return Math.max((w2 * max) / s2, s2 / (w2 * min));
}

/**
 * Lay out positive `values` (already sorted descending) in `bounds`.
 * Returns one rectangle per value, in the same order, whose areas are
 * proportional to the values and which exactly tile `bounds`.
 */
export function squarify(values: readonly number[], bounds: Rect): Rect[] {
  const out: Rect[] = [];
  const n = values.length;
  if (n === 0 || bounds.w <= 0 || bounds.h <= 0) {
    return values.map(() => ({ x: bounds.x, y: bounds.y, w: 0, h: 0 }));
  }
  let total = 0;
  for (const v of values) total += v;
  const scale = (bounds.w * bounds.h) / total;
  const areas = values.map((v) => v * scale);

  let { x, y, w, h } = bounds;
  let i = 0;
  while (i < n) {
    const side = Math.min(w, h);
    let sum = areas[i]!;
    let min = sum;
    let max = sum;
    let ratio = worst(sum, min, max, side);
    let j = i + 1;
    while (j < n) {
      const a = areas[j]!;
      const nextSum = sum + a;
      const nextRatio = worst(nextSum, Math.min(min, a), Math.max(max, a), side);
      if (nextRatio > ratio) break;
      sum = nextSum;
      min = Math.min(min, a);
      max = Math.max(max, a);
      ratio = nextRatio;
      j++;
    }
    const last = j === n;
    if (w >= h) {
      // Column along the left edge.
      const colW = last ? w : sum / h;
      let cy = y;
      for (let k = i; k < j; k++) {
        const rh = k === j - 1 ? y + h - cy : areas[k]! / colW;
        out.push({ x, y: cy, w: colW, h: rh });
        cy += rh;
      }
      x += colW;
      w -= colW;
    } else {
      // Row along the top edge.
      const rowH = last ? h : sum / w;
      let cx = x;
      for (let k = i; k < j; k++) {
        const rw = k === j - 1 ? x + w - cx : areas[k]! / rowH;
        out.push({ x: cx, y, w: rw, h: rowH });
        cx += rw;
      }
      y += rowH;
      h -= rowH;
    }
    i = j;
  }
  return out;
}

function inset(r: Rect, gutter: number): Rect {
  const g = gutter / 2;
  if (r.w <= gutter || r.h <= gutter) return r;
  return { x: r.x + g, y: r.y + g, w: r.w - gutter, h: r.h - gutter };
}

/** Full layout for one navigation level. */
export function layoutLevel<T>(
  items: readonly T[],
  value: (t: T) => number,
  key: (t: T) => string,
  bounds: Rect,
  options: LayoutOptions = {},
): Layout<T> {
  const maxRects = Math.max(1, options.maxRects ?? MAX_RECTS);
  const minArea = options.minBlockArea ?? MIN_BLOCK_AREA;
  const gutter = options.gutter ?? GUTTER;

  const excluded: T[] = [];
  const entries: { item: T; value: number; key: string }[] = [];
  for (const item of items) {
    const v = value(item);
    if (v > 0 && Number.isFinite(v)) entries.push({ item, value: v, key: key(item) });
    else excluded.push(item);
  }
  entries.sort(compareEntries);

  let total = 0;
  for (const e of entries) total += e.value;
  const area = Math.max(0, bounds.w) * Math.max(0, bounds.h);

  // Keep the largest items that fit the cap and are big enough to select.
  const limit = entries.length <= maxRects ? entries.length : maxRects - 1;
  let cut = limit;
  for (let i = 0; i < limit; i++) {
    if ((entries[i]!.value / total) * area < minArea) {
      cut = i;
      break;
    }
  }
  const shown = entries.slice(0, cut);
  const rest = entries.slice(cut);

  const drawn: { item: T | null; value: number; key: string; count: number }[] = shown.map(
    (e) => ({ ...e, count: 1 }),
  );
  if (rest.length > 0) {
    let restValue = 0;
    for (const e of rest) restValue += e.value;
    drawn.push({ item: null, value: restValue, key: SMALLER_ITEMS_KEY, count: rest.length });
    drawn.sort(compareEntries);
  }

  const rects = squarify(
    drawn.map((d) => d.value),
    bounds,
  );
  const blocks: Block<T>[] = drawn.map((d, i) => {
    const rect = rects[i]!;
    return { ...d, rect, inner: inset(rect, gutter) };
  });
  return { blocks, smaller: rest.map((e) => e.item), excluded, total };
}

/** The block under a point, if any. Linear: 1,500 blocks is cheap. */
export function hitTest<T>(blocks: readonly Block<T>[], px: number, py: number): Block<T> | null {
  for (const b of blocks) {
    const r = b.rect;
    if (px >= r.x && px < r.x + r.w && py >= r.y && py < r.y + r.h) return b;
  }
  return null;
}
