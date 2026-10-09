// Pure helpers for the virtualized list. The list is the accessible
// equivalent of the map, so it must stay fast at any size: only visible rows
// (plus overscan) are in the DOM, and rows are fetched in pages of at most
// 200 items (specification FR04).

export const PAGE_SIZE = 200;

export interface Range {
  /** First row index, inclusive. */
  start: number;
  /** Last row index, exclusive. */
  end: number;
}

export function visibleRange(
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  count: number,
  overscan = 8,
): Range {
  if (count <= 0 || rowHeight <= 0) return { start: 0, end: 0 };
  const first = Math.floor(Math.max(0, scrollTop) / rowHeight);
  const visible = Math.ceil(Math.max(0, viewportHeight) / rowHeight) + 1;
  const start = Math.max(0, first - overscan);
  const end = Math.min(count, first + visible + overscan);
  return { start: Math.min(start, end), end };
}

/** Page numbers covering a row range. */
export function pagesFor(range: Range, pageSize = PAGE_SIZE): number[] {
  if (range.end <= range.start) return [];
  const first = Math.floor(range.start / pageSize);
  const last = Math.floor((range.end - 1) / pageSize);
  const out: number[] = [];
  for (let p = first; p <= last; p++) out.push(p);
  return out;
}

/** The scrollTop that brings `index` fully into view, or null if it is. */
export function scrollToReveal(
  index: number,
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
): number | null {
  const top = index * rowHeight;
  const bottom = top + rowHeight;
  if (top < scrollTop) return top;
  if (bottom > scrollTop + viewportHeight) return bottom - viewportHeight;
  return null;
}

/** Keyboard navigation for a single-column-focus grid. */
export function nextIndex(
  key: string,
  current: number,
  count: number,
  pageRows: number,
): number | null {
  if (count <= 0) return null;
  const clamp = (i: number) => Math.min(count - 1, Math.max(0, i));
  switch (key) {
    case "ArrowDown":
      return clamp(current + 1);
    case "ArrowUp":
      return clamp(current - 1);
    case "PageDown":
      return clamp(current + Math.max(1, pageRows - 1));
    case "PageUp":
      return clamp(current - Math.max(1, pageRows - 1));
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}
