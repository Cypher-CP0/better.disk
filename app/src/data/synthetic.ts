// Deterministic synthetic levels. The Rust shell generates the same data
// with the same algorithm (app/src-tauri/src/synthetic.rs), so browser
// development and the native shell show the same picture.

import type { BucketId } from "../treemap/colors";
import { ordinalCompare } from "../treemap/squarify";
import type { AppInfo, DataSource, Item, LevelSpec, Page, SortKey, StartupReport } from "./model";

export const WORDS = [
  "report", "invoice", "holiday", "backup", "photo", "project", "notes", "draft",
  "final", "budget", "video", "track", "scan", "export", "archive", "data",
] as const;

export const EXTS: readonly (readonly [string, string])[] = [
  ["pdf", "Documents"], ["docx", "Documents"], ["txt", "Documents"], ["mp4", "Videos"],
  ["mkv", "Videos"], ["zip", "Archives"], ["7z", "Archives"], ["mp3", "Music"],
  ["jpg", "Images"], ["png", "Images"], ["dll", "Other"], ["bin", "Other"],
];

/** mulberry32: a tiny 32-bit generator, identical in Rust and TypeScript. */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export function bucketForDays(days: number | null): BucketId {
  if (days === null || days < 0) return "unknown";
  if (days <= 7) return "within7Days";
  if (days <= 30) return "days8To30";
  if (days <= 90) return "days31To90";
  if (days <= 365) return "days91To365";
  return "overOneYear";
}

export function generateLevel(seed: number, count: number, even = false): Item[] {
  const rnd = mulberry32(seed);
  const pick = (n: number) => Math.min(n - 1, Math.floor(rnd() * n));
  const items: Item[] = [];
  for (let i = 0; i < count; i++) {
    const folder = rnd() < 0.3;
    const word = WORDS[pick(WORDS.length)]!;
    const [ext, type] = EXTS[pick(EXTS.length)]!;
    const exponent = pick(folder ? 36 : 31);
    const zero = !folder && rnd() < 0.03;
    const bytes = zero ? 0 : Math.floor((1 + rnd()) * 2 ** (even ? 30 : exponent));
    const files = folder ? 1 + pick(5000) : 1;
    const unknown = rnd() < 0.04;
    const r = rnd();
    const days = unknown ? null : Math.floor(r * r * 2000);
    items.push({
      id: i,
      name: folder ? `${word} ${i}` : `${word}-${i}.${ext}`,
      kind: folder ? "folder" : "file",
      bytes,
      files,
      bucket: bucketForDays(days),
      fileType: folder ? "Folder" : type,
      modifiedDaysAgo: days,
    });
  }
  return items;
}

export function sortItems(items: Item[], sort: SortKey): Item[] {
  const out = [...items];
  switch (sort) {
    case "sizeDesc":
      out.sort((a, b) => b.bytes - a.bytes || ordinalCompare(a.name, b.name) || a.id - b.id);
      break;
    case "nameAsc":
      out.sort((a, b) => ordinalCompare(a.name, b.name) || a.id - b.id);
      break;
    case "modifiedDesc":
      out.sort(
        (a, b) =>
          (a.modifiedDaysAgo ?? Number.MAX_SAFE_INTEGER) -
            (b.modifiedDaysAgo ?? Number.MAX_SAFE_INTEGER) ||
          ordinalCompare(a.name, b.name) ||
          a.id - b.id,
      );
      break;
  }
  return out;
}

/** A data source for running the UI in a plain browser (`npm run dev`). */
export class SyntheticSource implements DataSource {
  readonly name = "browser (synthetic)";
  private cache = new Map<string, Item[]>();

  private sorted(spec: LevelSpec, sort: SortKey): Item[] {
    const key = `${spec.seed}:${spec.count}:${spec.even}:${sort}`;
    let v = this.cache.get(key);
    if (!v) {
      v = sortItems(generateLevel(spec.seed, spec.count, spec.even), sort);
      this.cache.set(key, v);
    }
    return v;
  }

  async appInfo(): Promise<AppInfo> {
    return { version: "dev", platform: "browser", webview: navigator.userAgent, native: false };
  }

  async reportFirstPaint(jsMs: number): Promise<StartupReport> {
    return {
      processToMainMs: null,
      processToSetupMs: null,
      processToFirstPaintMs: null,
      mainToFirstPaintMs: null,
      jsNavigationToPaintMs: jsMs,
    };
  }

  async levelItems(spec: LevelSpec): Promise<Item[]> {
    return generateLevel(spec.seed, spec.count, spec.even);
  }

  async levelPage(spec: LevelSpec, sort: SortKey, offset: number, limit: number): Promise<Page> {
    const all = this.sorted(spec, sort);
    const lim = Math.min(200, Math.max(0, limit));
    return { offset, total: all.length, items: all.slice(offset, offset + lim) };
  }

  async indexOf(spec: LevelSpec, sort: SortKey, id: number): Promise<number> {
    return this.sorted(spec, sort).findIndex((i) => i.id === id);
  }
}
