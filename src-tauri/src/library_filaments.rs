use library_core::{Filament, FilamentInput, Model, ModelFilamentAssignment, VersionedTarget};
use tauri::State;

use crate::extension_api::AppError;
use crate::library_commands::{library_error, LibraryState};

#[tauri::command]
pub fn library_list_filaments(
    state: State<'_, LibraryState>,
    include_unavailable: bool,
) -> Result<Vec<Filament>, AppError> {
    state.with_db(|db| {
        db.list_filaments(include_unavailable)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_create_filament(
    state: State<'_, LibraryState>,
    input: FilamentInput,
) -> Result<Filament, AppError> {
    state.with_db(|db| db.create_filament(&input).map_err(library_error))
}

#[tauri::command]
pub fn library_update_filament(
    state: State<'_, LibraryState>,
    id: String,
    expected_version: u64,
    input: FilamentInput,
) -> Result<Filament, AppError> {
    state.with_db(|db| {
        db.update_filament(&id, expected_version, &input)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_delete_filament(
    state: State<'_, LibraryState>,
    id: String,
    expected_version: u64,
) -> Result<Option<Filament>, AppError> {
    state.with_db(|db| {
        db.delete_filament(&id, expected_version)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_set_model_filaments(
    state: State<'_, LibraryState>,
    model_id: String,
    expected_version: u64,
    filament_ids: Vec<String>,
) -> Result<Model, AppError> {
    state.with_db(|db| {
        db.set_model_filaments(&model_id, expected_version, &filament_ids)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_bulk_set_model_filaments(
    state: State<'_, LibraryState>,
    targets: Vec<VersionedTarget>,
    filament_ids: Vec<String>,
) -> Result<Vec<Model>, AppError> {
    state.with_db(|db| {
        db.bulk_set_model_filaments(&targets, &filament_ids)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_bulk_assign_model_filaments(
    state: State<'_, LibraryState>,
    assignments: Vec<ModelFilamentAssignment>,
) -> Result<Vec<Model>, AppError> {
    state.with_db(|db| {
        db.bulk_assign_model_filaments(&assignments)
            .map_err(library_error)
    })
}
