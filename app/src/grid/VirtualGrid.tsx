// Virtualized, accessible list of the current level (role="grid").
//
// The list is the accessible equivalent of the map: keyboard-only use,
// screen-reader names, visible focus, sorting. Only rows near the viewport
// exist in the DOM; data arrives in pages of at most 200 rows.
//
// Two focus strategies are implemented so Stage 0 can test both with
// Narrator: a roving tabindex (DOM focus moves to the row) and
// aria-activedescendant (focus stays on the grid, which points at the row).

import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { Item, SortKey } from "../data/model";
import { formatBytes } from "../format";
import { bucket } from "../treemap/colors";
import { PAGE_SIZE, nextIndex, pagesFor, scrollToReveal, visibleRange } from "./virtual";

export type FocusMode = "roving" | "activedescendant";

interface Props {
  count: number;
  /** Changes whenever the underlying rows change (level, sort, filter). */
  dataKey: string;
  fetchPage: (offset: number, limit: number) => Promise<Item[]>;
  sort: SortKey;
  onSortChange: (sort: SortKey) => void;
  selectedId: number | null;
  onSelect: (item: Item, index: number) => void;
  onActivate: (item: Item, index: number) => void;
  focusMode: FocusMode;
  reveal: { index: number; token: number } | null;
  onPageTiming?: (ms: number) => void;
  rowHeight?: number;
}

const COLUMNS: { label: string; sort: SortKey | null; className: string }[] = [
  { label: "Name", sort: "nameAsc", className: "c-name" },
  { label: "Size on disk", sort: "sizeDesc", className: "c-size" },
  { label: "Last modified", sort: "modifiedDesc", className: "c-date" },
  { label: "Type", sort: null, className: "c-type" },
];

function describeAge(item: Item): string {
  const label = bucket(item.bucket).label;
  if (item.modifiedDaysAgo === null) return label;
  const d = item.modifiedDaysAgo;
  const ago = d === 0 ? "today" : d === 1 ? "1 day ago" : `${d.toLocaleString("en-US")} days ago`;
  return item.kind === "folder" ? `${label} (dominant)` : `${label}, ${ago}`;
}

