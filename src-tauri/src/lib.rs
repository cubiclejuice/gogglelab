mod archive;
mod cad_engine;
mod commands;
mod error;
pub mod extension_api;
mod filetree;
mod folder_watch;
mod fs_portability;
mod fusion_preview;
mod geometry_buffer;
mod handoff;
mod library_archives;
mod library_bundle;
mod library_commands;
mod library_filaments;
mod library_images;
mod library_integrations;
mod library_management;
mod library_scan;
mod library_thumbnail;
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
        .manage(library_bundle::BundlePreviewState::default())
        .manage(fusion_preview::FusionBridgeState::default())
        .manage(folder_watch::FolderWatchState::default())
        .manage(OpenedUrls::default())
        .manage(settings::SettingsState::default())
        .invoke_handler(tauri::generate_handler![
            cad_engine::read_cad_engine,
            commands::open_model,
            commands::model_geometry,
            step_preview::read_step_file,
            fusion_preview::read_fusion_file,
            fusion_preview::install_fusion_bridge,
            fusion_preview::cancel_fusion_preview,
            cli_arg_path,
            opened_urls::opened_urls,
            slicers::detect_slicers,
            handoff::open_in_slicer,
            library_bundle::library_prepare_bundle_preview,
            library_bundle::library_bundle_preview_geometry,
            library_bundle::library_close_bundle_preview,
            library_bundle::library_send_bundle_to_slicer,
            library_management::library_duplicate_groups,
            library_management::library_remove_models,
            library_management::library_list_removed_models,
            library_management::library_restore_removal,
            library_management::library_reveal_model_source,
            library_management::library_trash_loose_source,
            library_management::library_undo_trash,
            library_management::library_create_backup,
            library_management::library_restore_backup,
            library_management::library_purge_catalog,
            library_management::library_cleanup_orphan_cache,
            library_management::library_management_status,
            library_images::library_set_custom_thumbnail,
            library_images::library_clear_custom_thumbnail,
            library_integrations::integration_get_settings,
            library_integrations::integration_save_spoolman_config,
            library_integrations::spoolman_pull,
            library_integrations::integration_save_ai_provider,
            library_integrations::integration_delete_ai_provider,
            library_integrations::ai_suggest_tags,
            library_integrations::ai_apply_tag_review,
            library_integrations::ai_discard_tag_review,
            library_filaments::library_list_filaments,
            library_filaments::library_create_filament,
            library_filaments::library_update_filament,
            library_filaments::library_delete_filament,
            library_filaments::library_set_model_filaments,
            library_filaments::library_bulk_set_model_filaments,
            library_filaments::library_bulk_assign_model_filaments,
            library_commands::library_list_folders,
            library_commands::library_list_models,
            library_commands::library_create_folder,
            library_commands::library_rename_folder,
            library_commands::library_move_folder,
            library_commands::library_delete_folder,
            library_commands::library_import_file,
            library_commands::library_open_model,
            library_commands::library_list_roots,
            library_commands::library_add_root,
            library_commands::library_update_root,
            library_commands::library_remove_root,
            library_commands::library_start_scan,
            library_commands::library_scan_status,
            library_commands::library_cancel_scan,
            library_commands::library_get_thumbnail,
            library_commands::library_purge_thumbnails,
            library_commands::library_get_catalog_settings,
            library_commands::library_set_catalog_settings,
            library_commands::library_start_thumbnail_job,
            library_commands::library_list_tags,
            library_commands::library_create_tag,
            library_commands::library_update_tag,
            library_commands::library_delete_tag,
            library_commands::library_update_model_metadata,
            library_commands::library_update_models_metadata,
            library_commands::library_create_print_attempt,
            library_commands::library_list_print_attempts,
            library_commands::library_get_print_attempt,
            library_commands::library_update_print_attempt,
            library_commands::library_delete_print_attempt,
            library_commands::library_create_bundle,
            library_commands::library_set_model_bundle,
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
            let library_root = app.path().app_data_dir()?.join("library");
            app.manage(library_commands::LibraryState::new(library_root));
            app.manage(library_integrations::IntegrationState::load(
                app.path().app_config_dir()?.join("integrations.json"),
            )?);
            library_scan::start_background_scanner(app.handle().clone());
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
