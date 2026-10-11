use std::any::{Any, TypeId};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime};

use library_core::{
    CaptureDownloadInput as CoreCaptureDownload, CapturePageInput as CoreCapturePage,
    CaptureReceipt as CoreCaptureReceipt, CaptureReceiptInput as CoreCaptureReceiptInput,
    CaptureSummary as CoreCaptureSummary, Filament as CoreFilament,
    FilamentInput as CoreFilamentInput, Folder as CoreFolder, LibraryDb, LibraryError,
    MetadataPatch as CoreMetadataPatch, Model,
    ModelFilamentAssignment as CoreModelFilamentAssignment,
    ModelGroupFilter as CoreModelGroupFilter, ModelPageQuery as CoreModelPageQuery,
    ModelSort as CoreModelSort, PrintAttempt as CorePrintAttempt,
    PrintAttemptInput as CorePrintAttemptInput, PrintOutcome as CorePrintOutcome,
    PrintStatus as CorePrintStatus, SortDirection as CoreSortDirection, TagMode as CoreTagMode,
    VersionedTarget as CoreVersionedTarget,
};
use library_protocol::{
    BundleEngine, BundlePreviewSummary, Capability, CaptureDownloadResult, CaptureDownloadStatus,
    CaptureEngine, CaptureEnvelope, CaptureImportResult, CaptureItemStatus, CaptureSummary,
    CatalogEngine, CatalogModel, ContentEngine, ContentLease, DownloadEngine, Filament,
    FilamentEngine, FilamentInput, Folder, FolderEngine, GalleryEngine, MediaLease, MetadataEngine,
    MetadataPatch, ModelAvailability, ModelFilamentAssignment, ModelGroupFilter, ModelQuery,
    ModelSort, MutationVersion, Page, PageRequest, PrintAttempt, PrintAttemptInput,
    PrintHistoryEngine, PrintOutcome, PrintStatus, RequestContext, RootAccess, RootHandle,
    RuntimeError, RuntimeErrorCode, RuntimeEvent, RuntimeInfo, RuntimeInfoEngine, RuntimeOwnership,
    RuntimeStatus, RuntimeStatusEngine, ScanEngine, ScanStatus, SortDirection, SourceLease, Tag,
    TagEngine, TagMode, ThumbnailDescriptor, UploadEngine, UploadIntent, UploadToken,
    VersionedTarget, CAPTURE_ENVELOPE_VERSION, MAX_PAGE_LIMIT,
};
use sha2::{Digest, Sha256};

pub mod archives;
pub mod bundles;
pub mod images;
pub mod integration_state;
pub mod scanner;
pub mod thumbnails;

const MAX_ADMITTED_ROOTS: usize = 256;
const MAX_CURSORS: usize = 256;
const MAX_EVENTS: usize = 1024;
const MAX_LEASES: usize = 256;
const LEASE_TTL: Duration = Duration::from_secs(5 * 60);

type EpochInvalidator = Arc<dyn Fn(u64) + Send + Sync>;
type ScanCompletedHook = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct RuntimeRootPolicy {
    pub id: String,
    pub catalog_root_id: Option<String>,
    pub label: String,
    pub path: PathBuf,
    pub access: RootAccess,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub library_root: PathBuf,
    pub integration_settings_path: PathBuf,
    pub roots: Vec<RuntimeRootPolicy>,
    pub capabilities: BTreeSet<Capability>,
    pub local_catalog_root_access: bool,
}

impl RuntimeConfig {
    pub fn desktop(library_root: PathBuf, integration_settings_path: PathBuf) -> Self {
        Self {
            library_root,
            integration_settings_path,
            roots: Vec::new(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::CatalogWrite,
                Capability::MediaRead,
                Capability::MediaWrite,
                Capability::Scan,
                Capability::Integration,
                Capability::Maintenance,
                Capability::PhysicalSourceWrite,
                Capability::HostPaths,
                Capability::HostSlicer,
            ]),
            local_catalog_root_access: true,
        }
    }
}

#[derive(Debug, Clone)]
struct AdmittedRoot {
    handle: RootHandle,
    catalog_root_id: Option<String>,
    path: PathBuf,
}

struct CatalogLock {
    file: File,
    identity: String,
}

impl CatalogLock {
    fn acquire(library_root: &Path) -> Result<Self, RuntimeError> {
        let parent = library_root
            .parent()
            .ok_or_else(|| invalid("Library root must have a stable parent directory"))?;
        let parent_metadata = match fs::symlink_metadata(parent) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(parent).map_err(|_| io_error())?;
                fs::symlink_metadata(parent).map_err(|_| io_error())?
            }
            Err(_) => return Err(io_error()),
        };
        if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
            return Err(invalid("Library parent must be a real directory"));
        }
        let canonical_parent = parent.canonicalize().map_err(|_| io_error())?;
        let leaf = library_root
            .file_name()
            .ok_or_else(|| invalid("Library root is invalid"))?;
        let mut digest = Sha256::new();
        digest.update(canonical_parent.as_os_str().as_encoded_bytes());
        digest.update([0]);
        digest.update(leaf.as_encoded_bytes());
        let identity = format!("{:x}", digest.finalize());
        let lock_path = canonical_parent.join(format!(".gogglelab-{}.lock", &identity[..24]));
        match fs::symlink_metadata(&lock_path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Conflict,
                    message: "Library ownership lock path is unsafe".into(),
                    retryable: false,
                    current_version: None,
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(io_error()),
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&lock_path).map_err(|_| io_error())?;
        let metadata = file.metadata().map_err(|_| io_error())?;
        if !metadata.is_file() {
            return Err(invalid("Library ownership lock is not a regular file"));
        }
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => RuntimeError {
                code: RuntimeErrorCode::Busy,
                message: "Library is already owned by another process".into(),
                retryable: true,
                current_version: None,
            },
            std::fs::TryLockError::Error(_) => RuntimeError {
                code: RuntimeErrorCode::Unsupported,
                message: "Library filesystem does not support exclusive ownership locking".into(),
                retryable: false,
                current_version: None,
            },
        })?;
        Ok(Self { file, identity })
    }
}

impl Drop for CatalogLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[derive(Default)]
struct LifecycleState {
    maintenance: bool,
    active_io: usize,
}

#[derive(Clone)]
struct ModelCursor {
    query: ModelQuery,
    sort_value: String,
    model_id: String,
    revision: u64,
}

#[derive(Clone)]
struct CaptureCursor {
    created_at: String,
    item_id: String,
    revision: u64,
}

#[derive(Clone)]
struct ContentLeaseEntry {
    descriptor: SourceLease,
    path: PathBuf,
    expires: SystemTime,
    principal_id: String,
}

#[derive(Clone)]
struct UploadEntry {
    token: UploadToken,
    path: PathBuf,
    expires: SystemTime,
    sha256: String,
    bytes: u64,
    principal_id: String,
}

pub struct LibraryRuntime {
    db: Mutex<Option<LibraryDb>>,
    root: PathBuf,
    integration_settings_path: PathBuf,
    roots: Vec<AdmittedRoot>,
    capabilities: BTreeSet<Capability>,
    local_catalog_root_access: bool,
    _ownership: CatalogLock,
    maintenance: AtomicBool,
    lifecycle: Mutex<LifecycleState>,
    library_epoch: AtomicU64,
    catalog_revision: AtomicU64,
    invalidators: Mutex<Vec<EpochInvalidator>>,
    scan_completed_hooks: Mutex<Vec<ScanCompletedHook>>,
    cursors: Mutex<HashMap<String, ModelCursor>>,
    capture_cursors: Mutex<HashMap<String, CaptureCursor>>,
    events: Mutex<VecDeque<RuntimeEvent>>,
    shared_states: Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    leases: Mutex<HashMap<String, ContentLeaseEntry>>,
    uploads: Mutex<HashMap<String, UploadEntry>>,
    scan: Arc<scanner::ScanControl>,
    bundle_previews: bundles::BundlePreviewStore,
    self_weak: OnceLock<Weak<LibraryRuntime>>,
}

impl LibraryRuntime {
    pub fn open(config: RuntimeConfig) -> Result<Arc<Self>, RuntimeError> {
        validate_identifier(&config.library_root, "Library root")?;
        validate_identifier(
            &config.integration_settings_path,
            "Integration settings path",
        )?;
        if config.roots.len() > MAX_ADMITTED_ROOTS {
            return Err(invalid("Too many admitted roots"));
        }
        let ownership = CatalogLock::acquire(&config.library_root)?;
        let mut roots = admit_roots(config.roots)?;
        let (db, root) = if config.local_catalog_root_access {
            (None, prepare_lazy_desktop_root(&config.library_root)?)
        } else {
            let db = LibraryDb::open(&config.library_root).map_err(runtime_library_error)?;
            let root = db.root().to_path_buf();
            map_operator_catalog_roots(&db, &mut roots)?;
            (Some(db), root)
        };
        let runtime = Arc::new(Self {
            db: Mutex::new(db),
            root,
            integration_settings_path: config.integration_settings_path,
            roots,
            capabilities: config.capabilities,
            local_catalog_root_access: config.local_catalog_root_access,
            _ownership: ownership,
            maintenance: AtomicBool::new(false),
            lifecycle: Mutex::new(LifecycleState::default()),
            library_epoch: AtomicU64::new(1),
            catalog_revision: AtomicU64::new(1),
            invalidators: Mutex::new(Vec::new()),
            scan_completed_hooks: Mutex::new(Vec::new()),
            cursors: Mutex::new(HashMap::new()),
            capture_cursors: Mutex::new(HashMap::new()),
            events: Mutex::new(VecDeque::new()),
            shared_states: Mutex::new(HashMap::new()),
            leases: Mutex::new(HashMap::new()),
            uploads: Mutex::new(HashMap::new()),
            scan: Arc::new(scanner::ScanControl::new()),
            bundle_previews: bundles::BundlePreviewStore::default(),
            self_weak: OnceLock::new(),
        });
        runtime
            .self_weak
            .set(Arc::downgrade(&runtime))
            .map_err(|_| internal("Runtime self reference could not be initialized"))?;
        Ok(runtime)
    }

    pub fn library_epoch(&self) -> u64 {
        self.library_epoch.load(Ordering::Acquire)
    }

    pub fn catalog_revision(&self) -> u64 {
        self.catalog_revision.load(Ordering::Acquire)
    }

    pub fn maintenance_active(&self) -> bool {
        self.maintenance.load(Ordering::Acquire)
    }

    pub fn ownership_mode(&self) -> RuntimeOwnership {
        RuntimeOwnership {
            lock_identity: self._ownership.identity.clone(),
            exclusive: true,
        }
    }

