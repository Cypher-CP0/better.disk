// The native data source: typed wrappers over the shell's custom commands.
// Each command is individually permitted for the main window only
// (src-tauri/capabilities/main.json); nothing else is exposed.

import { invoke } from "@tauri-apps/api/core";
import type { AppInfo, DataSource, Item, LevelSpec, Page, SortKey, StartupReport } from "./model";
import { SyntheticSource } from "./synthetic";

export function isNative(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

class NativeSource implements DataSource {
  readonly name = "native (Tauri IPC)";

  appInfo(): Promise<AppInfo> {
    return invoke<AppInfo>("app_info");
  }

  reportFirstPaint(jsMs: number): Promise<StartupReport> {
    return invoke<StartupReport>("report_first_paint", { jsMs });
  }

  levelItems(spec: LevelSpec): Promise<Item[]> {
    return invoke<Item[]>("level_items", { spec });
  }

  levelPage(spec: LevelSpec, sort: SortKey, offset: number, limit: number): Promise<Page> {
    return invoke<Page>("level_page", { spec, sort, offset, limit });
  }

  indexOf(spec: LevelSpec, sort: SortKey, id: number): Promise<number> {
    return invoke<number>("level_index_of", { spec, sort, id });
  }

  reportResults(results: unknown): Promise<void> {
    return invoke("report_results", { results });
  }

  async securityProbe(): Promise<Record<string, string>> {
    const attempt = async (cmd: string, args?: Record<string, unknown>) => {
      try {
        await invoke(cmd, args);
        return "ALLOWED";
      } catch (e) {
        return `refused (${String(e).slice(0, 100)})`;
      }
    };
    let fetchResult: string;
    try {
      await fetch("https://example.com/", { mode: "no-cors" });
      fetchResult = "ALLOWED";
    } catch {
      fetchResult = "blocked";
    }
    const opened = window.open("https://example.com/", "_blank");
    if (opened) opened.close();
    return {
      ungrantedCoreCommand: await attempt("plugin:app|version"),
      missingPluginCommand: await attempt("plugin:fs|read_dir", { path: "/" }),
      externalFetch: fetchResult,
      newWindow: opened ? "OPENED" : "blocked",
    };
  }
}

export function createSource(): DataSource {
  return isNative() ? new NativeSource() : new SyntheticSource();
}
