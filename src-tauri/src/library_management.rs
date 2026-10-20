use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use library_core::{
    DuplicateGroup, IndexRoot, LibraryDb, Model, RemovalResult, RemovedModel, VersionedTarget,
};
use library_runtime::{GeneratedControlKind, GeneratedTransactionJournal};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::Manager;

use crate::extension_api::{AppError, AppState};
use crate::library_commands::{library_error, LibraryState};

static MANAGEMENT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
struct SwapFailure {
    message: String,
    recovered: bool,
}

struct OwnedScratchDirectory {
    parent: library_runtime::GuardedDirectoryParent,
    name: std::ffi::OsString,
    directory: library_runtime::GuardedDirectory,
}

impl OwnedScratchDirectory {
    fn create(prefix: &str) -> Result<Self, AppError> {
        let parent_path = std::env::temp_dir().canonicalize().map_err(map_io)?;
        let parent = library_runtime::GuardedDirectoryParent::open(&parent_path).map_err(map_io)?;
        let name = std::ffi::OsString::from(format!(".gogglelab-{prefix}-{}", unique_suffix()));
        let directory = parent.create_child_directory(&name).map_err(map_io)?;
        Ok(Self {
            parent,
            name,
            directory,
        })
    }

    fn path(&self) -> Result<PathBuf, AppError> {
        self.directory.io_path().map_err(map_io)
    }
}

