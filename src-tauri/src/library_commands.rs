use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex, Weak};

use crate::extension_api::{AppError, AppState, ModelSummary};
use library_core::{
    CatalogSettings, Folder, ImportResult, IndexRoot, LibraryDb, LibraryError, MetadataPatch,
    Model, PartStock, PartStockInput, PrintAttempt, PrintAttemptImportItem,
    PrintAttemptImportResult, PrintAttemptInput, PrintAttemptInputWithPrinter, Tag,
    VersionedTarget,
};
use library_runtime::{LibraryRuntime, RuntimeConfig, RuntimeIoGuard, RuntimeMaintenanceGuard};

use crate::library_scan::{LibraryScanStatus, ScanControl};

static THUMBNAIL_RENDER_LIMITER: LazyLock<RenderLimiter> = LazyLock::new(RenderLimiter::new);
static THUMBNAIL_KEY_LOCKS: LazyLock<Mutex<HashMap<String, Weak<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct RenderLimiter {
    active: Mutex<usize>,
    available: Condvar,
}

struct RenderPermit<'a>(&'a RenderLimiter);

impl RenderLimiter {
    fn new() -> Self {
        Self {
            active: Mutex::new(0),
            available: Condvar::new(),
        }
    }

    fn acquire(&self, limit: usize) -> RenderPermit<'_> {
        let mut active = self.active.lock().unwrap();
        while *active >= limit.max(1) {
            active = self.available.wait(active).unwrap();
        }
        *active += 1;
        RenderPermit(self)
    }
    fn wait_idle(&self) {
        let mut active = self.active.lock().unwrap();
        while *active != 0 {
            active = self.available.wait(active).unwrap();
        }
    }
}

impl Drop for RenderPermit<'_> {
    fn drop(&mut self) {
        let mut active = self.0.active.lock().unwrap();
        *active = active.saturating_sub(1);
        self.0.available.notify_one();
    }
}

fn thumbnail_key_lock(key: &str) -> Arc<Mutex<()>> {
    let mut locks = THUMBNAIL_KEY_LOCKS.lock().unwrap();
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key.to_owned(), Arc::downgrade(&lock));
    lock
}

#[derive(Clone)]
pub struct LibraryState {
    inner: Arc<LibraryStateInner>,
}

pub(crate) struct LibraryStateInner {
    runtime: Arc<LibraryRuntime>,
}

impl LibraryState {
    #[cfg(test)]
    pub fn new(root: PathBuf) -> Self {
        Self::try_new(root).expect("Library runtime initialization failed")
    }

    #[cfg(test)]
    pub fn try_new(root: PathBuf) -> Result<Self, AppError> {
        let settings_path = root
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("integrations.json");
        Self::try_new_with_config(root, settings_path)
    }

    pub fn try_new_with_config(
        root: PathBuf,
        integration_settings_path: PathBuf,
    ) -> Result<Self, AppError> {
        let runtime = LibraryRuntime::open(RuntimeConfig::desktop(root, integration_settings_path))
            .map_err(AppError::from)?;
        Ok(Self::from_runtime(runtime))
    }

    pub fn from_runtime(runtime: Arc<LibraryRuntime>) -> Self {
        let state = Self {
            inner: Arc::new(LibraryStateInner {
                runtime: runtime.clone(),
            }),
        };
        let weak = state.weak_inner();
        runtime.register_scan_completed_hook(Arc::new(move |root_id| {
            if let Some(inner) = weak.upgrade() {
                render_thumbnails_for_root(&LibraryState::from_inner(inner), root_id);
            }
        }));
        state
    }

    pub fn runtime(&self) -> &Arc<LibraryRuntime> {
        &self.inner.runtime
    }

    pub(crate) fn from_inner(inner: Arc<LibraryStateInner>) -> Self {
        Self { inner }
    }

    pub(crate) fn weak_inner(&self) -> Weak<LibraryStateInner> {
        Arc::downgrade(&self.inner)
    }

