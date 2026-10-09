// Stage 0 shell spike: one synthetic folder level shown as a canvas treemap
// and an accessible virtual list that share selection, plus the startup,
// frame-rate and IPC measurements the specification asks for.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { AppInfo, DataSource, Item, LevelSpec, SortKey, StartupReport } from "./data/model";
import { formatBytes, formatCount } from "./format";
import { type FocusMode, VirtualGrid } from "./grid/VirtualGrid";
import type { FrameStats } from "./perf/fps";
import { BUCKETS, bucket } from "./treemap/colors";
import { type MapSelection, TreemapCanvas } from "./treemap/TreemapCanvas";

const COUNTS = [100, 1500, 5000, 100_000];
const FRAME_TEST_MS = 5000;

interface Props {
  source: DataSource;
  startup: Promise<StartupReport>;
  /** Called once after the first commit; used to time first paint. */
  onMounted: () => void;
}

interface Timings {
  levelItemsMs: number | null;
  levelItemsPayloadBytes: number | null;
  pageFetches: number[];
  layout: { blocks: number; smaller: number; excluded: number; layoutMs: number } | null;
}

function p95(values: number[]): number | null {
  if (values.length === 0) return null;
  const s = [...values].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.round((s.length - 1) * 0.95))]!;
}

export function App({ source, startup, onMounted }: Props) {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [startupReport, setStartupReport] = useState<StartupReport | null>(null);
  const [count, setCount] = useState(1500);
  const [seed, setSeed] = useState(7);
  const [even, setEven] = useState(false);
  const spec: LevelSpec = useMemo(() => ({ seed, count, even }), [seed, count, even]);
  const [sort, setSort] = useState<SortKey>("sizeDesc");
  const [focusMode, setFocusMode] = useState<FocusMode>("roving");
  const [items, setItems] = useState<Item[]>([]);
  const [selection, setSelection] = useState<MapSelection>(null);
  const [smaller, setSmaller] = useState<Item[]>([]);
  const [reveal, setReveal] = useState<{ index: number; token: number } | null>(null);
  const [stress, setStress] = useState(false);
  const [frameStats, setFrameStats] = useState<FrameStats | null>(null);
  const [frameTest, setFrameTest] = useState<FrameStats | null>(null);
  const [status, setStatus] = useState("");
  const timings = useRef<Timings>({
    levelItemsMs: null,
    levelItemsPayloadBytes: null,
    pageFetches: [],
    layout: null,
  });
  const [, bump] = useState(0);
  const [security, setSecurity] = useState<Record<string, string> | null>(null);

  useEffect(() => {
    onMounted();
  }, [onMounted]);

  useEffect(() => {
    source.appInfo().then(setInfo, () => setInfo(null));
    startup.then(setStartupReport, () => setStartupReport(null));
  }, [source, startup]);

  // After startup is measured, check that the shell refuses what it must.
  useEffect(() => {
    if (!source.securityProbe || !startupReport) return;
    source.securityProbe().then(setSecurity, (e) => setSecurity({ error: String(e) }));
  }, [source, startupReport]);

  useEffect(() => {
    let cancelled = false;
    const t = performance.now();
    timings.current.pageFetches = [];
    source.levelItems(spec).then((loaded) => {
      if (cancelled) return;
      timings.current.levelItemsMs = performance.now() - t;
      timings.current.levelItemsPayloadBytes = JSON.stringify(loaded).length;
      setItems(loaded);
      setSelection(null);
      setStatus(`Loaded ${formatCount(loaded.length)} items.`);
    });
    return () => {
      cancelled = true;
    };
  }, [source, spec]);

  const byId = useMemo(() => new Map(items.map((i) => [i.id, i])), [items]);
  const dataKey = `${seed}:${count}:${even}:${sort}`;

  const fetchPage = useCallback(
    async (offset: number, limit: number) => (await source.levelPage(spec, sort, offset, limit)).items,
    [source, spec, sort],
  );

  const revealId = useCallback(
    (id: number) => {
      source.indexOf(spec, sort, id).then((index) => {
        if (index >= 0) setReveal({ index, token: performance.now() });
      });
    },
    [source, spec, sort],
  );

  // Keep the selected item visible in the list when the sort changes.
  useEffect(() => {
    if (selection?.kind === "item") revealId(selection.id);
  }, [sort]);

  const onMapSelect = useCallback(
    (sel: MapSelection, smallerItems: Item[]) => {
      setSelection(sel);
      setSmaller(smallerItems);
      if (sel?.kind === "item") revealId(sel.id);
      if (sel?.kind === "smaller" && smallerItems[0]) revealId(smallerItems[0].id);
    },
    [revealId],
  );

  const onMapActivate = useCallback(
    (sel: MapSelection, smallerItems: Item[]) => {
      onMapSelect(sel, smallerItems);
      if (sel?.kind === "smaller") {
        setStatus(`Smaller items: ${formatCount(smallerItems.length)} items, shown in the list.`);
      } else if (sel?.kind === "item") {
        const item = byId.get(sel.id);
        if (item) setStatus(item.kind === "folder" ? `Would open folder ${item.name}.` : `Details for ${item.name}.`);
      }
    },
    [onMapSelect, byId],
  );

  const latestStats = useRef<FrameStats | null>(null);
  const onFrameStats = useCallback((s: FrameStats) => {
    latestStats.current = s;
    setFrameStats(s);
  }, []);
  const onLayout = useCallback((l: Timings["layout"]) => {
    timings.current.layout = l;
    bump((n) => n + 1);
  }, []);
  const onPageTiming = useCallback((ms: number) => {
    timings.current.pageFetches.push(ms);
    bump((n) => n + 1);
  }, []);

  const runFrameTest = () => {
    latestStats.current = null;
    setFrameStats(null);
    setFrameTest(null);
    setStress(true);
    setStatus("Measuring frame rate for 5 seconds…");
    window.setTimeout(() => {
      setStress(false);
      const s = latestStats.current;
      setFrameTest(s);
      setStatus(
        s
          ? `Frame test: ${s.fps.toFixed(1)} fps, p95 frame ${s.p95FrameMs.toFixed(1)} ms.`
          : "Frame test produced no frames.",
      );
    }, FRAME_TEST_MS);
  };

  const selectedItem = selection?.kind === "item" ? byId.get(selection.id) : undefined;
  const t = timings.current;
  const results = {
    source: source.name,
    app: info,
    startup: startupReport,
    level: { seed, count, even, ...t.layout },
    ipc: {
      levelItemsMs: t.levelItemsMs,
      levelItemsPayloadBytes: t.levelItemsPayloadBytes,
      pageFetches: t.pageFetches.length,
      pageFetchP95Ms: p95(t.pageFetches),
    },
    frameTest,
    security,
    devicePixelRatio: window.devicePixelRatio,
    focusMode,
  };

  // Hand finished measurements to the shell (written only if the shell was
  // started with BD_RESULTS_FILE).
  const resultsRef = useRef(results);
  resultsRef.current = results;
  const reportKey = JSON.stringify([startupReport, security, frameTest, t.layout, t.levelItemsMs]);
  useEffect(() => {
    source.reportResults?.(resultsRef.current).catch(() => {});
  }, [source, reportKey]);

  return (
    <div className="app">
      <header className="topbar">
        <h1>Better Disk Manager</h1>
        <span className="tag">Stage 0 shell spike · synthetic data</span>
        <span className="spacer" />
        <span className="muted">
          {startupReport
            ? startupReport.processToFirstPaintMs !== null
              ? `Launch to first paint ${startupReport.processToFirstPaintMs.toFixed(0)} ms`
              : `Navigation to first paint ${startupReport.jsNavigationToPaintMs.toFixed(0)} ms`
            : "Measuring startup…"}
        </span>
      </header>

      <div className="toolbar" role="toolbar" aria-label="Spike controls">
        <label>
          Items
          <select value={count} onChange={(e) => setCount(Number(e.target.value))}>
            {COUNTS.map((c) => (
              <option key={c} value={c}>
                {formatCount(c)}
              </option>
            ))}
          </select>
        </label>
        <label>
          Seed
          <input
            type="number"
            min={1}
            value={seed}
            onChange={(e) => setSeed(Math.max(1, Number(e.target.value) || 1))}
          />
        </label>
        <label>
          <input type="checkbox" checked={even} onChange={(e) => setEven(e.target.checked)} />
          Even sizes (most blocks)
        </label>
        <label>
          Sort list by
          <select value={sort} onChange={(e) => setSort(e.target.value as SortKey)}>
            <option value="sizeDesc">Size on disk</option>
            <option value="nameAsc">Name</option>
            <option value="modifiedDesc">Last modified</option>
          </select>
        </label>
        <fieldset>
          <legend>List focus</legend>
          <label>
            <input
              type="radio"
              name="focus"
              checked={focusMode === "roving"}
              onChange={() => setFocusMode("roving")}
            />
            Roving tabindex
          </label>
          <label>
            <input
              type="radio"
              name="focus"
              checked={focusMode === "activedescendant"}
              onChange={() => setFocusMode("activedescendant")}
            />
            Active descendant
          </label>
        </fieldset>
        <label>
          <input type="checkbox" checked={stress} onChange={(e) => setStress(e.target.checked)} />
          Stress redraw
        </label>
        <button type="button" onClick={runFrameTest} disabled={stress}>
          Run 5-second frame test
        </button>
      </div>

      <main className="workspace">
        <section className="map-area" aria-label="Treemap">
          <TreemapCanvas
            items={items}
            selection={selection}
            onSelect={onMapSelect}
            onActivate={onMapActivate}
            stress={stress}
            onFrameStats={onFrameStats}
            onLayout={onLayout}
          />
          <ul className="legend" aria-label="Last modified legend">
            {BUCKETS.map((b) => (
              <li key={b.id}>
                <span className={`swatch swatch-${b.id}`} aria-hidden="true" />
                {b.label}
              </li>
            ))}
            <li className="muted">Shade shows age only. It does not mean safe to delete.</li>
          </ul>
        </section>

        <section className="list-area" aria-label="List">
          <VirtualGrid
            count={count}
            dataKey={dataKey}
            fetchPage={fetchPage}
            sort={sort}
            onSortChange={setSort}
            selectedId={selection?.kind === "item" ? selection.id : null}
            onSelect={(item) => setSelection({ kind: "item", id: item.id })}
            onActivate={(item) =>
              setStatus(item.kind === "folder" ? `Would open folder ${item.name}.` : `Details for ${item.name}.`)
            }
            focusMode={focusMode}
            reveal={reveal}
            onPageTiming={onPageTiming}
          />
        </section>

        <aside className="details" aria-label="Details">
          <h2>Details</h2>
          {selectedItem ? (
            <dl>
              <dt>Name</dt>
              <dd>{selectedItem.name}</dd>
              <dt>Size on disk</dt>
              <dd>
                {formatBytes(selectedItem.bytes)} ({selectedItem.bytes.toLocaleString("en-US")} bytes)
              </dd>
              <dt>{selectedItem.kind === "folder" ? "Dominant date bucket" : "Last modified"}</dt>
              <dd>
                {bucket(selectedItem.bucket).label}
                {selectedItem.modifiedDaysAgo !== null ? ` (${selectedItem.modifiedDaysAgo} days ago)` : ""}
              </dd>
              <dt>Type</dt>
              <dd>{selectedItem.kind === "folder" ? `Folder, ${formatCount(selectedItem.files)} files` : selectedItem.fileType}</dd>
            </dl>
          ) : selection?.kind === "smaller" ? (
            <p>
              Smaller items: {formatCount(smaller.length)} items too small to draw individually. They are listed
              in the list.
            </p>
          ) : (
            <p className="muted">Select a block or a row.</p>
          )}

          <h2>Measurements</h2>
          {frameStats && (
            <p>
              {frameStats.fps.toFixed(1)} fps · p95 frame {frameStats.p95FrameMs.toFixed(1)} ms · draw p95{" "}
              {frameStats.drawP95Ms.toFixed(2)} ms · {frameStats.blocks} blocks
            </p>
          )}
          <pre className="results" aria-label="Results as JSON">
            {JSON.stringify(
              results,
              (_k, v: unknown) => (typeof v === "number" && !Number.isInteger(v) ? Math.round(v * 10) / 10 : v),
              2,
            )}
          </pre>
        </aside>
      </main>

      <div className="status" role="status" aria-live="polite">
        {status}
      </div>
    </div>
  );
}
