// Canvas treemap for one navigation level.
//
// The canvas is a visual aid only: every item is also reachable through the
// list (role="grid") below it, which is the accessible equivalent. Drawing
// is in CSS pixels scaled by devicePixelRatio, so blocks stay sharp at 100
// to 200 percent Windows scaling.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Item } from "../data/model";
import { formatBytes } from "../format";
import { type FrameStats, summarizeFrames } from "../perf/fps";
import { bucket, dominantBucket, textColorFor } from "./colors";
import { type Block, type Layout, hitTest, layoutLevel } from "./squarify";

export type MapSelection = { kind: "item"; id: number } | { kind: "smaller" } | null;

interface Props {
  items: Item[];
  selection: MapSelection;
  onSelect: (sel: MapSelection, smaller: Item[]) => void;
  onActivate: (sel: MapSelection, smaller: Item[]) => void;
  /** Continuously move a synthetic pointer and redraw every frame. */
  stress: boolean;
  onFrameStats: (stats: FrameStats) => void;
  onLayout?: (info: { blocks: number; smaller: number; excluded: number; layoutMs: number }) => void;
}

interface Theme {
  selection: string;
  selectionInner: string;
  hover: string;
  background: string;
  forced: boolean;
}

function readTheme(el: Element): Theme {
  const css = getComputedStyle(el);
  const forced = window.matchMedia("(forced-colors: active)").matches;
  return {
    selection: css.getPropertyValue("--map-selection").trim() || "#0b57d0",
    selectionInner: css.getPropertyValue("--map-selection-inner").trim() || "#ffffff",
    hover: css.getPropertyValue("--map-hover").trim() || "#111111",
    background: css.getPropertyValue("--surface").trim() || "#ffffff",
    forced,
  };
}

function keyOf(i: Item): string {
  // Name first (display order), id to keep keys unique.
  return `${i.name}\u0000${i.id}`;
}

function blockSelected(b: Block<Item>, sel: MapSelection): boolean {
  if (!sel) return false;
  if (sel.kind === "smaller") return b.item === null;
  return b.item !== null && b.item.id === sel.id;
}

const FONT = "12px 'Segoe UI', system-ui, sans-serif";

