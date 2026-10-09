//! Better Disk Manager desktop shell — Stage 0 spike.
//!
//! What this measures and demonstrates:
//!
//! * WebView2 startup: OS process creation to Rust `main`, to the setup
//!   hook, and to the first painted frame reported by the page.
//! * Permissioned IPC: six custom commands, each declared in `build.rs`
//!   and granted only to the `main` window in `capabilities/main.json`. No
//!   core plugin permissions (filesystem, shell, window control) are
//!   granted at all.
//! * Locked navigation: the main window is created in code so it can refuse
//!   any navigation away from the bundled app and every new-window request.
//! * Paged data: list pages are capped at 200 rows (FR04).
//!
//! Set `BD_STARTUP_REPORT=<file>` to write the startup report as JSON, and
//! `BD_EXIT_AFTER_FIRST_PAINT=1` to exit right after, for scripted cold and
//! warm start measurements (see `scripts/measure-startup.ps1`). Set
//! `BD_RESULTS_FILE=<file>` to have the page's results panel (frame test,
//! IPC timings, security probe) written there whenever it changes. The page
//! never chooses a path.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod synthetic;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bd_core::time::Ticks;
use serde::Serialize;
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, State, Url, WebviewUrl, WebviewWindowBuilder};

use synthetic::{Item, LevelSpec, SortKey};

const PAGE_LIMIT: u32 = 200;
const DEV_PORT: u16 = 5173;

struct Startup {
    main_at: Instant,
    /// OS process creation time (Windows), the true start of a launch.
    process_created: Option<Ticks>,
    process_to_main_ms: Option<f64>,
    process_to_setup_ms: Mutex<Option<f64>>,
}

type SortedLevels = HashMap<(LevelSpec, SortKey), Arc<Vec<Item>>>;

#[derive(Default)]
struct Levels {
    sorted: Mutex<SortedLevels>,
}

impl Levels {
    fn get(&self, spec: LevelSpec, sort: SortKey) -> Arc<Vec<Item>> {
        let mut cache = self.sorted.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = cache.get(&(spec, sort)) {
            return Arc::clone(v);
        }
        // Keep memory bounded: the spike only ever looks at one level.
        cache.retain(|(s, _), _| *s == spec);
        let mut items = synthetic::generate(spec, Ticks::now());
        synthetic::sort(&mut items, sort);
        let v = Arc::new(items);
        cache.insert((spec, sort), Arc::clone(&v));
        v
    }
}