    pub(crate) fn scan_control(&self) -> &Arc<ScanControl> {
        self.inner.runtime.scan_control()
    }

    pub(crate) fn with_db<T>(
        &self,
        operation: impl FnOnce(&LibraryDb) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        with_db(self, operation)
    }
    pub(crate) fn root(&self) -> &Path {
        self.inner.runtime.host_library_root()
    }
    pub(crate) fn management_generation(&self) -> u64 {
        self.inner.runtime.library_epoch()
    }
    pub(crate) fn maintenance_active(&self) -> bool {
        self.inner.runtime.maintenance_active()
    }
    pub(crate) fn begin_io(&self) -> Result<RuntimeIoGuard, AppError> {
        self.inner.runtime.begin_io().map_err(AppError::from)
    }
    pub(crate) fn wait_for_thumbnail_idle(&self) -> Result<(), AppError> {
        if self.scan_control().status().running {
            return Err(AppError::Library {
                message: "Library job is running".into(),
            });
        }
        THUMBNAIL_RENDER_LIMITER.wait_idle();
        Ok(())
    }
    pub(crate) fn begin_maintenance(&self) -> Result<RuntimeMaintenanceGuard, AppError> {
        if self.scan_control().status().running {
            return Err(AppError::Library {
                message: "Library job is running".into(),
            });
        }
        let guard = self
            .inner
            .runtime
            .begin_maintenance()
            .map_err(AppError::from)?;
        THUMBNAIL_RENDER_LIMITER.wait_idle();
        Ok(guard)
    }

    pub(crate) fn begin_source_mutation_maintenance(
        &self,
    ) -> Result<RuntimeMaintenanceGuard, AppError> {
        if self.scan_control().status().running {
            return Err(AppError::Library {
                message: "Library job is running".into(),
            });
        }
        let guard = self
            .inner
            .runtime
            .begin_source_mutation_maintenance()
            .map_err(AppError::from)?;
        THUMBNAIL_RENDER_LIMITER.wait_idle();
        Ok(guard)
    }
}

pub(crate) fn library_error(error: LibraryError) -> AppError {
    AppError::Library {
        message: error.to_string(),
    }
}

#[cfg(test)]
fn open_library(root: &Path) -> Result<LibraryDb, AppError> {
    LibraryDb::open(root).map_err(library_error)
}

