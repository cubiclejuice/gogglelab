use std::path::Path;

use library_recovery::{
    export_library_zip, export_raw_colorway_json, inspect_library, RecoveryManifest,
};
use tauri::{AppHandle, Manager};

use crate::error::AppError;
use crate::library_commands::LibraryState;

fn library_root(app: &AppHandle) -> Result<std::path::PathBuf, AppError> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("library"))
        .map_err(|error| AppError::Library {
            message: error.to_string(),
        })
}

fn recovery_error(error: library_recovery::RecoveryError) -> AppError {
    AppError::Library {
        message: error.to_string(),
    }
}

/// Read-only catalog inspection; this is available without a Pro entitlement.
#[tauri::command]
pub fn recovery_list_library(app: AppHandle) -> Result<RecoveryManifest, AppError> {
    inspect_library(&library_root(&app)?).map_err(recovery_error)
}

/// Destination is chosen by the caller. The recovery crate refuses overwrite.
#[tauri::command]
pub fn recovery_export_library(
    app: AppHandle,
    model_ids: Vec<String>,
    destination_path: String,
) -> Result<RecoveryManifest, AppError> {
    export_library_zip(
        &library_root(&app)?,
        &model_ids,
        Path::new(&destination_path),
    )
    .map_err(recovery_error)
}

/// Export the supported library JSON contract to a new file.
/// The user chooses the destination; existing files are never overwritten.
#[tauri::command]
pub fn recovery_export_printventory_json(
    state: tauri::State<'_, LibraryState>,
    settings: tauri::State<'_, crate::settings::SettingsState>,
    destination_path: String,
) -> Result<usize, AppError> {
    let slicers = settings
        .0
        .lock()
        .map_err(|_| AppError::Library {
            message: "Slicer settings are unavailable".into(),
        })?
        .slicer_profiles
        .iter()
        .map(|profile| library_runtime::SlicerJsonExportEntry {
            name: profile.name.clone(),
            path: profile.path.clone(),
        })
        .collect::<Vec<_>>();
    let destination = Path::new(&destination_path);
    let stage = state.with_db(|db| {
        library_runtime::stage_library_json_export_with_slicers(db, destination, &slicers).map_err(
            |error| AppError::Library {
                message: error.to_string(),
            },
        )
    })?;
    let _maintenance = state.begin_maintenance()?;
    let result = stage
        .publish(destination)
        .map_err(|error| AppError::Library {
            message: error.to_string(),
        })?;
    Ok(result.models)
}

/// The frontend must pass localStorage.getItem's raw value from this webview.
/// This command does not read or modify localStorage.
#[tauri::command]
pub fn recovery_export_colorways(
    raw_json: String,
    destination_path: String,
) -> Result<u64, AppError> {
    export_raw_colorway_json(&raw_json, Path::new(&destination_path)).map_err(recovery_error)
}