    pub fn root_handles(&self, context: &RequestContext) -> Result<Vec<RootHandle>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        Ok(self.roots.iter().map(|root| root.handle.clone()).collect())
    }

    pub fn register_epoch_invalidator(&self, callback: Arc<dyn Fn(u64) + Send + Sync>) {
        self.invalidators.lock().unwrap().push(callback);
    }

    pub fn register_scan_completed_hook(&self, callback: Arc<dyn Fn(&str) + Send + Sync>) {
        self.scan_completed_hooks.lock().unwrap().push(callback);
    }

    pub(crate) fn notify_scan_completed(&self, root_id: &str) {
        for callback in self.scan_completed_hooks.lock().unwrap().iter() {
            callback(root_id);
        }
    }

    pub fn scan_control(&self) -> &Arc<scanner::ScanControl> {
        &self.scan
    }

    pub fn install_shared_state<T>(&self, state: Arc<T>) -> Result<(), RuntimeError>
    where
        T: Any + Send + Sync,
    {
        let mut states = self.shared_states.lock().unwrap();
        match states.entry(TypeId::of::<T>()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(state);
                Ok(())
            }
            std::collections::hash_map::Entry::Occupied(_) => Err(RuntimeError {
                code: RuntimeErrorCode::Conflict,
                message: "Runtime shared state is already installed".into(),
                retryable: false,
                current_version: None,
            }),
        }
    }

    pub fn shared_state<T>(&self) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        self.shared_states
            .lock()
            .unwrap()
            .get(&TypeId::of::<T>())
            .cloned()
            .and_then(|state| state.downcast::<T>().ok())
    }

    pub fn begin_io(self: &Arc<Self>) -> Result<RuntimeIoGuard, RuntimeError> {
        let mut lifecycle = self.lifecycle.lock().unwrap();
        if lifecycle.maintenance {
            return Err(maintenance_error());
        }
        lifecycle.active_io = lifecycle
            .active_io
            .checked_add(1)
            .ok_or_else(|| internal("Runtime I/O admission overflow"))?;
        Ok(RuntimeIoGuard {
            runtime: Arc::clone(self),
        })
    }

    fn begin_borrowed_io(&self) -> Result<BorrowedIoGuard<'_>, RuntimeError> {
        let mut lifecycle = self.lifecycle.lock().unwrap();
        if lifecycle.maintenance {
            return Err(maintenance_error());
        }
        lifecycle.active_io = lifecycle
            .active_io
            .checked_add(1)
            .ok_or_else(|| internal("Runtime I/O admission overflow"))?;
        Ok(BorrowedIoGuard { runtime: self })
    }

    pub fn begin_maintenance(self: &Arc<Self>) -> Result<RuntimeMaintenanceGuard, RuntimeError> {
        if self.scan_control().status().running {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Busy,
                message: "Library scan is running".into(),
                retryable: true,
                current_version: None,
            });
        }
        {
            let mut lifecycle = self.lifecycle.lock().unwrap();
            if lifecycle.maintenance || lifecycle.active_io != 0 {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Busy,
                    message: "Library I/O or maintenance is already running".into(),
                    retryable: true,
                    current_version: None,
                });
            }
            lifecycle.maintenance = true;
            self.maintenance.store(true, Ordering::Release);
        }
        *self.db.lock().unwrap() = None;
        Ok(RuntimeMaintenanceGuard {
            runtime: Arc::clone(self),
            release_on_drop: true,
        })
    }

    pub fn read_db<T>(
        &self,
        operation: impl FnOnce(&LibraryDb) -> Result<T, LibraryError>,
    ) -> Result<T, RuntimeError> {
        if self.maintenance.load(Ordering::Acquire) {
            return Err(maintenance_error());
        }
        let mut guard = self.db.lock().unwrap();
        if self.maintenance.load(Ordering::Acquire) {
            return Err(maintenance_error());
        }
        if guard.is_none() {
            *guard = Some(LibraryDb::open(&self.root).map_err(runtime_library_error)?);
        }
        operation(guard.as_ref().unwrap()).map_err(runtime_library_error)
    }

    pub fn with_db<T, E>(&self, operation: impl FnOnce(&LibraryDb) -> Result<T, E>) -> Result<T, E>
    where
        E: From<RuntimeError>,
    {
        if self.maintenance.load(Ordering::Acquire) {
            return Err(E::from(maintenance_error()));
        }
        let mut guard = self.db.lock().unwrap();
        if self.maintenance.load(Ordering::Acquire) {
            return Err(E::from(maintenance_error()));
        }
        if guard.is_none() {
            *guard = Some(
                LibraryDb::open(&self.root)
                    .map_err(runtime_library_error)
                    .map_err(E::from)?,
            );
        }
        let db = guard.as_ref().unwrap();
        let before = db.total_changes();
        let result = operation(db);
        let changed = db.total_changes() > before;
        drop(guard);
        if changed {
            self.record_mutation("catalogChanged", Vec::new());
        }
        result
    }

    pub fn mutate_db<T>(
        &self,
        operation: impl FnOnce(&LibraryDb) -> Result<T, LibraryError>,
    ) -> Result<T, RuntimeError> {
        let value = self.read_db(operation)?;
        self.record_mutation("catalogChanged", Vec::new());
        Ok(value)
    }

    pub fn host_library_root(&self) -> &Path {
        &self.root
    }

    pub fn host_integration_settings_path(&self) -> &Path {
        &self.integration_settings_path
    }

    pub fn admitted_root_path(
        &self,
        context: &RequestContext,
        root_id: &str,
        required: RootAccess,
    ) -> Result<PathBuf, RuntimeError> {
        let capability = match required {
            RootAccess::ReadOnly | RootAccess::Inbox => Capability::CatalogRead,
            RootAccess::Writable => Capability::PhysicalSourceWrite,
        };
        self.require(context, capability)?;
        self.roots
            .iter()
            .find(|root| root.handle.id == root_id && access_allows(root.handle.access, required))
            .map(|root| root.path.clone())
            .ok_or_else(|| RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Root handle is not admitted for this operation".into(),
                retryable: false,
                current_version: None,
            })
    }

    pub(crate) fn validate_scan_root(
        &self,
        root: &library_core::IndexRoot,
    ) -> Result<(), RuntimeError> {
        let canonical = root.path.canonicalize().map_err(|_| RuntimeError {
            code: RuntimeErrorCode::Io,
            message: "Index root is unavailable".into(),
            retryable: true,
            current_version: None,
        })?;
        if canonical != root.path {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Index root identity changed".into(),
                retryable: false,
                current_version: None,
            });
        }
        if self.local_catalog_root_access {
            return Ok(());
        }
        if self.roots.iter().any(|admitted| {
            admitted.catalog_root_id.as_deref() == Some(root.id.as_str())
                && admitted.path == canonical
                && access_allows(admitted.handle.access, RootAccess::ReadOnly)
        }) {
            Ok(())
        } else {
            Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Index root is not admitted by operator policy".into(),
                retryable: false,
                current_version: None,
            })
        }
    }

    pub(crate) fn validate_indexed_source_admission(
        &self,
        model: &Model,
        source: &library_core::ResolvedModelSource,
    ) -> Result<(), RuntimeError> {
        if model.source_kind != "indexed" || self.local_catalog_root_access {
            return Ok(());
        }
        let root_id = model
            .source_root_id
            .as_deref()
            .ok_or_else(|| internal("Indexed source has no root identity"))?;
        if self.roots.iter().any(|root| {
            root.catalog_root_id.as_deref() == Some(root_id)
                && source.outer_path.starts_with(&root.path)
                && access_allows(root.handle.access, RootAccess::ReadOnly)
        }) {
            Ok(())
        } else {
            Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Indexed source root is not admitted".into(),
                retryable: false,
                current_version: None,
            })
        }
    }

    fn require(
        &self,
        context: &RequestContext,
        capability: Capability,
    ) -> Result<(), RuntimeError> {
        if !self.capabilities.contains(&capability) || !context.capabilities.contains(&capability) {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Principal lacks the required capability".into(),
                retryable: false,
                current_version: None,
            });
        }
        Ok(())
    }

    fn require_upload_write(&self, context: &RequestContext) -> Result<(), RuntimeError> {
        let allowed = [Capability::CatalogWrite, Capability::MediaWrite]
            .into_iter()
            .any(|capability| {
                self.capabilities.contains(&capability)
                    && context.capabilities.contains(&capability)
            });
        if allowed {
            Ok(())
        } else {
            Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Principal lacks upload capability".into(),
                retryable: false,
                current_version: None,
            })
        }
    }

    fn advance_epoch(&self) -> u64 {
        let epoch = self.library_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.record_mutation("libraryReplaced", Vec::new());
        self.cursors.lock().unwrap().clear();
        self.capture_cursors.lock().unwrap().clear();
        self.leases.lock().unwrap().clear();
        self.bundle_previews.invalidate_all();
        let uploads = std::mem::take(&mut *self.uploads.lock().unwrap());
        for upload in uploads.into_values() {
            let _ = fs::remove_file(upload.path);
        }
        for callback in self.invalidators.lock().unwrap().iter() {
            callback(epoch);
        }
        epoch
    }

    fn record_mutation(&self, kind: &str, entity_ids: Vec<String>) -> u64 {
        let revision = self.catalog_revision.fetch_add(1, Ordering::AcqRel) + 1;
        let mut events = self.events.lock().unwrap();
        events.push_back(RuntimeEvent {
            revision,
            library_epoch: self.library_epoch(),
            kind: kind.into(),
            entity_ids,
        });
        while events.len() > MAX_EVENTS {
            events.pop_front();
        }
        revision
    }
}

fn prepare_lazy_desktop_root(path: &Path) -> Result<PathBuf, RuntimeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(invalid("Library root must be a real directory"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|_| io_error())?;
        }
        Err(_) => return Err(io_error()),
    }
    path.canonicalize().map_err(|_| io_error())
}

impl RuntimeInfoEngine for LibraryRuntime {
    fn runtime_info(&self, context: &RequestContext) -> Result<RuntimeInfo, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        Ok(RuntimeInfo {
            library_epoch: self.library_epoch(),
            catalog_revision: self.catalog_revision(),
            ownership: self.ownership_mode(),
            capabilities: self
                .capabilities
                .intersection(&context.capabilities)
                .cloned()
                .collect(),
            roots: self.roots.iter().map(|root| root.handle.clone()).collect(),
        })
    }
}

impl CatalogEngine for LibraryRuntime {
    fn list_models(
        &self,
        context: &RequestContext,
        query: ModelQuery,
        page: PageRequest,
    ) -> Result<Page<CatalogModel>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        if page.limit == 0 || page.limit > MAX_PAGE_LIMIT {
            return Err(invalid("Page limit is outside the supported range"));
        }
        let revision = self.catalog_revision();
        let cursor = if let Some(token) = page.cursor.as_deref() {
            let cursor = self
                .cursors
                .lock()
                .unwrap()
                .get(token)
                .cloned()
                .ok_or_else(|| stale("Catalog cursor is unknown or expired"))?;
            if cursor.revision != revision || cursor.query != query {
                return Err(stale("Catalog cursor became stale"));
            }
            Some(cursor)
        } else {
            None
        };
        let core_query = core_model_query(&query);
        let (models, has_more) = self.read_db(|db| {
            let (models, has_more) = db.list_models_page(
                &core_query,
                cursor.as_ref().map(|cursor| cursor.sort_value.as_str()),
                cursor.as_ref().map(|cursor| cursor.model_id.as_str()),
                page.limit,
            )?;
            let models = models
                .into_iter()
                .map(|model| {
                    let thumbnail_id = db
                        .list_thumbnail_gallery(&model.id)?
                        .into_iter()
                        .find(|thumbnail| thumbnail.is_active)
                        .map(|thumbnail| thumbnail.id);
                    Ok((model, thumbnail_id))
                })
                .collect::<Result<Vec<_>, LibraryError>>()?;
            Ok((models, has_more))
        })?;
        if self.catalog_revision() != revision {
            return Err(stale("Catalog changed while the page was being read"));
        }
        let next_cursor = if has_more {
            let last = &models
                .last()
                .ok_or_else(|| internal("Paged model query lost its final item"))?
                .0;
            let token = uuid::Uuid::new_v4().simple().to_string();
            let mut cursors = self.cursors.lock().unwrap();
            cursors.retain(|_, cursor| cursor.revision == revision);
            if cursors.len() >= MAX_CURSORS {
                if let Some(oldest) = cursors.keys().next().cloned() {
                    cursors.remove(&oldest);
                }
            }
            cursors.insert(
                token.clone(),
                ModelCursor {
                    sort_value: model_sort_value(last, query.sort),
                    model_id: last.id.clone(),
                    revision,
                    query,
                },
            );
            Some(token)
        } else {
            None
        };
        Ok(Page {
            items: models
                .into_iter()
                .map(|(model, thumbnail_id)| catalog_model(model, thumbnail_id))
                .collect(),
            next_cursor,
            snapshot_revision: revision,
        })
    }
}

impl MetadataEngine for LibraryRuntime {
    fn update_metadata(
        &self,
        context: &RequestContext,
        target: VersionedTarget,
        patch: MetadataPatch,
    ) -> Result<MutationVersion, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let patch = core_metadata_patch(patch);
        let model = self.mutate_db(|db| {
            db.update_model_metadata(&target.model_id, target.expected_version, &patch)
        })?;
        Ok(mutation_version(model))
    }

    fn bulk_update_metadata(
        &self,
        context: &RequestContext,
        targets: Vec<VersionedTarget>,
        patch: MetadataPatch,
    ) -> Result<Vec<MutationVersion>, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let targets = targets
            .into_iter()
            .map(|target| CoreVersionedTarget {
                model_id: target.model_id,
                expected_version: target.expected_version,
            })
            .collect::<Vec<_>>();
        let patch = core_metadata_patch(patch);
        self.mutate_db(|db| db.bulk_update_metadata(&targets, &patch))
            .map(|models| models.into_iter().map(mutation_version).collect())
    }
}

impl TagEngine for LibraryRuntime {
    fn list_tags(&self, context: &RequestContext) -> Result<Vec<Tag>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.list_tags()).map(|tags| {
            tags.into_iter()
                .map(|tag| Tag {
                    id: tag.id,
                    name: tag.name,
                    category: tag.category,
                })
                .collect()
        })
    }

    fn create_tag(
        &self,
        context: &RequestContext,
        name: String,
        category: Option<String>,
    ) -> Result<Tag, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.create_tag(&name, category.as_deref()))
            .map(|tag| Tag {
                id: tag.id,
                name: tag.name,
                category: tag.category,
            })
    }

    fn update_tag(
        &self,
        context: &RequestContext,
        tag_id: &str,
        name: String,
        category: Option<String>,
    ) -> Result<Tag, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.rename_tag(tag_id, &name, category.as_deref()))
            .map(|tag| Tag {
                id: tag.id,
                name: tag.name,
                category: tag.category,
            })
    }

    fn delete_tag(&self, context: &RequestContext, tag_id: &str) -> Result<(), RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.delete_tag(tag_id))
    }
}

