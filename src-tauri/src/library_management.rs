use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use library_core::{
    DuplicateGroup, LibraryDb, Model, RemovalResult, RemovedModel, VersionedTarget,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::Manager;

use crate::extension_api::{AppError, AppState};
use crate::library_commands::{library_error, LibraryState};

static MANAGEMENT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct SwapFailure {
    message: String,
    recovered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLocation {
    pub path: String,
    pub archive_member: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashResult {
    pub snapshot_id: String,
    pub model_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    pub previous_library_backup: String,
    pub models: usize,
    pub generation: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResult {
    pub files: u64,
    pub bytes: u64,
    pub handoff_cleanup_skipped: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagementStatus {
    pub maintenance: bool,
    pub generation: u64,
}
#[derive(Debug, Serialize, Deserialize)]
struct TrashMetadata {
    model_id: String,
    sha256: String,
    bytes: u64,
}

#[tauri::command]
pub fn library_duplicate_groups(
    state: tauri::State<'_, LibraryState>,
    model_ids: Vec<String>,
) -> Result<Vec<DuplicateGroup>, AppError> {
    state.with_db(|db| db.duplicate_groups(&model_ids).map_err(library_error))
}
#[tauri::command]
pub fn library_remove_models(
    state: tauri::State<'_, LibraryState>,
    targets: Vec<VersionedTarget>,
) -> Result<RemovalResult, AppError> {
    ensure_idle(&state)?;
    state.with_db(|db| {
        db.remove_models_recoverably(&targets)
            .map_err(library_error)
    })
}
#[tauri::command]
pub fn library_list_removed_models(
    state: tauri::State<'_, LibraryState>,
) -> Result<Vec<RemovedModel>, AppError> {
    state.with_db(|db| db.list_removed_models().map_err(library_error))
}
#[tauri::command]
pub fn library_restore_removal(
    state: tauri::State<'_, LibraryState>,
    snapshot_id: String,
) -> Result<Vec<Model>, AppError> {
    restore_removal_impl(&state, snapshot_id)
}

fn restore_removal_impl(state: &LibraryState, snapshot_id: String) -> Result<Vec<Model>, AppError> {
    validate_snapshot_id(&snapshot_id).map_err(|message| AppError::Library { message })?;
    let owned = state
        .root()
        .join("trash")
        .join(format!("source-{snapshot_id}"));
    match fs::symlink_metadata(&owned) {
        Ok(_) => undo_trash_impl(state, snapshot_id).map(|model| vec![model]),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure_idle(state)?;
            state.with_db(|db| db.restore_removal(&snapshot_id).map_err(library_error))
        }
        Err(error) => Err(map_io(error)),
    }
}
#[tauri::command]
pub fn library_reveal_model_source(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
) -> Result<SourceLocation, AppError> {
    let _io_guard = state.begin_io()?;
    state.with_db(|db| {
        let source = db
            .resolve_model_source_ref(&model_id)
            .map_err(library_error)?;
        reveal_source(&source.outer_path, |program, args| {
            let status = std::process::Command::new(program).args(args).status()?;
            if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other("Source reveal command failed"))
            }
        })
        .map_err(|message| AppError::Library { message })?;
        Ok(SourceLocation {
            path: source.outer_path.to_string_lossy().into_owned(),
            archive_member: source.archive_member,
        })
    })
}

#[tauri::command]
pub async fn library_trash_loose_source(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
    expected_version: u64,
) -> Result<TrashResult, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        trash_loose_source_impl(&state, model_id, expected_version)
    })
    .await
    .map_err(|error| AppError::Library {
        message: format!("Trash worker failed: {error}"),
    })?
}
fn trash_loose_source_impl(
    state: &LibraryState,
    model_id: String,
    expected_version: u64,
) -> Result<TrashResult, AppError> {
    ensure_idle(state)?;
    let _io_guard = state.begin_io()?;
    let (source, hash, bytes) = state.with_db(|db| {
        let model = db
            .get_model(&model_id)
            .map_err(library_error)?
            .ok_or_else(|| AppError::Library {
                message: "Model not found".into(),
            })?;
        if model.source_kind != "indexed" || model.source_archive_member.is_some() {
            return Err(AppError::Library {
                message: "Only loose indexed sources can be trashed explicitly".into(),
            });
        }
        let source = db
            .resolve_model_source_ref(&model_id)
            .map_err(library_error)?;
        Ok((
            source.outer_path,
            model.content_hash.unwrap_or_default(),
            model.bytes,
        ))
    })?;
    let removal = state.with_db(|db| {
        db.remove_models_recoverably(&[VersionedTarget {
            model_id: model_id.clone(),
            expected_version,
        }])
        .map_err(library_error)
    })?;
    let owned = state
        .root()
        .join("trash")
        .join(format!("source-{}", removal.snapshot_id));
    if let Err(error) = copy_owned_recovery(state.root(), &source, &owned, &model_id, &hash, bytes)
    {
        let _ = state.with_db(|db| {
            db.restore_removal(&removal.snapshot_id)
                .map_err(library_error)
        });
        return Err(AppError::Library { message: error });
    }
    let trash_result = state.with_db(|db| {
        let fresh = db
            .resolve_model_source_ref(&model_id)
            .map_err(library_error)?;
        if fresh.outer_path != source || fresh.archive_member.is_some() {
            return Err(AppError::Library {
                message: "Loose source identity changed before Trash".into(),
            });
        }
        verify_file(&fresh.outer_path, &hash, bytes)
            .map_err(|message| AppError::Library { message })?;
        trash::delete(&fresh.outer_path).map_err(|error| AppError::Library {
            message: error.to_string(),
        })?;
        Ok(())
    });
    if let Err(error) = trash_result {
        let _ = state.with_db(|db| {
            db.restore_removal(&removal.snapshot_id)
                .map_err(library_error)
        });
        let _ = fs::remove_dir_all(&owned);
        return Err(error);
    }
    Ok(TrashResult {
        snapshot_id: removal.snapshot_id,
        model_id,
    })
}

