use std::path::Path;

use library_recovery::{
    export_library_zip, export_raw_colorway_json, inspect_library, RecoveryManifest,
};
use tauri::{AppHandle, Manager};

use crate::error::AppError;

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

/// The frontend must pass localStorage.getItem's raw value from this webview.
/// This command does not read or modify localStorage.
#[tauri::command]
pub fn recovery_export_colorways(
    raw_json: String,
    destination_path: String,
) -> Result<u64, AppError> {
    export_raw_colorway_json(&raw_json, Path::new(&destination_path)).map_err(recovery_error)
}