pub(crate) fn with_db<T>(
    state: &LibraryState,
    operation: impl FnOnce(&LibraryDb) -> Result<T, AppError>,
) -> Result<T, AppError> {
    state.inner.runtime.with_db(operation)
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
pub fn library_list_tags(state: tauri::State<'_, LibraryState>) -> Result<Vec<Tag>, AppError> {
    with_db(&state, |db| db.list_tags().map_err(library_error))
}

#[tauri::command]
pub fn library_create_tag(
    state: tauri::State<'_, LibraryState>,
    name: String,
    category: Option<String>,
) -> Result<Tag, AppError> {
    with_db(&state, |db| {
        db.create_tag(&name, category.as_deref())
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_update_tag(
    state: tauri::State<'_, LibraryState>,
    tag_id: String,
    name: String,
    category: Option<String>,
) -> Result<Tag, AppError> {
    with_db(&state, |db| {
        db.rename_tag(&tag_id, &name, category.as_deref())
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_delete_tag(
    state: tauri::State<'_, LibraryState>,
    tag_id: String,
) -> Result<(), AppError> {
    with_db(&state, |db| db.delete_tag(&tag_id).map_err(library_error))
}

#[tauri::command]
pub fn library_update_model_metadata(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
    expected_version: u64,
    patch: MetadataPatch,
) -> Result<Model, AppError> {
    with_db(&state, |db| {
        db.update_model_metadata(&model_id, expected_version, &patch)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_update_models_metadata(
    state: tauri::State<'_, LibraryState>,
    targets: Vec<VersionedTarget>,
    patch: MetadataPatch,
) -> Result<Vec<Model>, AppError> {
    with_db(&state, |db| {
        db.bulk_update_metadata(&targets, &patch)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_list_part_stock(
    state: tauri::State<'_, LibraryState>,
) -> Result<Vec<PartStock>, AppError> {
    with_db(&state, |db| db.list_part_stock().map_err(library_error))
}

#[tauri::command]
pub fn library_create_part_stock(
    state: tauri::State<'_, LibraryState>,
    input: PartStockInput,
) -> Result<PartStock, AppError> {
    with_db(&state, |db| {
        db.create_part_stock(&input).map_err(library_error)
    })
}

#[tauri::command]
pub fn library_update_part_stock(
    state: tauri::State<'_, LibraryState>,
    id: String,
    expected_version: u64,
    input: PartStockInput,
) -> Result<PartStock, AppError> {
    with_db(&state, |db| {
        db.update_part_stock(&id, expected_version, &input)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_delete_part_stock(
    state: tauri::State<'_, LibraryState>,
    id: String,
    expected_version: u64,
) -> Result<(), AppError> {
    with_db(&state, |db| {
        db.delete_part_stock(&id, expected_version)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_create_print_attempt(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
    input: PrintAttemptInput,
) -> Result<PrintAttempt, AppError> {
    with_db(&state, |db| {
        db.create_print_attempt(&model_id, &input)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_create_print_attempt_with_printer(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
    input: PrintAttemptInputWithPrinter,
) -> Result<PrintAttempt, AppError> {
    with_db(&state, |db| {
        db.create_print_attempt_with_printer(&model_id, &input)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_import_print_attempt(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
    input: PrintAttemptInput,
    created_at: Option<String>,
    printer_id: Option<String>,
    printer_name: Option<String>,
) -> Result<PrintAttempt, AppError> {
    with_db(&state, |db| {
        db.import_print_attempt_with_printer(
            &model_id,
            &input,
            created_at.as_deref(),
            printer_id.as_deref(),
            printer_name.as_deref(),
        )
        .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_import_print_attempts(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
    attempts: Vec<PrintAttemptImportItem>,
) -> Result<PrintAttemptImportResult, AppError> {
    with_db(&state, |db| {
        db.import_print_attempts_deduplicated(&model_id, &attempts)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_list_print_attempts(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
) -> Result<Vec<PrintAttempt>, AppError> {
    with_db(&state, |db| {
        db.list_print_attempts(&model_id).map_err(library_error)
    })
}

#[tauri::command]
pub fn library_get_print_attempt(
    state: tauri::State<'_, LibraryState>,
    attempt_id: String,
) -> Result<Option<PrintAttempt>, AppError> {
    with_db(&state, |db| {
        db.get_print_attempt(&attempt_id).map_err(library_error)
    })
}

#[tauri::command]
pub fn library_update_print_attempt(
    state: tauri::State<'_, LibraryState>,
    attempt_id: String,
    input: PrintAttemptInput,
) -> Result<PrintAttempt, AppError> {
    with_db(&state, |db| {
        db.update_print_attempt(&attempt_id, &input)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_update_print_attempt_with_printer(
    state: tauri::State<'_, LibraryState>,
    attempt_id: String,
    input: PrintAttemptInputWithPrinter,
) -> Result<PrintAttempt, AppError> {
    with_db(&state, |db| {
        db.update_print_attempt_with_printer(&attempt_id, &input)
            .map_err(library_error)
    })
}

#[tauri::command]
pub fn library_delete_print_attempt(
    state: tauri::State<'_, LibraryState>,
    attempt_id: String,
) -> Result<(), AppError> {
    with_db(&state, |db| {
        db.delete_print_attempt(&attempt_id).map_err(library_error)
    })
}

#[tauri::command]
pub fn library_create_bundle(
    state: tauri::State<'_, LibraryState>,
    model_ids: Vec<String>,
) -> Result<String, AppError> {
    with_db(&state, |db| {
        db.create_bundle(&model_ids).map_err(library_error)
    })
}

#[tauri::command]
pub fn library_set_model_bundle(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
    bundle_id: Option<String>,
) -> Result<Model, AppError> {
    with_db(&state, |db| {
        db.set_model_bundle(&model_id, bundle_id.as_deref())
            .map_err(library_error)
    })
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
pub fn library_get_catalog_settings(
    state: tauri::State<'_, LibraryState>,
) -> Result<CatalogSettings, AppError> {
    with_db(&state, |db| {
        db.get_catalog_settings().map_err(library_error)
    })
}

#[tauri::command]
pub fn library_set_catalog_settings(
    state: tauri::State<'_, LibraryState>,
    settings: CatalogSettings,
) -> Result<CatalogSettings, AppError> {
    with_db(&state, |db| {
        db.update_catalog_settings(&settings).map_err(library_error)
    })
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
    scan_zip: bool,
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
            scan_zip,
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

const MAX_THUMBNAIL_JOB_MODELS: usize = 200_000;

#[tauri::command]
pub fn library_start_thumbnail_job(
    state: tauri::State<'_, LibraryState>,
    regenerate: bool,
) -> Result<(), AppError> {
    if state.maintenance_active() {
        return Err(AppError::Library {
            message: "Library maintenance is in progress".into(),
        });
    }
    let state = state.inner().clone();
    let model_ids = thumbnail_job_models(&state, None)?;
    if !state.scan_control().try_begin(0, "thumbnails") {
        return Err(AppError::Library {
            message: "A library scan or thumbnail job is already running".into(),
        });
    }
    if state.maintenance_active() {
        state.scan_control().finish();
        return Err(AppError::Library {
            message: "Library maintenance is in progress".into(),
        });
    }
    state.scan_control().set_thumbnail_progress(model_ids.len());
    let worker_state = state.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("gogglelab-thumbnail-job".into())
        .spawn(move || {
            render_thumbnail_models(&worker_state, model_ids, regenerate);
            worker_state.scan_control().finish();
        })
    {
        let message = error.to_string();
        state.scan_control().failed_to_spawn(message.clone());
        return Err(AppError::Library { message });
    }
    Ok(())
}

fn thumbnail_job_models(
    state: &LibraryState,
    root_id: Option<&str>,
) -> Result<Vec<String>, AppError> {
    let models = with_db(state, |db| {
        db.list_models(None, None).map_err(library_error)
    })?;
    if models.len() > MAX_THUMBNAIL_JOB_MODELS {
        return Err(AppError::Library {
            message: format!(
                "Thumbnail job exceeds the safety limit of {MAX_THUMBNAIL_JOB_MODELS} models"
            ),
        });
    }
    Ok(models
        .into_iter()
        .filter(|model| {
            !model.source_missing
                && model.content_hash.is_some()
                && root_id.is_none_or(|id| model.source_root_id.as_deref() == Some(id))
        })
        .map(|model| model.id)
        .collect())
}

pub(crate) fn render_thumbnails_for_root(state: &LibraryState, root_id: &str) {
    let model_ids = match thumbnail_job_models(state, Some(root_id)) {
        Ok(model_ids) => model_ids,
        Err(error) => {
            state
                .scan_control()
                .error(format!("Thumbnail job: {error:?}"));
            return;
        }
    };
    state.scan_control().set_job_kind("thumbnails");
    state.scan_control().set_thumbnail_progress(model_ids.len());
    render_thumbnail_models(state, model_ids, false);
    state.scan_control().set_job_kind("scan");
}

fn render_thumbnail_models(state: &LibraryState, model_ids: Vec<String>, regenerate: bool) {
    let concurrency = with_db(state, |db| {
        db.get_catalog_settings()
            .map(|settings| settings.thumbnail_concurrency.clamp(1, 2) as usize)
            .map_err(library_error)
    })
    .unwrap_or(1);
    let queue = Arc::new(Mutex::new(VecDeque::from(model_ids)));
    std::thread::scope(|scope| {
        for _ in 0..concurrency {
            let queue = Arc::clone(&queue);
            let state = state.clone();
            scope.spawn(move || loop {
                if state.scan_control().cancelled() {
                    break;
                }
                let Some(model_id) = queue.lock().unwrap().pop_front() else {
                    break;
                };
                if let Err(error) = get_thumbnail(&state, &model_id, regenerate) {
                    state
                        .scan_control()
                        .error(format!("Thumbnail {model_id}: {error:?}"));
                }
                state.scan_control().thumbnail_completed();
            });
        }
    });
}

#[tauri::command]
pub async fn library_get_catalog_thumbnail(
    state: tauri::State<'_, LibraryState>,
    model_id: String,
) -> Result<Option<String>, AppError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || get_catalog_thumbnail(&state, &model_id))
        .await
        .map_err(|error| AppError::Library {
            message: format!("Thumbnail reader failed: {error}"),
        })?
}

fn get_catalog_thumbnail(state: &LibraryState, model_id: &str) -> Result<Option<String>, AppError> {
    let _io_guard = state.begin_io()?;
    with_db(state, |db| cached_thumbnail(db, model_id))
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

pub(crate) fn get_thumbnail(
    state: &LibraryState,
    model_id: &str,
    regenerate: bool,
) -> Result<Option<String>, AppError> {
    let _io_guard = state.begin_io()?;
    let custom = with_db(state, |db| {
        db.thumbnail_cache_entry_for_model(model_id)
            .map(|entry| entry.is_some_and(|entry| entry.is_custom))
            .map_err(library_error)
    })?;
    if custom || !regenerate {
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
        if model.source_kind == "indexed" {
            let root_id = model
                .source_root_id
                .as_deref()
                .ok_or_else(|| AppError::Library {
                    message: "Indexed model has no source root".into(),
                })?;
            let limit = db
                .list_index_roots()
                .map_err(library_error)?
                .into_iter()
                .find(|root| root.id == root_id)
                .ok_or_else(|| AppError::Library {
                    message: "Indexed model source root was removed".into(),
                })?
                .max_file_bytes;
            if model.bytes > limit {
                return Err(AppError::Library {
                    message: format!(
                        "Indexed model exceeds its source root limit of {limit} bytes"
                    ),
                });
            }
        }
        let source = db
            .resolve_model_source_ref(model_id)
            .map_err(library_error)?;
        Ok(Some((
            source,
            content_hash,
            db.root().to_path_buf(),
            db.get_catalog_settings().map_err(library_error)?,
        )))
    })?;
    let Some((source, content_hash, library_root, settings)) = source else {
        return Ok(None);
    };
    let key = crate::library_thumbnail::thumbnail_relative_path(&content_hash, &settings)
        .to_string_lossy()
        .into_owned();
    let key_lock = thumbnail_key_lock(&key);
    let _key_guard = key_lock.lock().unwrap();
    if !regenerate {
        if let Ok(Some(cached)) = with_db(state, |db| cached_thumbnail(db, model_id)) {
            return Ok(Some(cached));
        }
    }
    let _render_permit =
        THUMBNAIL_RENDER_LIMITER.acquire(settings.thumbnail_concurrency.clamp(1, 2) as usize);
    if state.maintenance_active() {
        return Err(AppError::Library {
            message: "Library maintenance is in progress".into(),
        });
    }
    let mut processing = stl_core::package_bounds::PackageProcessingContext::new(
        stl_core::zip_bounds::ZipResourceObserver::new(),
    );
    let temporary_member = if let Some(member) = source.archive_member.as_deref() {
        Some(
            crate::library_archives::extract_member_to_temporary_in_context_typed(
                &library_root,
                &source.outer_path,
                member,
                source
                    .outer_fingerprint
                    .as_deref()
                    .ok_or_else(|| AppError::Library {
                        message: "Archive model has no source fingerprint".into(),
                    })?,
                &source.content_hash,
                source.bytes,
                &mut processing,
            )
            .map_err(AppError::from)?,
        )
    } else {
        None
    };
    let source_path = temporary_member
        .as_ref()
        .map_or(source.outer_path.as_path(), |member| member.path());
    let thumbnail = crate::library_thumbnail::ensure_thumbnail_in_context_typed(
        &library_root,
        source_path,
        &content_hash,
        source.bytes,
        &settings,
        regenerate,
        &mut processing,
    )
    .map_err(AppError::from)?;
    processing.observer().check().map_err(AppError::from)?;
    with_db(state, |db| {
        persist_generated_thumbnail(db, model_id, &content_hash, &settings, &thumbnail)
    })?;
    with_db(state, |db| cached_thumbnail(db, model_id))
}

fn persist_generated_thumbnail(
    db: &LibraryDb,
    model_id: &str,
    content_hash: &str,
    settings: &CatalogSettings,
    thumbnail: &crate::library_thumbnail::ThumbnailArtifact,
) -> Result<(), AppError> {
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
    let current_settings = db.get_catalog_settings().map_err(library_error)?;
    if current_settings.thumbnail_size != settings.thumbnail_size
        || current_settings.thumbnail_background != settings.thumbnail_background
    {
        return Err(AppError::Library {
            message: "Thumbnail settings changed while rendering".into(),
        });
    }
    db.set_thumbnail_for_source_and_settings(
        model_id,
        content_hash,
        settings,
        &thumbnail.png_hash,
        &thumbnail.relative_path,
    )
    .map_err(library_error)
}

fn cached_thumbnail(db: &LibraryDb, model_id: &str) -> Result<Option<String>, AppError> {
    let Some(entry) = db
        .thumbnail_cache_entry_for_model(model_id)
        .map_err(library_error)?
    else {
        return Ok(None);
    };
    if !entry.is_custom {
        let model_hash = db
            .get_model(model_id)
            .map_err(library_error)?
            .and_then(|model| model.content_hash)
            .ok_or_else(|| AppError::Library {
                message: "Generated thumbnail has no model source hash".into(),
            })?;
        let settings = db.get_catalog_settings().map_err(library_error)?;
        let expected = db
            .root()
            .join(crate::library_thumbnail::thumbnail_relative_path(
                &model_hash,
                &settings,
            ))
            .canonicalize()
            .map_err(|error| crate::error::map_io_error(&error))?;
        if entry.path != expected {
            return Ok(None);
        }
    }
    crate::library_thumbnail::png_data_uri_with_hash(&entry.path, &entry.png_hash)
        .map(Some)
        .map_err(|message| AppError::Library { message })
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
    let _io_guard = library_state.begin_io()?;
    let (source, library_root, indexed_limit) = with_db(&library_state, |db| {
        let model = db
            .get_model(&model_id)
            .map_err(library_error)?
            .ok_or_else(|| AppError::Library {
                message: format!("Model not found: {model_id}"),
            })?;
        let indexed_limit = if model.source_kind == "indexed" {
            let root_id = model
                .source_root_id
                .as_deref()
                .ok_or_else(|| AppError::Library {
                    message: "Indexed model has no source root".into(),
                })?;
            Some(
                db.list_index_roots()
                    .map_err(library_error)?
                    .into_iter()
                    .find(|root| root.id == root_id)
                    .ok_or_else(|| AppError::Library {
                        message: "Indexed model source root was removed".into(),
                    })?
                    .max_file_bytes,
            )
        } else {
            None
        };
        Ok((
            db.resolve_model_source_ref(&model_id)
                .map_err(library_error)?,
            db.root().to_path_buf(),
            indexed_limit,
        ))
    })?;
    validate_indexed_source_limit(&source, indexed_limit)?;
    let mut processing = stl_core::package_bounds::PackageProcessingContext::new(
        stl_core::zip_bounds::ZipResourceObserver::new(),
    );
    let path = if let Some(member) = source.archive_member.as_deref() {
        crate::library_archives::extract_member_to_cache_in_context_typed(
            &library_root,
            &source.outer_path,
            member,
            source
                .outer_fingerprint
                .as_deref()
                .ok_or_else(|| AppError::Library {
                    message: "Archive model has no source fingerprint".into(),
                })?,
            &source.content_hash,
            source.bytes,
            &mut processing,
        )
        .map_err(AppError::from)?
    } else {
        source.outer_path
    };
    crate::extension_api::load_model_from_path_in_context(
        app,
        &viewer_state,
        &path,
        generation,
        &library_source_identity(&model_id),
        &mut processing,
    )
}

pub(crate) fn validate_indexed_source_limit(
    source: &library_core::ResolvedModelSource,
    indexed_limit: Option<u64>,
) -> Result<(), AppError> {
    let Some(limit) = indexed_limit else {
        return Ok(());
    };
    let actual = if source.archive_member.is_some() {
        source.bytes
    } else {
        std::fs::symlink_metadata(&source.outer_path)
            .map_err(|error| crate::error::map_io_error(&error))?
            .len()
    };
    if actual > limit {
        return Err(AppError::Library {
            message: format!("Indexed model exceeds its source root limit of {limit} bytes"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod access_tests {
    use super::{open_library, with_db, LibraryState};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn catalog_thumbnail_read_does_not_recreate_reset_entries() {
        let nonce = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "gogglelab-catalog-preview-read-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("fixture.stl");
        let bytes = vec![0_u8; 84];
        std::fs::write(&source, &bytes).unwrap();
        let state = LibraryState::try_new(root.join("library")).unwrap();
        let id = state
            .with_db(|db| {
                db.import_file(&source, None, None)
                    .map(|result| result.model.id)
                    .map_err(super::library_error)
            })
            .unwrap();
        assert!(super::get_thumbnail(&state, &id, true).unwrap().is_some());
        let path = state
            .with_db(|db| {
                let path = db
                    .thumbnail_for_model(&id)
                    .map_err(super::library_error)?
                    .unwrap();
                let snapshot = db
                    .default_thumbnail_reset_snapshot()
                    .map_err(super::library_error)?;
                assert_eq!(
                    db.reset_default_thumbnail_records(&snapshot)
                        .map_err(super::library_error)?,
                    1
                );
                Ok(path)
            })
            .unwrap();
        let png = std::fs::read(&path).unwrap();
        assert!(super::get_catalog_thumbnail(&state, &id).unwrap().is_none());
        state
            .with_db(|db| {
                assert!(db
                    .thumbnail_for_model(&id)
                    .map_err(super::library_error)?
                    .is_none());
                Ok(())
            })
            .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), png);
        assert_eq!(std::fs::read(source).unwrap(), bytes);
        assert!(super::get_thumbnail(&state, &id, true).unwrap().is_some());
        assert!(super::get_catalog_thumbnail(&state, &id).unwrap().is_some());
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

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
    use super::{
        cached_thumbnail, library_source_identity, open_library, persist_generated_thumbnail,
        render_thumbnail_models, LibraryState,
    };
    use library_core::ScannedModel;
    use sha2::{Digest, Sha256};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn library_source_identity_is_model_scoped() {
        assert_eq!(library_source_identity("model-42"), "library:model-42");
    }

    #[test]
    fn corrupt_optional_library_does_not_prevent_native_state_construction() {
        let nonce = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "gogglelab-library-corrupt-startup-{}-{nonce}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let db_path = root.join("library.sqlite3");
        std::fs::write(&db_path, b"not a sqlite database").unwrap();
        let before = std::fs::read(&db_path).unwrap();

        let state = LibraryState::try_new(root.clone()).unwrap();
        assert_eq!(state.management_generation(), 1);
        assert!(state
            .runtime()
            .read_db(|db| db.list_models(None, None))
            .is_err());
        assert_eq!(std::fs::read(&db_path).unwrap(), before);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
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
                    archive_member: None,
                    outer_fingerprint: None,
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
            &db.get_catalog_settings().unwrap(),
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

    #[test]
    fn bulk_thumbnail_render_generates_all_and_honors_cancellation_without_source_writes() {
        let nonce = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "gogglelab-library-bulk-{}-{nonce}",
            std::process::id()
        ));
        let source_root = std::env::temp_dir().join(format!(
            "gogglelab-library-bulk-source-{}-{nonce}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&source_root);
        std::fs::create_dir_all(&source_root).unwrap();
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        let sources = [source_root.join("one.stl"), source_root.join("two.stl")];
        for source in &sources {
            std::fs::write(source, &bytes).unwrap();
        }
        let before = sources
            .iter()
            .map(|source| std::fs::metadata(source).unwrap().modified().unwrap())
            .collect::<Vec<_>>();
        let state = LibraryState::new(root.clone());
        let model_ids = state
            .with_db(|db| {
                sources
                    .iter()
                    .map(|source| {
                        db.import_file(source, None, None)
                            .map(|result| result.model.id)
                            .map_err(super::library_error)
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap();
        assert!(state.scan_control().try_begin(0, "thumbnails"));
        state.scan_control().set_thumbnail_progress(model_ids.len());
        render_thumbnail_models(&state, model_ids.clone(), false);
        assert_eq!(state.scan_control().status().thumbnails_completed, 2);
        state
            .with_db(|db| {
                for model_id in &model_ids {
                    assert!(db.thumbnail_for_model(model_id).unwrap().is_some());
                }
                Ok(())
            })
            .unwrap();
        state.scan_control().finish();
        assert_eq!(
            state.scan_control().status().job_kind.as_deref(),
            Some("thumbnails")
        );

        let cached = state
            .with_db(|db| {
                Ok(db
                    .thumbnail_for_model(&model_ids[0])
                    .map_err(super::library_error)?
                    .unwrap())
            })
            .unwrap();
        std::fs::write(&cached, b"corrupt cache").unwrap();
        assert!(state.scan_control().try_begin(0, "thumbnails"));
        state.scan_control().set_thumbnail_progress(1);
        render_thumbnail_models(&state, vec![model_ids[0].clone()], true);
        assert_eq!(state.scan_control().status().thumbnails_completed, 1);
        assert!(crate::library_thumbnail::png_data_uri(&cached).is_ok());
        state.scan_control().finish();

        assert!(state.scan_control().try_begin(0, "thumbnails"));
        state.scan_control().set_thumbnail_progress(model_ids.len());
        state.scan_control().cancel();
        render_thumbnail_models(&state, model_ids, true);
        assert_eq!(state.scan_control().status().thumbnails_completed, 0);
        assert_eq!(
            before,
            sources
                .iter()
                .map(|source| std::fs::metadata(source).unwrap().modified().unwrap())
                .collect::<Vec<_>>()
        );
        state.scan_control().finish();
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(source_root);
    }

    #[test]
    fn stale_render_is_not_published_after_appearance_settings_change() {
        let nonce = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "gogglelab-library-settings-race-{}-{nonce}",
            std::process::id()
        ));
        let source_root = std::env::temp_dir().join(format!(
            "gogglelab-library-settings-race-source-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&source_root).unwrap();
        let source = source_root.join("model.stl");
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        std::fs::write(&source, &bytes).unwrap();
        let db = open_library(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        let original_settings = db.get_catalog_settings().unwrap();
        let thumbnail = crate::library_thumbnail::ensure_thumbnail(
            &root,
            &source,
            model.content_hash.as_deref().unwrap(),
            bytes.len() as u64,
            &original_settings,
            false,
        )
        .unwrap();
        let mut changed = original_settings.clone();
        changed.thumbnail_background = "#FFFFFF".into();
        db.update_catalog_settings(&changed).unwrap();

        assert!(persist_generated_thumbnail(
            &db,
            &model.id,
            model.content_hash.as_deref().unwrap(),
            &original_settings,
            &thumbnail,
        )
        .is_err());
        assert!(db.thumbnail_for_model(&model.id).unwrap().is_none());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(source_root);
    }
}