#[tauri::command]
pub async fn library_undo_trash(
    state: tauri::State<'_, LibraryState>,
    snapshot_id: String,
) -> Result<Model, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || undo_trash_impl(&state, snapshot_id))
        .await
        .map_err(|error| AppError::Library {
            message: format!("Undo Trash worker failed: {error}"),
        })?
}
fn undo_trash_impl(state: &LibraryState, snapshot_id: String) -> Result<Model, AppError> {
    validate_snapshot_id(&snapshot_id).map_err(|message| AppError::Library { message })?;
    let guard = state.begin_maintenance()?;
    let db = LibraryDb::open(guard.root()).map_err(library_error)?;
    let owned = guard
        .root()
        .join("trash")
        .join(format!("source-{snapshot_id}"));
    validate_owned_snapshot_dir(guard.root(), &owned)
        .map_err(|message| AppError::Library { message })?;
    let metadata_bytes = read_bounded(&owned.join("metadata.json"), 16 * 1024)
        .map_err(|message| AppError::Library { message })?;
    let metadata: TrashMetadata =
        serde_json::from_slice(&metadata_bytes).map_err(|e| AppError::Library {
            message: e.to_string(),
        })?;
    let removed = db
        .list_removed_models()
        .map_err(library_error)?
        .into_iter()
        .find(|removed| removed.snapshot_id == snapshot_id && removed.model.id == metadata.model_id)
        .ok_or_else(|| AppError::Library {
            message: "Removed model snapshot does not match owned recovery".into(),
        })?;
    if removed.model.source_kind != "indexed" || removed.model.source_archive_member.is_some() {
        return Err(AppError::Library {
            message: "Owned recovery is not a loose indexed source".into(),
        });
    }
    let destination = validated_removed_destination(&db, &removed.model)?;
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(AppError::Library {
            message: "Original destination already exists".into(),
        });
    }
    let copy = owned.join("source.bin");
    if metadata.sha256 != removed.model.content_hash.clone().unwrap_or_default()
        || metadata.bytes != removed.model.bytes
    {
        return Err(AppError::Library {
            message: "Owned recovery metadata does not match catalog".into(),
        });
    }
    verify_file(&copy, &metadata.sha256, metadata.bytes)
        .map_err(|message| AppError::Library { message })?;
    let mut input = File::open(&copy).map_err(map_io)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(map_io)?;
    let copied = match std::io::copy(
        &mut (&mut input).take(metadata.bytes.saturating_add(1)),
        &mut output,
    ) {
        Ok(copied) => copied,
        Err(error) => {
            drop(output);
            let _ = fs::remove_file(&destination);
            return Err(map_io(error));
        }
    };
    drop(output);
    if copied != metadata.bytes {
        let _ = fs::remove_file(&destination);
        return Err(AppError::Library {
            message: "Owned recovery copy size changed".into(),
        });
    }
    if let Err(message) = verify_file(&destination, &metadata.sha256, metadata.bytes) {
        let _ = fs::remove_file(&destination);
        return Err(AppError::Library { message });
    }
    let destination_metadata = match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
        Ok(_) => {
            let _ = fs::remove_file(&destination);
            return Err(AppError::Library {
                message: "Restored source changed file type".into(),
            });
        }
        Err(error) => {
            let _ = fs::remove_file(&destination);
            return Err(map_io(error));
        }
    };
    let modified_at = modified_nanos(&destination_metadata);
    match db
        .restore_removal_with_owned_source(
            &snapshot_id,
            &metadata.model_id,
            &metadata.sha256,
            metadata.bytes,
            modified_at,
        )
        .map_err(library_error)
    {
        Ok(model) => {
            let _ = fs::remove_dir_all(owned);
            Ok(model)
        }
        Err(error) => {
            let _ = fs::remove_file(destination);
            Err(error)
        }
    }
}

fn modified_nanos(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |value| {
            u64::try_from(value.as_nanos()).unwrap_or(u64::MAX)
        })
}

