mod archive;
mod commands;
mod error;
pub mod extension_api;
mod filetree;
mod folder_watch;
mod fusion_preview;
mod geometry_buffer;
mod opened_urls;
mod printers;
mod recovery;
mod settings;
mod slicers;
mod state;
mod step_preview;
mod trash;
mod types;

pub use folder_watch::{FolderWatchState, SelectedRootHandle};

use opened_urls::OpenedUrls;
use state::AppState;
use tauri::{Emitter, Manager};

/// Narrow S6 primitive: how the frontend learns the CLI-arg path for a cold
/// start not driven by Finder (e.g. `./app foo.stl` for testing without a
/// bundle). The webview never sees argv directly.
#[tauri::command]
fn cli_arg_path() -> Option<String> {
    std::env::args().nth(1)
}

pub fn configure_builder() -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .manage(fusion_preview::FusionBridgeState::default())
        .manage(folder_watch::FolderWatchState::default())
        .manage(OpenedUrls::default())
        .manage(settings::SettingsState::default())
        .invoke_handler(tauri::generate_handler![
            commands::open_model,
            commands::model_geometry,
            step_preview::read_step_file,
            fusion_preview::read_fusion_file,
            fusion_preview::install_fusion_bridge,
            fusion_preview::cancel_fusion_preview,
            cli_arg_path,
            opened_urls::opened_urls,
            slicers::detect_slicers,
            settings::get_default_slicer,
            settings::set_default_slicer,
            settings::get_settings,
            settings::set_active_printer,
            settings::set_display_units,
            settings::set_appearance_mode,
            settings::set_default_folder,
            settings::set_accent_color,
            settings::set_default_color_preview,
            printers::detect_printers,
            recovery::recovery_list_library,
            recovery::recovery_export_library,
            recovery::recovery_export_colorways,
            filetree::move_tree_entry,
            filetree::list_dir,
            filetree::search_dir,
            filetree::list_model_files,
            filetree::list_zip_files,
            archive::extract_zip,
            folder_watch::watch_folder,
            trash::move_tree_file_to_trash,
        ])
        .setup(|app| {
            let loaded = settings::load(app.handle());
            app.set_theme(loaded.appearance_mode.native_theme());
            *app.state::<settings::SettingsState>().0.lock().unwrap() = loaded;
            Ok(())
        })
}

pub fn handle_run_event(app: &tauri::AppHandle<tauri::Wry>, event: tauri::RunEvent) {
    // Finder "Open With" / double-click (spec §1, S6). Fires both at
    // cold start (before the frontend exists -- handled by stashing
    // into OpenedUrls for the opened_urls command to drain) and while
    // already running (handled by emitting "opened" directly).
    #[cfg(target_os = "macos")]
    if let tauri::RunEvent::Opened { urls } = event {
        let state = app.state::<OpenedUrls>();
        state.0.lock().unwrap().extend(urls.iter().cloned());
        let paths: Vec<String> = urls.iter().filter_map(opened_urls::url_to_path).collect();
        let _ = app.emit("opened", paths);
    }
}

#[cfg(feature = "community")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    configure_builder()
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(handle_run_event);
}

#[cfg(test)]
mod extension_api_tests {
    #[test]
    fn exposes_a_composable_core_builder() {
        let _builder = crate::configure_builder();
    }
}