export function VirtualGrid(props: Props) {
  const {
    count,
    dataKey,
    fetchPage,
    sort,
    onSortChange,
    selectedId,
    onSelect,
    onActivate,
    focusMode,
    reveal,
    onPageTiming,
    rowHeight = 32,
  } = props;
  const idPrefix = useId();
  const gridRef = useRef<HTMLDivElement>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const rowRefs = useRef(new Map<number, HTMLDivElement>());
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState(320);
  const [active, setActive] = useState(0);
  const [pages, setPages] = useState(() => new Map<number, Item[]>());
  const inflight = useRef(new Set<string>());
  const keyRef = useRef(dataKey);

  // New data: drop cached pages.
  useEffect(() => {
    keyRef.current = dataKey;
    inflight.current.clear();
    setPages(new Map());
    setActive((a) => Math.min(a, Math.max(0, count - 1)));
  }, [dataKey, count]);

  useLayoutEffect(() => {
    const el = scrollerRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setViewport(el.clientHeight));
    ro.observe(el);
    setViewport(el.clientHeight);
    return () => ro.disconnect();
  }, []);

  const range = visibleRange(scrollTop, viewport, rowHeight, count, 6);

  // Fetch the pages the visible range needs.
  useEffect(() => {
    for (const p of pagesFor(range, PAGE_SIZE)) {
      const flightKey = `${dataKey}#${p}`;
      if (pages.has(p) || inflight.current.has(flightKey)) continue;
      inflight.current.add(flightKey);
      const started = performance.now();
      fetchPage(p * PAGE_SIZE, PAGE_SIZE).then(
        (rows) => {
          inflight.current.delete(flightKey);
          if (keyRef.current !== dataKey) return;
          onPageTiming?.(performance.now() - started);
          setPages((prev) => new Map(prev).set(p, rows));
        },
        () => inflight.current.delete(flightKey),
      );
    }
  }, [range.start, range.end, dataKey, pages, fetchPage, onPageTiming]);

  const rowAt = useCallback(
    (i: number): Item | undefined => pages.get(Math.floor(i / PAGE_SIZE))?.[i % PAGE_SIZE],
    [pages],
  );

  const ensureVisible = useCallback(
    (index: number, center = false) => {
      const el = scrollerRef.current;
      if (!el) return;
      const target = center
        ? Math.max(0, index * rowHeight - (el.clientHeight - rowHeight) / 2)
        : scrollToReveal(index, el.scrollTop, el.clientHeight, rowHeight);
      if (target !== null) {
        el.scrollTop = target;
        setScrollTop(target);
      }
    },
    [rowHeight],
  );

  // Reveal requests from the map (selection sync).
  useEffect(() => {
    if (!reveal || reveal.index < 0) return;
    setActive(reveal.index);
    ensureVisible(reveal.index, true);
  }, [reveal, ensureVisible]);

  // Roving mode: keep DOM focus on the active row while the grid has focus.
  useEffect(() => {
    if (focusMode !== "roving") return;
    const grid = gridRef.current;
    if (!grid || !grid.contains(document.activeElement)) return;
    rowRefs.current.get(active)?.focus({ preventScroll: true });
  }, [active, focusMode, range.start, range.end, pages]);

  const move = (to: number) => {
    setActive(to);
    ensureVisible(to);
    const item = rowAt(to);
    if (item) onSelect(item, to);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    const pageRows = Math.max(1, Math.floor(viewport / rowHeight));
    const to = nextIndex(e.key, active, count, pageRows);
    if (to !== null) {
      e.preventDefault();
      move(to);
      return;
    }
    const item = rowAt(active);
    if (!item) return;
    if (e.key === "Enter") {
      e.preventDefault();
      onActivate(item, active);
    } else if (e.key === " ") {
      e.preventDefault();
      onSelect(item, active);
    }
  };

  const activeRendered = active >= range.start && active < range.end;
  const rowId = (i: number) => `${idPrefix}-row-${i}`;
  const ariaSort = (s: SortKey | null) =>
    s === null || s !== sort ? undefined : s === "nameAsc" ? "ascending" : "descending";

  const rows: React.ReactNode[] = [];
  for (let i = range.start; i < range.end; i++) {
    const item = rowAt(i);
    const isActive = i === active;
    const selected = item !== undefined && item.id === selectedId;
    rows.push(
      <div
        key={i}
        id={rowId(i)}
        role="row"
        aria-rowindex={i + 2}
        aria-selected={item ? selected : undefined}
        tabIndex={focusMode === "roving" ? (isActive ? 0 : -1) : undefined}
        className={`row${selected ? " selected" : ""}${isActive ? " active" : ""}`}
        style={{ top: i * rowHeight, height: rowHeight }}
        ref={(el) => {
          if (el) rowRefs.current.set(i, el);
          else rowRefs.current.delete(i);
        }}
        onClick={() => {
          setActive(i);
          if (item) onSelect(item, i);
        }}
        onDoubleClick={() => item && onActivate(item, i)}
      >
        {item ? (
          <>
            <div role="gridcell" className="c-name">
              <span className={`kind kind-${item.kind}`} aria-hidden="true" />
              {item.name}
            </div>
            <div role="gridcell" className="c-size" title={`${item.bytes.toLocaleString("en-US")} bytes`}>
              {formatBytes(item.bytes)}
            </div>
            <div role="gridcell" className="c-date">
              <span className={`swatch swatch-${item.bucket}`} aria-hidden="true" />
              {describeAge(item)}
            </div>
            <div role="gridcell" className="c-type">
              {item.kind === "folder" ? `Folder, ${item.files.toLocaleString("en-US")} files` : item.fileType}
            </div>
          </>
        ) : (
          <div role="gridcell" className="c-loading">
            Loading…
          </div>
        )}
      </div>,
    );
  }

  return (
    <div
      ref={gridRef}
      role="grid"
      aria-label="Items in this folder"
      aria-rowcount={count + 1}
      aria-colcount={COLUMNS.length}
      aria-busy={rows.length > 0 && range.end > range.start && !rowAt(range.start) ? true : undefined}
      aria-activedescendant={focusMode === "activedescendant" && activeRendered ? rowId(active) : undefined}
      tabIndex={focusMode === "activedescendant" || !activeRendered ? 0 : -1}
      className={`grid mode-${focusMode}`}
      onKeyDown={onKeyDown}
      onFocus={(e) => {
        if (focusMode === "roving" && e.target === gridRef.current) {
          ensureVisible(active);
          requestAnimationFrame(() => rowRefs.current.get(active)?.focus({ preventScroll: true }));
        }
      }}
    >
      <div role="rowgroup" className="header">
        <div role="row" aria-rowindex={1} className="row header-row">
          {COLUMNS.map((c) => (
            <div key={c.label} role="columnheader" className={c.className} aria-sort={ariaSort(c.sort)}>
              {c.sort ? (
                <button type="button" tabIndex={-1} onClick={() => onSortChange(c.sort!)}>
                  {c.label}
                  {c.sort === sort ? (c.sort === "nameAsc" ? " ▲" : " ▼") : ""}
                </button>
              ) : (
                c.label
              )}
            </div>
          ))}
        </div>
      </div>
      <div
        ref={scrollerRef}
        role="presentation"
        className="scroller"
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
      >
        <div role="rowgroup" className="body" style={{ height: count * rowHeight }}>
          {rows}
        </div>
      </div>
    </div>
  );
}
