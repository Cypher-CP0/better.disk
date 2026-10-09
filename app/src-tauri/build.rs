fn main() {
    // Every custom command is declared here so tauri-build generates an
    // `allow-<command>` permission for it. Commands are then usable only by
    // windows whose capability grants that permission (capabilities/main.json);
    // registering a handler alone does not expose it.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "app_info",
            "report_first_paint",
            "level_items",
            "level_page",
            "level_index_of",
            "report_results",
        ]),
    ))
    .expect("failed to run tauri-build");
}
