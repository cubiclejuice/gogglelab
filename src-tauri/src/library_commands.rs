use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use crate::extension_api::{AppError, AppState, ModelSummary};
use library_core::{Folder, ImportResult, LibraryDb, LibraryError, Model};

pub struct LibraryState {
    db: Mutex<Option<LibraryDb>>,
    root: PathBuf,
}

impl LibraryState {
    pub fn new(root: PathBuf) -> Self {
        Self {
            db: Mutex::new(None),
            root,
        }
    }
}

fn library_error(error: LibraryError) -> AppError {
    AppError::Library {
        message: error.to_string(),
    }
}

fn open_library(root: &Path) -> Result<LibraryDb, AppError> {
    LibraryDb::open(root).map_err(library_error)
}

fn with_db<T>(
    state: &LibraryState,
    operation: impl FnOnce(&LibraryDb) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let mut guard = state.db.lock().unwrap();
    if guard.is_none() {
        *guard = Some(open_library(&state.root)?);
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

fn managed_model_path(db: &LibraryDb, model_id: &str) -> Result<PathBuf, AppError> {
    let model = db
        .get_model(model_id)
        .map_err(library_error)?
        .ok_or_else(|| AppError::Library {
            message: format!("Model not found: {model_id}"),
        })?;
    let relative = model.storage_path.ok_or_else(|| AppError::Library {
        message: "This catalog item has no imported source file".into(),
    })?;
    let parts: Vec<_> = relative.split('/').collect();
    let relative_path = Path::new(&relative);
    if relative.contains('\\')
        || relative.contains('\0')
        || parts.len() < 2
        || parts[0] != "originals"
        || parts
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
        || relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AppError::Library {
            message: "The catalog contains an unsafe managed path".into(),
        });
    }

    let root = db
        .root()
        .canonicalize()
        .map_err(|error| crate::error::map_io_error(&error))?;
    let mut path = root.clone();
    for part in parts {
        path.push(part);
        let metadata = path
            .symlink_metadata()
            .map_err(|error| crate::error::map_io_error(&error))?;
        if metadata.file_type().is_symlink() {
            return Err(AppError::Library {
                message: "The catalog references a symlinked managed asset".into(),
            });
        }
    }
    let path = path
        .canonicalize()
        .map_err(|error| crate::error::map_io_error(&error))?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err(AppError::Library {
            message: "A managed asset escapes the Library root or is not a file".into(),
        });
    }
    Ok(path)
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
        let mut guard = library_state.db.lock().unwrap();
        if guard.is_none() {
            *guard = Some(open_library(&library_state.root)?);
        }
        managed_model_path(guard.as_ref().unwrap(), &model_id)?
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
    use super::{managed_model_path, open_library, with_db, LibraryState};
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
                let managed_path = managed_model_path(db, &model_id)?;
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
    use super::{library_source_identity, managed_model_path, open_library};
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

        assert!(matches!(
            managed_model_path(&db, &imported.model.id),
            Err(crate::error::AppError::Library { .. })
        ));

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(source_root);
    }
}