impl Drop for OwnedScratchDirectory {
    fn drop(&mut self) {
        let _ = self.parent.remove_child_tree(&self.name);
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_progress: Option<library_protocol::ThumbnailProgress>,
    /// Generated, rebuildable files permanently removed by this cleanup.
    pub files: u64,
    pub bytes: u64,
    pub issues: Vec<String>,
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
    #[serde(default)]
    held_source: Option<HeldTrashSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HeldTrashSource {
    hold_name: String,
    journal_name: String,
    device: u64,
    inode: u64,
    mode: u32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TauriPurgeJournal {
    version: u32,
    root_path: String,
    nonce: String,
    staged_name: String,
    previous_name: String,
    committed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TauriRestoreJournal {
    version: u32,
    root_path: String,
    nonce: String,
    staged_name: String,
    previous_name: String,
    failed_name: String,
    #[serde(default)]
    committed: bool,
}

#[cfg(test)]
fn purge_journal_name_for_runtime(root: &Path) -> Result<std::ffi::OsString, AppError> {
    let digest = Sha256::digest(root.as_os_str().as_encoded_bytes());
    let suffix = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(std::ffi::OsString::from(format!(
        ".gogglelab-purge-{suffix}.journal"
    )))
}

#[cfg(test)]
fn restore_journal_name_for_runtime(root: &Path) -> Result<std::ffi::OsString, AppError> {
    let digest = Sha256::digest(root.as_os_str().as_encoded_bytes());
    let suffix = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(std::ffi::OsString::from(format!(
        ".gogglelab-restore-{suffix}.journal"
    )))
}

fn write_tauri_restore_journal(
    parent: &library_runtime::GuardedDirectoryParent,
    root: &Path,
    nonce: &str,
    staged_name: &std::ffi::OsStr,
    previous_name: &std::ffi::OsStr,
    failed_name: &std::ffi::OsStr,
) -> Result<GeneratedTransactionJournal, AppError> {
    let root_text = root.to_str().ok_or_else(|| AppError::Library {
        message: "Restore journal requires a Unicode Library path".into(),
    })?;
    let root_name = root.file_name().unwrap_or_default().to_string_lossy();
    let journal = TauriRestoreJournal {
        version: 1,
        root_path: root_text.to_owned(),
        nonce: nonce.to_owned(),
        staged_name: staged_name.to_string_lossy().into_owned(),
        previous_name: previous_name.to_string_lossy().into_owned(),
        failed_name: failed_name.to_string_lossy().into_owned(),
        committed: false,
    };
    if nonce.len() != 32
        || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
        || journal.staged_name != format!(".gogglelab-restore-{nonce}")
        || journal.previous_name != format!("{root_name}.previous-{nonce}")
        || journal.failed_name != format!(".gogglelab-failed-restore-{nonce}")
    {
        return Err(AppError::Library {
            message: "Restore journal does not match the Library root".into(),
        });
    }
    let bytes = serde_json::to_vec(&journal).map_err(|error| AppError::Library {
        message: error.to_string(),
    })?;
    GeneratedTransactionJournal::create(parent, root, GeneratedControlKind::Restore, &bytes)
        .map_err(map_io)
}
fn mark_tauri_restore_committed(
    parent: &library_runtime::GuardedDirectoryParent,
    root: &Path,
    journal: &mut GeneratedTransactionJournal,
    nonce: &str,
) -> Result<(), AppError> {
    if journal.root() != root
        || journal.nonce() != nonce
        || journal.kind() != GeneratedControlKind::Restore
    {
        return Err(AppError::Library {
            message: "Restore journal changed before commit".into(),
        });
    }
    journal.mark_committed(parent).map_err(map_io)
}

fn finish_tauri_restore_commit(
    parent: &library_runtime::GuardedDirectoryParent,
    root: &Path,
    journal_name: &mut GeneratedTransactionJournal,
    nonce: &str,
) -> Result<(), AppError> {
    mark_tauri_restore_committed(parent, root, journal_name, nonce)?;
    clear_tauri_restore_journal(parent, journal_name)
}

fn clear_tauri_restore_journal(
    parent: &library_runtime::GuardedDirectoryParent,
    journal: &mut GeneratedTransactionJournal,
) -> Result<(), AppError> {
    journal.clear(parent).map_err(map_io)
}

fn write_tauri_purge_journal(
    parent: &library_runtime::GuardedDirectoryParent,
    root: &Path,
    nonce: &str,
    staged_name: &std::ffi::OsStr,
    previous_name: &std::ffi::OsStr,
) -> Result<GeneratedTransactionJournal, AppError> {
    let root_text = root.to_str().ok_or_else(|| AppError::Library {
        message: "Purge journal requires a Unicode Library path".into(),
    })?;
    let root_name = root.file_name().unwrap_or_default().to_string_lossy();
    let journal = TauriPurgeJournal {
        version: 1,
        root_path: root_text.to_owned(),
        nonce: nonce.to_owned(),
        staged_name: staged_name.to_string_lossy().into_owned(),
        previous_name: previous_name.to_string_lossy().into_owned(),
        committed: false,
    };
    if nonce.len() != 32
        || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
        || journal.staged_name != format!(".gogglelab-empty-{nonce}")
        || journal.previous_name != format!("{root_name}.previous-purge-{nonce}")
    {
        return Err(AppError::Library {
            message: "Purge journal does not match the Library root".into(),
        });
    }
    let bytes = serde_json::to_vec(&journal).map_err(|error| AppError::Library {
        message: error.to_string(),
    })?;
    GeneratedTransactionJournal::create(parent, root, GeneratedControlKind::Purge, &bytes)
        .map_err(map_io)
}
fn mark_tauri_purge_journal_committed(
    parent: &library_runtime::GuardedDirectoryParent,
    root: &Path,
    journal: &mut GeneratedTransactionJournal,
    nonce: &str,
) -> Result<(), AppError> {
    if journal.root() != root
        || journal.nonce() != nonce
        || journal.kind() != GeneratedControlKind::Purge
    {
        return Err(AppError::Library {
            message: "Purge journal changed before commit".into(),
        });
    }
    journal.mark_committed(parent).map_err(map_io)
}

fn clear_tauri_purge_journal(
    parent: &library_runtime::GuardedDirectoryParent,
    journal: &mut GeneratedTransactionJournal,
) -> Result<(), AppError> {
    journal.clear(parent).map_err(map_io)
}

fn index_root_settings_match(actual: &[IndexRoot], expected: &[IndexRoot]) -> bool {
    actual.len() == expected.len()
        && actual.iter().all(|root| {
            expected.iter().any(|candidate| {
                root.id == candidate.id
                    && root.path == candidate.path
                    && root.enabled == candidate.enabled
                    && root.scan_interval_minutes == candidate.scan_interval_minutes
                    && root.max_file_bytes == candidate.max_file_bytes
                    && root.scan_zip == candidate.scan_zip
            })
        })
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
    if snapshot_id.starts_with("source-trash-") {
        let metadata_path = owned.join("metadata.json");
        match fs::symlink_metadata(&metadata_path) {
            Ok(_) => return undo_trash_impl(state, snapshot_id).map(|model| vec![model]),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(map_io(error)),
        }
        return restore_source_trash_without_metadata(state, snapshot_id);
    }
    match fs::symlink_metadata(&owned) {
        Ok(_) => undo_trash_impl(state, snapshot_id).map(|model| vec![model]),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure_idle(state)?;
            state.with_db(|db| db.restore_removal(&snapshot_id).map_err(library_error))
        }
        Err(error) => Err(map_io(error)),
    }
}

fn restore_source_trash_without_metadata(
    state: &LibraryState,
    snapshot_id: String,
) -> Result<Vec<Model>, AppError> {
    ensure_idle(state)?;
    let guard = state.begin_source_mutation_maintenance()?;
    let db = guard.catalog().map_err(AppError::from)?;
    let removed = db
        .list_removed_models()
        .map_err(library_error)?
        .into_iter()
        .filter(|removed| removed.snapshot_id == snapshot_id)
        .collect::<Vec<_>>();
    if removed.is_empty() {
        // A retry after the catalog commit is already idempotent.
        return Ok(Vec::new());
    }
    if removed.len() != 1
        || removed[0].model.source_kind != "indexed"
        || removed[0].model.source_archive_member.is_some()
    {
        return db.restore_removal(&snapshot_id).map_err(library_error);
    }
    let model = &removed[0].model;
    let expected_hash = model.content_hash.clone().unwrap_or_default();
    let destination = validated_removed_destination(db, model)?;
    let destination_parent_path = destination.parent().ok_or_else(|| AppError::Library {
        message: "Original destination parent is invalid".into(),
    })?;
    let destination_name = destination.file_name().ok_or_else(|| AppError::Library {
        message: "Original destination name is invalid".into(),
    })?;
    let destination_parent =
        library_runtime::GuardedDirectoryParent::open(destination_parent_path).map_err(map_io)?;
    if !destination_parent
        .matches_path(destination_parent_path)
        .map_err(map_io)?
    {
        return Err(AppError::Library {
            message: "Original destination parent changed before source recovery".into(),
        });
    }
    let (hold_name, journal_name) = held_source_names(&snapshot_id, &model.id);
    let held_source = HeldTrashSource {
        hold_name: hold_name.to_string_lossy().into_owned(),
        journal_name: journal_name.to_string_lossy().into_owned(),
        device: 0,
        inode: 0,
        mode: 0,
    };
    ensure_held_source_restored(
        &destination_parent,
        destination_name,
        &held_source,
        model.bytes,
        &expected_hash,
        &destination,
    )?;
    let destination_directory =
        library_runtime::GuardedDirectory::open(destination_parent_path).map_err(map_io)?;
    if !destination_directory
        .matches_path(destination_parent_path)
        .map_err(map_io)?
    {
        return Err(AppError::Library {
            message: "Original destination parent changed after source recovery".into(),
        });
    }
    let modified_at = destination_directory
        .modified_nanos(destination_name)
        .map_err(map_io)?;
    if !db.catalog_path_matches() {
        return Err(AppError::Library {
            message: "Library catalog changed during source recovery; the restored source remains in place and the catalog pin was preserved".into(),
        });
    }
    let restored = db
        .restore_removal_with_owned_source(
            &snapshot_id,
            &model.id,
            &expected_hash,
            model.bytes,
            modified_at,
        )
        .map_err(library_error)?;
    if !db.catalog_path_matches() {
        return Err(AppError::Library {
            message: "Library catalog changed during source recovery; the restored source remains in place and the catalog pin was preserved".into(),
        });
    }
    Ok(vec![restored])
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
    trash_loose_source_impl_with_hook(state, model_id, expected_version, || {})
}

fn trash_loose_source_impl_with_hook<F>(
    state: &LibraryState,
    model_id: String,
    expected_version: u64,
    before_source_hold: F,
) -> Result<TrashResult, AppError>
where
    F: FnOnce(),
{
    require_handle_relative_management()?;
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
    let source_parent_path = source.parent().ok_or_else(|| AppError::Library {
        message: "Loose source has no parent directory".into(),
    })?;
    let source_name = source.file_name().ok_or_else(|| AppError::Library {
        message: "Loose source has no file name".into(),
    })?;
    let source_parent =
        library_runtime::GuardedDirectoryParent::open(source_parent_path).map_err(map_io)?;
    if !source_parent
        .matches_path(source_parent_path)
        .map_err(map_io)?
    {
        return Err(AppError::Library {
            message: "Loose source parent changed before it could be quarantined".into(),
        });
    }
    // Commit the version-checked catalog removal before moving the file. A
    // crash before the hold leaves the original source in place; a crash after
    // the hold can be recovered from the snapshot-derived journal names.
    let snapshot_id = library_core::new_source_trash_snapshot_id();
    state.with_db(|db| {
        db.remove_models_recoverably_with_snapshot_id(
            &[VersionedTarget {
                model_id: model_id.clone(),
                expected_version,
            }],
            &snapshot_id,
        )
        .map_err(library_error)
    })?;
    before_source_hold();
    state.with_db(|_| Ok(()))?;

    let (hold_name, journal_name) = held_source_names(&snapshot_id, &model_id);
    let held = source_parent
        .hold_verified_file_as(
            source_name,
            bytes,
            &hash,
            library_core::MAX_INDEXED_FILE_BYTES,
            &hold_name,
            &journal_name,
        )
        .map_err(|error| AppError::Library {
            message: format!(
                "Catalog removal is recorded as snapshot {snapshot_id}, but the loose source hold did not complete. Undo will inspect the original path and deterministic recovery journal {}: {error}",
                source_parent_path.join(&journal_name).display()
            ),
        })?;
    if state.with_db(|_| Ok(())).is_err() {
        return Err(AppError::Library {
            message: format!(
                "Source hold is recorded as snapshot {snapshot_id}, but the catalog could not be revalidated after the hold. Recovery data remains at {} with journal {}",
                source_parent_path.join(&held.hold_name).display(),
                source_parent_path.join(&held.journal_name).display()
            ),
        });
    }
    if !source_parent
        .matches_path(source_parent_path)
        .map_err(map_io)?
    {
        return Err(AppError::Library {
            message: format!(
                "Source hold is recorded as snapshot {snapshot_id}, but its parent changed. Recovery data remains at {} with journal {}.",
                source_parent_path.join(&held.hold_name).display(),
                source_parent_path.join(&held.journal_name).display()
            ),
        });
    }

    let owned = state
        .root()
        .join("trash")
        .join(format!("source-{snapshot_id}"));
    let held_metadata = HeldTrashSource {
        hold_name: held.hold_name.to_string_lossy().into_owned(),
        journal_name: held.journal_name.to_string_lossy().into_owned(),
        device: held.device,
        inode: held.inode,
        mode: held.mode,
    };
    if let Err(error) = write_held_recovery_metadata(
        state.root(),
        &owned,
        &TrashMetadata {
            model_id: model_id.clone(),
            sha256: hash,
            bytes,
            held_source: Some(held_metadata),
        },
    ) {
        return Err(AppError::Library {
            message: format!(
                "Source hold is recorded as snapshot {snapshot_id}, but its owned recovery metadata could not be written: {error}; hold {} with journal {} remains recoverable",
                source_parent_path.join(&held.hold_name).display(),
                source_parent_path.join(&held.journal_name).display()
            ),
        });
    }
    Ok(TrashResult {
        snapshot_id,
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
    undo_trash_impl_with_hook(state, snapshot_id, || {})
}

fn undo_trash_impl_with_hook<F>(
    state: &LibraryState,
    snapshot_id: String,
    before_restore: F,
) -> Result<Model, AppError>
where
    F: FnOnce(),
{
    require_handle_relative_management()?;
    validate_snapshot_id(&snapshot_id).map_err(|message| AppError::Library { message })?;
    if snapshot_id.starts_with("source-trash-") {
        before_restore();
        let mut restored = restore_source_trash_without_metadata(state, snapshot_id)?;
        return restored.pop().ok_or_else(|| AppError::Library {
            message: "Source-trash snapshot is already restored or no longer available".into(),
        });
    }
    let guard = state.begin_source_mutation_maintenance()?;
    let db = guard.catalog().map_err(AppError::from)?;
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
    let destination = validated_removed_destination(db, &removed.model)?;
    before_restore();
    if !db.catalog_path_matches() {
        return Err(AppError::Library {
            message: "Library catalog changed before Trash recovery; source recovery data remains available".into(),
        });
    }
    if let Some(held_source) = metadata.held_source.as_ref() {
        if metadata.sha256 != removed.model.content_hash.clone().unwrap_or_default()
            || metadata.bytes != removed.model.bytes
        {
            return Err(AppError::Library {
                message: "Held source metadata does not match the removed catalog record".into(),
            });
        }
        validate_held_source_names(held_source).map_err(|message| AppError::Library { message })?;
        let destination_parent_path = destination.parent().ok_or_else(|| AppError::Library {
            message: "Original destination parent is invalid".into(),
        })?;
        let destination_name = destination.file_name().ok_or_else(|| AppError::Library {
            message: "Original destination name is invalid".into(),
        })?;
        let destination_parent =
            library_runtime::GuardedDirectoryParent::open(destination_parent_path)
                .map_err(map_io)?;
        if !destination_parent
            .matches_path(destination_parent_path)
            .map_err(map_io)?
        {
            return Err(AppError::Library {
                message: "Original destination parent changed before restore".into(),
            });
        }
        ensure_held_source_restored(
            &destination_parent,
            destination_name,
            held_source,
            metadata.bytes,
            &metadata.sha256,
            &destination,
        )?;
        let destination_directory =
            library_runtime::GuardedDirectory::open(destination_parent_path).map_err(map_io)?;
        if !destination_directory
            .matches_path(destination_parent_path)
            .map_err(map_io)?
        {
            return Err(AppError::Library {
                message: "Original destination parent changed after source restore".into(),
            });
        }
        let modified_at = destination_directory
            .modified_nanos(destination_name)
            .map_err(map_io)?;
        let model = db
            .restore_removal_with_owned_source(
                &snapshot_id,
                &metadata.model_id,
                &metadata.sha256,
                metadata.bytes,
                modified_at,
            )
            .map_err(library_error)?;
        if !db.catalog_path_matches() {
            return Err(AppError::Library {
                message: "Library catalog changed during Trash recovery; restored source bytes and recovery metadata remain available".into(),
            });
        }
        if let Ok(trash_parent) =
            library_runtime::GuardedDirectoryParent::open(&guard.root().join("trash"))
        {
            let _ = trash_parent
                .remove_child_tree(std::ffi::OsStr::new(&format!("source-{snapshot_id}")));
        }
        return Ok(model);
    }
    match fs::symlink_metadata(&destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => {
            return Err(AppError::Library {
                message: "Original destination already exists".into(),
            });
        }
    }
    let destination_parent_path = destination.parent().unwrap();
    let destination_name = destination.file_name().unwrap();
    let destination_parent =
        library_runtime::GuardedDirectory::open(destination_parent_path).map_err(map_io)?;
    if !destination_parent
        .matches_path(destination_parent_path)
        .map_err(map_io)?
    {
        return Err(AppError::Library {
            message: "Original destination parent changed before restore".into(),
        });
    }
    if metadata.sha256 != removed.model.content_hash.clone().unwrap_or_default()
        || metadata.bytes != removed.model.bytes
    {
        return Err(AppError::Library {
            message: "Owned recovery metadata does not match catalog".into(),
        });
    }
    let owned_directory = library_runtime::GuardedDirectory::open(&owned).map_err(map_io)?;
    if owned_directory
        .hash_file(
            std::ffi::OsStr::new("source.bin"),
            library_core::MAX_INDEXED_FILE_BYTES,
        )
        .map_err(map_io)?
        != (metadata.bytes, metadata.sha256.clone())
    {
        return Err(AppError::Library {
            message: "Owned recovery bytes do not match catalog".into(),
        });
    }
    destination_parent
        .copy_file_from_directory(
            &owned_directory,
            std::ffi::OsStr::new("source.bin"),
            destination_name,
            metadata.bytes,
            &metadata.sha256,
            library_core::MAX_INDEXED_FILE_BYTES,
        )
        .map_err(map_io)?;
    let copied = destination_parent
        .hash_file(destination_name, library_core::MAX_INDEXED_FILE_BYTES)
        .map_err(map_io)?;
    if copied != (metadata.bytes, metadata.sha256.clone())
        || !destination_parent
            .matches_path(destination_parent_path)
            .map_err(map_io)?
    {
        let _ = destination_parent.remove_verified_file(
            Path::new(destination_name),
            metadata.bytes,
            &metadata.sha256,
            library_core::MAX_INDEXED_FILE_BYTES,
        );
        return Err(AppError::Library {
            message: "Restored source changed before catalog recovery".into(),
        });
    }
    let modified_at = match destination_parent.modified_nanos(destination_name) {
        Ok(value) => value,
        Err(error) => {
            let _ = destination_parent.remove_verified_file(
                Path::new(destination_name),
                metadata.bytes,
                &metadata.sha256,
                library_core::MAX_INDEXED_FILE_BYTES,
            );
            return Err(map_io(error));
        }
    };
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
            if !db.catalog_path_matches() {
                return Err(AppError::Library {
                    message: "Library catalog changed during Trash recovery; restored source bytes and recovery metadata remain available".into(),
                });
            }
            if let Ok(trash_parent) =
                library_runtime::GuardedDirectoryParent::open(&guard.root().join("trash"))
            {
                let _ = trash_parent
                    .remove_child_tree(std::ffi::OsStr::new(&format!("source-{snapshot_id}")));
            }
            Ok(model)
        }
        Err(error) => {
            let _ = destination_parent.remove_verified_file(
                Path::new(destination_name),
                metadata.bytes,
                &metadata.sha256,
                library_core::MAX_INDEXED_FILE_BYTES,
            );
            Err(error)
        }
    }
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
    library_runtime::create_backup_file(guard.root(), Path::new(&destination_path))
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
    require_handle_relative_management()?;
    let guard = state.begin_maintenance()?;
    let root = guard.root().to_path_buf();
    let parent = root.parent().ok_or_else(|| AppError::Library {
        message: "Library root has no parent".into(),
    })?;
    let entropy = unique_suffix();
    let digest = Sha256::digest(entropy.as_bytes());
    let nonce = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let root_name = root
        .file_name()
        .ok_or_else(|| AppError::Library {
            message: "Library root name is invalid".into(),
        })?
        .to_os_string();
    let staged_name = std::ffi::OsString::from(format!(".gogglelab-restore-{nonce}"));
    let previous_name =
        std::ffi::OsString::from(format!("{}.previous-{nonce}", root_name.to_string_lossy()));
    let failed_name = std::ffi::OsString::from(format!(".gogglelab-failed-restore-{nonce}"));
    let parent_handle = library_runtime::GuardedDirectoryParent::open(parent).map_err(map_io)?;
    let original = parent_handle.open_child(&root_name).map_err(map_io)?;
    if !parent_handle.matches_path(parent).map_err(map_io)?
        || !original.matches_path(&root).map_err(map_io)?
    {
        return Err(AppError::Library {
            message: "Library or parent directory identity changed before restore".into(),
        });
    }
    let current_db = LibraryDb::open(&root).map_err(library_error)?;
    if current_db.root() != root {
        return Err(AppError::Library {
            message: "Library root identity changed before restore".into(),
        });
    }
    let floor = current_db.max_metadata_version().map_err(library_error)?;
    drop(current_db);

    let scratch = OwnedScratchDirectory::create("restore-backup")?;
    let restored_name = std::ffi::OsString::from("restored-library");
    let mut restored_directory = scratch
        .directory
        .create_child_directory(&restored_name)
        .map_err(map_io)?;
    let source_path = Path::new(&source_path);
    let source_parent_path = source_path
        .parent()
        .ok_or_else(|| AppError::Library {
            message: "Restore backup path has no parent directory".into(),
        })?
        .canonicalize()
        .map_err(map_io)?;
    let source_parent =
        library_runtime::GuardedDirectory::open(&source_parent_path).map_err(map_io)?;
    let source_name = source_path.file_name().ok_or_else(|| AppError::Library {
        message: "Restore backup path has no file name".into(),
    })?;
    let source_file = source_parent
        .open_file_handle(source_name)
        .map_err(map_io)?;
    library_backup::restore_backup_from_reader(source_file, &mut restored_directory)
        .map_err(|message| AppError::Library { message })?;
    let restored_root = scratch
        .path()?
        .canonicalize()
        .map_err(map_io)?
        .join(&restored_name);
    if !restored_directory
        .matches_path(&restored_root)
        .map_err(map_io)?
    {
        return Err(AppError::Library {
            message: "Restore staging directory identity changed".into(),
        });
    }
    let restored_db = LibraryDb::open(&restored_root).map_err(library_error)?;
    restored_db
        .bump_all_metadata_versions_above(floor)
        .map_err(library_error)?;
    let models = restored_db
        .list_models(None, None)
        .map_err(library_error)?
        .len();
    drop(restored_db);
    if !restored_directory
        .matches_path(&restored_root)
        .map_err(map_io)?
    {
        return Err(AppError::Library {
            message: "Restore staging directory identity changed after catalog validation".into(),
        });
    }
    let expected_database = restored_directory
        .hash_file(
            std::ffi::OsStr::new("library.sqlite3"),
            library_core::MAX_INDEXED_FILE_BYTES,
        )
        .map_err(map_io)?;

    let staged = parent_handle
        .create_child_directory(&staged_name)
        .map_err(map_io)?;
    if let Err(error) =
        staged.copy_tree_from_directory(&restored_directory, library_core::MAX_INDEXED_FILE_BYTES)
    {
        let _ = parent_handle.remove_child_tree(&staged_name);
        return Err(map_io(error));
    }
    if staged
        .hash_file(
            std::ffi::OsStr::new("library.sqlite3"),
            library_core::MAX_INDEXED_FILE_BYTES,
        )
        .map_err(map_io)?
        != expected_database
    {
        let _ = parent_handle.remove_child_tree(&staged_name);
        return Err(AppError::Library {
            message: "Restored catalog staging database changed during copy".into(),
        });
    }
    if !parent_handle.matches_path(parent).map_err(map_io)?
        || !original.matches_path(&root).map_err(map_io)?
    {
        let _ = parent_handle.remove_child_tree(&staged_name);
        return Err(AppError::Library {
            message: "Library or parent directory identity changed before restore swap".into(),
        });
    }

    staged.sync_tree().map_err(map_io)?;
    parent_handle.sync().map_err(map_io)?;
    let mut journal_name = write_tauri_restore_journal(
        &parent_handle,
        &root,
        &nonce,
        &staged_name,
        &previous_name,
        &failed_name,
    )?;

    let generation = guard.invalidate();
    invalidate_viewer(viewer);
    if let Err((message, recovered)) =
        parent_handle.swap_library_children(&root_name, &staged_name, &previous_name)
    {
        if recovered {
            if let Err(cleanup_error) = parent_handle
                .remove_child_tree(&staged_name)
                .and_then(|()| parent_handle.sync())
            {
                guard.remain_armed();
                return Err(AppError::Library {
                    message: format!(
                        "Restore swap failed and staging cleanup is uncertain: {message}; {cleanup_error}"
                    ),
                });
            }
            if let Err(cleanup_error) =
                clear_tauri_restore_journal(&parent_handle, &mut journal_name)
            {
                guard.remain_armed();
                return Err(cleanup_error);
            }
        } else {
            guard.remain_armed();
        }
        return Err(AppError::Library { message });
    }
    if let Err(error) = staged.sync_tree().and_then(|()| parent_handle.sync()) {
        guard.remain_armed();
        return Err(map_io(error));
    }
    let installed_valid = match staged.matches_path(&root) {
        Ok(true) => staged
            .hash_file(
                std::ffi::OsStr::new("library.sqlite3"),
                library_core::MAX_INDEXED_FILE_BYTES,
            )
            .map(|actual| actual == expected_database),
        Ok(false) => Ok(false),
        Err(error) => Err(error),
    };
    match installed_valid {
        Ok(true) => {}
        Ok(false) => {
            guard.remain_armed();
            return Err(AppError::Library {
                message: "Restored Library path changed after installation; previous Library remains recoverable".into(),
            });
        }
        Err(error) => {
            guard.remain_armed();
            return Err(map_io(error));
        }
    }
    if let Err(error) =
        finish_tauri_restore_commit(&parent_handle, &root, &mut journal_name, &nonce)
    {
        guard.remain_armed();
        return Err(error);
    }
    Ok(RestoreResult {
        previous_library_backup: parent.join(&previous_name).to_string_lossy().into_owned(),
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
    require_handle_relative_management()?;
    if confirmation != "PURGE" {
        return Err(AppError::Library {
            message: "Purge confirmation must equal PURGE".into(),
        });
    }
    let guard = state.begin_maintenance()?;
    let root = guard.root().to_path_buf();
    let parent = root.parent().unwrap();
    let root_name = root.file_name().unwrap().to_os_string();
    let parent_handle = library_runtime::GuardedDirectoryParent::open(parent).map_err(map_io)?;
    let original = parent_handle.open_child(&root_name).map_err(map_io)?;
    if !parent_handle.matches_path(parent).map_err(map_io)?
        || !original.matches_path(&root).map_err(map_io)?
    {
        return Err(AppError::Library {
            message: "Library or parent directory identity changed before purge".into(),
        });
    }
    let current_db = LibraryDb::open(&root).map_err(library_error)?;
    if current_db.root() != root || !original.matches_path(&root).map_err(map_io)? {
        return Err(AppError::Library {
            message: "Library root identity changed before purge".into(),
        });
    }
    let expected_roots = current_db.list_index_roots().map_err(library_error)?;
    let destination = Path::new(&destination_path);
    let backup_name = destination.file_name().ok_or_else(|| AppError::Library {
        message: "Backup destination name is invalid".into(),
    })?;
    let backup_parent_path = destination
        .parent()
        .filter(|_| destination.is_absolute())
        .ok_or_else(|| AppError::Library {
            message: "Backup destination must be absolute".into(),
        })?
        .canonicalize()
        .map_err(map_io)?;
    if backup_parent_path.starts_with(&root) {
        return Err(AppError::Library {
            message: "Backup destination must be outside the Library".into(),
        });
    }
    let backup_parent =
        library_runtime::GuardedDirectory::open(&backup_parent_path).map_err(map_io)?;
    if !backup_parent
        .matches_path(&backup_parent_path)
        .map_err(map_io)?
    {
        return Err(AppError::Library {
            message: "Backup destination parent identity changed".into(),
        });
    }
    let scratch = OwnedScratchDirectory::create("purge-backup")?;
    let scratch_backup = scratch.path()?.join(backup_name);
    let root_for_backup = original.io_path().map_err(map_io)?;
    let scratch_result = library_runtime::create_backup_file(&root_for_backup, &scratch_backup)
        .map_err(|message| AppError::Library { message })?;
    if !parent_handle.matches_path(parent).map_err(map_io)?
        || !original.matches_path(&root).map_err(map_io)?
    {
        return Err(AppError::Library {
            message: "Library parent or root changed while creating the purge backup".into(),
        });
    }
    let (backup_bytes, backup_hash) = scratch
        .directory
        .hash_file(backup_name, library_backup::MAX_BACKUP_ARCHIVE_BYTES)
        .map_err(map_io)?;
    if backup_bytes == 0 || backup_hash != scratch_result.sha256 {
        return Err(AppError::Library {
            message: "Library backup failed integrity verification".into(),
        });
    }
    if !backup_parent
        .matches_path(&backup_parent_path)
        .map_err(map_io)?
    {
        return Err(AppError::Library {
            message: "Backup destination parent identity changed during purge".into(),
        });
    }
    let backup_limit = 8 * 1024 * 1024 * 1024 + 16 * 1024 * 1024;
    backup_parent
        .copy_file_from_directory(
            &scratch.directory,
            backup_name,
            backup_name,
            backup_bytes,
            &backup_hash,
            backup_limit,
        )
        .map_err(map_io)?;
    if backup_parent
        .hash_file(backup_name, backup_limit)
        .map_err(map_io)?
        != (backup_bytes, backup_hash.clone())
        || !backup_parent
            .matches_path(&backup_parent_path)
            .map_err(map_io)?
    {
        return Err(AppError::Library {
            message: "Purge backup changed after writing".into(),
        });
    }
    let backup = library_backup::BackupResult {
        path: destination.to_path_buf(),
        files: scratch_result.files,
        bytes: scratch_result.bytes,
        sha256: scratch_result.sha256,
    };
    let nonce_entropy = unique_suffix();
    let nonce_digest = Sha256::digest(nonce_entropy.as_bytes());
    let nonce = nonce_digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let staged_name = std::ffi::OsString::from(format!(".gogglelab-empty-{nonce}"));
    let previous_name = std::ffi::OsString::from(format!(
        "{}.previous-purge-{nonce}",
        root_name.to_string_lossy()
    ));
    let staged = parent_handle
        .create_empty_library_child(&staged_name)
        .map_err(map_io)?;
    let mut marker_receipt = None;
    let staged_setup = (|| {
        let staged_path = staged.io_path().map_err(map_io)?;
        if !staged.matches_path(&staged_path).map_err(map_io)? {
            return Err(AppError::Library {
                message: "Purge staging identity changed before catalog configuration".into(),
            });
        }
        let staged_db = LibraryDb::open(staged_path).map_err(library_error)?;
        staged_db
            .copy_catalog_configuration_from(&current_db)
            .map_err(library_error)?;
        let staged_models = staged_db.list_models(None, None).map_err(library_error)?;
        let staged_roots = staged_db.list_index_roots().map_err(library_error)?;
        if !staged_models.is_empty() || !index_root_settings_match(&staged_roots, &expected_roots) {
            return Err(AppError::Library {
                message: "Purged Library staging failed to preserve configured index roots".into(),
            });
        }
        drop(staged_db);
        if !staged
            .matches_path(&staged.io_path().map_err(map_io)?)
            .map_err(map_io)?
        {
            return Err(AppError::Library {
                message: "Purge staging identity changed during catalog configuration".into(),
            });
        }
        marker_receipt = Some(
            library_runtime::create_generated_purge_marker(&staged, &root, &nonce)
                .map_err(map_io)?,
        );
        staged.sync_tree().map_err(map_io)?;
        parent_handle.sync().map_err(map_io)?;
        Ok(())
    })();
    if let Err(error) = staged_setup {
        let _ = parent_handle.remove_child_tree(&staged_name);
        return Err(error);
    }
    drop(current_db);
    if !parent_handle.matches_path(parent).map_err(map_io)?
        || !original.matches_path(&root).map_err(map_io)?
    {
        let _ = parent_handle.remove_child_tree(&staged_name);
        return Err(AppError::Library {
            message: "Library or parent directory identity changed before purge".into(),
        });
    }
    let mut journal_name = match write_tauri_purge_journal(
        &parent_handle,
        &root,
        &nonce,
        &staged_name,
        &previous_name,
    ) {
        Ok(name) => name,
        Err(error) => {
            let _ = parent_handle.remove_child_tree(&staged_name);
            return Err(error);
        }
    };
    guard.invalidate();
    invalidate_viewer(viewer);
    if let Err((message, recovered)) =
        parent_handle.swap_library_children(&root_name, &staged_name, &previous_name)
    {
        if recovered {
            if let Err(error) = clear_tauri_purge_journal(&parent_handle, &mut journal_name) {
                guard.remain_armed();
                return Err(error);
            }
            let _ = parent_handle.remove_child_tree(&staged_name);
        } else {
            guard.remain_armed();
        }
        return Err(AppError::Library { message });
    }
    let validation = staged.io_path().map_err(map_io).and_then(|staged_path| {
        let db = LibraryDb::open(staged_path).map_err(library_error)?;
        let models = db.list_models(None, None).map_err(library_error)?;
        let roots = db.list_index_roots().map_err(library_error)?;
        if !models.is_empty() || !index_root_settings_match(&roots, &expected_roots) {
            return Err(AppError::Library {
                message: "Purged Library failed model or index-root validation".into(),
            });
        }
        Ok(())
    });
    if !staged.matches_path(&root).map_err(map_io)? {
        guard.remain_armed();
        return Err(AppError::Library {
            message: "Library path changed after purge swap; previous Library remains recoverable"
                .into(),
        });
    }
    if validation.is_err() {
        if let Err(rollback_error) = parent_handle.rollback_library_children(
            &root_name,
            &previous_name,
            &std::ffi::OsString::from(format!(".gogglelab-failed-purge-{nonce}")),
        ) {
            guard.remain_armed();
            return Err(AppError::Library {
                message: format!(
                    "Purged catalog failed validation; rollback failed: {rollback_error}"
                ),
            });
        }
        if let Err(error) = clear_tauri_purge_journal(&parent_handle, &mut journal_name) {
            guard.remain_armed();
            return Err(error);
        }
        return Err(AppError::Library {
            message: "Purged catalog failed validation and previous Library was recovered".into(),
        });
    }
    if let Err(error) =
        mark_tauri_purge_journal_committed(&parent_handle, &root, &mut journal_name, &nonce)
    {
        guard.remain_armed();
        return Err(error);
    }
    let mut marker_receipt = marker_receipt.ok_or_else(|| AppError::Library {
        message: "Purge marker receipt is missing".into(),
    })?;
    if let Err(error) = staged.delete_generated_file(&mut marker_receipt) {
        guard.remain_armed();
        return Err(map_io(error));
    }
    if let Err(error) = clear_tauri_purge_journal(&parent_handle, &mut journal_name) {
        guard.remain_armed();
        return Err(error);
    }
    Ok(backup)
}

fn thumbnail_cleanup_context() -> library_protocol::RequestContext {
    use library_protocol::Capability;
    library_protocol::RequestContext {
        principal_id: "native-thumbnail-cleanup".into(),
        capabilities: std::collections::BTreeSet::from([
            Capability::CatalogRead,
            Capability::CatalogWrite,
            Capability::MediaRead,
            Capability::MediaWrite,
            Capability::Maintenance,
        ]),
        request_id: format!("native-thumbnail-cleanup-{}", unique_suffix()),
    }
}

#[tauri::command]
pub async fn library_review_thumbnail_cleanup(
    state: tauri::State<'_, LibraryState>,
    request: library_protocol::ThumbnailCleanupRequest,
) -> Result<library_protocol::ActionPlan, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state
            .runtime()
            .review_thumbnail_cleanup(&thumbnail_cleanup_context(), request)
            .map_err(AppError::from)
    })
    .await
    .map_err(|error| AppError::Library {
        message: format!("Preview review worker failed: {error}"),
    })?
}

#[tauri::command]
pub async fn library_execute_thumbnail_cleanup(
    app: tauri::AppHandle,
    state: tauri::State<'_, LibraryState>,
    token: String,
) -> Result<CleanupResult, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let viewer = app.state::<AppState>();
        execute_thumbnail_cleanup_with_viewer_lock(&state, &viewer, &token, || {})
    })
    .await
    .map_err(|error| AppError::Library {
        message: format!("Preview cleanup worker failed: {error}"),
    })?
}