#[tauri::command]
pub async fn library_create_backup(
    state: tauri::State<'_, LibraryState>,
    destination_path: String,
) -> Result<library_backup::BackupResult, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || create_backup_impl(&state, destination_path))
        .await
        .map_err(|error| AppError::Library {
            message: format!("Backup worker failed: {error}"),
        })?
}
fn create_backup_impl(
    state: &LibraryState,
    destination_path: String,
) -> Result<library_backup::BackupResult, AppError> {
    let guard = state.begin_maintenance()?;
    library_backup::create_backup(guard.root(), Path::new(&destination_path))
        .map_err(|message| AppError::Library { message })
}

#[tauri::command]
pub async fn library_restore_backup(
    app: tauri::AppHandle,
    state: tauri::State<'_, LibraryState>,
    source_path: String,
) -> Result<RestoreResult, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let viewer = app.state::<AppState>();
        restore_backup_impl(&state, viewer.inner(), source_path)
    })
    .await
    .map_err(|error| AppError::Library {
        message: format!("Restore worker failed: {error}"),
    })?
}
fn restore_backup_impl(
    state: &LibraryState,
    viewer: &AppState,
    source_path: String,
) -> Result<RestoreResult, AppError> {
    let mut guard = state.begin_maintenance()?;
    let root = guard.root().to_path_buf();
    let floor = LibraryDb::open(&root)
        .map_err(library_error)?
        .max_metadata_version()
        .map_err(library_error)?;
    let parent = root.parent().ok_or_else(|| AppError::Library {
        message: "Library root has no parent".into(),
    })?;
    let nonce = unique_suffix();
    let staged = parent.join(format!(".gogglelab-restore-{nonce}"));
    library_backup::restore_backup_to_staging(Path::new(&source_path), &staged)
        .map_err(|message| AppError::Library { message })?;
    {
        let staged_db = LibraryDb::open(&staged).map_err(library_error)?;
        staged_db
            .bump_all_metadata_versions_above(floor)
            .map_err(library_error)?;
    }
    let previous = parent.join(format!("library.previous-{nonce}"));
    let generation = guard.invalidate();
    invalidate_viewer(viewer);
    if let Err(failure) = swap_library_dirs(&root, &staged, &previous, false) {
        if !failure.recovered {
            guard.remain_armed();
        }
        return Err(AppError::Library {
            message: failure.message,
        });
    }
    let validation = LibraryDb::open(&root).and_then(|db| db.list_models(None, None));
    let models = match validation {
        Ok(models) => models.len(),
        Err(error) => {
            if let Err(rollback_error) =
                rollback_validated_replacement(&root, &previous, parent, &nonce)
            {
                guard.remain_armed();
                return Err(AppError::Library{message:format!("Restored catalog failed validation: {error}; rollback failed: {rollback_error}; previous: {}",previous.display())});
            }
            return Err(AppError::Library { message: format!("Restored catalog failed validation and previous Library was recovered: {error}") });
        }
    };
    Ok(RestoreResult {
        previous_library_backup: previous.to_string_lossy().into_owned(),
        models,
        generation,
    })
}

#[tauri::command]
pub async fn library_purge_catalog(
    app: tauri::AppHandle,
    state: tauri::State<'_, LibraryState>,
    confirmation: String,
    destination_path: String,
) -> Result<library_backup::BackupResult, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let viewer = app.state::<AppState>();
        purge_catalog_impl(&state, viewer.inner(), confirmation, destination_path)
    })
    .await
    .map_err(|error| AppError::Library {
        message: format!("Purge worker failed: {error}"),
    })?
}
fn purge_catalog_impl(
    state: &LibraryState,
    viewer: &AppState,
    confirmation: String,
    destination_path: String,
) -> Result<library_backup::BackupResult, AppError> {
    if confirmation != "PURGE" {
        return Err(AppError::Library {
            message: "Purge confirmation must equal PURGE".into(),
        });
    }
    let mut guard = state.begin_maintenance()?;
    let backup = library_backup::create_backup(guard.root(), Path::new(&destination_path))
        .map_err(|message| AppError::Library { message })?;
    let root = guard.root().to_path_buf();
    let parent = root.parent().unwrap();
    let nonce = unique_suffix();
    let staged = parent.join(format!(".gogglelab-empty-{nonce}"));
    LibraryDb::open(&staged).map_err(library_error)?;
    let previous = parent.join(format!("library.previous-purge-{nonce}"));
    guard.invalidate();
    invalidate_viewer(viewer);
    if let Err(failure) = swap_library_dirs(&root, &staged, &previous, false) {
        if !failure.recovered {
            guard.remain_armed();
        }
        return Err(AppError::Library {
            message: failure.message,
        });
    }
    if let Err(error) = LibraryDb::open(&root).and_then(|db| db.list_models(None, None).map(|_| ()))
    {
        if let Err(rollback_error) =
            rollback_validated_replacement(&root, &previous, parent, &format!("purge-{nonce}"))
        {
            guard.remain_armed();
            return Err(AppError::Library{message:format!("Purged catalog failed validation: {error}; rollback failed: {rollback_error}; previous: {}",previous.display())});
        }
        return Err(AppError::Library {
            message: format!(
                "Purged catalog failed validation and previous Library was recovered: {error}"
            ),
        });
    }
    Ok(backup)
}

