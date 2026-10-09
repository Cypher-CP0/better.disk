import type { BucketId } from "../treemap/colors";

/** One immediate child at the current navigation level. */
export interface Item {
  id: number;
  name: string;
  kind: "file" | "folder";
  /** Size on disk (counted allocation) in bytes. */
  bytes: number;
  /** Files represented (1 for a file). */
  files: number;
  /** Dominant date bucket for folders; the file's own bucket for files. */
  bucket: BucketId;
  fileType: string;
  modifiedDaysAgo: number | null;
}

export type SortKey = "sizeDesc" | "nameAsc" | "modifiedDesc";

export interface Page {
  offset: number;
  total: number;
  items: Item[];
}

export interface AppInfo {
  version: string;
  platform: string;
  webview: string | null;
  /** True when running inside the Tauri shell, false in a plain browser. */
  native: boolean;
}

export interface StartupReport {
  /** OS process creation to Rust `main` (Windows only). */
  processToMainMs: number | null;
  /** OS process creation to the Tauri setup hook (Windows only). */
  processToSetupMs: number | null;
  /** OS process creation to the first painted frame (Windows only). */
  processToFirstPaintMs: number | null;
  /** Rust `main` to the first painted frame. */
  mainToFirstPaintMs: number | null;
  /** WebView navigation start to the first painted frame. */
  jsNavigationToPaintMs: number;
}

/** Which synthetic level to show. */
export interface LevelSpec {
  seed: number;
  count: number;
  /** Similar sizes, so the map draws as many blocks as possible (stress). */
  even: boolean;
}

export interface DataSource {
  readonly name: string;
  appInfo(): Promise<AppInfo>;
  reportFirstPaint(jsMs: number): Promise<StartupReport>;
  /** Every item of the level (the map needs all of them to lay out). */
  levelItems(spec: LevelSpec): Promise<Item[]>;
  /** One page of the sorted level; `limit` is capped at 200. */
  levelPage(spec: LevelSpec, sort: SortKey, offset: number, limit: number): Promise<Page>;
  /** Row index of an item under a sort, or -1. */
  indexOf(spec: LevelSpec, sort: SortKey, id: number): Promise<number>;
  /**
   * Native only: try things the shell must refuse (an ungranted core
   * command, a plugin that is not installed, a network fetch, a new
   * window) and report what happened.
   */
  securityProbe?(): Promise<Record<string, string>>;
  /** Native only: hand the results panel to the shell for automation. */
  reportResults?(results: unknown): Promise<void>;
}
