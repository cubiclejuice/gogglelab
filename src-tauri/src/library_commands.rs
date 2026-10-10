use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use crate::extension_api::{AppError, AppState, ModelSummary};
use library_core::{Folder, ImportResult, IndexRoot, LibraryDb, LibraryError, Model};

use crate::library_scan::{LibraryScanStatus, ScanControl};

static THUMBNAIL_RENDER_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone)]
pub struct LibraryState {
    inner: Arc<LibraryStateInner>,
}

pub(crate) struct LibraryStateInner {
    db: Mutex<Option<LibraryDb>>,
    root: PathBuf,
    pub(crate) scan: Arc<ScanControl>,
}

impl LibraryState {
    pub fn new(root: PathBuf) -> Self {
        Self {
            inner: Arc::new(LibraryStateInner {
                db: Mutex::new(None),
                root,
                scan: Arc::new(ScanControl::new()),
            }),
        }
    }

    pub(crate) fn from_inner(inner: Arc<LibraryStateInner>) -> Self {
        Self { inner }
    }

    pub(crate) fn weak_inner(&self) -> Weak<LibraryStateInner> {
        Arc::downgrade(&self.inner)
    }

    pub(crate) fn scan_control(&self) -> &Arc<ScanControl> {
        &self.inner.scan
    }

    pub(crate) fn with_db<T>(
        &self,
        operation: impl FnOnce(&LibraryDb) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        with_db(self, operation)
    }
}

pub(crate) fn library_error(error: LibraryError) -> AppError {
    AppError::Library {
        message: error.to_string(),
    }
}

fn open_library(root: &Path) -> Result<LibraryDb, AppError> {
    LibraryDb::open(root).map_err(library_error)
}

pub(crate) fn with_db<T>(
    state: &LibraryState,
    operation: impl FnOnce(&LibraryDb) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let mut guard = state.inner.db.lock().unwrap();
    if guard.is_none() {
        *guard = Some(open_library(&state.inner.root)?);
    }
    operation(guard.as_ref().unwrap())
}

#[tauri::command]
pub fn library_list_folders(
    state: tauri::State<'_, LibraryState>,
) -> Result<Vec<Folder>, AppError> {
    with_db(&state, |db| db.list_folders().map_err(library_error))
}