impl FolderEngine for LibraryRuntime {
    fn list_folders(&self, context: &RequestContext) -> Result<Vec<Folder>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.list_folders())
            .map(|folders| folders.into_iter().map(folder).collect())
    }

    fn create_folder(
        &self,
        context: &RequestContext,
        name: String,
        parent_id: Option<String>,
    ) -> Result<Folder, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.create_folder(&name, parent_id.as_deref()))
            .map(folder)
    }

    fn rename_folder(
        &self,
        context: &RequestContext,
        folder_id: &str,
        name: String,
    ) -> Result<Folder, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.rename_folder(folder_id, &name))
            .map(folder)
    }

    fn move_folder(
        &self,
        context: &RequestContext,
        folder_id: &str,
        parent_id: &str,
    ) -> Result<Folder, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.move_folder(folder_id, parent_id))
            .map(folder)
    }

    fn delete_folder(&self, context: &RequestContext, folder_id: &str) -> Result<(), RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.delete_folder(folder_id))
    }
}

impl FilamentEngine for LibraryRuntime {
    fn list_filaments(
        &self,
        context: &RequestContext,
        include_unavailable: bool,
    ) -> Result<Vec<Filament>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.list_filaments(include_unavailable))
            .map(|filaments| filaments.into_iter().map(filament).collect())
    }

    fn create_filament(
        &self,
        context: &RequestContext,
        input: FilamentInput,
    ) -> Result<Filament, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = core_filament_input(input);
        self.mutate_db(|db| db.create_filament(&input))
            .map(filament)
    }

    fn update_filament(
        &self,
        context: &RequestContext,
        filament_id: &str,
        expected_version: u64,
        input: FilamentInput,
    ) -> Result<Filament, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = core_filament_input(input);
        self.mutate_db(|db| db.update_filament(filament_id, expected_version, &input))
            .map(filament)
    }

    fn delete_filament(
        &self,
        context: &RequestContext,
        filament_id: &str,
        expected_version: u64,
    ) -> Result<Option<Filament>, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.delete_filament(filament_id, expected_version))
            .map(|value| value.map(filament))
    }

    fn bulk_assign_model_filaments(
        &self,
        context: &RequestContext,
        assignments: Vec<ModelFilamentAssignment>,
    ) -> Result<Vec<MutationVersion>, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let assignments = assignments
            .into_iter()
            .map(|assignment| CoreModelFilamentAssignment {
                target: CoreVersionedTarget {
                    model_id: assignment.target.model_id,
                    expected_version: assignment.target.expected_version,
                },
                filament_ids: assignment.filament_ids,
            })
            .collect::<Vec<_>>();
        self.mutate_db(|db| db.bulk_assign_model_filaments(&assignments))
            .map(|models| models.into_iter().map(mutation_version).collect())
    }
}

impl PrintHistoryEngine for LibraryRuntime {
    fn list_print_attempts(
        &self,
        context: &RequestContext,
        model_id: &str,
    ) -> Result<Vec<PrintAttempt>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.read_db(|db| db.list_print_attempts(model_id))
            .map(|attempts| attempts.into_iter().map(print_attempt).collect())
    }

    fn create_print_attempt(
        &self,
        context: &RequestContext,
        model_id: &str,
        input: PrintAttemptInput,
    ) -> Result<PrintAttempt, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = core_print_attempt_input(input);
        self.mutate_db(|db| db.create_print_attempt(model_id, &input))
            .map(print_attempt)
    }

    fn update_print_attempt(
        &self,
        context: &RequestContext,
        attempt_id: &str,
        input: PrintAttemptInput,
    ) -> Result<PrintAttempt, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        let input = core_print_attempt_input(input);
        self.mutate_db(|db| db.update_print_attempt(attempt_id, &input))
            .map(print_attempt)
    }

    fn delete_print_attempt(
        &self,
        context: &RequestContext,
        attempt_id: &str,
    ) -> Result<(), RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.mutate_db(|db| db.delete_print_attempt(attempt_id))
    }
}

impl RuntimeStatusEngine for LibraryRuntime {
    fn runtime_status(&self, context: &RequestContext) -> Result<RuntimeStatus, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        Ok(RuntimeStatus {
            library_epoch: self.library_epoch(),
            catalog_revision: self.catalog_revision(),
            maintenance: self.maintenance.load(Ordering::Acquire),
        })
    }

    fn events_since(
        &self,
        context: &RequestContext,
        after_revision: u64,
        page: PageRequest,
    ) -> Result<Page<RuntimeEvent>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        if page.cursor.is_some() || page.limit == 0 || page.limit > MAX_PAGE_LIMIT {
            return Err(invalid("Event page request is invalid"));
        }
        let revision = self.catalog_revision();
        let items = self
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.revision > after_revision)
            .take(usize::from(page.limit))
            .cloned()
            .collect();
        Ok(Page {
            items,
            next_cursor: None,
            snapshot_revision: revision,
        })
    }
}

impl ScanEngine for LibraryRuntime {
    fn start_scan(
        &self,
        context: &RequestContext,
        catalog_root_id: Option<String>,
    ) -> Result<ScanStatus, RuntimeError> {
        self.require(context, Capability::Scan)?;
        let runtime = self
            .self_weak
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| internal("Runtime self reference is unavailable"))?;
        if self.local_catalog_root_access {
            scanner::start_manual_scan(runtime, catalog_root_id).map_err(scan_error)?;
        } else {
            let catalog_roots = self.read_db(|db| db.list_index_roots())?;
            let admitted_ids = self
                .roots
                .iter()
                .filter_map(|policy| {
                    let catalog_id = policy.catalog_root_id.as_ref()?;
                    catalog_roots
                        .iter()
                        .find(|root| &root.id == catalog_id && root.path == policy.path)
                        .map(|_| catalog_id.clone())
                })
                .collect::<BTreeSet<_>>();
            let selected = if let Some(root_id) = catalog_root_id {
                if !admitted_ids.contains(&root_id) {
                    return Err(RuntimeError {
                        code: RuntimeErrorCode::Forbidden,
                        message: "Catalog root is not admitted for scanning".into(),
                        retryable: false,
                        current_version: None,
                    });
                }
                vec![root_id]
            } else {
                admitted_ids.into_iter().collect::<Vec<_>>()
            };
            if selected.is_empty() {
                return Err(RuntimeError {
                    code: RuntimeErrorCode::Forbidden,
                    message: "No catalog roots are admitted for scanning".into(),
                    retryable: false,
                    current_version: None,
                });
            }
            scanner::start_selected_scan(runtime, selected).map_err(scan_error)?;
        }
        Ok(scan_status(
            self.scan_control().status(),
            self.capabilities.contains(&Capability::HostPaths)
                && context.capabilities.contains(&Capability::HostPaths),
        ))
    }

    fn cancel_scan(&self, context: &RequestContext) -> Result<ScanStatus, RuntimeError> {
        self.require(context, Capability::Scan)?;
        self.scan_control().cancel();
        Ok(scan_status(
            self.scan_control().status(),
            self.capabilities.contains(&Capability::HostPaths)
                && context.capabilities.contains(&Capability::HostPaths),
        ))
    }

    fn scan_status(&self, context: &RequestContext) -> Result<ScanStatus, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        Ok(scan_status(
            self.scan_control().status(),
            self.capabilities.contains(&Capability::HostPaths)
                && context.capabilities.contains(&Capability::HostPaths),
        ))
    }
}

impl BundleEngine for LibraryRuntime {
    fn prepare_bundle_preview(
        &self,
        context: &RequestContext,
        bundle_id: &str,
    ) -> Result<BundlePreviewSummary, RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        self.bundle_previews.prepare(self, context, bundle_id)
    }

    fn describe_bundle_geometry(
        &self,
        context: &RequestContext,
        token: &str,
    ) -> Result<ContentLease, RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        self.bundle_previews
            .describe(self, &context.principal_id, token)
    }

    fn stream_bundle_geometry(
        &self,
        context: &RequestContext,
        token: &str,
        sink: &mut dyn Write,
    ) -> Result<(), RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        self.bundle_previews
            .stream(self, &context.principal_id, token, sink)
    }

    fn close_bundle_preview(
        &self,
        context: &RequestContext,
        token: &str,
    ) -> Result<(), RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        self.bundle_previews.close(&context.principal_id, token)
    }
}

impl GalleryEngine for LibraryRuntime {
    fn list_thumbnails(
        &self,
        context: &RequestContext,
        model_id: &str,
    ) -> Result<Vec<ThumbnailDescriptor>, RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        let root = self.root.clone();
        self.read_db(|db| db.list_thumbnail_gallery(model_id))?
            .into_iter()
            .map(|item| {
                let path = root.join(&item.relative_path);
                let metadata = fs::symlink_metadata(&path).map_err(|_| io_error())?;
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || metadata.len() > 4 * 1024 * 1024
                {
                    return Err(RuntimeError {
                        code: RuntimeErrorCode::Corrupt,
                        message: "Thumbnail asset is missing or unsafe".into(),
                        retryable: false,
                        current_version: None,
                    });
                }
                Ok(ThumbnailDescriptor {
                    thumbnail_id: item.id,
                    model_id: item.model_id,
                    ordinal: item.ordinal,
                    active: item.is_active,
                    custom: item.is_custom,
                    mime: "image/png".into(),
                    bytes: metadata.len(),
                    sha256: item.content_hash,
                })
            })
            .collect()
    }

    fn set_active_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        thumbnail_id: &str,
    ) -> Result<u64, RuntimeError> {
        self.require(context, Capability::MediaWrite)?;
        let model =
            self.mutate_db(|db| db.set_active_thumbnail(model_id, expected_version, thumbnail_id))?;
        Ok(model.metadata_version)
    }

    fn add_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        upload_token: &str,
    ) -> Result<u64, RuntimeError> {
        self.require(context, Capability::MediaWrite)?;
        let _io = self.begin_borrowed_io()?;
        let upload = self
            .uploads
            .lock()
            .unwrap()
            .get(upload_token)
            .cloned()
            .ok_or_else(|| stale("Thumbnail upload token is unknown or expired"))?;
        if upload.expires <= SystemTime::now()
            || upload.token.library_epoch != self.library_epoch()
            || upload.principal_id != context.principal_id
        {
            return Err(stale("Thumbnail upload token is stale"));
        }
        let (relative, hash) = images::import_custom_image_file(&self.root, &upload.path)
            .map_err(|_| invalid("Thumbnail upload is not a valid bounded PNG or JPEG"))?;
        let (model, _) = self.mutate_db(|db| {
            db.add_custom_thumbnail(model_id, expected_version, &hash, &relative)
        })?;
        let mut uploads = self.uploads.lock().unwrap();
        if uploads
            .get(upload_token)
            .is_some_and(|upload| upload.principal_id != context.principal_id)
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Upload token belongs to another principal".into(),
                retryable: false,
                current_version: None,
            });
        }
        if let Some(upload) = uploads.remove(upload_token) {
            let _ = fs::remove_file(upload.path);
        }
        Ok(model.metadata_version)
    }

    fn delete_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        thumbnail_id: &str,
    ) -> Result<u64, RuntimeError> {
        self.require(context, Capability::MediaWrite)?;
        let _io = self.begin_borrowed_io()?;
        let (model, path) = self.mutate_db(|db| {
            db.delete_thumbnail_gallery_item(model_id, expected_version, thumbnail_id)
        })?;
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(io_error()),
        }
        Ok(model.metadata_version)
    }

    fn issue_thumbnail_media(
        &self,
        context: &RequestContext,
        model_id: &str,
        thumbnail_id: &str,
    ) -> Result<MediaLease, RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        let item = self
            .read_db(|db| db.list_thumbnail_gallery(model_id))?
            .into_iter()
            .find(|item| item.id == thumbnail_id)
            .ok_or_else(|| RuntimeError {
                code: RuntimeErrorCode::NotFound,
                message: "Thumbnail was not found".into(),
                retryable: false,
                current_version: None,
            })?;
        let path = self.root.join(&item.relative_path);
        let metadata = fs::symlink_metadata(&path).map_err(|_| io_error())?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() > 4 * 1024 * 1024
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Thumbnail asset is missing or unsafe".into(),
                retryable: false,
                current_version: None,
            });
        }
        verify_content_file(&path, metadata.len(), &item.content_hash)?;
        let expires = SystemTime::now() + LEASE_TTL;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let descriptor = MediaLease {
            id: id.clone(),
            file_name: format!("thumbnail-{thumbnail_id}.png"),
            mime: "image/png".into(),
            bytes: metadata.len(),
            sha256: item.content_hash,
            library_epoch: self.library_epoch(),
            expires_at: format_system_time(expires)?,
        };
        let mut leases = self.leases.lock().unwrap();
        let epoch = self.library_epoch();
        let now = SystemTime::now();
        leases.retain(|_, lease| lease.expires > now && lease.descriptor.library_epoch == epoch);
        if leases.len() >= MAX_LEASES {
            if let Some(oldest) = leases.keys().next().cloned() {
                leases.remove(&oldest);
            }
        }
        leases.insert(
            id,
            ContentLeaseEntry {
                descriptor: descriptor.clone(),
                path,
                expires,
                principal_id: context.principal_id.clone(),
            },
        );
        Ok(descriptor)
    }
}