#[tauri::command]
pub async fn library_cleanup_orphan_cache(
    app: tauri::AppHandle,
    state: tauri::State<'_, LibraryState>,
) -> Result<CleanupResult, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let viewer = app.state::<AppState>();
        let current = viewer
            .current
            .lock()
            .unwrap()
            .as_ref()
            .map(|loaded| PathBuf::from(&loaded.path));
        cleanup_orphan_cache_impl(&state, current.as_deref())
    })
    .await
    .map_err(|error| AppError::Library {
        message: format!("Cache cleanup worker failed: {error}"),
    })?
}
fn cleanup_orphan_cache_impl(
    state: &LibraryState,
    current_viewer: Option<&Path>,
) -> Result<CleanupResult, AppError> {
    let guard = state.begin_maintenance()?;
    let root = guard.root().canonicalize().map_err(map_io)?;
    let current_viewer = current_viewer.and_then(|path| path.canonicalize().ok());
    let db = LibraryDb::open(&root).map_err(library_error)?;
    let referenced = db.thumbnail_relative_paths().map_err(library_error)?;
    let content_hashes = db.catalog_content_hashes().map_err(library_error)?;
    drop(db);
    let thumbnails = root.join("thumbnails");
    let mut files = 0;
    let mut bytes = 0;
    let thumbnail_meta = fs::symlink_metadata(&thumbnails).map_err(map_io)?;
    if thumbnail_meta.file_type().is_symlink() || !thumbnail_meta.is_dir() {
        return Err(AppError::Library {
            message: "Thumbnail namespace is unsafe".into(),
        });
    }
    if thumbnails.is_dir() {
        for entry in fs::read_dir(&thumbnails).map_err(map_io)? {
            let entry = entry.map_err(map_io)?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).map_err(map_io)?;
            if meta.file_type().is_symlink() || !meta.is_file() {
                continue;
            }
            let relative = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if !referenced.contains(&relative)
                && is_generated_thumbnail_name(&entry.file_name().to_string_lossy())
            {
                fs::remove_file(&path).map_err(map_io)?;
                files += 1;
                bytes += meta.len();
            }
        }
    }
    let indexed_cache = root.join("indexed-cache");
    if let Ok(cache_meta) = fs::symlink_metadata(&indexed_cache) {
        if cache_meta.file_type().is_symlink() || !cache_meta.is_dir() {
            return Err(AppError::Library {
                message: "Indexed cache namespace is unsafe".into(),
            });
        }
        for entry in fs::read_dir(&indexed_cache).map_err(map_io)? {
            let entry = entry.map_err(map_io)?;
            let dir = entry.path();
            let meta = fs::symlink_metadata(&dir).map_err(map_io)?;
            let hash = entry.file_name().to_string_lossy().into_owned();
            if meta.file_type().is_symlink()
                || !meta.is_dir()
                || hash.len() != 64
                || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(AppError::Library {
                    message: "Indexed cache contains an unsafe entry".into(),
                });
            }
            if content_hashes.contains(&hash)
                || current_viewer
                    .as_deref()
                    .is_some_and(|path| path.starts_with(&dir))
            {
                continue;
            }
            let mut dir_files = Vec::new();
            for child in fs::read_dir(&dir).map_err(map_io)? {
                let child = child.map_err(map_io)?;
                let path = child.path();
                let meta = fs::symlink_metadata(&path).map_err(map_io)?;
                if meta.file_type().is_symlink() || !meta.is_file() {
                    return Err(AppError::Library {
                        message: "Indexed cache hash directory is unsafe".into(),
                    });
                }
                let (actual_bytes, actual_hash) =
                    hash_file(&path).map_err(|message| AppError::Library { message })?;
                if actual_hash != hash {
                    return Err(AppError::Library {
                        message: "Indexed cache file hash does not match its namespace".into(),
                    });
                }
                dir_files.push((path, actual_bytes));
            }
            for (path, size) in dir_files {
                fs::remove_file(path).map_err(map_io)?;
                files += 1;
                bytes += size;
            }
            fs::remove_dir(&dir).map_err(map_io)?;
        }
    }
    Ok(CleanupResult {
        files,
        bytes,
        handoff_cleanup_skipped: true,
    })
}
#[tauri::command]
pub fn library_management_status(state: tauri::State<'_, LibraryState>) -> ManagementStatus {
    ManagementStatus {
        maintenance: state.maintenance_active(),
        generation: state.management_generation(),
    }
}