#[tauri::command]
pub async fn library_discard_management_action(
    state: tauri::State<'_, LibraryState>,
    token: String,
) -> Result<(), AppError> {
    use library_protocol::ManagementEngine;
    state
        .runtime()
        .discard_action(&thumbnail_cleanup_context(), &token)
        .map_err(AppError::from)
}

fn execute_thumbnail_cleanup_with_viewer_lock(
    state: &LibraryState,
    viewer: &AppState,
    token: &str,
    before_execute: impl FnOnce(),
) -> Result<CleanupResult, AppError> {
    let _request_guard = viewer.request_commit.lock().unwrap();
    viewer.latest_requested.fetch_add(1, Ordering::SeqCst);
    let _current_guard = viewer.current.lock().unwrap();
    state.wait_for_thumbnail_idle()?;
    before_execute();
    execute_thumbnail_cleanup(state, token)
}

fn execute_thumbnail_cleanup(state: &LibraryState, token: &str) -> Result<CleanupResult, AppError> {
    use library_protocol::{ManagementEngine, ManagementResult};
    match state
        .runtime()
        .execute_action(&thumbnail_cleanup_context(), token)
        .map_err(AppError::from)?
    {
        ManagementResult::CleanupCompleted {
            files,
            bytes,
            thumbnail_progress,
        } => Ok(CleanupResult {
            files,
            bytes,
            thumbnail_progress,
            issues: Vec::new(),
            handoff_cleanup_skipped: true,
        }),
        _ => Err(AppError::Library {
            message: "Preview cleanup returned an unexpected result".into(),
        }),
    }
}