impl UploadEngine for LibraryRuntime {
    fn stage_upload(
        &self,
        context: &RequestContext,
        intent: UploadIntent,
        source: &mut dyn Read,
    ) -> Result<UploadToken, RuntimeError> {
        self.require_upload_write(context)?;
        validate_upload_intent(&intent)?;
        let _io = self.begin_borrowed_io()?;
        self.read_db(|_| Ok(()))?;
        let staging = self.root.join("staging");
        let metadata = fs::symlink_metadata(&staging).map_err(|_| io_error())?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || staging.canonicalize().map_err(|_| io_error())? != staging
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Runtime staging directory is unsafe".into(),
                retryable: false,
                current_version: None,
            });
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let path = staging.join(format!("upload-{id}.part"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| io_error())?;
        let mut digest = Sha256::new();
        let mut copied = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        let result = (|| {
            loop {
                let count = source.read(&mut buffer).map_err(|_| io_error())?;
                if count == 0 {
                    break;
                }
                copied = copied
                    .checked_add(count as u64)
                    .ok_or_else(|| invalid("Upload size overflow"))?;
                if copied > intent.bytes {
                    return Err(invalid("Upload exceeds its declared size"));
                }
                digest.update(&buffer[..count]);
                file.write_all(&buffer[..count]).map_err(|_| io_error())?;
            }
            if copied != intent.bytes || format!("{:x}", digest.finalize()) != intent.sha256 {
                return Err(stale("Upload bytes do not match their declared digest"));
            }
            file.sync_all().map_err(|_| io_error())?;
            Ok(())
        })();
        if let Err(error) = result {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        let expires = SystemTime::now() + LEASE_TTL;
        let token = UploadToken {
            id: id.clone(),
            item_id: intent.item_id,
            capture_id: intent.capture_id,
            expires_at: format_system_time(expires)?,
            library_epoch: self.library_epoch(),
        };
        let mut uploads = self.uploads.lock().unwrap();
        let now = SystemTime::now();
        let epoch = self.library_epoch();
        uploads.retain(|_, upload| {
            let keep = upload.expires > now && upload.token.library_epoch == epoch;
            if !keep {
                let _ = fs::remove_file(&upload.path);
            }
            keep
        });
        if uploads.len() >= MAX_LEASES {
            let _ = fs::remove_file(&path);
            return Err(RuntimeError {
                code: RuntimeErrorCode::Busy,
                message: "Upload staging limit reached".into(),
                retryable: true,
                current_version: None,
            });
        }
        uploads.insert(
            id,
            UploadEntry {
                token: token.clone(),
                path,
                expires,
                sha256: intent.sha256,
                bytes: intent.bytes,
                principal_id: context.principal_id.clone(),
            },
        );
        Ok(token)
    }

    fn discard_upload(
        &self,
        context: &RequestContext,
        upload_token: &str,
    ) -> Result<(), RuntimeError> {
        self.require_upload_write(context)?;
        let mut uploads = self.uploads.lock().unwrap();
        if uploads
            .get(upload_token)
            .is_some_and(|upload| upload.principal_id != context.principal_id)
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Upload token belongs to another principal".into(),
                retryable: false,
                current_version: None,
            });
        }
        if let Some(upload) = uploads.remove(upload_token) {
            let _ = fs::remove_file(upload.path);
        }
        Ok(())
    }
}

impl DownloadEngine for LibraryRuntime {
    fn issue_model_download(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_hash: Option<&str>,
    ) -> Result<SourceLease, RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        let (model, source) = self.read_db(|db| {
            let model = db
                .get_model(model_id)?
                .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))?;
            let source = db.resolve_model_source_ref(model_id)?;
            Ok((model, source))
        })?;
        self.validate_indexed_source_admission(&model, &source)?;
        if expected_hash.is_some_and(|hash| hash != source.content_hash) {
            return Err(stale("Model source hash changed"));
        }
        let path = if let Some(member) = source.archive_member.as_deref() {
            archives::extract_member_to_cache(
                &self.root,
                &source.outer_path,
                member,
                source
                    .outer_fingerprint
                    .as_deref()
                    .ok_or_else(|| RuntimeError {
                        code: RuntimeErrorCode::Corrupt,
                        message: "Archive model has no outer fingerprint".into(),
                        retryable: false,
                        current_version: None,
                    })?,
                &source.content_hash,
                source.bytes,
            )
            .map_err(|_| RuntimeError {
                code: RuntimeErrorCode::Corrupt,
                message: "Archive member failed source verification".into(),
                retryable: false,
                current_version: None,
            })?
        } else {
            source.outer_path
        };
        verify_content_file(&path, source.bytes, &source.content_hash)?;
        let file_name = safe_download_name(
            &model.display_name,
            source
                .archive_member
                .as_deref()
                .map(Path::new)
                .unwrap_or(path.as_path()),
        );
        let expires = SystemTime::now() + LEASE_TTL;
        let expires_at = time::OffsetDateTime::from(expires)
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| internal("Lease expiry could not be formatted"))?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let descriptor = SourceLease {
            id: id.clone(),
            file_name,
            mime: if model
                .format
                .as_deref()
                .is_some_and(|value| value.contains("3mf"))
            {
                "model/3mf".into()
            } else {
                "model/stl".into()
            },
            bytes: source.bytes,
            sha256: source.content_hash,
            library_epoch: self.library_epoch(),
            expires_at,
        };
        let mut leases = self.leases.lock().unwrap();
        let epoch = self.library_epoch();
        let now = SystemTime::now();
        leases.retain(|_, lease| lease.expires > now && lease.descriptor.library_epoch == epoch);
        if leases.len() >= MAX_LEASES {
            if let Some(oldest) = leases.keys().next().cloned() {
                leases.remove(&oldest);
            }
        }
        leases.insert(
            id,
            ContentLeaseEntry {
                descriptor: descriptor.clone(),
                path,
                expires,
                principal_id: context.principal_id.clone(),
            },
        );
        Ok(descriptor)
    }
}

impl ContentEngine for LibraryRuntime {
    fn describe_content(
        &self,
        context: &RequestContext,
        lease_id: &str,
    ) -> Result<ContentLease, RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        let entry = self.admitted_content_entry(context, lease_id)?;
        verify_content_file(
            &entry.path,
            entry.descriptor.bytes,
            &entry.descriptor.sha256,
        )?;
        Ok(entry.descriptor)
    }

    fn stream_content(
        &self,
        context: &RequestContext,
        lease_id: &str,
        sink: &mut dyn Write,
    ) -> Result<(), RuntimeError> {
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        let entry = self.admitted_content_entry(context, lease_id)?;
        let bytes = read_verified_content(
            &entry.path,
            entry.descriptor.bytes,
            &entry.descriptor.sha256,
        )?;
        sink.write_all(&bytes).map_err(|_| io_error())?;
        Ok(())
    }
}

impl LibraryRuntime {
    fn admitted_content_entry(
        &self,
        context: &RequestContext,
        lease_id: &str,
    ) -> Result<ContentLeaseEntry, RuntimeError> {
        let entry = self
            .leases
            .lock()
            .unwrap()
            .get(lease_id)
            .cloned()
            .ok_or_else(|| stale("Content lease is unknown or expired"))?;
        if entry.principal_id != context.principal_id {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Content lease belongs to another principal".into(),
                retryable: false,
                current_version: None,
            });
        }
        if entry.expires <= SystemTime::now()
            || entry.descriptor.library_epoch != self.library_epoch()
        {
            self.leases.lock().unwrap().remove(lease_id);
            return Err(stale("Content lease is stale"));
        }
        Ok(entry)
    }
}

impl CaptureEngine for LibraryRuntime {
    fn list_capture_items(
        &self,
        context: &RequestContext,
        page: PageRequest,
    ) -> Result<Page<CaptureSummary>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        if page.limit == 0 || page.limit > MAX_PAGE_LIMIT {
            return Err(invalid("Page limit is outside the supported range"));
        }
        let revision = self.catalog_revision();
        let cursor = if let Some(token) = page.cursor.as_deref() {
            let cursor = self
                .capture_cursors
                .lock()
                .unwrap()
                .get(token)
                .cloned()
                .ok_or_else(|| stale("Capture cursor is unknown or expired"))?;
            if cursor.revision != revision {
                return Err(stale("Capture cursor became stale"));
            }
            Some(cursor)
        } else {
            None
        };
        let (summaries, has_more) = self.read_db(|db| {
            db.list_capture_summaries_page(
                cursor.as_ref().map(|cursor| cursor.created_at.as_str()),
                cursor.as_ref().map(|cursor| cursor.item_id.as_str()),
                page.limit,
            )
        })?;
        if self.catalog_revision() != revision {
            return Err(stale("Catalog changed while capture inbox was being read"));
        }
        let next_cursor = if has_more {
            let last = summaries
                .last()
                .ok_or_else(|| internal("Capture page lost its final item"))?;
            let token = uuid::Uuid::new_v4().simple().to_string();
            let mut cursors = self.capture_cursors.lock().unwrap();
            cursors.retain(|_, cursor| cursor.revision == revision);
            if cursors.len() >= MAX_CURSORS {
                if let Some(oldest) = cursors.keys().next().cloned() {
                    cursors.remove(&oldest);
                }
            }
            cursors.insert(
                token.clone(),
                CaptureCursor {
                    created_at: last.created_at.clone(),
                    item_id: last.item_id.clone(),
                    revision,
                },
            );
            Some(token)
        } else {
            None
        };
        Ok(Page {
            items: summaries.into_iter().map(capture_summary).collect(),
            next_cursor,
            snapshot_revision: revision,
        })
    }

    fn import_capture(
        &self,
        context: &RequestContext,
        envelope: CaptureEnvelope,
        uploads: Vec<UploadToken>,
    ) -> Result<CaptureImportResult, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        if envelope.version != CAPTURE_ENVELOPE_VERSION {
            return Err(invalid("Capture envelope version is unsupported"));
        }
        if envelope
            .downloads
            .iter()
            .any(|download| download.relative_inbox_path.is_some())
        {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Forbidden,
                message: "Browser capture cannot supply server filesystem paths".into(),
                retryable: false,
                current_version: None,
            });
        }
        let canonical = serde_json::to_vec(&envelope)
            .map_err(|_| invalid("Capture envelope could not be canonicalized"))?;
        let payload_hash = format!("{:x}", Sha256::digest(&canonical));
        let existing = self.read_db(|db| db.get_capture_receipt(&envelope.item_id))?;
        let upload_entries = {
            let stored = self.uploads.lock().unwrap();
            let mut entries = HashMap::new();
            for token in &uploads {
                let entry = stored
                    .get(&token.id)
                    .filter(|entry| entry.token == *token)
                    .cloned()
                    .ok_or_else(|| stale("Capture upload token is unknown or expired"))?;
                if entry.expires <= SystemTime::now()
                    || entry.token.library_epoch != self.library_epoch()
                    || entry.principal_id != context.principal_id
                    || entry.token.item_id != envelope.item_id
                    || !envelope
                        .downloads
                        .iter()
                        .any(|download| download.capture_id == entry.token.capture_id)
                    || entries
                        .insert(entry.token.capture_id.clone(), entry)
                        .is_some()
                {
                    return Err(stale("Capture upload token does not match its envelope"));
                }
            }
            entries
        };
        let page = envelope.page.clone();
        let downloads = envelope.downloads.clone();
        let input = CoreCaptureReceiptInput {
            item_id: envelope.item_id.clone(),
            payload_hash,
            created_at: envelope.created_at.clone(),
            page: CoreCapturePage {
                url: page.url.clone(),
                title: page.title.clone(),
                designer: page.designer.clone(),
                model_license: page.license.clone(),
                notes: page.notes.clone(),
                parent_external_id: page.parent_external_id.clone(),
            },
            downloads: downloads
                .iter()
                .map(|download| CoreCaptureDownload {
                    capture_id: download.capture_id.clone(),
                    file_name: download.file_name.clone(),
                    sha256: download.sha256.clone(),
                    bytes: download.bytes,
                })
                .collect(),
        };
        let mut receipt = self.mutate_db(|db| db.create_or_get_capture_receipt(&input))?;
        if existing.is_some_and(|receipt| receipt.status == "imported") {
            return Ok(capture_result(receipt, true));
        }
        let bookmark_id = receipt
            .page_model_id
            .clone()
            .ok_or_else(|| internal("Capture bookmark is missing"))?;
        for download in downloads {
            if receipt.downloads.iter().any(|result| {
                result.capture_id == download.capture_id
                    && matches!(result.status.as_str(), "imported" | "imported_removed")
            }) {
                continue;
            }
            let Some(upload) = upload_entries.get(&download.capture_id) else {
                continue;
            };
            if download
                .sha256
                .as_deref()
                .is_some_and(|hash| hash != upload.sha256)
                || download.bytes.is_some_and(|bytes| bytes != upload.bytes)
            {
                return Err(stale("Capture upload metadata changed"));
            }
            let imported = self.mutate_db(|db| {
                let model = db
                    .import_file(&upload.path, None, Some(&download.file_name))?
                    .model;
                db.update_model_metadata(
                    &model.id,
                    model.metadata_version,
                    &CoreMetadataPatch {
                        designer: page.designer.clone(),
                        source_url: Some(page.url.clone()),
                        notes: page.notes.clone(),
                        model_license: page.license.clone(),
                        parent_model_id: Some(bookmark_id.clone()),
                        ..CoreMetadataPatch::default()
                    },
                )
            });
            match imported {
                Ok(model) => {
                    receipt = self.mutate_db(|db| {
                        db.update_capture_download_result(
                            &envelope.item_id,
                            &download.capture_id,
                            Some(&model.id),
                            None,
                        )
                    })?;
                    if let Some(upload) = self.uploads.lock().unwrap().remove(&upload.token.id) {
                        let _ = fs::remove_file(upload.path);
                    }
                }
                Err(_) => {
                    receipt = self.mutate_db(|db| {
                        db.update_capture_download_result(
                            &envelope.item_id,
                            &download.capture_id,
                            None,
                            Some("import_failed"),
                        )
                    })?;
                }
            }
        }
        Ok(capture_result(receipt, false))
    }

    fn get_capture_result(
        &self,
        context: &RequestContext,
        item_id: &str,
    ) -> Result<CaptureImportResult, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        let receipt = self
            .read_db(|db| db.get_capture_receipt(item_id))?
            .ok_or_else(|| RuntimeError {
                code: RuntimeErrorCode::NotFound,
                message: "Capture receipt was not found".into(),
                retryable: false,
                current_version: None,
            })?;
        Ok(capture_result(receipt, false))
    }
}