export function TreemapCanvas(props: Props) {
  const { items, selection, onSelect, onActivate, stress, onFrameStats, onLayout } = props;
  const containerRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0, dpr: 1 });
  const hoverRef = useRef<string | null>(null);
  const labelCache = useRef(new Map<string, string>());
  const patternRef = useRef<CanvasPattern | null>(null);
  const themeRef = useRef<Theme | null>(null);

  useLayoutEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const update = () => {
      const r = el.getBoundingClientRect();
      setSize({ w: Math.floor(r.width), h: Math.floor(r.height), dpr: window.devicePixelRatio || 1 });
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    // devicePixelRatio changes (moving between monitors, zoom) do not resize.
    const mq = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    mq.addEventListener("change", update);
    return () => {
      ro.disconnect();
      mq.removeEventListener("change", update);
    };
  }, []);

  const layoutMs = useRef(0);
  const layout: Layout<Item> = useMemo(() => {
    const t = performance.now();
    const l = layoutLevel(items, (i) => i.bytes, keyOf, { x: 0, y: 0, w: size.w, h: size.h });
    labelCache.current.clear();
    layoutMs.current = performance.now() - t;
    return l;
  }, [items, size.w, size.h]);

  useEffect(() => {
    onLayout?.({
      blocks: layout.blocks.length,
      smaller: layout.smaller.length,
      excluded: layout.excluded.length,
      layoutMs: layoutMs.current,
    });
  }, [layout, onLayout]);

  const smallerBucket = useMemo(() => bucket(dominantBucket(layout.smaller)), [layout]);

  const truncate = useCallback((ctx: CanvasRenderingContext2D, text: string, max: number, key: string) => {
    const cacheKey = `${key}|${Math.floor(max)}`;
    const hit = labelCache.current.get(cacheKey);
    if (hit !== undefined) return hit;
    let out = text;
    if (ctx.measureText(text).width > max) {
      let lo = 0;
      let hi = text.length;
      while (lo < hi) {
        const mid = (lo + hi + 1) >> 1;
        if (ctx.measureText(`${text.slice(0, mid)}…`).width <= max) lo = mid;
        else hi = mid - 1;
      }
      out = lo > 0 ? `${text.slice(0, lo)}…` : "";
    }
    labelCache.current.set(cacheKey, out);
    return out;
  }, []);

  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas || size.w === 0 || size.h === 0) return;
    const ctx = canvas.getContext("2d", { alpha: false });
    if (!ctx) return;
    if (!themeRef.current && containerRef.current) themeRef.current = readTheme(containerRef.current);
    const theme = themeRef.current!;
    const { dpr } = size;
    if (canvas.width !== Math.round(size.w * dpr) || canvas.height !== Math.round(size.h * dpr)) {
      canvas.width = Math.round(size.w * dpr);
      canvas.height = Math.round(size.h * dpr);
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = theme.forced ? "Canvas" : theme.background;
    ctx.fillRect(0, 0, size.w, size.h);

    if (!patternRef.current) {
      const p = document.createElement("canvas");
      p.width = 8;
      p.height = 8;
      const pc = p.getContext("2d")!;
      pc.strokeStyle = bucket("unknown").pattern!;
      pc.lineWidth = 2;
      pc.beginPath();
      pc.moveTo(-2, 10);
      pc.lineTo(10, -2);
      pc.stroke();
      patternRef.current = ctx.createPattern(p, "repeat");
    }

    ctx.font = FONT;
    ctx.textBaseline = "top";
    let selected: Block<Item> | null = null;
    let hovered: Block<Item> | null = null;
    for (const b of layout.blocks) {
      const r = b.inner;
      if (r.w <= 0 || r.h <= 0) continue;
      const bk = b.item ? bucket(b.item.bucket) : smallerBucket;
      const radius = r.w > 6 && r.h > 6 ? 2 : 0;
      ctx.beginPath();
      ctx.roundRect(r.x, r.y, r.w, r.h, radius);
      if (theme.forced) {
        ctx.fillStyle = "Canvas";
        ctx.fill();
        ctx.strokeStyle = "CanvasText";
        ctx.lineWidth = 1;
        ctx.stroke();
      } else {
        ctx.fillStyle = bk.fill;
        ctx.fill();
        if (bk.pattern && patternRef.current) {
          ctx.fillStyle = patternRef.current;
          ctx.fill();
        }
      }
      if (r.w >= 56 && r.h >= 30) {
        const title = b.item ? b.item.name : `Smaller items (${b.count.toLocaleString("en-US")})`;
        const sub = theme.forced ? `${formatBytes(b.value)} · ${bk.label}` : formatBytes(b.value);
        ctx.fillStyle = theme.forced ? "CanvasText" : textColorFor(bk.fill);
        ctx.fillText(truncate(ctx, title, r.w - 12, b.key), r.x + 6, r.y + 6);
        if (r.h >= 46) ctx.fillText(truncate(ctx, sub, r.w - 12, `${b.key}#sub`), r.x + 6, r.y + 22);
      }
      if (blockSelected(b, selection)) selected = b;
      if (hoverRef.current === b.key) hovered = b;
    }
    if (hovered && hovered !== selected) {
      const r = hovered.inner;
      ctx.setLineDash([4, 3]);
      ctx.lineWidth = 1.5;
      ctx.strokeStyle = theme.forced ? "Highlight" : theme.hover;
      ctx.strokeRect(r.x + 0.75, r.y + 0.75, r.w - 1.5, r.h - 1.5);
      ctx.setLineDash([]);
    }
    if (selected) {
      // Selection is an outline independent of the age shade.
      const r = selected.rect;
      ctx.lineWidth = 3;
      ctx.strokeStyle = theme.forced ? "Highlight" : theme.selection;
      ctx.strokeRect(r.x + 1.5, r.y + 1.5, Math.max(0, r.w - 3), Math.max(0, r.h - 3));
      if (!theme.forced && r.w > 8 && r.h > 8) {
        ctx.lineWidth = 1;
        ctx.strokeStyle = theme.selectionInner;
        ctx.strokeRect(r.x + 3.5, r.y + 3.5, r.w - 7, r.h - 7);
      }
    }
  }, [layout, selection, size, smallerBucket, truncate]);

  useEffect(() => {
    draw();
  }, [draw]);

  // Re-read theme colors when the system theme or high contrast changes.
  useEffect(() => {
    const refresh = () => {
      if (containerRef.current) themeRef.current = readTheme(containerRef.current);
      draw();
    };
    const queries = ["(prefers-color-scheme: dark)", "(forced-colors: active)"].map((q) =>
      window.matchMedia(q),
    );
    for (const q of queries) q.addEventListener("change", refresh);
    return () => {
      for (const q of queries) q.removeEventListener("change", refresh);
    };
  }, [draw]);

  // Stress mode: a synthetic pointer sweeps the map; every frame does a
  // hit test and a full redraw of up to 1,500 blocks.
  useEffect(() => {
    if (!stress) return;
    let raf = 0;
    let last = performance.now();
    const deltas: number[] = [];
    const draws: number[] = [];
    let lastReport = last;
    const frame = (now: number) => {
      deltas.push(now - last);
      last = now;
      const t0 = performance.now();
      const px = (Math.sin(now / 700) * 0.5 + 0.5) * size.w;
      const py = (Math.sin(now / 530 + 1) * 0.5 + 0.5) * size.h;
      hoverRef.current = hitTest(layout.blocks, px, py)?.key ?? null;
      draw();
      draws.push(performance.now() - t0);
      if (now - lastReport >= 1000) {
        // Drop the first delta (time since the effect started, not a frame).
        onFrameStats(summarizeFrames(deltas.slice(1), draws.slice(1), layout.blocks.length));
        lastReport = now;
        if (deltas.length > 600) {
          deltas.splice(0, deltas.length - 300);
          draws.splice(0, draws.length - 300);
        }
      }
      raf = requestAnimationFrame(frame);
    };
    raf = requestAnimationFrame(frame);
    return () => {
      cancelAnimationFrame(raf);
      hoverRef.current = null;
      draw();
    };
  }, [stress, layout, size, draw, onFrameStats]);

  const blockAt = (e: React.MouseEvent) => {
    const r = canvasRef.current!.getBoundingClientRect();
    return hitTest(layout.blocks, e.clientX - r.left, e.clientY - r.top);
  };
  const selectionFor = (b: Block<Item> | null): MapSelection =>
    !b ? null : b.item ? { kind: "item", id: b.item.id } : { kind: "smaller" };

  const onMove = (e: React.MouseEvent) => {
    if (stress) return;
    const key = blockAt(e)?.key ?? null;
    if (key !== hoverRef.current) {
      hoverRef.current = key;
      requestAnimationFrame(draw);
    }
  };

  const drawn = layout.blocks.length;
  return (
    <div ref={containerRef} className="map">
      <canvas
        ref={canvasRef}
        role="img"
        aria-label={`Treemap with ${drawn} blocks${layout.smaller.length ? `, including Smaller items (${layout.smaller.length})` : ""}. Every item is also in the list below.`}
        style={{ width: size.w, height: size.h }}
        onMouseMove={onMove}
        onMouseLeave={() => {
          hoverRef.current = null;
          requestAnimationFrame(draw);
        }}
        onClick={(e) => onSelect(selectionFor(blockAt(e)), layout.smaller)}
        onDoubleClick={(e) => onActivate(selectionFor(blockAt(e)), layout.smaller)}
      />
    </div>
  );
}