fn copy_owned_recovery(
    library_root: &Path,
    source: &Path,
    owned: &Path,
    model_id: &str,
    expected_hash: &str,
    expected_bytes: u64,
) -> Result<(), String> {
    let trash_root = library_root.join("trash");
    let trash_meta = fs::symlink_metadata(&trash_root).map_err(|e| e.to_string())?;
    if trash_meta.file_type().is_symlink() || !trash_meta.is_dir() {
        return Err("Owned trash namespace is unsafe".into());
    }
    if owned.exists() {
        return Err("Owned recovery destination exists".into());
    }
    fs::create_dir(owned).map_err(|e| e.to_string())?;
    let result = (|| {
        let meta = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err("Loose source is not a regular file".into());
        }
        let destination = owned.join("source.bin");
        let mut input = File::open(source).map_err(|e| e.to_string())?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|e| e.to_string())?;
        let copied = std::io::copy(
            &mut (&mut input).take(expected_bytes.saturating_add(1)),
            &mut output,
        )
        .map_err(|e| e.to_string())?;
        let (bytes, hash) = hash_file(&destination)?;
        if copied != expected_bytes || bytes != expected_bytes || hash != expected_hash {
            return Err("Loose source hash changed".into());
        }
        let metadata = TrashMetadata {
            model_id: model_id.into(),
            sha256: hash,
            bytes: expected_bytes,
        };
        fs::write(
            owned.join("metadata.json"),
            serde_json::to_vec(&metadata).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(owned);
    }
    result
}
fn swap_library_dirs(
    root: &Path,
    staged: &Path,
    previous: &Path,
    fail_after_first: bool,
) -> Result<(), SwapFailure> {
    if fs::symlink_metadata(previous).is_ok() || !staged.is_dir() {
        return Err(SwapFailure {
            message: "Swap paths are invalid".into(),
            recovered: true,
        });
    }
    fs::rename(root, previous).map_err(|e| SwapFailure {
        message: e.to_string(),
        recovered: true,
    })?;
    if fail_after_first {
        let rollback = fs::rename(previous, root);
        return Err(SwapFailure {
            message: format!("Injected restore swap failure; rollback: {rollback:?}"),
            recovered: rollback.is_ok(),
        });
    }
    if let Err(error) = fs::rename(staged, root) {
        let rollback = fs::rename(previous, root);
        return Err(SwapFailure {
            message: format!("Restore swap failed: {error}; rollback: {rollback:?}"),
            recovered: rollback.is_ok(),
        });
    }
    Ok(())
}
fn rollback_validated_replacement(
    root: &Path,
    previous: &Path,
    parent: &Path,
    nonce: &str,
) -> Result<PathBuf, String> {
    rollback_validated_replacement_with(root, previous, parent, nonce, false)
}
fn rollback_validated_replacement_with(
    root: &Path,
    previous: &Path,
    parent: &Path,
    nonce: &str,
    inject_move_failure: bool,
) -> Result<PathBuf, String> {
    if inject_move_failure {
        return Err("Injected failed-replacement move failure".into());
    }
    let mut moved = None;
    for attempt in 0..32 {
        let candidate = parent.join(format!(".gogglelab-failed-{nonce}-{attempt}"));
        if fs::symlink_metadata(&candidate).is_ok() {
            continue;
        }
        match fs::rename(root, &candidate) {
            Ok(()) => {
                moved = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not isolate invalid replacement: {error}")),
        }
    }
    let failed = moved.ok_or("Could not allocate an unused failed-replacement path")?;
    if let Err(error) = fs::rename(previous, root) {
        return Err(format!("Previous Library remains at {} after rollback rename failed: {error}; invalid replacement: {}",previous.display(),failed.display()));
    }
    Ok(failed)
}
fn invalidate_viewer(viewer: &AppState) {
    let _guard = viewer.request_commit.lock().unwrap();
    viewer.latest_requested.fetch_add(1, Ordering::SeqCst);
    *viewer.current.lock().unwrap() = None;
}
fn unique_suffix() -> String {
    format!(
        "{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        MANAGEMENT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}
fn verify_file(path: &Path, hash: &str, bytes: u64) -> Result<(), String> {
    let (actual_bytes, actual_hash) = hash_file(path)?;
    if actual_bytes != bytes || actual_hash != hash {
        return Err("Owned recovery copy is invalid".into());
    }
    Ok(())
}
fn hash_file(path: &Path) -> Result<(u64, String), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink()
        || !meta.is_file()
        || meta.len() > stl_core::limits::MAX_FILE_BYTES
    {
        return Err("File is not regular".into());
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let bytes = std::io::copy(
        &mut (&mut file).take(stl_core::limits::MAX_FILE_BYTES + 1),
        &mut digest,
    )
    .map_err(|e| e.to_string())?;
    if bytes > stl_core::limits::MAX_FILE_BYTES {
        return Err("File grew beyond native size limit".into());
    }
    Ok((bytes, format!("{:x}", digest.finalize())))
}
fn validate_snapshot_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("Removal snapshot ID is invalid".into());
    }
    Ok(())
}
fn ensure_idle(state: &LibraryState) -> Result<(), AppError> {
    if state.scan_control().status().running || state.maintenance_active() {
        return Err(AppError::Library {
            message: "Library job or maintenance is in progress".into(),
        });
    }
    Ok(())
}
fn validate_owned_snapshot_dir(library_root: &Path, owned: &Path) -> Result<(), String> {
    let root = library_root.canonicalize().map_err(|e| e.to_string())?;
    let root_meta = fs::symlink_metadata(library_root).map_err(|e| e.to_string())?;
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return Err("Library root identity changed".into());
    }
    let trash = library_root.join("trash");
    let trash_meta = fs::symlink_metadata(&trash).map_err(|e| e.to_string())?;
    let owned_meta = fs::symlink_metadata(owned).map_err(|e| e.to_string())?;
    let canonical_trash = trash.canonicalize().map_err(|e| e.to_string())?;
    let canonical_owned = owned.canonicalize().map_err(|e| e.to_string())?;
    if trash_meta.file_type().is_symlink()
        || !trash_meta.is_dir()
        || canonical_trash != root.join("trash")
        || canonical_owned.parent() != Some(canonical_trash.as_path())
        || owned_meta.file_type().is_symlink()
        || !owned_meta.is_dir()
    {
        return Err("Owned recovery namespace is unsafe".into());
    }
    for name in ["metadata.json", "source.bin"] {
        let path = owned.join(name);
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err("Owned recovery file is unsafe".into());
        }
    }
    Ok(())
}
fn validated_removed_destination(db: &LibraryDb, model: &Model) -> Result<PathBuf, AppError> {
    let root_id = model
        .source_root_id
        .as_deref()
        .ok_or_else(|| AppError::Library {
            message: "Removed model has no index root".into(),
        })?;
    let root = db
        .list_index_roots()
        .map_err(library_error)?
        .into_iter()
        .find(|root| root.id == root_id)
        .ok_or_else(|| AppError::Library {
            message: "Removed model index root is missing".into(),
        })?;
    let canonical = root.path.canonicalize().map_err(map_io)?;
    if canonical != root.path {
        return Err(AppError::Library {
            message: "Index root identity changed".into(),
        });
    }
    let destination =
        PathBuf::from(
            model
                .source_path
                .as_deref()
                .ok_or_else(|| AppError::Library {
                    message: "Removed model source path is missing".into(),
                })?,
        );
    let relative = destination
        .strip_prefix(&canonical)
        .map_err(|_| AppError::Library {
            message: "Removed model source escapes its root".into(),
        })?;
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AppError::Library {
            message: "Removed model source path is unsafe".into(),
        });
    }
    let mut current = canonical;
    for component in &components[..components.len() - 1] {
        let Component::Normal(part) = component else {
            unreachable!()
        };
        current.push(part);
        let meta = fs::symlink_metadata(&current).map_err(map_io)?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(AppError::Library {
                message: "Removed model source parent is unsafe".into(),
            });
        }
    }
    match fs::symlink_metadata(&destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(destination),
        _ => Err(AppError::Library {
            message: "Original destination already exists or is unsafe".into(),
        }),
    }
}
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > limit {
        return Err("Bounded file is unsafe or oversized".into());
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Bounded file grew beyond limit".into());
    }
    Ok(bytes)
}
fn is_generated_thumbnail_name(name: &str) -> bool {
    let Some((hash, suffix)) = name
        .strip_suffix(".png")
        .and_then(|value| value.split_once("-isometric-v"))
    else {
        return false;
    };
    let parts = suffix.split('-').collect::<Vec<_>>();
    hash.len() == 64
        && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        && parts.len() == 3
        && !parts[0].is_empty()
        && parts[0].bytes().all(|byte| byte.is_ascii_digit())
        && matches!(parts[1], "128" | "256" | "512")
        && parts[2].len() == 6
        && parts[2].bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn reveal_source<F>(path: &Path, launch: F) -> Result<(), String>
where
    F: FnOnce(&str, &[std::ffi::OsString]) -> std::io::Result<()>,
{
    let (program, args) = reveal_command(path);
    launch(program, &args).map_err(|e| e.to_string())
}
#[cfg(target_os = "macos")]
fn reveal_command(path: &Path) -> (&'static str, Vec<std::ffi::OsString>) {
    ("open", vec!["-R".into(), path.as_os_str().to_owned()])
}
#[cfg(target_os = "linux")]
fn reveal_command(path: &Path) -> (&'static str, Vec<std::ffi::OsString>) {
    (
        "xdg-open",
        vec![path.parent().unwrap_or(path).as_os_str().to_owned()],
    )
}
#[cfg(target_os = "windows")]
fn reveal_command(path: &Path) -> (&'static str, Vec<std::ffi::OsString>) {
    (
        "explorer",
        vec!["/select,".into(), path.as_os_str().to_owned()],
    )
}
fn map_io(error: std::io::Error) -> AppError {
    crate::error::map_io_error(&error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use library_core::{ScannedModel, SourceRef, VersionedTarget};
    #[test]
    fn failed_swap_rolls_back_original_directory() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let staged = temp.path().join("staged");
        let previous = temp.path().join("previous");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&staged).unwrap();
        fs::write(root.join("marker"), b"old").unwrap();
        assert!(swap_library_dirs(&root, &staged, &previous, true).is_err());
        assert_eq!(fs::read(root.join("marker")).unwrap(), b"old");
        assert!(!previous.exists());
    }
    #[test]
    fn failed_replacement_is_not_hidden_when_rollback_cannot_isolate_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let previous = temp.path().join("previous");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&previous).unwrap();
        fs::write(root.join("invalid"), b"new").unwrap();
        fs::write(previous.join("valid"), b"old").unwrap();
        assert!(
            rollback_validated_replacement_with(&root, &previous, temp.path(), "test", true)
                .is_err()
        );
        assert!(root.join("invalid").is_file());
        assert!(previous.join("valid").is_file());
    }
    #[test]
    fn restore_rejects_triggered_database_without_replacing_current_library() {
        let temp = tempfile::tempdir().unwrap();
        let attack_root = temp.path().join("attack-library");
        LibraryDb::open(&attack_root).unwrap();
        let connection = rusqlite::Connection::open(attack_root.join("library.sqlite3")).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER malicious_metadata_rewrite \
                 AFTER UPDATE ON models BEGIN \
                 UPDATE models SET notes = 'rewritten' WHERE id = NEW.id; \
                 END;",
            )
            .unwrap();
        drop(connection);
        let backup = temp.path().join("triggered.zip");
        library_backup::create_backup(&attack_root, &backup).unwrap();

        let root = temp.path().join("current-library");
        let state = LibraryState::new(root.clone());
        state.with_db(|_| Ok(())).unwrap();
        fs::write(root.join("current-marker"), b"current").unwrap();
        let viewer = AppState::default();
        let error = restore_backup_impl(&state, &viewer, backup.to_string_lossy().into_owned())
            .unwrap_err();
        assert!(matches!(error, AppError::Library { .. }));
        assert_eq!(fs::read(root.join("current-marker")).unwrap(), b"current");
        assert!(LibraryDb::open(&root).is_ok());
        assert!(!state.maintenance_active());
    }
    #[test]
    fn reveal_uses_literal_arguments_for_metacharacter_path() {
        let path = PathBuf::from("/tmp/weird ; $(touch nope) ü.stl");
        let mut captured = None;
        reveal_source(&path, |program, args| {
            captured = Some((program.to_owned(), args.to_vec()));
            Ok(())
        })
        .unwrap();
        let (_, args) = captured.unwrap();
        assert!(args.iter().any(|arg| arg == path.as_os_str()));
    }
    #[test]
    fn snapshot_keys_and_maintenance_admission_fail_closed() {
        for invalid in ["", "../x", "a/b", "x y"] {
            assert!(validate_snapshot_id(invalid).is_err());
        }
        let temp = tempfile::tempdir().unwrap();
        let state = LibraryState::new(temp.path().join("library"));
        state.with_db(|_| Ok(())).unwrap();
        let io = state.begin_io().unwrap();
        assert!(state.begin_maintenance().is_err());
        drop(io);
        let guard = state.begin_maintenance().unwrap();
        assert!(state.with_db(|_| Ok(())).is_err());
        drop(guard);
        assert!(state.with_db(|_| Ok(())).is_ok());
        let fail_closed = LibraryState::new(temp.path().join("fail-closed"));
        fail_closed.with_db(|_| Ok(())).unwrap();
        let mut guard = fail_closed.begin_maintenance().unwrap();
        guard.remain_armed();
        drop(guard);
        assert!(fail_closed.maintenance_active());
        assert!(fail_closed.begin_io().is_err());
    }
    #[test]
    fn later_removal_restore_recovers_owned_source_after_missing_scan_and_restart() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let sources = temp.path().join("sources");
        fs::create_dir_all(&sources).unwrap();
        let source = sources.join("part.stl");
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        fs::write(&source, &bytes).unwrap();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let state = LibraryState::new(library.clone());
        let (model, snapshot, mut root) = state
            .with_db(|db| {
                let root = db
                    .add_index_root(&sources, 60, 1024)
                    .map_err(library_error)?;
                let entry = ScannedModel {
                    relative_path: "part.stl".into(),
                    content_hash: hash.clone(),
                    bytes: bytes.len() as u64,
                    format: "stl-binary".into(),
                    modified_at: 1,
                    display_name: None,
                    archive_member: None,
                    outer_fingerprint: None,
                };
                let added = db
                    .record_scan_entries(&root.id, std::slice::from_ref(&entry))
                    .map_err(library_error)?;
                db.complete_scan_sources_with_new_models(
                    &root.id,
                    &[SourceRef {
                        relative_path: "part.stl".into(),
                        archive_member: None,
                    }],
                    &added.added_model_ids,
                    "1",
                )
                .map_err(library_error)?;
                let model = db
                    .list_models(None, None)
                    .map_err(library_error)?
                    .pop()
                    .unwrap();
                let removal = db
                    .remove_models_recoverably(&[VersionedTarget {
                        model_id: model.id.clone(),
                        expected_version: model.metadata_version,
                    }])
                    .map_err(library_error)?;
                Ok((model, removal.snapshot_id, root))
            })
            .unwrap();
        let owned = library.join("trash").join(format!("source-{snapshot}"));
        copy_owned_recovery(
            &library,
            &source,
            &owned,
            &model.id,
            &hash,
            bytes.len() as u64,
        )
        .unwrap();
        fs::remove_file(&source).unwrap();
        state
            .with_db(|db| {
                db.complete_scan(&root.id, &[], "2")
                    .map_err(library_error)?;
                assert!(db.get_model(&model.id).unwrap().unwrap().source_missing);
                root.enabled = false;
                db.update_index_root(&root).map_err(library_error)?;
                Ok(())
            })
            .unwrap();
        drop(state);

        let state = LibraryState::new(library.clone());
        let mut restored = restore_removal_impl(&state, snapshot).unwrap();
        assert_eq!(restored.len(), 1);
        let restored = restored.pop().unwrap();
        assert_eq!(restored.id, model.id);
        assert!(!restored.source_missing);
        assert_eq!(fs::read(&source).unwrap(), bytes);
        let resolved = state
            .with_db(|db| {
                db.resolve_model_source_ref(&model.id)
                    .map_err(library_error)
            })
            .unwrap();
        assert_eq!(resolved.outer_path, source.canonicalize().unwrap());
        assert_eq!(resolved.content_hash, hash);
        assert_eq!(resolved.bytes, bytes.len() as u64);
    }

    #[test]
    fn later_source_restore_conflict_keeps_removed_record_and_owned_copy() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let sources = temp.path().join("sources");
        fs::create_dir_all(&sources).unwrap();
        let source = sources.join("part.stl");
        let bytes = b"trusted source";
        fs::write(&source, bytes).unwrap();
        let hash = format!("{:x}", Sha256::digest(bytes));
        let state = LibraryState::new(library.clone());
        let (model, snapshot) = state
            .with_db(|db| {
                let root = db
                    .add_index_root(&sources, 60, 1024)
                    .map_err(library_error)?;
                let entry = ScannedModel {
                    relative_path: "part.stl".into(),
                    content_hash: hash.clone(),
                    bytes: bytes.len() as u64,
                    format: "stl-binary".into(),
                    modified_at: 1,
                    display_name: None,
                    archive_member: None,
                    outer_fingerprint: None,
                };
                let added = db
                    .record_scan_entries(&root.id, std::slice::from_ref(&entry))
                    .map_err(library_error)?;
                db.complete_scan_sources_with_new_models(
                    &root.id,
                    &[SourceRef {
                        relative_path: "part.stl".into(),
                        archive_member: None,
                    }],
                    &added.added_model_ids,
                    "1",
                )
                .map_err(library_error)?;
                let model = db.list_models(None, None).unwrap().pop().unwrap();
                let removal = db
                    .remove_models_recoverably(&[VersionedTarget {
                        model_id: model.id.clone(),
                        expected_version: model.metadata_version,
                    }])
                    .map_err(library_error)?;
                Ok((model, removal.snapshot_id))
            })
            .unwrap();
        let owned = library.join("trash").join(format!("source-{snapshot}"));
        copy_owned_recovery(
            &library,
            &source,
            &owned,
            &model.id,
            &hash,
            bytes.len() as u64,
        )
        .unwrap();
        fs::remove_file(&source).unwrap();
        fs::write(&source, b"conflicting replacement").unwrap();

        assert!(restore_removal_impl(&state, snapshot.clone()).is_err());
        assert!(owned.join("source.bin").is_file());
        assert_eq!(fs::read(&source).unwrap(), b"conflicting replacement");
        let removed = state
            .with_db(|db| db.list_removed_models().map_err(library_error))
            .unwrap();
        assert!(removed
            .iter()
            .any(|item| item.snapshot_id == snapshot && item.model.id == model.id));
    }
    #[test]
    fn orphan_indexed_cache_cleanup_verifies_hash_and_preserves_current_viewer() {
        let temp = tempfile::tempdir().unwrap();
        let state = LibraryState::new(temp.path().join("library"));
        state.with_db(|_| Ok(())).unwrap();
        let bytes = b"cached member";
        let hash = format!("{:x}", Sha256::digest(bytes));
        let dir = state.root().join("indexed-cache").join(&hash);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("part.stl");
        fs::write(&file, bytes).unwrap();
        let preserved = cleanup_orphan_cache_impl(&state, Some(&file)).unwrap();
        assert_eq!(preserved.files, 0);
        assert!(file.exists());
        let removed = cleanup_orphan_cache_impl(&state, None).unwrap();
        assert_eq!(removed.files, 1);
        assert!(!dir.exists());
        assert!(removed.handoff_cleanup_skipped);
    }
}