pub struct RuntimeIoGuard {
    runtime: Arc<LibraryRuntime>,
}

struct BorrowedIoGuard<'a> {
    runtime: &'a LibraryRuntime,
}

impl Drop for BorrowedIoGuard<'_> {
    fn drop(&mut self) {
        let mut lifecycle = self.runtime.lifecycle.lock().unwrap();
        lifecycle.active_io = lifecycle.active_io.saturating_sub(1);
    }
}

impl Drop for RuntimeIoGuard {
    fn drop(&mut self) {
        let mut lifecycle = self.runtime.lifecycle.lock().unwrap();
        lifecycle.active_io = lifecycle.active_io.saturating_sub(1);
    }
}

pub struct RuntimeMaintenanceGuard {
    runtime: Arc<LibraryRuntime>,
    release_on_drop: bool,
}

impl RuntimeMaintenanceGuard {
    pub fn root(&self) -> &Path {
        &self.runtime.root
    }

    pub fn invalidate(&self) -> u64 {
        self.runtime.advance_epoch()
    }

    pub fn remain_armed(&mut self) {
        self.release_on_drop = false;
    }
}

impl Drop for RuntimeMaintenanceGuard {
    fn drop(&mut self) {
        if self.release_on_drop {
            let mut lifecycle = self.runtime.lifecycle.lock().unwrap();
            lifecycle.maintenance = false;
            self.runtime.maintenance.store(false, Ordering::Release);
        }
    }
}

fn admit_roots(policies: Vec<RuntimeRootPolicy>) -> Result<Vec<AdmittedRoot>, RuntimeError> {
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut roots = Vec::with_capacity(policies.len());
    for policy in policies {
        validate_plain_id(&policy.id, "Root handle")?;
        if let Some(catalog_root_id) = &policy.catalog_root_id {
            validate_plain_id(catalog_root_id, "Catalog root")?;
        }
        if policy.label.trim().is_empty() || policy.label.len() > 200 {
            return Err(invalid("Root label is invalid"));
        }
        if !ids.insert(policy.id.clone()) {
            return Err(invalid("Duplicate root handle"));
        }
        let metadata = fs::symlink_metadata(&policy.path).map_err(|_| io_error())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(invalid("Admitted root must be a real directory"));
        }
        let path = policy.path.canonicalize().map_err(|_| io_error())?;
        if !paths.insert(path.clone()) {
            return Err(invalid("Duplicate admitted root path"));
        }
        roots.push(AdmittedRoot {
            handle: RootHandle {
                id: policy.id,
                catalog_root_id: policy.catalog_root_id.clone(),
                label: policy.label.trim().into(),
                access: policy.access,
            },
            catalog_root_id: policy.catalog_root_id,
            path,
        });
    }
    Ok(roots)
}

fn map_operator_catalog_roots(
    db: &LibraryDb,
    roots: &mut [AdmittedRoot],
) -> Result<(), RuntimeError> {
    let mut catalog_roots = db.list_index_roots().map_err(runtime_library_error)?;
    for root in roots
        .iter_mut()
        .filter(|root| root.handle.access != RootAccess::Inbox)
    {
        let mapped = if let Some(catalog_root_id) = root.catalog_root_id.as_deref() {
            catalog_roots
                .iter()
                .find(|catalog| catalog.id == catalog_root_id && catalog.path == root.path)
                .cloned()
                .ok_or_else(|| RuntimeError {
                    code: RuntimeErrorCode::Forbidden,
                    message: "Configured catalog root ID does not match its admitted path".into(),
                    retryable: false,
                    current_version: None,
                })?
        } else if let Some(existing) = catalog_roots
            .iter()
            .find(|catalog| catalog.path == root.path)
            .cloned()
        {
            existing
        } else {
            let created = db
                .add_index_root(&root.path, 60, 50 * 1024 * 1024)
                .map_err(runtime_library_error)?;
            catalog_roots.push(created.clone());
            created
        };
        root.catalog_root_id = Some(mapped.id.clone());
        root.handle.catalog_root_id = Some(mapped.id);
    }
    Ok(())
}

fn access_allows(actual: RootAccess, required: RootAccess) -> bool {
    actual == required || (actual == RootAccess::Writable && required == RootAccess::ReadOnly)
}

fn validate_plain_id(value: &str, label: &str) -> Result<(), RuntimeError> {
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        })
    {
        return Err(invalid(&format!("{label} is invalid")));
    }
    Ok(())
}

fn validate_identifier(path: &Path, label: &str) -> Result<(), RuntimeError> {
    if path.file_name().is_none() || path.parent().is_none() {
        return Err(invalid(&format!("{label} is invalid")));
    }
    Ok(())
}

fn runtime_library_error(error: LibraryError) -> RuntimeError {
    match error {
        LibraryError::MetadataConflict {
            actual,
            expected: _,
            model_id: _,
        }
        | LibraryError::FilamentVersionConflict {
            actual,
            expected: _,
            filament_id: _,
        } => RuntimeError {
            code: RuntimeErrorCode::Conflict,
            message: "Stored version changed".into(),
            retryable: true,
            current_version: Some(actual),
        },
        LibraryError::ModelNotFound(_)
        | LibraryError::FolderNotFound(_)
        | LibraryError::IndexRootNotFound(_) => RuntimeError {
            code: RuntimeErrorCode::NotFound,
            message: "Library record was not found".into(),
            retryable: false,
            current_version: None,
        },
        LibraryError::SchemaTooNew { .. } | LibraryError::UnsupportedFormat(_) => RuntimeError {
            code: RuntimeErrorCode::Unsupported,
            message: "Library format is unsupported".into(),
            retryable: false,
            current_version: None,
        },
        LibraryError::Io { .. } => io_error(),
        LibraryError::Database(_) => RuntimeError {
            code: RuntimeErrorCode::Corrupt,
            message: "Library database could not be read safely".into(),
            retryable: false,
            current_version: None,
        },
        other => RuntimeError {
            code: RuntimeErrorCode::InvalidInput,
            message: other.to_string(),
            retryable: false,
            current_version: None,
        },
    }
}

fn core_metadata_patch(patch: MetadataPatch) -> CoreMetadataPatch {
    CoreMetadataPatch {
        designer: patch.designer,
        source_url: patch.source_url,
        notes: patch.notes,
        model_license: patch.model_license,
        print_status: patch.print_status.map(|status| match status {
            PrintStatus::Unprinted => CorePrintStatus::Unprinted,
            PrintStatus::Want => CorePrintStatus::Want,
            PrintStatus::Queued => CorePrintStatus::Queued,
            PrintStatus::Printing => CorePrintStatus::Printing,
            PrintStatus::Printed => CorePrintStatus::Printed,
            PrintStatus::Failed => CorePrintStatus::Failed,
        }),
        tags: patch.tag_ids,
        tag_mode: patch.tag_mode.map(|mode| match mode {
            TagMode::Replace => CoreTagMode::Replace,
            TagMode::Merge => CoreTagMode::Merge,
        }),
        folder_id: patch.folder_id,
        parent_model_id: patch.parent_model_id,
    }
}

fn core_model_query(query: &ModelQuery) -> CoreModelPageQuery {
    CoreModelPageQuery {
        folder_id: query.folder_id.clone(),
        search: query.search.clone(),
        sort: match query.sort {
            ModelSort::Name => CoreModelSort::Name,
            ModelSort::Size => CoreModelSort::Size,
            ModelSort::Date => CoreModelSort::Date,
        },
        direction: match query.direction {
            SortDirection::Asc => CoreSortDirection::Asc,
            SortDirection::Desc => CoreSortDirection::Desc,
        },
        tag_id: query.tag_id.clone(),
        category: query.category.clone(),
        designer: query.designer.clone(),
        model_license: query.model_license.clone(),
        print_status: query.print_status.map(|status| match status {
            PrintStatus::Unprinted => CorePrintStatus::Unprinted,
            PrintStatus::Want => CorePrintStatus::Want,
            PrintStatus::Queued => CorePrintStatus::Queued,
            PrintStatus::Printing => CorePrintStatus::Printing,
            PrintStatus::Printed => CorePrintStatus::Printed,
            PrintStatus::Failed => CorePrintStatus::Failed,
        }),
        ever_printed: query.ever_printed,
        bundle_id: query.bundle_id.clone(),
        root_id: query.root_id.clone(),
        filament_id: query.filament_id.clone(),
        source_missing: query.source_missing,
        duplicates_only: query.duplicates_only,
        source_kind: query.source_kind.clone(),
        group: query.group.map(|group| match group {
            ModelGroupFilter::Standalone => CoreModelGroupFilter::Standalone,
            ModelGroupFilter::Bundle => CoreModelGroupFilter::Bundle,
            ModelGroupFilter::Archive => CoreModelGroupFilter::Archive,
        }),
    }
}

fn model_sort_value(model: &Model, sort: ModelSort) -> String {
    match sort {
        ModelSort::Name => model.display_name.to_ascii_lowercase(),
        ModelSort::Size => model.bytes.to_string(),
        ModelSort::Date => model.created_at.clone(),
    }
}

fn mutation_version(model: Model) -> MutationVersion {
    MutationVersion {
        model_id: model.id,
        metadata_version: model.metadata_version,
    }
}

fn core_filament_input(input: FilamentInput) -> CoreFilamentInput {
    CoreFilamentInput {
        brand: input.brand,
        material: input.material,
        color: input.color,
        name: input.name,
        notes: input.notes,
    }
}