#[cfg(test)]
fn cleanup_orphan_cache_impl(
    state: &LibraryState,
    _current_viewer: Option<&Path>,
) -> Result<CleanupResult, AppError> {
    cleanup_orphan_cache_impl_after_root_admission(state, None, || {})
}
#[cfg(test)]
fn cleanup_orphan_cache_impl_after_root_admission(
    state: &LibraryState,
    _current_viewer: Option<&Path>,
    after_admission: impl FnOnce(),
) -> Result<CleanupResult, AppError> {
    let plan = state
        .runtime()
        .review_thumbnail_cleanup(
            &thumbnail_cleanup_context(),
            library_protocol::ThumbnailCleanupRequest {
                mode: library_protocol::ThumbnailCleanupMode::Orphans,
                continuation: None,
            },
        )
        .map_err(AppError::from)?;
    after_admission();
    execute_thumbnail_cleanup(state, &plan.token)
}
#[cfg(test)]
fn cleanup_orphan_cache_with_viewer_lock(
    state: &LibraryState,
    viewer: &AppState,
    after_admission: impl FnOnce(),
) -> Result<CleanupResult, AppError> {
    let plan = state
        .runtime()
        .review_thumbnail_cleanup(
            &thumbnail_cleanup_context(),
            library_protocol::ThumbnailCleanupRequest {
                mode: library_protocol::ThumbnailCleanupMode::Orphans,
                continuation: None,
            },
        )
        .map_err(AppError::from)?;
    execute_thumbnail_cleanup_with_viewer_lock(state, viewer, &plan.token, after_admission)
}

fn require_handle_relative_management() -> Result<(), AppError> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err(AppError::Library {
            message: "This operation is unavailable because handle-relative filesystem mutation is unsupported on this platform".into(),
        })
    }
}
#[tauri::command]
pub fn library_management_status(state: tauri::State<'_, LibraryState>) -> ManagementStatus {
    ManagementStatus {
        maintenance: state.maintenance_active(),
        generation: state.management_generation(),
    }
}

#[cfg(test)]
fn copy_owned_recovery(
    library_root: &Path,
    source: &Path,
    owned: &Path,
    model_id: &str,
    expected_hash: &str,
    expected_bytes: u64,
) -> Result<(), String> {
    let root_metadata = fs::symlink_metadata(library_root).map_err(|e| e.to_string())?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err("Library root is unsafe for owned recovery".into());
    }
    let library_root = library_root.canonicalize().map_err(|e| e.to_string())?;
    let trash_root = library_root.join("trash");
    let trash_directory =
        library_runtime::GuardedDirectoryParent::open(&trash_root).map_err(|e| e.to_string())?;
    let owned_name = owned
        .file_name()
        .ok_or_else(|| "Owned recovery destination is invalid".to_string())?;
    let owned_directory = trash_directory
        .create_child_directory(owned_name)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        let source_parent = source
            .parent()
            .ok_or_else(|| "Loose source has no parent directory".to_string())?
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let source_name = source
            .file_name()
            .ok_or_else(|| "Loose source has no file name".to_string())?;
        owned_directory
            .copy_file_from_path(
                &source_parent,
                source_name,
                std::ffi::OsStr::new("source.bin"),
                expected_bytes,
                expected_hash,
                library_core::MAX_INDEXED_FILE_BYTES,
            )
            .map_err(|e| e.to_string())?;
        let metadata = TrashMetadata {
            model_id: model_id.into(),
            sha256: expected_hash.into(),
            bytes: expected_bytes,
            held_source: None,
        };
        owned_directory
            .write_new_file(
                std::ffi::OsStr::new("metadata.json"),
                &serde_json::to_vec(&metadata).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = trash_directory.remove_child_tree(owned_name);
    }
    result
}

