use std::path::Path;

use library_core::Model;

use crate::error::AppError;
use crate::library_commands::{library_error, LibraryState};

#[tauri::command]
pub async fn library_set_custom_thumbnail(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
    expected_version: u64,
    path: String,
) -> Result<Model, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _io = state.begin_io()?;
        let model = state.with_db(|db| {
            db.get_model(&model_id)
                .map_err(library_error)?
                .ok_or_else(|| failure("Model no longer exists"))
        })?;
        if model.metadata_version != expected_version {
            return Err(failure(
                "Metadata version changed; reload the catalog and try again",
            ));
        }
        let (relative, png_hash) =
            library_runtime::images::import_custom_image_file(state.root(), Path::new(&path))
                .map_err(failure)?;
        state.with_db(|db| {
            db.set_custom_thumbnail(&model_id, expected_version, &png_hash, &relative)
                .map_err(library_error)
        })
    })
    .await
    .map_err(|error| failure(error.to_string()))?
}

#[tauri::command]
pub async fn library_clear_custom_thumbnail(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
    expected_version: u64,
) -> Result<Model, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.with_db(|db| {
            db.clear_custom_thumbnail(&model_id, expected_version)
                .map_err(library_error)
        })
    })
    .await
    .map_err(|error| failure(error.to_string()))?
}

fn failure(message: impl Into<String>) -> AppError {
    AppError::Library {
        message: message.into(),
    }
}