fn folder(value: CoreFolder) -> Folder {
    Folder {
        id: value.id,
        name: value.name,
        parent_id: value.parent_id,
        model_count: value.model_count,
    }
}

fn filament(value: CoreFilament) -> Filament {
    Filament {
        id: value.id,
        version: value.version,
        brand: value.brand,
        material: value.material,
        color: value.color,
        name: value.name,
        notes: value.notes,
        origin: match value.origin {
            library_core::FilamentOrigin::Manual => "manual",
            library_core::FilamentOrigin::Spoolman => "spoolman",
        }
        .into(),
        resource_kind: match value.resource_kind {
            library_core::FilamentResourceKind::Manual => "manual",
            library_core::FilamentResourceKind::Filament => "filament",
            library_core::FilamentResourceKind::Spool => "spool",
        }
        .into(),
        source_id: value.source_id,
        external_id: value.external_id,
        parent_filament_id: value.parent_filament_id,
        archived: value.archived,
        source_missing: value.source_missing,
    }
}

fn core_print_attempt_input(input: PrintAttemptInput) -> CorePrintAttemptInput {
    CorePrintAttemptInput {
        printed_at: input.printed_at,
        quantity: input.quantity,
        notes: input.notes,
        filament_ids: input.filament_ids,
        outcome: match input.outcome {
            PrintOutcome::Printed => CorePrintOutcome::Printed,
            PrintOutcome::Failed => CorePrintOutcome::Failed,
        },
    }
}

fn print_attempt(value: CorePrintAttempt) -> PrintAttempt {
    PrintAttempt {
        id: value.id,
        model_id: value.model_id,
        printed_at: value.printed_at,
        quantity: value.quantity,
        notes: value.notes,
        filament_ids: value.filament_ids,
        outcome: match value.outcome {
            CorePrintOutcome::Printed => PrintOutcome::Printed,
            CorePrintOutcome::Failed => PrintOutcome::Failed,
        },
        created_at: value.created_at,
        updated_at: value.updated_at,
    }
}

fn catalog_model(model: Model, thumbnail_id: Option<String>) -> CatalogModel {
    let availability = if model.catalog_kind == "bookmark" {
        ModelAvailability::Bookmark
    } else if model.source_missing {
        ModelAvailability::Missing
    } else {
        ModelAvailability::Available
    };
    let print_status = match model.metadata.print_status {
        CorePrintStatus::Unprinted => "unprinted",
        CorePrintStatus::Want => "want",
        CorePrintStatus::Queued => "queued",
        CorePrintStatus::Printing => "printing",
        CorePrintStatus::Printed => "printed",
        CorePrintStatus::Failed => "failed",
    };
    CatalogModel {
        id: model.id,
        display_name: model.display_name,
        folder_id: model.folder_id,
        format: model.format,
        bytes: model.bytes,
        content_hash: model.content_hash,
        metadata_version: model.metadata_version,
        tags: model.tags,
        filament_ids: model.filament_ids,
        source_kind: model.source_kind,
        availability,
        catalog_kind: model.catalog_kind,
        thumbnail_id,
        designer: model.metadata.designer,
        source_url: model.metadata.source_url,
        notes: model.metadata.notes,
        model_license: model.metadata.model_license,
        print_status: print_status.into(),
    }
}

fn capture_result(receipt: CoreCaptureReceipt, replayed_complete: bool) -> CaptureImportResult {
    let status = if replayed_complete {
        CaptureItemStatus::AlreadyImported
    } else {
        match receipt.status.as_str() {
            "imported" => CaptureItemStatus::Imported,
            "partial" => CaptureItemStatus::Partial,
            _ => CaptureItemStatus::Failed,
        }
    };
    CaptureImportResult {
        item_id: receipt.item_id,
        status,
        page_model_id: receipt.page_model_id,
        downloads: receipt
            .downloads
            .into_iter()
            .map(|download| CaptureDownloadResult {
                capture_id: download.capture_id,
                status: match download.status.as_str() {
                    "imported" => CaptureDownloadStatus::Imported,
                    "imported_removed" => CaptureDownloadStatus::AlreadyImportedRemoved,
                    "failed" => CaptureDownloadStatus::Failed,
                    _ => CaptureDownloadStatus::NeedsFile,
                },
                model_id: download.model_id,
                error_code: download.error_code,
            })
            .collect(),
    }
}

fn capture_summary(summary: CoreCaptureSummary) -> CaptureSummary {
    CaptureSummary {
        item_id: summary.item_id,
        created_at: summary.created_at,
        status: match summary.status.as_str() {
            "imported" => CaptureItemStatus::Imported,
            "failed" => CaptureItemStatus::Failed,
            _ => CaptureItemStatus::Partial,
        },
        page_model_id: summary.page_model_id,
        title: summary.title,
        url: summary.url,
        download_count: summary.download_count,
        failed_count: summary.failed_count,
        needs_file_count: summary.needs_file_count,
    }
}

fn scan_status(status: scanner::LibraryScanStatus, reveal_paths: bool) -> ScanStatus {
    ScanStatus {
        running: status.running,
        cancelling: status.cancelling,
        job_kind: status.job_kind,
        current_root_id: status.current_root_id,
        roots_total: status.roots_total,
        roots_completed: status.roots_completed,
        files_visited: status.files_visited,
        files_indexed: status.files_indexed,
        errors: if reveal_paths {
            status.errors
        } else {
            status
                .errors
                .into_iter()
                .map(|_| "A scan item failed; inspect the local desktop for details".into())
                .collect()
        },
    }
}

fn scan_error(message: String) -> RuntimeError {
    RuntimeError {
        code: if message.contains("already running") {
            RuntimeErrorCode::Busy
        } else if message.contains("maintenance") {
            RuntimeErrorCode::Maintenance
        } else {
            RuntimeErrorCode::InvalidInput
        },
        message,
        retryable: true,
        current_version: None,
    }
}

fn safe_download_name(display_name: &str, source_name: &Path) -> String {
    let mut stem = display_name
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, ' ' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    stem = stem.trim_matches([' ', '.']).to_owned();
    if stem.is_empty() {
        stem = "model".into();
    }
    let extension = source_name
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| {
            extension.eq_ignore_ascii_case("stl") || extension.eq_ignore_ascii_case("3mf")
        })
        .unwrap_or("stl")
        .to_ascii_lowercase();
    format!("{stem}.{extension}")
}

fn validate_upload_intent(intent: &UploadIntent) -> Result<(), RuntimeError> {
    for (value, label) in [
        (intent.item_id.as_str(), "Upload item ID"),
        (intent.capture_id.as_str(), "Upload capture ID"),
    ] {
        validate_plain_id(value, label)?;
    }
    if intent.file_name.is_empty()
        || intent.file_name.len() > 255
        || intent.file_name.contains(['/', '\\', '\0'])
        || intent.bytes == 0
        || intent.bytes > library_core::MAX_INDEXED_FILE_BYTES
        || intent.sha256.len() != 64
        || !intent.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(invalid("Upload intent is invalid"));
    }
    Ok(())
}

fn format_system_time(value: SystemTime) -> Result<String, RuntimeError> {
    time::OffsetDateTime::from(value)
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| internal("Timestamp could not be formatted"))
}

fn verify_content_file(
    path: &Path,
    expected_bytes: u64,
    expected_hash: &str,
) -> Result<(), RuntimeError> {
    read_verified_content(path, expected_bytes, expected_hash).map(|_| ())
}

fn read_verified_content(
    path: &Path,
    expected_bytes: u64,
    expected_hash: &str,
) -> Result<Vec<u8>, RuntimeError> {
    if expected_bytes > library_core::MAX_INDEXED_FILE_BYTES {
        return Err(RuntimeError {
            code: RuntimeErrorCode::Unsupported,
            message: "Model source exceeds the runtime download limit".into(),
            retryable: false,
            current_version: None,
        });
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| io_error())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != expected_bytes
    {
        return Err(stale("Model source file changed"));
    }
    let mut file = File::open(path).map_err(|_| io_error())?;
    let mut bytes = Vec::with_capacity(expected_bytes as usize);
    Read::by_ref(&mut file)
        .take(expected_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| io_error())?;
    if bytes.len() as u64 != expected_bytes
        || format!("{:x}", Sha256::digest(&bytes)) != expected_hash
    {
        return Err(stale("Model source bytes changed"));
    }
    Ok(bytes)
}

fn invalid(message: &str) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::InvalidInput,
        message: message.into(),
        retryable: false,
        current_version: None,
    }
}

fn io_error() -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Io,
        message: "Library filesystem operation failed".into(),
        retryable: false,
        current_version: None,
    }
}

fn maintenance_error() -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Maintenance,
        message: "Library maintenance is in progress".into(),
        retryable: true,
        current_version: None,
    }
}

fn stale(message: &str) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Stale,
        message: message.into(),
        retryable: true,
        current_version: None,
    }
}