#[tauri::command]
pub fn library_list_models(
    state: tauri::State<'_, LibraryState>,
    folder_id: Option<String>,
    query: Option<String>,
) -> Result<Vec<Model>, AppError> {
    with_db(&state, |db| {
        db.list_models(folder_id.as_deref(), query.as_deref())
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_create_folder(
    state: tauri::State<'_, LibraryState>,
    name: String,
    parent_id: Option<String>,
) -> Result<Folder, AppError> {
    with_db(&state, |db| {
        db.create_folder(&name, parent_id.as_deref())
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_rename_folder(
    state: tauri::State<'_, LibraryState>,
    id: String,
    name: String,
) -> Result<Folder, AppError> {
    with_db(&state, |db| {
        db.rename_folder(&id, &name).map_err(library_error)
    })
}

#[tauri::command]
pub fn library_move_folder(
    state: tauri::State<'_, LibraryState>,
    id: String,
    new_parent_id: String,
) -> Result<Folder, AppError> {
    with_db(&state, |db| {
        db.move_folder(&id, &new_parent_id).map_err(library_error)
    })
}

#[tauri::command]
pub fn library_delete_folder(
    state: tauri::State<'_, LibraryState>,
    id: String,
) -> Result<(), AppError> {
    with_db(&state, |db| db.delete_folder(&id).map_err(library_error))
}

#[tauri::command]
pub fn library_import_file(
    state: tauri::State<'_, LibraryState>,
    source_path: String,
    folder_id: Option<String>,
    display_name: Option<String>,
) -> Result<ImportResult, AppError> {
    with_db(&state, |db| {
        db.import_file(source_path, folder_id.as_deref(), display_name.as_deref())
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_list_roots(
    state: tauri::State<'_, LibraryState>,
) -> Result<Vec<IndexRoot>, AppError> {
    with_db(&state, |db| db.list_index_roots().map_err(library_error))
}

#[tauri::command]
pub fn library_add_root(
    state: tauri::State<'_, LibraryState>,
    path: String,
) -> Result<IndexRoot, AppError> {
    with_db(&state, |db| {
        db.add_index_root(path, 60, 50 * 1024 * 1024)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_update_root(
    state: tauri::State<'_, LibraryState>,
    root_id: String,
    enabled: bool,
    scan_interval_minutes: u32,
    max_file_bytes: u64,
) -> Result<IndexRoot, AppError> {
    if !(1..=1440).contains(&scan_interval_minutes)
        || max_file_bytes == 0
        || max_file_bytes > stl_core::limits::MAX_FILE_BYTES
    {
        return Err(AppError::Library {
            message: "Scan interval must be 1-1440 minutes and file limit at most 100 MiB".into(),
        });
    }
    with_db(&state, |db| {
        let root = db
            .list_index_roots()
            .map_err(library_error)?
            .into_iter()
            .find(|root| root.id == root_id)
            .ok_or_else(|| AppError::Library {
                message: format!("Index root not found: {root_id}"),
            })?;
        db.update_index_root(&IndexRoot {
            enabled,
            scan_interval_minutes,
            max_file_bytes,
            ..root
        })
        .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_remove_root(
    state: tauri::State<'_, LibraryState>,
    root_id: String,
) -> Result<(), AppError> {
    with_db(&state, |db| {
        db.remove_index_root(&root_id).map_err(library_error)
    })
}

#[tauri::command]
pub fn library_start_scan(
    state: tauri::State<'_, LibraryState>,
    root_id: Option<String>,
) -> Result<(), AppError> {
    crate::library_scan::start_manual_scan(state.inner().clone(), root_id)
        .map_err(|message| AppError::Library { message })
}

#[tauri::command]
pub fn library_scan_status(state: tauri::State<'_, LibraryState>) -> LibraryScanStatus {
    state.scan_control().status()
}

#[tauri::command]
pub fn library_cancel_scan(state: tauri::State<'_, LibraryState>) {
    state.scan_control().cancel();
}

#[tauri::command]
pub async fn library_get_thumbnail(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
    regenerate: Option<bool>,
) -> Result<Option<String>, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        get_thumbnail(&state, &model_id, regenerate.unwrap_or(false))
    })
    .await
    .map_err(|error| AppError::Library {
        message: format!("Thumbnail worker failed: {error}"),
    })?
}

fn get_thumbnail(
    state: &LibraryState,
    model_id: &str,
    regenerate: bool,
) -> Result<Option<String>, AppError> {
    if !regenerate {
        if let Ok(Some(cached)) = with_db(state, |db| cached_thumbnail(db, model_id)) {
            return Ok(Some(cached));
        }
    }
    let _render_guard = THUMBNAIL_RENDER_LOCK.lock().unwrap();
    if !regenerate {
        if let Ok(Some(cached)) = with_db(state, |db| cached_thumbnail(db, model_id)) {
            return Ok(Some(cached));
        }
    }
    let source = with_db(state, |db| {
        let model = db
            .get_model(model_id)
            .map_err(library_error)?
            .ok_or_else(|| AppError::Library {
                message: format!("Model not found: {model_id}"),
            })?;
        let Some(content_hash) = model.content_hash else {
            return Ok(None);
        };
        let source = db.resolve_model_source(model_id).map_err(library_error)?;
        Ok(Some((
            source,
            content_hash,
            model.bytes,
            db.root().to_path_buf(),
        )))
    })?;
    let Some((source_path, content_hash, source_bytes, library_root)) = source else {
        return Ok(None);
    };
    let thumbnail = crate::library_thumbnail::ensure_thumbnail(
        &library_root,
        &source_path,
        &content_hash,
        source_bytes,
        regenerate,
    )
    .map_err(|message| AppError::Library { message })?;
    with_db(state, |db| {
        let current_hash = db
            .get_model(model_id)
            .map_err(library_error)?
            .and_then(|model| model.content_hash)
            .ok_or_else(|| AppError::Library {
                message: "Model source changed while rendering its thumbnail".into(),
            })?;
        if current_hash != content_hash {
            return Err(AppError::Library {
                message: "Model source changed while rendering its thumbnail".into(),
            });
        }
        db.set_thumbnail_for_source(
            model_id,
            &content_hash,
            &thumbnail.png_hash,
            &thumbnail.relative_path,
        )
        .map_err(library_error)
    })?;
    crate::library_thumbnail::png_data_uri(&library_root.join(thumbnail.relative_path))
        .map(Some)
        .map_err(|message| AppError::Library { message })
}

fn cached_thumbnail(db: &LibraryDb, model_id: &str) -> Result<Option<String>, AppError> {
    let Some((path, png_hash)) = db
        .thumbnail_for_model_with_hash(model_id)
        .map_err(library_error)?
    else {
        return Ok(None);
    };
    crate::library_thumbnail::png_data_uri_with_hash(&path, &png_hash)
        .map(Some)
        .map_err(|message| AppError::Library { message })
}

#[tauri::command]
pub async fn library_purge_thumbnails(
    state: tauri::State<'_, LibraryState>,
) -> Result<u64, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || purge_thumbnails(&state))
        .await
        .map_err(|error| AppError::Library {
            message: format!("Thumbnail purge worker failed: {error}"),
        })?
}

fn purge_thumbnails(state: &LibraryState) -> Result<u64, AppError> {
    let paths = with_db(state, |db| db.purge_thumbnails().map_err(library_error))?;
    let mut removed = 0_u64;
    for path in paths {
        match std::fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(crate::error::map_io_error(&error)),
        }
    }
    Ok(removed)
}

fn library_source_identity(model_id: &str) -> String {
    format!("library:{model_id}")
}

#[tauri::command]
pub fn library_open_model(
    app: tauri::AppHandle,
    viewer_state: tauri::State<'_, AppState>,
    library_state: tauri::State<'_, LibraryState>,
    model_id: String,
    generation: u64,
) -> Result<ModelSummary, AppError> {
    let path = {
        let mut guard = library_state.inner.db.lock().unwrap();
        if guard.is_none() {
            *guard = Some(open_library(&library_state.inner.root)?);
        }
        guard
            .as_ref()
            .unwrap()
            .resolve_model_source(&model_id)
            .map_err(library_error)?
    };
    crate::extension_api::load_model_from_path(
        app,
        &viewer_state,
        &path,
        generation,
        &library_source_identity(&model_id),
    )
}

#[cfg(test)]
mod access_tests {
    use super::{open_library, with_db, LibraryState};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn basic_library_operations_do_not_require_license_state() {
        let root = std::env::temp_dir().join(format!(
            "gogglelab-library-community-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        assert!(!root.exists());
        let db = open_library(&root).unwrap();
        let folder = db.create_folder("Community", None).unwrap();
        let source_root = std::env::temp_dir().join(format!(
            "gogglelab-library-community-source-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&source_root);
        std::fs::create_dir_all(&source_root).unwrap();
        let source = source_root.join("Persisted Bracket.stl");
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        std::fs::write(&source, &bytes).unwrap();
        let imported = db
            .import_file(&source, Some(&folder.id), Some("Persisted Bracket"))
            .unwrap();
        let model_id = imported.model.id.clone();
        let content_hash = imported.model.content_hash.clone();
        drop(db);

        let state = LibraryState::new(root.clone());
        let (names, reopened, managed_path): (Vec<String>, library_core::Model, _) =
            with_db(&state, |db| {
                let names = db
                    .list_folders()
                    .map(|folders| folders.into_iter().map(|folder| folder.name).collect())
                    .map_err(super::library_error)?;
                let reopened = db
                    .get_model(&model_id)
                    .map_err(super::library_error)?
                    .expect("persisted model should reopen");
                let managed_path = db
                    .resolve_model_source(&model_id)
                    .map_err(super::library_error)?;
                Ok((names, reopened, managed_path))
            })
            .unwrap();
        assert!(names.contains(&"Community".to_string()));
        assert!(root.join("library.sqlite3").is_file());
        assert_eq!(
            folder.parent_id.as_deref(),
            Some(library_core::ROOT_FOLDER_ID)
        );
        assert_eq!(reopened.id, model_id);
        assert_eq!(reopened.display_name, "Persisted Bracket");
        assert_eq!(reopened.content_hash, content_hash);
        assert_eq!(std::fs::read(managed_path).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(source_root);
    }
}

#[cfg(test)]
mod tests {
    use super::{cached_thumbnail, library_source_identity, open_library};
    use library_core::ScannedModel;
    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn library_source_identity_is_model_scoped() {
        assert_eq!(library_source_identity("model-42"), "library:model-42");
    }

    #[cfg(unix)]
    #[test]
    fn managed_model_path_rejects_symlinked_assets() {
        use std::os::unix::fs::symlink;

        let nonce = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "gogglelab-library-path-{}-{nonce}",
            std::process::id()
        ));
        let source_root = std::env::temp_dir().join(format!(
            "gogglelab-library-path-source-{}-{nonce}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&source_root);
        std::fs::create_dir_all(&source_root).unwrap();
        let source = source_root.join("safe.stl");
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        std::fs::write(&source, bytes).unwrap();

        let db = open_library(&root).unwrap();
        let imported = db.import_file(&source, None, None).unwrap();
        let managed = root.join(imported.model.storage_path.as_ref().unwrap());
        std::fs::remove_file(&managed).unwrap();
        symlink(&source, &managed).unwrap();

        assert!(db.resolve_model_source(&imported.model.id).is_err());

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(source_root);
    }

    #[test]
    fn indexed_thumbnail_remains_available_while_source_is_offline() {
        let nonce = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "gogglelab-library-offline-{}-{nonce}",
            std::process::id()
        ));
        let source_root = std::env::temp_dir().join(format!(
            "gogglelab-library-offline-source-{}-{nonce}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&source_root);
        std::fs::create_dir_all(&source_root).unwrap();
        let source = source_root.join("cached.stl");
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        std::fs::write(&source, &bytes).unwrap();
        let source_hash: String = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();

        let db = open_library(&root).unwrap();
        let index_root = db.add_index_root(&source_root, 60, 1024 * 1024).unwrap();
        let recorded = db
            .record_scan_entries(
                &index_root.id,
                &[ScannedModel {
                    relative_path: PathBuf::from("cached.stl"),
                    content_hash: source_hash.clone(),
                    bytes: bytes.len() as u64,
                    format: "stl-binary".into(),
                    modified_at: 1,
                    display_name: Some("Cached".into()),
                }],
            )
            .unwrap();
        let model_id = recorded.added_model_ids[0].clone();
        db.complete_scan_with_new_models(
            &index_root.id,
            &[PathBuf::from("cached.stl")],
            &recorded.added_model_ids,
            "1",
        )
        .unwrap();
        let thumbnail = crate::library_thumbnail::ensure_thumbnail(
            &root,
            &source,
            &source_hash,
            bytes.len() as u64,
            false,
        )
        .unwrap();
        db.set_thumbnail_for_source(
            &model_id,
            &source_hash,
            &thumbnail.png_hash,
            Path::new(&thumbnail.relative_path),
        )
        .unwrap();

        std::fs::remove_dir_all(&source_root).unwrap();
        let data_uri = cached_thumbnail(&db, &model_id).unwrap().unwrap();
        assert!(data_uri.starts_with("data:image/png;base64,"));
        let _ = std::fs::remove_dir_all(root);
    }
}