fn write_held_recovery_metadata(
    library_root: &Path,
    owned: &Path,
    metadata: &TrashMetadata,
) -> Result<(), String> {
    let root_metadata = fs::symlink_metadata(library_root).map_err(|error| error.to_string())?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err("Library root is unsafe for held-source recovery metadata".into());
    }
    let canonical_root = library_root
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let trash_path = canonical_root.join("trash");
    let trash = library_runtime::GuardedDirectoryParent::open(&trash_path)
        .map_err(|error| error.to_string())?;
    let owned_name = owned
        .file_name()
        .ok_or_else(|| "Held-source recovery destination is invalid".to_string())?;
    let directory = trash
        .create_child_directory(owned_name)
        .map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(metadata).map_err(|error| error.to_string())?;
    directory
        .write_new_file(std::ffi::OsStr::new("metadata.json"), &bytes)
        .map_err(|error| error.to_string())
}
#[cfg(test)]
fn swap_library_dirs(
    root: &Path,
    staged: &Path,
    previous: &Path,
    fail_after_first: bool,
) -> Result<(), SwapFailure> {
    let Some(parent) = root.parent() else {
        return Err(SwapFailure {
            message: "Swap paths are invalid".into(),
            recovered: true,
        });
    };
    if staged.parent() != Some(parent) || previous.parent() != Some(parent) {
        return Err(SwapFailure {
            message: "Swap paths must share one parent directory".into(),
            recovered: true,
        });
    }
    let parent_handle =
        library_runtime::GuardedDirectoryParent::open(parent).map_err(|error| SwapFailure {
            message: error.to_string(),
            recovered: true,
        })?;
    if !parent_handle
        .matches_path(parent)
        .map_err(|error| SwapFailure {
            message: error.to_string(),
            recovered: true,
        })?
    {
        return Err(SwapFailure {
            message: "Swap parent identity changed".into(),
            recovered: true,
        });
    }
    let root_name = root.file_name().unwrap_or_default();
    let staged_name = staged.file_name().unwrap_or_default();
    let previous_name = previous.file_name().unwrap_or_default();
    parent_handle
        .open_child(staged_name)
        .map_err(|error| SwapFailure {
            message: error.to_string(),
            recovered: true,
        })?;
    parent_handle
        .rename_child_exclusive(root_name, previous_name)
        .map_err(|error| SwapFailure {
            message: error.to_string(),
            recovered: true,
        })?;
    if parent_handle.sync().is_err() {
        return Err(SwapFailure {
            message: "Could not sync saved Library directory".into(),
            recovered: false,
        });
    }
    if fail_after_first {
        let rollback = parent_handle
            .rename_child_exclusive(previous_name, root_name)
            .and_then(|()| parent_handle.sync());
        return Err(SwapFailure {
            message: format!("Injected restore swap failure; rollback: {rollback:?}"),
            recovered: rollback.is_ok(),
        });
    }
    if let Err(error) = parent_handle.rename_child_exclusive(staged_name, root_name) {
        let rollback = parent_handle
            .rename_child_exclusive(previous_name, root_name)
            .and_then(|()| parent_handle.sync());
        return Err(SwapFailure {
            message: format!("Restore swap failed: {error}; rollback: {rollback:?}"),
            recovered: rollback.is_ok(),
        });
    }
    if parent_handle.sync().is_err() {
        return Err(SwapFailure {
            message: "Could not sync the installed Library directory".into(),
            recovered: false,
        });
    }
    Ok(())
}
#[cfg(test)]
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
    let parent_handle =
        library_runtime::GuardedDirectoryParent::open(parent).map_err(|error| error.to_string())?;
    if !parent_handle
        .matches_path(parent)
        .map_err(|error| error.to_string())?
    {
        return Err("Library parent identity changed during rollback".into());
    }
    let root_name = root.file_name().ok_or("Invalid replacement root name")?;
    let previous_name = previous
        .file_name()
        .ok_or("Previous Library name is invalid")?;
    let mut moved = None;
    for attempt in 0..32 {
        let candidate_name =
            std::ffi::OsString::from(format!(".gogglelab-failed-{nonce}-{attempt}"));
        match parent_handle.rename_child_exclusive(root_name, &candidate_name) {
            Ok(()) => {
                parent_handle
                    .sync()
                    .map_err(|error| format!("Could not sync invalid replacement move: {error}"))?;
                moved = Some(parent.join(candidate_name));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not isolate invalid replacement: {error}")),
        }
    }
    let failed = moved.ok_or("Could not allocate an unused failed-replacement path")?;
    if let Err(error) = parent_handle
        .rename_child_exclusive(previous_name, root_name)
        .and_then(|()| parent_handle.sync())
    {
        return Err(format!(
            "Previous Library remains at {} after rollback rename failed: {error}; invalid replacement: {}",
            previous.display(),
            failed.display()
        ));
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
    let metadata_path = owned.join("metadata.json");
    let metadata_meta = fs::symlink_metadata(&metadata_path).map_err(|e| e.to_string())?;
    if metadata_meta.file_type().is_symlink() || !metadata_meta.is_file() {
        return Err("Owned recovery metadata file is unsafe".into());
    }
    let source_copy = owned.join("source.bin");
    match fs::symlink_metadata(&source_copy) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
            return Err("Owned recovery source file is unsafe".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    Ok(())
}

fn validate_held_source_names(held: &HeldTrashSource) -> Result<(), String> {
    let Some(nonce) = held
        .hold_name
        .strip_prefix(".gogglelab-held-")
        .and_then(|name| name.strip_suffix(".data"))
    else {
        return Err("Held source name is invalid".into());
    };
    if nonce.len() != 32
        || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
        || held.journal_name != format!(".gogglelab-held-{nonce}.journal")
    {
        return Err("Held source recovery journal is invalid".into());
    }
    Ok(())
}

fn held_source_names(
    snapshot_id: &str,
    model_id: &str,
) -> (std::ffi::OsString, std::ffi::OsString) {
    let mut digest = Sha256::new();
    digest.update(snapshot_id.as_bytes());
    digest.update([0]);
    digest.update(model_id.as_bytes());
    let digest = digest.finalize();
    let nonce = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let stem = format!(".gogglelab-held-{nonce}");
    (
        format!("{stem}.data").into(),
        format!("{stem}.journal").into(),
    )
}

fn ensure_held_source_restored(
    parent: &library_runtime::GuardedDirectoryParent,
    destination_name: &std::ffi::OsStr,
    held_source: &HeldTrashSource,
    expected_bytes: u64,
    expected_hash: &str,
    destination: &Path,
) -> Result<(), AppError> {
    validate_held_source_names(held_source).map_err(|message| AppError::Library { message })?;
    let expected = (expected_bytes, expected_hash.to_owned());
    match parent.hash_file(destination_name, library_core::MAX_INDEXED_FILE_BYTES) {
        Ok(actual) if actual == expected => return Ok(()),
        Ok(_) => {
            return Err(AppError::Library {
                message: format!(
                    "Original destination {} contains different bytes; held source remains at {} with journal {}",
                    destination.display(), held_source.hold_name, held_source.journal_name
                ),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(map_io(error)),
    }

    let journal_name = std::ffi::OsString::from(&held_source.journal_name);
    let hold_name = std::ffi::OsString::from(&held_source.hold_name);
    let held =
        parent
            .recover_held_entry(&journal_name)
            .map_err(|error| AppError::Library {
                message: format!(
                "Source {} is missing and its hold could not be recovered from {} ({}): {error}",
                destination.display(), held_source.hold_name, held_source.journal_name
            ),
            })?;
    if held.hold_name != hold_name
        || held
            .expected
            .as_ref()
            .is_some_and(|held_expected| held_expected != &expected)
        || (held_source.device != 0 && held.device != held_source.device)
        || (held_source.inode != 0 && held.inode != held_source.inode)
        || (held_source.mode != 0 && held.mode != held_source.mode)
    {
        return Err(AppError::Library {
            message: format!(
                "Held source identity does not match the removed model. Recovery remains at {} with journal {}",
                held_source.hold_name, held_source.journal_name
            ),
        });
    }
    parent
        .restore_held_entry(&held, destination_name)
        .map_err(|error| AppError::Library {
            message: format!(
                "Could not restore the held source without overwriting another file. Recovery remains beside {} as {} with journal {}: {error}",
                destination.display(), held_source.hold_name, held_source.journal_name
            ),
        })?;
    let restored = parent
        .hash_file(destination_name, library_core::MAX_INDEXED_FILE_BYTES)
        .map_err(map_io)?;
    if restored != expected {
        return Err(AppError::Library {
            message: format!(
                "Source bytes restored to {} changed before catalog recovery",
                destination.display()
            ),
        });
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
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => {
            let expected_hash = model.content_hash.as_deref().unwrap_or_default();
            if metadata.len() == model.bytes
                && hash_file(&destination)
                    .is_ok_and(|actual| actual == (model.bytes, expected_hash.to_owned()))
            {
                Ok(destination)
            } else {
                Err(AppError::Library {
                    message:
                        "Original destination exists with bytes that do not match the removed model"
                            .into(),
                })
            }
        }
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

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn library_with_index_root(root: &Path, external: &Path) -> IndexRoot {
        LibraryDb::open(root)
            .unwrap()
            .add_index_root(external, 60, 50 * 1024 * 1024)
            .unwrap()
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn staged_empty_library_with_roots(
        root: &Path,
        staged_name: &std::ffi::OsStr,
        nonce: &str,
    ) -> (
        library_runtime::GuardedDirectoryParent,
        library_runtime::GuardedDirectory,
    ) {
        let parent_path = root.parent().unwrap();
        let parent = library_runtime::GuardedDirectoryParent::open(parent_path).unwrap();
        let staged = parent.create_empty_library_child(staged_name).unwrap();
        let active_db = LibraryDb::open(root).unwrap();
        let staged_path = staged.io_path().unwrap();
        let staged_db = LibraryDb::open(staged_path).unwrap();
        staged_db
            .copy_catalog_configuration_from(&active_db)
            .unwrap();
        assert!(staged_db.list_models(None, None).unwrap().is_empty());
        assert!(index_root_settings_match(
            &staged_db.list_index_roots().unwrap(),
            &active_db.list_index_roots().unwrap()
        ));
        drop(staged_db);
        drop(active_db);
        staged
            .write_new_file(
                std::ffi::OsStr::new(".gogglelab-purge-stage"),
                nonce.as_bytes(),
            )
            .unwrap();
        staged.sync().unwrap();
        parent.sync().unwrap();
        (parent, staged)
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn indexed_source(
        state: &LibraryState,
        source_root: &Path,
        source: &Path,
        bytes: &[u8],
    ) -> Model {
        let hash = format!("{:x}", Sha256::digest(bytes));
        let relative = source.strip_prefix(source_root).unwrap();
        state
            .with_db(|db| {
                let root = db
                    .add_index_root(source_root, 60, 50 * 1024 * 1024)
                    .map_err(library_error)?;
                let entry = ScannedModel {
                    relative_path: relative.to_path_buf(),
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
                        relative_path: relative.to_path_buf(),
                        archive_member: None,
                    }],
                    &added.added_model_ids,
                    "1",
                )
                .map_err(library_error)?;
                db.list_models(None, None)
                    .map_err(library_error)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| AppError::Library {
                        message: "Indexed test source was not recorded".into(),
                    })
            })
            .unwrap()
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn loose_source_trash_uses_a_held_original_and_undo_restores_it_without_copying() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let sources = temp.path().join("sources");
        fs::create_dir_all(&sources).unwrap();
        let sources = sources.canonicalize().unwrap();
        let source = sources.join("part.stl");
        let bytes = b"verified loose source";
        fs::write(&source, bytes).unwrap();
        let state = LibraryState::new(library.clone());
        let model = indexed_source(&state, &sources, &source, bytes);

        let trashed =
            trash_loose_source_impl(&state, model.id.clone(), model.metadata_version).unwrap();
        assert_eq!(trashed.model_id, model.id);
        assert!(!source.exists());
        let owned = library
            .join("trash")
            .join(format!("source-{}", trashed.snapshot_id));
        let metadata: TrashMetadata =
            serde_json::from_slice(&fs::read(owned.join("metadata.json")).unwrap()).unwrap();
        assert!(metadata.held_source.is_some());
        assert!(!owned.join("source.bin").exists());
        let held = metadata.held_source.unwrap();
        let held_path = sources.join(&held.hold_name);
        assert_eq!(fs::read(&held_path).unwrap(), bytes);
        assert!(sources.join(held.journal_name).is_file());

        let restored = undo_trash_impl(&state, trashed.snapshot_id).unwrap();
        assert_eq!(restored.id, model.id);
        assert_eq!(fs::read(&source).unwrap(), bytes);
        assert!(!held_path.exists());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn source_trash_undo_rejects_catalog_replacement_before_restoring_source() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let sources = temp.path().join("sources");
        fs::create_dir_all(&sources).unwrap();
        let sources = sources.canonicalize().unwrap();
        let source = sources.join("part.stl");
        let bytes = b"verified loose source";
        fs::write(&source, bytes).unwrap();
        let state = LibraryState::new(library.clone());
        let model = indexed_source(&state, &sources, &source, bytes);
        let trashed =
            trash_loose_source_impl(&state, model.id.clone(), model.metadata_version).unwrap();
        let owned = library
            .join("trash")
            .join(format!("source-{}", trashed.snapshot_id));
        let metadata: TrashMetadata =
            serde_json::from_slice(&fs::read(owned.join("metadata.json")).unwrap()).unwrap();
        let held = metadata.held_source.unwrap();
        let held_path = sources.join(&held.hold_name);

        let catalog = library.join("library.sqlite3");
        let displaced = library.join("library.sqlite3.displaced");
        fs::rename(&catalog, &displaced).unwrap();
        fs::copy(&displaced, &catalog).unwrap();

        assert!(undo_trash_impl(&state, trashed.snapshot_id).is_err());
        assert!(!source.exists());
        assert_eq!(fs::read(&held_path).unwrap(), bytes);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn stale_loose_source_trash_does_not_move_the_source_before_version_rejection() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let sources = temp.path().join("sources");
        fs::create_dir_all(&sources).unwrap();
        let sources = sources.canonicalize().unwrap();
        let source = sources.join("part.stl");
        let bytes = b"verified loose source";
        fs::write(&source, bytes).unwrap();
        let state = LibraryState::new(library.clone());
        let model = indexed_source(&state, &sources, &source, bytes);

        assert!(
            trash_loose_source_impl(&state, model.id.clone(), model.metadata_version + 1).is_err()
        );
        assert_eq!(fs::read(&source).unwrap(), bytes);
        assert!(state
            .with_db(|db| db.list_removed_models().map_err(library_error))
            .unwrap()
            .is_empty());
        assert!(!fs::read_dir(&sources).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".gogglelab-held-")));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn loose_source_trash_rejects_catalog_swap_before_holding_source() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let sources = temp.path().join("sources");
        fs::create_dir_all(&sources).unwrap();
        let sources = sources.canonicalize().unwrap();
        let source = sources.join("part.stl");
        let bytes = b"verified loose source";
        fs::write(&source, bytes).unwrap();
        let state = LibraryState::new(library.clone());
        let model = indexed_source(&state, &sources, &source, bytes);

        let catalog = library.join("library.sqlite3");
        let replacement = library.join("library.sqlite3.replacement");
        let displaced = library.join("library.sqlite3.displaced");
        fs::copy(&catalog, &replacement).unwrap();
        let result = trash_loose_source_impl_with_hook(
            &state,
            model.id.clone(),
            model.metadata_version,
            move || {
                fs::rename(&catalog, &displaced).unwrap();
                fs::rename(&replacement, &catalog).unwrap();
            },
        );

        assert!(result.is_err());
        assert_eq!(fs::read(&source).unwrap(), bytes);
        assert!(!fs::read_dir(&sources).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".gogglelab-held-")));
        assert!(LibraryDb::open(&library)
            .unwrap()
            .get_model(&model.id)
            .unwrap()
            .is_some());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn source_trash_undo_recovers_deterministic_hold_without_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let sources = temp.path().join("sources");
        fs::create_dir_all(&sources).unwrap();
        let sources = sources.canonicalize().unwrap();
        let source = sources.join("part.stl");
        let bytes = b"verified loose source";
        fs::write(&source, bytes).unwrap();
        let hash = format!("{:x}", Sha256::digest(bytes));
        let state = LibraryState::new(library.clone());
        let model = indexed_source(&state, &sources, &source, bytes);
        let snapshot_id = library_core::new_source_trash_snapshot_id();

        state
            .with_db(|db| {
                db.remove_models_recoverably_with_snapshot_id(
                    &[VersionedTarget {
                        model_id: model.id.clone(),
                        expected_version: model.metadata_version,
                    }],
                    &snapshot_id,
                )
                .map_err(library_error)
            })
            .unwrap();
        let (hold_name, journal_name) = held_source_names(&snapshot_id, &model.id);
        let parent = library_runtime::GuardedDirectoryParent::open(&sources).unwrap();
        parent
            .hold_verified_file_as(
                source.file_name().unwrap(),
                bytes.len() as u64,
                &hash,
                library_core::MAX_INDEXED_FILE_BYTES,
                &hold_name,
                &journal_name,
            )
            .unwrap();
        assert!(!library
            .join("trash")
            .join(format!("source-{snapshot_id}"))
            .exists());

        let restored = restore_removal_impl(&state, snapshot_id).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, model.id);
        assert_eq!(fs::read(&source).unwrap(), bytes);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn source_trash_undo_after_catalog_commit_verifies_the_still_active_source() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let source_root = temp.path().join("sources");
        fs::create_dir_all(&source_root).unwrap();
        let sources = source_root.canonicalize().unwrap();
        let source = sources.join("part.stl");
        let bytes = b"verified loose source";
        fs::write(&source, bytes).unwrap();
        let state = LibraryState::new(library);
        let model = indexed_source(&state, &sources, &source, bytes);
        let snapshot_id = library_core::new_source_trash_snapshot_id();
        state
            .with_db(|db| {
                db.remove_models_recoverably_with_snapshot_id(
                    &[VersionedTarget {
                        model_id: model.id.clone(),
                        expected_version: model.metadata_version,
                    }],
                    &snapshot_id,
                )
                .map_err(library_error)
            })
            .unwrap();
        assert_eq!(fs::read(&source).unwrap(), bytes);

        let restored = restore_removal_impl(&state, snapshot_id).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, model.id);
        assert_eq!(fs::read(&source).unwrap(), bytes);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn source_trash_undo_retry_accepts_a_source_restored_before_catalog_commit() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let sources = temp.path().join("sources");
        fs::create_dir_all(&sources).unwrap();
        let sources = sources.canonicalize().unwrap();
        let source = sources.join("part.stl");
        let bytes = b"verified loose source";
        fs::write(&source, bytes).unwrap();
        let state = LibraryState::new(library.clone());
        let model = indexed_source(&state, &sources, &source, bytes);
        let trashed =
            trash_loose_source_impl(&state, model.id.clone(), model.metadata_version).unwrap();
        let owned = library
            .join("trash")
            .join(format!("source-{}", trashed.snapshot_id));
        let metadata: TrashMetadata =
            serde_json::from_slice(&fs::read(owned.join("metadata.json")).unwrap()).unwrap();
        let held_source = metadata.held_source.unwrap();
        let parent = library_runtime::GuardedDirectoryParent::open(&sources).unwrap();
        let journal_name = std::ffi::OsString::from(&held_source.journal_name);
        let held = parent.recover_held_entry(&journal_name).unwrap();
        parent
            .restore_held_entry(&held, source.file_name().unwrap())
            .unwrap();
        assert_eq!(fs::read(&source).unwrap(), bytes);

        let restored = undo_trash_impl(&state, trashed.snapshot_id).unwrap();
        assert_eq!(restored.id, model.id);
        assert_eq!(fs::read(&source).unwrap(), bytes);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn loose_source_undo_collision_retains_quarantined_original() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let sources = temp.path().join("sources");
        fs::create_dir_all(&sources).unwrap();
        let source = sources.join("part.stl");
        let bytes = b"verified loose source";
        fs::write(&source, bytes).unwrap();
        let state = LibraryState::new(library.clone());
        let model = indexed_source(&state, &sources, &source, bytes);

        let trashed =
            trash_loose_source_impl(&state, model.id.clone(), model.metadata_version).unwrap();
        let owned = library
            .join("trash")
            .join(format!("source-{}", trashed.snapshot_id));
        let metadata: TrashMetadata =
            serde_json::from_slice(&fs::read(owned.join("metadata.json")).unwrap()).unwrap();
        let held = metadata.held_source.unwrap();
        let held_path = sources.join(&held.hold_name);
        let error = undo_trash_impl_with_hook(&state, trashed.snapshot_id.clone(), || {
            fs::write(&source, b"concurrent replacement").unwrap();
        })
        .unwrap_err();
        assert!(matches!(error, AppError::Library { message } if message.contains("do not match")));
        assert_eq!(fs::read(&source).unwrap(), b"concurrent replacement");
        assert_eq!(fs::read(&held_path).unwrap(), bytes);
        let removed = state
            .with_db(|db| db.list_removed_models().map_err(library_error))
            .unwrap();
        assert!(removed
            .iter()
            .any(|item| item.snapshot_id == trashed.snapshot_id));
    }

    #[test]
    fn failed_swap_rolls_back_original_directory() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let root = temp_root.join("library");
        let staged = temp_root.join("staged");
        let previous = temp_root.join("previous");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&staged).unwrap();
        fs::write(root.join("marker"), b"old").unwrap();
        let failure = swap_library_dirs(&root, &staged, &previous, true).unwrap_err();
        assert!(failure.recovered);
        assert!(failure.message.contains("Injected restore swap failure"));
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

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn restore_backup_impl_commits_the_installed_library_and_keeps_previous_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let source_library = temp.path().join("source-library");
        LibraryDb::open(&source_library)
            .unwrap()
            .create_model("Restored model", None)
            .unwrap();
        let backup = temp.path().join("source-library.zip");
        library_backup::create_backup(&source_library, &backup).unwrap();

        let root = temp.path().join("current-library");
        LibraryDb::open(&root)
            .unwrap()
            .create_model("Existing model", None)
            .unwrap();
        let state = LibraryState::new(root.clone());
        let viewer = AppState::default();
        let result =
            restore_backup_impl(&state, &viewer, backup.to_string_lossy().into_owned()).unwrap();

        assert_eq!(result.models, 1);
        let restored_models = state
            .with_db(|db| db.list_models(None, None).map_err(library_error))
            .unwrap();
        assert_eq!(restored_models[0].display_name, "Restored model");
        let previous_models = LibraryDb::open(&result.previous_library_backup)
            .unwrap()
            .list_models(None, None)
            .unwrap();
        assert_eq!(previous_models[0].display_name, "Existing model");
        let canonical_root = root.canonicalize().unwrap();
        let journal = restore_journal_name_for_runtime(&canonical_root).unwrap();
        assert!(!canonical_root.parent().unwrap().join(journal).exists());
        assert!(!state.maintenance_active());
    }

    #[cfg(unix)]
    #[test]
    fn native_transaction_journal_cycles_leave_no_generated_recovery_holds() {
        let temp = tempfile::tempdir().unwrap();
        for (kind, nonce) in [("purge", "a".repeat(32)), ("restore", "b".repeat(32))] {
            let root = temp.path().join(kind);
            fs::create_dir(&root).unwrap();
            fs::write(root.join("sentinel"), b"preserve native library").unwrap();
            let root = root.canonicalize().unwrap();
            let parent =
                library_runtime::GuardedDirectoryParent::open(root.parent().unwrap()).unwrap();
            let previous = std::ffi::OsString::from(if kind == "purge" {
                format!("purge.previous-purge-{nonce}")
            } else {
                format!("restore.previous-{nonce}")
            });
            if kind == "purge" {
                let staged = std::ffi::OsString::from(format!(".gogglelab-empty-{nonce}"));
                let mut journal =
                    write_tauri_purge_journal(&parent, &root, &nonce, &staged, &previous).unwrap();
                let directory = library_runtime::GuardedDirectory::open(&root).unwrap();
                let mut marker =
                    library_runtime::create_generated_purge_marker(&directory, &root, &nonce)
                        .unwrap();
                mark_tauri_purge_journal_committed(&parent, &root, &mut journal, &nonce).unwrap();
                directory.delete_generated_file(&mut marker).unwrap();
                clear_tauri_purge_journal(&parent, &mut journal).unwrap();
            } else {
                let staged = std::ffi::OsString::from(format!(".gogglelab-restore-{nonce}"));
                let failed = std::ffi::OsString::from(format!(".gogglelab-failed-restore-{nonce}"));
                let mut journal = write_tauri_restore_journal(
                    &parent, &root, &nonce, &staged, &previous, &failed,
                )
                .unwrap();
                finish_tauri_restore_commit(&parent, &root, &mut journal, &nonce).unwrap();
            }
            assert_eq!(
                fs::read(root.join("sentinel")).unwrap(),
                b"preserve native library"
            );
            assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        }
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn restore_archive_writes_stay_in_the_existing_child_handle_after_path_swap() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let source_library = temp_root.join("source-library");
        LibraryDb::open(&source_library).unwrap();
        let backup = temp_root.join("backup.zip");
        library_backup::create_backup(&source_library, &backup).unwrap();

        let scratch_path = temp_root.join("scratch");
        let outside = temp_root.join("outside");
        fs::create_dir(&scratch_path).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("preserve.txt"), b"external sentinel").unwrap();
        let scratch = library_runtime::GuardedDirectory::open(&scratch_path).unwrap();
        let stage_name = std::ffi::OsString::from("restored-library");
        let mut stage = scratch.create_child_directory(&stage_name).unwrap();
        let stage_path = scratch_path.join(&stage_name);
        let displaced = temp_root.join("displaced-staging");
        let ready = Arc::new(Barrier::new(2));
        let proceed = Arc::new(Barrier::new(2));
        let worker_ready = ready.clone();
        let worker_proceed = proceed.clone();
        let backup_thread = backup.clone();
        let handle = thread::spawn(move || {
            worker_ready.wait();
            worker_proceed.wait();
            let result = library_backup::restore_backup_to_target(&backup_thread, &mut stage);
            (result, stage)
        });

        ready.wait();
        fs::rename(&stage_path, &displaced).unwrap();
        std::os::unix::fs::symlink(&outside, &stage_path).unwrap();
        proceed.wait();
        let (result, stage) = handle.join().unwrap();

        result.unwrap();
        assert!(stage.matches_path(&displaced).unwrap());
        assert!(!stage.matches_path(&stage_path).unwrap_or(false));
        assert_eq!(
            fs::read(outside.join("preserve.txt")).unwrap(),
            b"external sentinel"
        );
        assert!(!outside.join("library.sqlite3").exists());
        assert!(displaced.join("library.sqlite3").is_file());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn tauri_purge_journal_recovers_precommit_and_preserves_committed_roots() {
        let temp = tempfile::tempdir().unwrap();
        let external = temp.path().join("external-models");
        fs::create_dir(&external).unwrap();

        // The crash window after the original root is moved but before the staged
        // empty catalog is installed must restore the previous root and roots.
        let first_root = temp.path().join("first-library");
        let first_root_entry = library_with_index_root(&first_root, &external);
        let first_root = first_root.canonicalize().unwrap();
        let first_nonce = "d".repeat(32);
        let first_staged_name = std::ffi::OsString::from(format!(".gogglelab-empty-{first_nonce}"));
        let first_previous_name = std::ffi::OsString::from(format!(
            "{}.previous-purge-{first_nonce}",
            first_root.file_name().unwrap().to_string_lossy()
        ));
        let (first_parent, _first_staged) =
            staged_empty_library_with_roots(&first_root, &first_staged_name, &first_nonce);
        let first_journal = write_tauri_purge_journal(
            &first_parent,
            &first_root,
            &first_nonce,
            &first_staged_name,
            &first_previous_name,
        )
        .unwrap();
        first_parent
            .rename_child_exclusive(first_root.file_name().unwrap(), &first_previous_name)
            .unwrap();
        first_parent.sync().unwrap();
        drop(first_parent);

        let first_state = LibraryState::try_new_with_config(
            first_root.clone(),
            temp.path().join("first-integrations.json"),
        )
        .unwrap();
        let first_roots = first_state
            .with_db(|db| db.list_index_roots().map_err(library_error))
            .unwrap();
        assert_eq!(first_roots.len(), 1);
        assert_eq!(first_roots[0].id, first_root_entry.id);
        assert_eq!(first_roots[0].path, first_root_entry.path);
        assert!(!first_root
            .parent()
            .unwrap()
            .join(&first_previous_name)
            .exists());
        assert!(!first_root
            .parent()
            .unwrap()
            .join(&first_staged_name)
            .exists());
        assert!(!first_root.parent().unwrap().join(&first_journal).exists());
        drop(first_state);

        // A committed journal distinguishes the new root from an interrupted
        // swap, so startup keeps the configured roots and previous catalog.
        let second_root = temp.path().join("second-library");
        let second_root_entry = library_with_index_root(&second_root, &external);
        let second_root = second_root.canonicalize().unwrap();
        let second_nonce = "e".repeat(32);
        let second_staged_name =
            std::ffi::OsString::from(format!(".gogglelab-empty-{second_nonce}"));
        let second_previous_name = std::ffi::OsString::from(format!(
            "{}.previous-purge-{second_nonce}",
            second_root.file_name().unwrap().to_string_lossy()
        ));
        let (second_parent, _second_staged) =
            staged_empty_library_with_roots(&second_root, &second_staged_name, &second_nonce);
        let mut second_journal = write_tauri_purge_journal(
            &second_parent,
            &second_root,
            &second_nonce,
            &second_staged_name,
            &second_previous_name,
        )
        .unwrap();
        second_parent
            .rename_child_exclusive(second_root.file_name().unwrap(), &second_previous_name)
            .unwrap();
        second_parent
            .rename_child_exclusive(&second_staged_name, second_root.file_name().unwrap())
            .unwrap();
        mark_tauri_purge_journal_committed(
            &second_parent,
            &second_root,
            &mut second_journal,
            &second_nonce,
        )
        .unwrap();
        drop(second_parent);

        let second_state = LibraryState::try_new_with_config(
            second_root.clone(),
            temp.path().join("second-integrations.json"),
        )
        .unwrap();
        let second_roots = second_state
            .with_db(|db| db.list_index_roots().map_err(library_error))
            .unwrap();
        assert_eq!(second_roots.len(), 1);
        assert_eq!(second_roots[0].id, second_root_entry.id);
        assert_eq!(second_roots[0].path, second_root_entry.path);
        assert!(second_root
            .parent()
            .unwrap()
            .join(&second_previous_name)
            .is_dir());
        assert!(!second_root.parent().unwrap().join(&second_journal).exists());
        assert!(!second_root.join(".gogglelab-purge-stage").exists());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn tauri_restore_journal_recovers_precommit_and_preserves_committed_root() {
        fn create_library(root: &Path, model_name: &str) {
            let db = LibraryDb::open(root).unwrap();
            db.create_model(model_name, None).unwrap();
        }

        let temp = tempfile::tempdir().unwrap();

        // Simulate a crash after root -> previous but before stage -> root.
        let first_root = temp.path().join("first-restore-library");
        create_library(&first_root, "Original before restore");
        let first_root = first_root.canonicalize().unwrap();
        let first_parent =
            library_runtime::GuardedDirectoryParent::open(first_root.parent().unwrap()).unwrap();
        let first_nonce = "f".repeat(32);
        let first_staged_name =
            std::ffi::OsString::from(format!(".gogglelab-restore-{first_nonce}"));
        let first_previous_name = std::ffi::OsString::from(format!(
            "{}.previous-{first_nonce}",
            first_root.file_name().unwrap().to_string_lossy()
        ));
        let first_failed_name =
            std::ffi::OsString::from(format!(".gogglelab-failed-restore-{first_nonce}"));
        let first_staged = first_parent
            .create_child_directory(&first_staged_name)
            .unwrap();
        create_library(
            &first_staged.io_path().unwrap(),
            "Uncommitted restored catalog",
        );
        let first_journal = write_tauri_restore_journal(
            &first_parent,
            &first_root,
            &first_nonce,
            &first_staged_name,
            &first_previous_name,
            &first_failed_name,
        )
        .unwrap();
        first_parent
            .rename_child_exclusive(first_root.file_name().unwrap(), &first_previous_name)
            .unwrap();
        first_parent.sync().unwrap();
        drop(first_staged);
        drop(first_parent);

        let first_state = LibraryState::try_new_with_config(
            first_root.clone(),
            temp.path().join("first-restore-integrations.json"),
        )
        .unwrap();
        let first_models = first_state
            .with_db(|db| db.list_models(None, None).map_err(library_error))
            .unwrap();
        assert_eq!(first_models[0].display_name, "Original before restore");
        assert!(!first_root
            .parent()
            .unwrap()
            .join(&first_previous_name)
            .exists());
        assert!(!first_root
            .parent()
            .unwrap()
            .join(&first_staged_name)
            .exists());
        assert!(!first_root.parent().unwrap().join(&first_journal).exists());
        drop(first_state);

        // A durable commit bit after stage activation tells startup to retain
        // the restored catalog and its previous Library backup.
        let second_root = temp.path().join("second-restore-library");
        create_library(&second_root, "Original retained as previous");
        let second_root = second_root.canonicalize().unwrap();
        let second_parent =
            library_runtime::GuardedDirectoryParent::open(second_root.parent().unwrap()).unwrap();
        let second_nonce = "a".repeat(32);
        let second_staged_name =
            std::ffi::OsString::from(format!(".gogglelab-restore-{second_nonce}"));
        let second_previous_name = std::ffi::OsString::from(format!(
            "{}.previous-{second_nonce}",
            second_root.file_name().unwrap().to_string_lossy()
        ));
        let second_failed_name =
            std::ffi::OsString::from(format!(".gogglelab-failed-restore-{second_nonce}"));
        let second_staged = second_parent
            .create_child_directory(&second_staged_name)
            .unwrap();
        create_library(
            &second_staged.io_path().unwrap(),
            "Committed restored catalog",
        );
        let mut second_journal = write_tauri_restore_journal(
            &second_parent,
            &second_root,
            &second_nonce,
            &second_staged_name,
            &second_previous_name,
            &second_failed_name,
        )
        .unwrap();
        second_parent
            .rename_child_exclusive(second_root.file_name().unwrap(), &second_previous_name)
            .unwrap();
        second_parent
            .rename_child_exclusive(&second_staged_name, second_root.file_name().unwrap())
            .unwrap();
        second_parent.sync().unwrap();
        mark_tauri_restore_committed(
            &second_parent,
            &second_root,
            &mut second_journal,
            &second_nonce,
        )
        .unwrap();
        drop(second_staged);
        drop(second_parent);

        let second_state = LibraryState::try_new_with_config(
            second_root.clone(),
            temp.path().join("second-restore-integrations.json"),
        )
        .unwrap();
        let second_models = second_state
            .with_db(|db| db.list_models(None, None).map_err(library_error))
            .unwrap();
        assert_eq!(second_models[0].display_name, "Committed restored catalog");
        let previous_models =
            LibraryDb::open(second_root.parent().unwrap().join(&second_previous_name))
                .unwrap()
                .list_models(None, None)
                .unwrap();
        assert_eq!(
            previous_models[0].display_name,
            "Original retained as previous"
        );
        assert!(!second_root.parent().unwrap().join(&second_journal).exists());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn tauri_purge_keeps_configured_index_roots_and_previous_library() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let external = temp.path().join("external-models");
        let backups = temp.path().join("backups");
        fs::create_dir(&external).unwrap();
        fs::create_dir(&backups).unwrap();
        let root = library_with_index_root(&library, &external);
        let state = LibraryState::new(library.clone());
        let viewer = AppState::default();
        let backup_path = backups.join("before-purge.zip");

        let backup = purge_catalog_impl(
            &state,
            &viewer,
            "PURGE".into(),
            backup_path.to_string_lossy().into_owned(),
        )
        .unwrap();

        assert!(backup.path.is_file());
        let (models, roots) = state
            .with_db(|db| {
                Ok::<_, AppError>((
                    db.list_models(None, None).map_err(library_error)?,
                    db.list_index_roots().map_err(library_error)?,
                ))
            })
            .unwrap();
        assert!(models.is_empty());
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, root.id);
        assert_eq!(roots[0].path, root.path);
        assert!(!library
            .parent()
            .unwrap()
            .join(purge_journal_name_for_runtime(&library).unwrap())
            .exists());
        let previous = fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name().is_some_and(|name| {
                    name.to_string_lossy()
                        .starts_with("library.previous-purge-")
                })
            })
            .unwrap();
        assert!(previous.join("library.sqlite3").is_file());
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
        let guard = fail_closed.begin_maintenance().unwrap();
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
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn native_thumbnail_cleanup_discard_and_reset_keep_unknown_files() {
        use library_protocol::{ManagementEngine, ThumbnailCleanupMode, ThumbnailCleanupRequest};
        let temp = tempfile::tempdir().unwrap();
        let state = LibraryState::new(temp.path().join("library"));
        state.with_db(|_| Ok(())).unwrap();
        let unknown = state.root().join("thumbnails/unknown.png");
        fs::write(&unknown, b"uploaded or unknown bytes").unwrap();
        let context = thumbnail_cleanup_context();
        let plan = state
            .runtime()
            .review_thumbnail_cleanup(
                &context,
                ThumbnailCleanupRequest {
                    mode: ThumbnailCleanupMode::Orphans,
                    continuation: None,
                },
            )
            .unwrap();
        assert_eq!(plan.thumbnail_review.as_ref().unwrap().eligible_files, 0);
        state
            .runtime()
            .discard_action(&context, &plan.token)
            .unwrap();
        assert!(execute_thumbnail_cleanup(&state, &plan.token).is_err());
        assert_eq!(fs::read(&unknown).unwrap(), b"uploaded or unknown bytes");
        let reset = state
            .runtime()
            .review_thumbnail_cleanup(
                &context,
                ThumbnailCleanupRequest {
                    mode: ThumbnailCleanupMode::ResetDefaults,
                    continuation: None,
                },
            )
            .unwrap();
        let result = execute_thumbnail_cleanup_with_viewer_lock(
            &state,
            &AppState::default(),
            &reset.token,
            || {},
        )
        .unwrap();
        assert_eq!((result.files, result.bytes), (0, 0));
        assert_eq!(
            result.thumbnail_progress.unwrap().default_records_cleared,
            0
        );
        assert_eq!(fs::read(&unknown).unwrap(), b"uploaded or unknown bytes");
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn native_thumbnail_cleanup_rejects_execution_while_scan_is_running() {
        use library_protocol::{ManagementEngine, ThumbnailCleanupMode, ThumbnailCleanupRequest};
        let temp = tempfile::tempdir().unwrap();
        let state = LibraryState::new(temp.path().join("library"));
        state.with_db(|_| Ok(())).unwrap();
        let context = thumbnail_cleanup_context();
        let plan = state
            .runtime()
            .review_thumbnail_cleanup(
                &context,
                ThumbnailCleanupRequest {
                    mode: ThumbnailCleanupMode::Orphans,
                    continuation: None,
                },
            )
            .unwrap();
        assert!(state.scan_control().try_begin(0, "scan"));
        assert!(execute_thumbnail_cleanup_with_viewer_lock(
            &state,
            &AppState::default(),
            &plan.token,
            || {}
        )
        .is_err());
        state.scan_control().finish();
        state
            .runtime()
            .discard_action(&context, &plan.token)
            .unwrap();
    }

    #[test]
    fn orphan_indexed_cache_cleanup_preserves_source_copies_with_and_without_viewer() {
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
        assert_eq!(preserved.bytes, 0);
        assert!(file.exists());
        let removed = cleanup_orphan_cache_impl(&state, None).unwrap();
        assert_eq!(removed.files, 0);
        assert_eq!(removed.bytes, 0);
        assert!(removed.issues.is_empty());
        assert_eq!(fs::read(&file).unwrap(), bytes);
        assert!(dir.is_dir());
        assert!(
            removed
                .thumbnail_progress
                .as_ref()
                .unwrap()
                .review
                .protected_source_copies
                > 0
        );
        assert!(removed.handoff_cleanup_skipped);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn orphan_cleanup_holds_viewer_commit_gate_and_preserves_active_cache() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let state = LibraryState::new(library.clone());
        state.with_db(|_| Ok(())).unwrap();
        let bytes = b"cached member";
        let hash = format!("{:x}", Sha256::digest(bytes));
        let dir = library.join("indexed-cache").join(&hash);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("part.stl");
        fs::write(&file, bytes).unwrap();

        let mut parsed_bytes = vec![0_u8; 134];
        parsed_bytes[80..84].copy_from_slice(&1_u32.to_le_bytes());
        let viewer = AppState::default();
        viewer.register_generation(7);
        *viewer.current.lock().unwrap() = Some(crate::state::LoadedModel {
            generation: 7,
            path: file.to_string_lossy().into_owned(),
            source_digest: String::new(),
            parsed: std::sync::Arc::new(stl_core::parse_model_with_plates(&parsed_bytes).unwrap()),
        });

        let cleanup = cleanup_orphan_cache_with_viewer_lock(&state, &viewer, || {
            assert!(viewer.request_commit.try_lock().is_err());
        })
        .unwrap();

        assert_eq!(cleanup.files, 0);
        assert_eq!(cleanup.bytes, 0);
        assert!(file.is_file());
        assert!(!viewer.is_latest_locked(7));
        assert!(viewer.request_commit.try_lock().is_ok());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn orphan_cleanup_stops_if_the_admitted_library_root_is_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let state = LibraryState::new(library.clone());
        state.with_db(|_| Ok(())).unwrap();
        let file_name = format!("{}-isometric-v1-256-ffffff.png", "c".repeat(64));
        let original_file = library.join("thumbnails").join(&file_name);
        let generated = b"synthetic generated thumbnail";
        fs::write(&original_file, generated).unwrap();

        let outside = temp.path().join("outside");
        let outside_thumbnails = outside.join("thumbnails");
        fs::create_dir_all(&outside_thumbnails).unwrap();
        let replacement_file = outside_thumbnails.join(&file_name);
        fs::write(&replacement_file, generated).unwrap();
        let displaced = temp.path().join("library-displaced");

        let result = cleanup_orphan_cache_impl_after_root_admission(&state, None, || {
            fs::rename(&library, &displaced).unwrap();
            std::os::unix::fs::symlink(&outside, &library).unwrap();
        });

        assert!(result.is_err());
        assert_eq!(
            fs::read(displaced.join("thumbnails").join(&file_name)).unwrap(),
            generated
        );
        assert_eq!(fs::read(&replacement_file).unwrap(), generated);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn orphan_indexed_cache_cleanup_preserves_active_download_lease() {
        use library_protocol::{Capability, ContentEngine, DownloadEngine, RequestContext};
        use std::io::{Cursor, Write};

        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let state = LibraryState::new(library.clone());
        state.with_db(|_| Ok(())).unwrap();

        let sources = temp.path().join("sources");
        fs::create_dir(&sources).unwrap();
        let archive_path = sources.join("models.zip");
        let mut member_bytes = vec![0_u8; 84];
        member_bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file("part.stl", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&member_bytes).unwrap();
        let archive_bytes = archive.finish().unwrap().into_inner();
        fs::write(&archive_path, &archive_bytes).unwrap();

        let archive_scan =
            library_runtime::archives::scan_archive(&archive_path, 50 * 1024 * 1024).unwrap();
        let member = archive_scan.members.into_iter().next().unwrap();
        let content_hash = member.content_hash.clone();
        let outer_fingerprint = archive_scan.outer_fingerprint;
        let (root_id, model) = state
            .with_db(|db| {
                let root = db
                    .add_index_root(&sources, 60, 50 * 1024 * 1024)
                    .map_err(library_error)?;
                let root_id = root.id.clone();
                let entry = ScannedModel {
                    relative_path: "models.zip".into(),
                    content_hash: content_hash.clone(),
                    bytes: member.bytes,
                    format: member.format,
                    modified_at: 1,
                    display_name: member.display_name,
                    archive_member: Some(member.member_path),
                    outer_fingerprint: Some(outer_fingerprint),
                };
                let added = db
                    .record_scan_entries(&root.id, std::slice::from_ref(&entry))
                    .map_err(library_error)?;
                db.complete_scan_sources_with_new_models(
                    &root.id,
                    &[SourceRef {
                        relative_path: "models.zip".into(),
                        archive_member: Some("part.stl".into()),
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
                Ok((root_id, model))
            })
            .unwrap();

        let context = RequestContext {
            principal_id: "desktop-download".into(),
            capabilities: std::collections::BTreeSet::from([Capability::MediaRead]),
            request_id: "desktop-download-request".into(),
        };
        let lease = state
            .runtime()
            .issue_model_download(&context, &model.id, Some(&content_hash))
            .unwrap();
        let cached = library
            .join("indexed-cache")
            .join(&content_hash)
            .join("part.stl");
        assert!(cached.is_file());

        state
            .with_db(|db| db.remove_index_root(&root_id).map_err(library_error))
            .unwrap();

        let cleanup = cleanup_orphan_cache_impl(&state, None).unwrap();
        assert_eq!(cleanup.files, 0);
        assert!(cached.is_file());

        let mut streamed = Vec::new();
        state
            .runtime()
            .stream_content(&context, &lease.id, &mut streamed)
            .unwrap();
        assert_eq!(streamed, member_bytes);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn orphan_thumbnail_cleanup_preserves_active_media_lease() {
        use library_protocol::{Capability, ContentEngine, GalleryEngine, RequestContext};

        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let state = LibraryState::new(library.clone());
        state.with_db(|_| Ok(())).unwrap();

        let sources = temp.path().join("sources");
        fs::create_dir(&sources).unwrap();
        let source = sources.join("part.stl");
        let mut model_bytes = vec![0_u8; 84];
        model_bytes[80..84].copy_from_slice(&1_u32.to_le_bytes());
        for value in [
            0.0_f32, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0,
        ] {
            model_bytes.extend_from_slice(&value.to_le_bytes());
        }
        model_bytes.extend_from_slice(&0_u16.to_le_bytes());
        fs::write(&source, &model_bytes).unwrap();
        let content_hash = format!("{:x}", Sha256::digest(&model_bytes));

        let (root_id, model) = state
            .with_db(|db| {
                let root = db
                    .add_index_root(&sources, 60, 50 * 1024 * 1024)
                    .map_err(library_error)?;
                let root_id = root.id.clone();
                let entry = ScannedModel {
                    relative_path: "part.stl".into(),
                    content_hash: content_hash.clone(),
                    bytes: model_bytes.len() as u64,
                    format: "stl-binary".into(),
                    modified_at: 1,
                    display_name: Some("Part".into()),
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
                Ok((root_id, model))
            })
            .unwrap();

        let settings = state
            .with_db(|db| db.get_catalog_settings().map_err(library_error))
            .unwrap();
        let thumbnail = crate::library_thumbnail::ensure_thumbnail(
            &library,
            &source,
            &content_hash,
            model_bytes.len() as u64,
            &settings,
            false,
        )
        .unwrap();
        state
            .with_db(|db| {
                db.set_thumbnail_for_source_and_settings(
                    &model.id,
                    &content_hash,
                    &settings,
                    &thumbnail.png_hash,
                    &thumbnail.relative_path,
                )
                .map_err(library_error)
            })
            .unwrap();
        let thumbnail_path = library.join(&thumbnail.relative_path);
        let expected_png = fs::read(&thumbnail_path).unwrap();

        let context = RequestContext {
            principal_id: "desktop-thumbnail".into(),
            capabilities: std::collections::BTreeSet::from([Capability::MediaRead]),
            request_id: "desktop-thumbnail-request".into(),
        };
        let item = state
            .runtime()
            .list_thumbnails(&context, &model.id)
            .unwrap()
            .pop()
            .unwrap();
        let lease = state
            .runtime()
            .issue_thumbnail_media(&context, &model.id, &item.thumbnail_id)
            .unwrap();

        state
            .with_db(|db| db.remove_index_root(&root_id).map_err(library_error))
            .unwrap();

        let cleanup = cleanup_orphan_cache_impl(&state, None).unwrap();
        assert_eq!(cleanup.files, 0);
        assert!(thumbnail_path.is_file());

        let mut streamed = Vec::new();
        state
            .runtime()
            .stream_content(&context, &lease.id, &mut streamed)
            .unwrap();
        assert_eq!(streamed, expected_png);
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn guarded_file_mutations_are_denied_without_handle_relative_support() {
        assert!(require_handle_relative_management().is_err());
    }
}