fn internal(message: &str) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Internal,
        message: message.into(),
        retryable: false,
        current_version: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use library_protocol::{
        BundleEngine, CaptureDownload, CaptureEngine, CaptureEnvelope, CaptureItemStatus,
        CapturePage, CatalogEngine, ContentEngine, DownloadEngine, FilamentEngine, GalleryEngine,
        MetadataEngine, ModelQuery, PageRequest, RuntimeStatusEngine, UploadEngine, UploadIntent,
        CAPTURE_ENVELOPE_VERSION,
    };

    fn config(root: PathBuf) -> RuntimeConfig {
        let settings = root
            .parent()
            .unwrap()
            .join("runtime-fixture-integrations.json");
        RuntimeConfig::desktop(root, settings)
    }

    fn triangle_stl(x: f32) -> Vec<u8> {
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&1_u32.to_le_bytes());
        for value in [
            0.0_f32,
            0.0,
            1.0,
            x,
            0.0,
            0.0,
            x + 1.0,
            0.0,
            0.0,
            x,
            1.0,
            0.0,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes
    }

    #[test]
    fn exclusive_lock_survives_library_root_swap_and_releases_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let runtime = LibraryRuntime::open(config(root.clone())).unwrap();
        let second = LibraryRuntime::open(config(root.clone())).err().unwrap();
        assert_eq!(second.code, RuntimeErrorCode::Busy);
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::exclusive_lock_child_probe",
                "--nocapture",
            ])
            .env("GOGGLELAB_LOCK_PROBE", &root)
            .status()
            .unwrap();
        assert!(child.success());

        let previous = temp.path().join("previous");
        fs::rename(&root, &previous).unwrap();
        fs::create_dir(&root).unwrap();
        let while_swapped = LibraryRuntime::open(config(root.clone())).err().unwrap();
        assert_eq!(while_swapped.code, RuntimeErrorCode::Busy);
        drop(runtime);
        assert!(LibraryRuntime::open(config(root)).is_ok());
    }

    #[test]
    fn desktop_open_defers_corrupt_database_failure_but_headless_fails_eagerly() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        fs::create_dir(&root).unwrap();
        let db_path = root.join("library.sqlite3");
        let corrupt = b"not a sqlite database".to_vec();
        fs::write(&db_path, &corrupt).unwrap();

        let desktop = LibraryRuntime::open(config(root.clone())).unwrap();
        assert_eq!(fs::read(&db_path).unwrap(), corrupt);
        assert_eq!(
            desktop
                .read_db(|db| db.list_models(None, None))
                .unwrap_err()
                .code,
            RuntimeErrorCode::Corrupt
        );
        assert_eq!(fs::read(&db_path).unwrap(), corrupt);
        drop(desktop);

        let mut headless = config(root);
        headless.local_catalog_root_access = false;
        assert_eq!(
            LibraryRuntime::open(headless).err().unwrap().code,
            RuntimeErrorCode::Corrupt
        );
        assert_eq!(fs::read(&db_path).unwrap(), corrupt);
    }

    #[test]
    fn desktop_open_defers_newer_schema_failure_without_mutating_database() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        drop(LibraryDb::open(&root).unwrap());
        let db_path = root.join("library.sqlite3");
        let connection = rusqlite::Connection::open(&db_path).unwrap();
        connection
            .pragma_update(
                None,
                "user_version",
                library_core::CURRENT_SCHEMA_VERSION + 1,
            )
            .unwrap();
        drop(connection);
        let before = fs::read(&db_path).unwrap();

        let desktop = LibraryRuntime::open(config(root.clone())).unwrap();
        assert_eq!(
            desktop
                .read_db(|db| db.list_models(None, None))
                .unwrap_err()
                .code,
            RuntimeErrorCode::Unsupported
        );
        assert_eq!(fs::read(&db_path).unwrap(), before);
        drop(desktop);

        let mut headless = config(root);
        headless.local_catalog_root_access = false;
        assert_eq!(
            LibraryRuntime::open(headless).err().unwrap().code,
            RuntimeErrorCode::Unsupported
        );
        assert_eq!(fs::read(&db_path).unwrap(), before);
    }

    #[test]
    fn exclusive_lock_child_probe() {
        let Some(root) = std::env::var_os("GOGGLELAB_LOCK_PROBE") else {
            return;
        };
        let error = LibraryRuntime::open(config(PathBuf::from(root)))
            .err()
            .unwrap();
        assert_eq!(error.code, RuntimeErrorCode::Busy);
    }

    #[test]
    fn maintenance_and_io_admission_share_one_epoch_and_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let io = runtime.begin_io().unwrap();
        assert_eq!(
            runtime.begin_maintenance().err().unwrap().code,
            RuntimeErrorCode::Busy
        );
        drop(io);
        let mut maintenance = runtime.begin_maintenance().unwrap();
        let before = runtime.library_epoch();
        assert_eq!(maintenance.invalidate(), before + 1);
        assert_eq!(
            runtime.begin_io().err().unwrap().code,
            RuntimeErrorCode::Maintenance
        );
        maintenance.remain_armed();
        drop(maintenance);
        assert_eq!(
            runtime.begin_io().err().unwrap().code,
            RuntimeErrorCode::Maintenance
        );
    }

    #[test]
    fn admitted_roots_are_opaque_and_capability_checked() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("models");
        fs::create_dir(&source).unwrap();
        let mut config = config(temp.path().join("library"));
        config.roots.push(RuntimeRootPolicy {
            id: "models".into(),
            catalog_root_id: None,
            label: "Models".into(),
            path: source.clone(),
            access: RootAccess::ReadOnly,
        });
        let runtime = LibraryRuntime::open(config).unwrap();
        let context = RequestContext {
            principal_id: "operator".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead]),
            request_id: "request".into(),
        };
        assert_eq!(runtime.root_handles(&context).unwrap()[0].id, "models");
        assert_eq!(
            runtime
                .admitted_root_path(&context, "models", RootAccess::Writable)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert_eq!(
            runtime
                .admitted_root_path(&context, "models", RootAccess::ReadOnly)
                .unwrap(),
            source.canonicalize().unwrap()
        );
    }

    #[test]
    fn scan_rejects_restored_root_when_operator_path_mapping_does_not_match() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        let source = temp.path().join("source");
        let other = temp.path().join("other");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&other).unwrap();
        let db = LibraryDb::open(&library_root).unwrap();
        let root = db
            .add_index_root(&source, 60, library_core::MAX_INDEXED_FILE_BYTES)
            .unwrap();
        drop(db);
        let error = LibraryRuntime::open(RuntimeConfig {
            library_root,
            integration_settings_path: temp.path().join("integrations.json"),
            roots: vec![RuntimeRootPolicy {
                id: "operator-root".into(),
                catalog_root_id: Some(root.id.clone()),
                label: "Other".into(),
                path: other,
                access: RootAccess::ReadOnly,
            }],
            capabilities: BTreeSet::from([Capability::CatalogRead, Capability::Scan]),
            local_catalog_root_access: false,
        })
        .err()
        .unwrap();
        assert_eq!(error.code, RuntimeErrorCode::Forbidden);
    }

    #[test]
    fn fresh_headless_read_only_root_maps_scans_and_downloads_without_path_input() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("part.stl"), triangle_stl(0.0)).unwrap();
        let runtime = LibraryRuntime::open(RuntimeConfig {
            library_root,
            integration_settings_path: temp.path().join("integrations.json"),
            roots: vec![RuntimeRootPolicy {
                id: "operator-root".into(),
                catalog_root_id: None,
                label: "Models".into(),
                path: source,
                access: RootAccess::ReadOnly,
            }],
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::MediaRead,
                Capability::Scan,
            ]),
            local_catalog_root_access: false,
        })
        .unwrap();
        let context = RequestContext {
            principal_id: "headless".into(),
            capabilities: BTreeSet::from([
                Capability::CatalogRead,
                Capability::MediaRead,
                Capability::Scan,
            ]),
            request_id: "fresh-root".into(),
        };
        let handle = runtime.root_handles(&context).unwrap().remove(0);
        let catalog_id = handle.catalog_root_id.unwrap();
        let root = runtime
            .read_db(|db| db.list_index_roots())
            .unwrap()
            .into_iter()
            .find(|root| root.id == catalog_id)
            .unwrap();
        scanner::scan_root(&runtime, &root).unwrap();
        let model = runtime
            .read_db(|db| db.list_models(None, None))
            .unwrap()
            .remove(0);
        assert!(runtime
            .issue_model_download(&context, &model.id, model.content_hash.as_deref())
            .is_ok());
    }

    #[test]
    fn remote_scan_status_redacts_host_paths_without_explicit_path_capability() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        runtime
            .scan_control()
            .error("/secret/models/part.stl: permission denied".into());
        let remote = RequestContext {
            principal_id: "remote".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead]),
            request_id: "status".into(),
        };
        let status = runtime.scan_status(&remote).unwrap();
        assert!(!status.errors.join(" ").contains("/secret"));
        let local = RequestContext {
            principal_id: "desktop".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead, Capability::HostPaths]),
            request_id: "local-status".into(),
        };
        assert!(runtime
            .scan_status(&local)
            .unwrap()
            .errors
            .join(" ")
            .contains("/secret"));
    }

    #[test]
    fn bundle_preview_is_principal_bound_and_revalidates_membership() {
        let temp = tempfile::tempdir().unwrap();
        let first_path = temp.path().join("first.stl");
        let second_path = temp.path().join("second.stl");
        fs::write(&first_path, triangle_stl(0.0)).unwrap();
        fs::write(&second_path, triangle_stl(2.0)).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let first = runtime
            .mutate_db(|db| {
                db.import_file(&first_path, None, None)
                    .map(|value| value.model)
            })
            .unwrap();
        let second = runtime
            .mutate_db(|db| {
                db.import_file(&second_path, None, None)
                    .map(|value| value.model)
            })
            .unwrap();
        let bundle_id = runtime
            .mutate_db(|db| db.create_bundle(&[first.id, second.id]))
            .unwrap();
        let context = RequestContext {
            principal_id: "bundle-reader".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "bundle-preview".into(),
        };
        let summary = runtime
            .prepare_bundle_preview(&context, &bundle_id)
            .unwrap();
        assert_eq!(summary.member_count, 2);
        assert_eq!(summary.triangle_count, 2);
        let descriptor = runtime
            .describe_bundle_geometry(&context, &summary.token)
            .unwrap();
        let mut geometry = Vec::new();
        runtime
            .stream_bundle_geometry(&context, &summary.token, &mut geometry)
            .unwrap();
        assert_eq!(geometry.len() as u64, descriptor.bytes);
        assert_eq!(&geometry[..4], b"STL2");
        let other = RequestContext {
            principal_id: "other".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "other".into(),
        };
        assert_eq!(
            runtime
                .describe_bundle_geometry(&other, &summary.token)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        runtime
            .mutate_db(|db| db.create_model("Changed", None))
            .unwrap();
        assert_eq!(
            runtime
                .stream_bundle_geometry(&context, &summary.token, &mut Vec::new())
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
    }

    #[test]
    fn catalog_pages_are_bounded_path_free_and_stale_after_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        runtime.mutate_db(|db| db.create_model("A", None)).unwrap();
        runtime.mutate_db(|db| db.create_model("B", None)).unwrap();
        let context = RequestContext {
            principal_id: "reader".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead]),
            request_id: "page".into(),
        };
        let first = runtime
            .list_models(
                &context,
                ModelQuery::default(),
                PageRequest {
                    cursor: None,
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].folder_id, library_core::ROOT_FOLDER_ID);
        let by_name = runtime
            .list_models(
                &context,
                ModelQuery {
                    sort: ModelSort::Name,
                    direction: SortDirection::Desc,
                    ..ModelQuery::default()
                },
                PageRequest {
                    cursor: None,
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(by_name.items[0].display_name, "B");
        let by_name_second = runtime
            .list_models(
                &context,
                ModelQuery {
                    sort: ModelSort::Name,
                    direction: SortDirection::Desc,
                    ..ModelQuery::default()
                },
                PageRequest {
                    cursor: by_name.next_cursor,
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(by_name_second.items[0].display_name, "A");
        let cursor = first.next_cursor.clone().unwrap();
        let second = runtime
            .list_models(
                &context,
                ModelQuery::default(),
                PageRequest {
                    cursor: Some(cursor.clone()),
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(second.items.len(), 1);
        assert_ne!(first.items[0].id, second.items[0].id);
        runtime.mutate_db(|db| db.create_model("C", None)).unwrap();
        let stale = runtime
            .list_models(
                &context,
                ModelQuery::default(),
                PageRequest {
                    cursor: Some(cursor),
                    limit: 1,
                },
            )
            .unwrap_err();
        assert_eq!(stale.code, RuntimeErrorCode::Stale);
        let status = runtime.runtime_status(&context).unwrap();
        assert!(status.catalog_revision > first.snapshot_revision);
        assert!(!runtime
            .events_since(&context, 0, PageRequest::default())
            .unwrap()
            .items
            .is_empty());
    }

    #[test]
    fn metadata_bulk_update_is_atomic_on_version_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let first = runtime.mutate_db(|db| db.create_model("A", None)).unwrap();
        let second = runtime.mutate_db(|db| db.create_model("B", None)).unwrap();
        let context = RequestContext {
            principal_id: "editor".into(),
            capabilities: BTreeSet::from([Capability::CatalogWrite]),
            request_id: "metadata".into(),
        };
        let patch = MetadataPatch {
            designer: Some("Designer".into()),
            ..MetadataPatch::default()
        };
        let error = runtime
            .bulk_update_metadata(
                &context,
                vec![
                    VersionedTarget {
                        model_id: first.id.clone(),
                        expected_version: first.metadata_version,
                    },
                    VersionedTarget {
                        model_id: second.id.clone(),
                        expected_version: second.metadata_version + 1,
                    },
                ],
                patch.clone(),
            )
            .unwrap_err();
        assert_eq!(error.code, RuntimeErrorCode::Conflict);
        assert!(runtime
            .read_db(|db| db.get_model(&first.id))
            .unwrap()
            .unwrap()
            .metadata
            .designer
            .is_none());

        let changed = runtime
            .bulk_update_metadata(
                &context,
                vec![
                    VersionedTarget {
                        model_id: first.id.clone(),
                        expected_version: first.metadata_version,
                    },
                    VersionedTarget {
                        model_id: second.id.clone(),
                        expected_version: second.metadata_version,
                    },
                ],
                patch,
            )
            .unwrap();
        assert_eq!(changed.len(), 2);
        assert!(changed
            .iter()
            .all(|model| model.metadata_version == first.metadata_version + 1));
    }

    #[test]
    fn filament_assignment_uses_one_atomic_per_target_transaction() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let first = runtime.mutate_db(|db| db.create_model("A", None)).unwrap();
        let second = runtime.mutate_db(|db| db.create_model("B", None)).unwrap();
        let context = RequestContext {
            principal_id: "editor".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
            request_id: "filaments".into(),
        };
        let filament = runtime
            .create_filament(
                &context,
                FilamentInput {
                    brand: "Brand".into(),
                    material: "PLA".into(),
                    color: "#abcdef".into(),
                    name: "Blue".into(),
                    notes: String::new(),
                },
            )
            .unwrap();
        let assignments = vec![
            ModelFilamentAssignment {
                target: VersionedTarget {
                    model_id: first.id.clone(),
                    expected_version: first.metadata_version,
                },
                filament_ids: vec![filament.id.clone()],
            },
            ModelFilamentAssignment {
                target: VersionedTarget {
                    model_id: second.id.clone(),
                    expected_version: second.metadata_version + 1,
                },
                filament_ids: vec![filament.id.clone()],
            },
        ];
        assert_eq!(
            runtime
                .bulk_assign_model_filaments(&context, assignments)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Conflict
        );
        for id in [&first.id, &second.id] {
            assert!(runtime
                .read_db(|db| db.get_model(id))
                .unwrap()
                .unwrap()
                .filament_ids
                .is_empty());
        }
    }

    #[test]
    fn page_capture_is_idempotent_and_creates_no_fake_source_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let context = RequestContext {
            principal_id: "capture".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
            request_id: "capture-request".into(),
        };
        let envelope = CaptureEnvelope {
            version: CAPTURE_ENVELOPE_VERSION,
            item_id: "item-1".into(),
            created_at: "2026-10-11T00:00:00Z".into(),
            page: CapturePage {
                url: "https://example.test/model".into(),
                title: "Captured model".into(),
                designer: None,
                license: None,
                notes: None,
                parent_external_id: None,
            },
            downloads: vec![],
        };
        let first = runtime
            .import_capture(&context, envelope.clone(), vec![])
            .unwrap();
        assert_eq!(first.status, CaptureItemStatus::Imported);
        let repeated = runtime
            .import_capture(&context, envelope.clone(), vec![])
            .unwrap();
        assert_eq!(repeated.status, CaptureItemStatus::AlreadyImported);
        let mut second_envelope = envelope.clone();
        second_envelope.item_id = "item-2".into();
        second_envelope.created_at = "2026-10-11T00:00:01Z".into();
        runtime
            .import_capture(&context, second_envelope, vec![])
            .unwrap();
        let first_page = runtime
            .list_capture_items(
                &context,
                PageRequest {
                    cursor: None,
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(first_page.items[0].item_id, "item-2");
        let second_page = runtime
            .list_capture_items(
                &context,
                PageRequest {
                    cursor: first_page.next_cursor,
                    limit: 1,
                },
            )
            .unwrap();
        assert_eq!(second_page.items[0].item_id, "item-1");
        let model = runtime
            .read_db(|db| db.get_model(first.page_model_id.as_ref().unwrap()))
            .unwrap()
            .unwrap();
        assert_eq!(model.catalog_kind, "bookmark");
        assert!(model.content_hash.is_none());
        let mut conflict = envelope;
        conflict.created_at = "2026-10-11T00:00:01Z".into();
        assert_eq!(
            runtime
                .import_capture(&context, conflict, vec![])
                .unwrap_err()
                .code,
            RuntimeErrorCode::InvalidInput
        );
    }

    #[test]
    fn shared_process_state_is_single_install_and_same_arc_for_embedded_consumers() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let state = Arc::new(Mutex::new(7_u32));
        runtime.install_shared_state(Arc::clone(&state)).unwrap();
        let loaded = runtime.shared_state::<Mutex<u32>>().unwrap();
        assert!(Arc::ptr_eq(&state, &loaded));
        assert_eq!(
            runtime
                .install_shared_state(Arc::new(Mutex::new(9_u32)))
                .unwrap_err()
                .code,
            RuntimeErrorCode::Conflict
        );
        let retained = runtime.shared_state::<Mutex<u32>>().unwrap();
        assert!(Arc::ptr_eq(&state, &retained));
        assert_eq!(*retained.lock().unwrap(), 7);
    }

    #[test]
    fn gallery_uses_stable_ids_active_selection_and_guarded_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| db.create_model("Gallery", None))
            .unwrap();
        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[10, 20, 30, 255])
                .unwrap();
        }
        let hash = format!("{:x}", Sha256::digest(&png_bytes));
        let first_path = PathBuf::from("thumbnails/runtime-first.png");
        let second_path = PathBuf::from("thumbnails/runtime-second.png");
        fs::write(runtime.root.join(&first_path), &png_bytes).unwrap();
        fs::write(runtime.root.join(&second_path), &png_bytes).unwrap();
        let (model, first) = runtime
            .mutate_db(|db| {
                db.add_custom_thumbnail(&model.id, model.metadata_version, &hash, &first_path)
            })
            .unwrap();
        let (model, second) = runtime
            .mutate_db(|db| {
                db.add_custom_thumbnail(&model.id, model.metadata_version, &hash, &second_path)
            })
            .unwrap();
        let context = RequestContext {
            principal_id: "gallery".into(),
            capabilities: BTreeSet::from([Capability::MediaRead, Capability::MediaWrite]),
            request_id: "gallery-request".into(),
        };
        let gallery = runtime.list_thumbnails(&context, &model.id).unwrap();
        assert_eq!(gallery.len(), 2);
        assert_eq!(gallery.iter().filter(|item| item.active).count(), 1);
        let media = runtime
            .issue_thumbnail_media(&context, &model.id, &second.id)
            .unwrap();
        let mut streamed = Vec::new();
        runtime
            .stream_content(&context, &media.id, &mut streamed)
            .unwrap();
        assert_eq!(streamed, png_bytes);
        let upload = runtime
            .stage_upload(
                &context,
                UploadIntent {
                    item_id: "gallery-upload".into(),
                    capture_id: "gallery-image".into(),
                    file_name: "preview.png".into(),
                    bytes: png_bytes.len() as u64,
                    sha256: hash.clone(),
                },
                &mut png_bytes.as_slice(),
            )
            .unwrap();
        let version = runtime
            .add_thumbnail(&context, &model.id, model.metadata_version, &upload.id)
            .unwrap();
        assert_eq!(
            runtime.list_thumbnails(&context, &model.id).unwrap().len(),
            3
        );
        let version = runtime
            .set_active_thumbnail(&context, &model.id, version, &first.id)
            .unwrap();
        assert!(runtime
            .delete_thumbnail(&context, &model.id, version, &first.id)
            .is_err());
        let version = runtime
            .delete_thumbnail(&context, &model.id, version, &second.id)
            .unwrap();
        assert!(version > model.metadata_version);
        assert!(!runtime.root.join(second_path).exists());
    }

    #[test]
    fn source_lease_streams_verified_bytes_and_epoch_invalidation_revokes_it() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("part.stl");
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        fs::write(&source, &bytes).unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let model = runtime
            .mutate_db(|db| {
                db.import_file(&source, None, None)
                    .map(|result| result.model)
            })
            .unwrap();
        let context = RequestContext {
            principal_id: "download".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "download-request".into(),
        };
        let lease = runtime
            .issue_model_download(&context, &model.id, model.content_hash.as_deref())
            .unwrap();
        assert_eq!(lease.bytes, bytes.len() as u64);
        assert!(!lease.file_name.contains('/'));
        assert_eq!(
            runtime.describe_content(&context, &lease.id).unwrap(),
            lease
        );
        let mut streamed = Vec::new();
        runtime
            .stream_content(&context, &lease.id, &mut streamed)
            .unwrap();
        assert_eq!(streamed, bytes);
        let other = RequestContext {
            principal_id: "other-principal".into(),
            capabilities: BTreeSet::from([Capability::MediaRead]),
            request_id: "other-download".into(),
        };
        assert_eq!(
            runtime
                .describe_content(&other, &lease.id)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert_eq!(
            runtime
                .stream_content(&other, &lease.id, &mut Vec::new())
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        let maintenance = runtime.begin_maintenance().unwrap();
        maintenance.invalidate();
        drop(maintenance);
        assert_eq!(
            runtime
                .describe_content(&context, &lease.id)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
        assert_eq!(
            runtime
                .stream_content(&context, &lease.id, &mut Vec::new())
                .unwrap_err()
                .code,
            RuntimeErrorCode::Stale
        );
    }

    #[test]
    fn indexed_download_requires_operator_admitted_catalog_root() {
        use library_core::{ScannedModel, SourceRef};

        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("library");
        let sources = temp.path().join("sources");
        fs::create_dir(&sources).unwrap();
        let path = sources.join("part.stl");
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        fs::write(&path, &bytes).unwrap();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let db = LibraryDb::open(&library).unwrap();
        let root = db.add_index_root(&sources, 60, 1024).unwrap();
        let scanned = ScannedModel {
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
            .record_scan_entries(&root.id, std::slice::from_ref(&scanned))
            .unwrap();
        db.complete_scan_sources_with_new_models(
            &root.id,
            &[SourceRef {
                relative_path: "part.stl".into(),
                archive_member: None,
            }],
            &added.added_model_ids,
            "1",
        )
        .unwrap();
        let model = db.list_models(None, None).unwrap().pop().unwrap();
        drop(db);
        let context = RequestContext {
            principal_id: "remote".into(),
            capabilities: BTreeSet::from([Capability::MediaRead, Capability::HostPaths]),
            request_id: "remote-download".into(),
        };
        let mut headless = config(library.clone());
        headless.local_catalog_root_access = false;
        let runtime = LibraryRuntime::open(headless).unwrap();
        let bundle_id = runtime
            .mutate_db(|db| db.create_bundle(std::slice::from_ref(&model.id)))
            .unwrap();
        assert_eq!(
            runtime
                .issue_model_download(&context, &model.id, Some(&hash))
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert_eq!(
            runtime
                .prepare_bundle_preview(&context, &bundle_id)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        drop(runtime);
        let desktop = LibraryRuntime::open(config(library.clone())).unwrap();
        assert!(desktop
            .issue_model_download(&context, &model.id, Some(&hash))
            .is_ok());
        drop(desktop);
        let mut admitted = config(library);
        admitted.local_catalog_root_access = false;
        admitted.roots.push(RuntimeRootPolicy {
            id: "operator-models".into(),
            catalog_root_id: Some(root.id),
            label: "Models".into(),
            path: sources,
            access: RootAccess::ReadOnly,
        });
        let runtime = LibraryRuntime::open(admitted).unwrap();
        assert!(runtime
            .issue_model_download(&context, &model.id, Some(&hash))
            .is_ok());
    }

    #[test]
    fn staged_capture_upload_imports_verified_model_and_consumes_token() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = LibraryRuntime::open(config(temp.path().join("library"))).unwrap();
        let context = RequestContext {
            principal_id: "capture-upload".into(),
            capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
            request_id: "capture-upload-request".into(),
        };
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let intent = UploadIntent {
            item_id: "upload-item".into(),
            capture_id: "capture-1".into(),
            file_name: "part.stl".into(),
            bytes: bytes.len() as u64,
            sha256: hash.clone(),
        };
        let token = runtime
            .stage_upload(&context, intent, &mut bytes.as_slice())
            .unwrap();
        let envelope = CaptureEnvelope {
            version: CAPTURE_ENVELOPE_VERSION,
            item_id: "upload-item".into(),
            created_at: "2026-10-11T02:00:00Z".into(),
            page: CapturePage {
                url: "https://example.test/upload".into(),
                title: "Uploaded model".into(),
                designer: Some("Designer".into()),
                license: Some("CC0".into()),
                notes: None,
                parent_external_id: None,
            },
            downloads: vec![CaptureDownload {
                capture_id: "capture-1".into(),
                file_name: "part.stl".into(),
                relative_inbox_path: None,
                sha256: Some(hash.clone()),
                bytes: Some(bytes.len() as u64),
            }],
        };
        let result = runtime
            .import_capture(&context, envelope.clone(), vec![token.clone()])
            .unwrap();
        assert_eq!(result.status, CaptureItemStatus::Imported);
        let model_id = result.downloads[0].model_id.as_ref().unwrap();
        let model = runtime
            .read_db(|db| db.get_model(model_id))
            .unwrap()
            .unwrap();
        assert_eq!(model.content_hash.as_deref(), Some(hash.as_str()));
        assert_eq!(model.metadata.parent_model_id, result.page_model_id);
        assert!(!runtime.uploads.lock().unwrap().contains_key(&token.id));
        assert_eq!(
            runtime
                .import_capture(&context, envelope, vec![])
                .unwrap()
                .status,
            CaptureItemStatus::AlreadyImported
        );
    }

    #[test]
    fn upload_requires_runtime_and_context_write_capability_intersection() {
        let temp = tempfile::tempdir().unwrap();
        let readonly_root = temp.path().join("readonly-library");
        let mut readonly_config = config(readonly_root.clone());
        readonly_config.capabilities = BTreeSet::from([Capability::CatalogRead]);
        let readonly = LibraryRuntime::open(readonly_config).unwrap();
        let context = RequestContext {
            principal_id: "writer".into(),
            capabilities: BTreeSet::from([Capability::CatalogWrite, Capability::MediaWrite]),
            request_id: "forged-write".into(),
        };
        let bytes = b"verified upload".to_vec();
        let intent = UploadIntent {
            item_id: "item".into(),
            capture_id: "capture".into(),
            file_name: "preview.png".into(),
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        assert_eq!(
            readonly
                .stage_upload(&context, intent.clone(), &mut bytes.as_slice())
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        assert!(!readonly_root.join("staging").exists());
        drop(readonly);

        let authorized =
            LibraryRuntime::open(config(temp.path().join("writable-library"))).unwrap();
        let token = authorized
            .stage_upload(&context, intent, &mut bytes.as_slice())
            .unwrap();
        let staged_path = authorized
            .uploads
            .lock()
            .unwrap()
            .get(&token.id)
            .unwrap()
            .path
            .clone();
        assert_eq!(fs::read(&staged_path).unwrap(), bytes);
        authorized.discard_upload(&context, &token.id).unwrap();
        assert!(!staged_path.exists());
    }
}