fn process_created() -> Option<Ticks> {
    #[cfg(windows)]
    {
        bd_win::process::process_times().map(|t| t.created)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

fn ms_since(t: Option<Ticks>) -> Option<f64> {
    t.map(|t| t.elapsed_until(Ticks::now()) as f64 / 10_000.0)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: &'static str,
    platform: &'static str,
    webview: Option<String>,
    native: bool,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        webview: tauri::webview_version().ok(),
        native: true,
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupReport {
    process_to_main_ms: Option<f64>,
    process_to_setup_ms: Option<f64>,
    process_to_first_paint_ms: Option<f64>,
    main_to_first_paint_ms: Option<f64>,
    js_navigation_to_paint_ms: f64,
}

#[tauri::command]
fn report_first_paint(app: AppHandle, startup: State<'_, Startup>, js_ms: f64) -> StartupReport {
    let report = StartupReport {
        process_to_main_ms: startup.process_to_main_ms,
        process_to_setup_ms: *startup
            .process_to_setup_ms
            .lock()
            .unwrap_or_else(|p| p.into_inner()),
        process_to_first_paint_ms: ms_since(startup.process_created),
        main_to_first_paint_ms: Some(startup.main_at.elapsed().as_secs_f64() * 1000.0),
        js_navigation_to_paint_ms: js_ms,
    };
    if let Ok(path) = std::env::var("BD_STARTUP_REPORT")
        && let Ok(text) = serde_json::to_string_pretty(&report)
    {
        let _ = std::fs::write(path, text);
    }
    if std::env::var_os("BD_EXIT_AFTER_FIRST_PAINT").is_some() {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            app.exit(0);
        });
    }
    report
}

/// Write the page's measurements to `BD_RESULTS_FILE`, if set.
#[tauri::command]
fn report_results(results: serde_json::Value) {
    if let Some(path) = std::env::var_os("BD_RESULTS_FILE")
        && let Ok(text) = serde_json::to_string_pretty(&results)
    {
        let _ = std::fs::write(path, text);
    }
}

#[tauri::command]
async fn level_items(levels: State<'_, Levels>, spec: LevelSpec) -> Result<Vec<Item>, String> {
    Ok(levels.get(spec, SortKey::SizeDesc).as_ref().clone())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    offset: u32,
    total: u32,
    items: Vec<Item>,
}

#[tauri::command]
async fn level_page(
    levels: State<'_, Levels>,
    spec: LevelSpec,
    sort: SortKey,
    offset: u32,
    limit: u32,
) -> Result<Page, String> {
    let all = levels.get(spec, sort);
    let start = (offset as usize).min(all.len());
    let end = (start + limit.min(PAGE_LIMIT) as usize).min(all.len());
    Ok(Page {
        offset,
        total: all.len() as u32,
        items: all[start..end].to_vec(),
    })
}

#[tauri::command]
async fn level_index_of(
    levels: State<'_, Levels>,
    spec: LevelSpec,
    sort: SortKey,
    id: u32,
) -> Result<i64, String> {
    Ok(levels
        .get(spec, sort)
        .iter()
        .position(|i| i.id == id)
        .map_or(-1, |p| p as i64))
}

/// Only the bundled app may load in the main window.
fn navigation_allowed(url: &Url) -> bool {
    match url.scheme() {
        // macOS and Linux custom protocol.
        "tauri" => url.host_str() == Some("localhost"),
        // Windows serves the app from http(s)://tauri.localhost.
        "http" | "https" if url.host_str() == Some("tauri.localhost") => true,
        // The Vite dev server, in development builds only.
        "http" if tauri::is_dev() => {
            url.host_str() == Some("localhost") && url.port() == Some(DEV_PORT)
        }
        _ => false,
    }
}

fn main() {
    let main_at = Instant::now();
    let created = process_created();
    let startup = Startup {
        main_at,
        process_created: created,
        process_to_main_ms: ms_since(created),
        process_to_setup_ms: Mutex::new(None),
    };

    tauri::Builder::default()
        .manage(startup)
        .manage(Levels::default())
        .invoke_handler(tauri::generate_handler![
            app_info,
            report_first_paint,
            level_items,
            level_page,
            level_index_of,
            report_results
        ])
        .setup(|app| {
            let startup = app.state::<Startup>();
            *startup
                .process_to_setup_ms
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = ms_since(startup.process_created);

            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Better Disk Manager")
                .inner_size(1280.0, 820.0)
                .min_inner_size(900.0, 600.0)
                .on_navigation(navigation_allowed)
                .on_new_window(|_url, _features| NewWindowResponse::Deny)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_is_locked_to_the_app() {
        assert!(navigation_allowed(
            &Url::parse("http://tauri.localhost/index.html").unwrap()
        ));
        assert!(navigation_allowed(
            &Url::parse("https://tauri.localhost/").unwrap()
        ));
        assert!(navigation_allowed(
            &Url::parse("tauri://localhost/").unwrap()
        ));
        assert!(!navigation_allowed(
            &Url::parse("https://example.com/").unwrap()
        ));
        assert!(!navigation_allowed(
            &Url::parse("http://tauri.localhost.evil.test/").unwrap()
        ));
        assert!(!navigation_allowed(
            &Url::parse("file:///C:/Windows/").unwrap()
        ));
        assert!(!navigation_allowed(&Url::parse("tauri://evil/").unwrap()));
    }
}
