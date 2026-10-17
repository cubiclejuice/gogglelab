use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::catalog_vfs::CatalogVfs;
use crate::error::{io, LibraryError};
use crate::models::{
    CaptureDownloadRecord, CaptureReceipt, CaptureReceiptInput, CaptureSummary, CatalogFacetValue,
    CatalogFacets, CatalogSettings, CatalogStats, DuplicateGroup, Filament, FilamentInput,
    FilamentOrigin, FilamentResourceKind, FilamentSyncSummary, Folder, ImportResult, IndexRoot,
    IndexedModelMove, IndexedSourceMetadata, LegacyPrinter, LegacyPrinterImportSummary,
    MetadataPatch, Model, ModelFilamentAssignment, ModelGroupFilter, ModelMetadata, ModelPageQuery,
    ModelSort, PartStock, PartStockInput, PartUsageInput, PartUsageSnapshot, PrintAttempt,
    PrintAttemptBatchItem, PrintAttemptImportItem, PrintAttemptImportResult, PrintAttemptInput,
    PrintAttemptInputWithPrinter, PrintAttemptPart, PrintOutcome, PrintStatus, Printer,
    PrinterConnectionStatus, PrinterDataImport, PrinterDataImportResult, PrinterImportRefMapEntry,
    PrinterInput, PrinterJobStatus, PrinterMaintenanceInput, PrinterMaintenanceLog,
    PrinterReminder, PrinterReminderCompletion, PrinterReminderCompletionInput,
    PrinterReminderInput, PrinterReminderStatus, PrinterSourceKind, RemovalResult, RemovedModel,
    ResolvedModelSource, ReviewedTagStrategy, ReviewedTagTarget, ScanReconcile, ScannedModel,
    SortDirection, SourceRef, SpoolmanFilamentRecord, SpoolmanSource, SpoolmanSpoolRecord, Tag,
    TagMode, ThumbnailCacheEntry, ThumbnailGalleryItem, VersionedModelUpdate, VersionedTarget,
    MAX_MODEL_PAGE_OFFSET,
};

pub const CURRENT_SCHEMA_VERSION: i64 = 11;
const MAX_GLOBAL_METADATA_TARGETS: i64 = 50_000;
const MAX_PRINTERS: usize = 5_000;
const MAX_PRINTER_IMPORT_COUNT: usize = 5_000;
const MAX_PRINTER_DATA_IMPORT_PRINTERS: usize = MAX_PRINTER_IMPORT_COUNT;
const MAX_DISCOVERED_PRINTER_BATCH: usize = 32;
pub const MAX_PRINTER_JSON_EXPORT_PRINTERS: usize = MAX_PRINTERS;
pub const MAX_PRINTER_JSON_EXPORT_MAINTENANCE_LOGS: usize = 50_000;
pub const MAX_PRINTER_JSON_EXPORT_REMINDERS: usize = 10_000;
pub const MAX_PRINTER_JSON_EXPORT_RECORDS: usize = MAX_PRINTER_JSON_EXPORT_PRINTERS
    + MAX_PRINTER_JSON_EXPORT_MAINTENANCE_LOGS
    + MAX_PRINTER_JSON_EXPORT_REMINDERS;
const MAX_PRINTER_UI_MAINTENANCE_LOG_ROWS: usize = MAX_PRINTER_JSON_EXPORT_MAINTENANCE_LOGS;
const MAX_PRINTER_UI_REMINDER_ROWS: usize = MAX_PRINTER_JSON_EXPORT_REMINDERS;
const MAX_PRINTER_DATA_IMPORT_RECORDS: usize = MAX_PRINTER_JSON_EXPORT_RECORDS;
pub const MAX_LIBRARY_JSON_EXPORT_MODELS: usize = 5_000;
const PRINTER_JSON_EXPORT_PAGE_ROWS: usize = 500;
const MAX_PRINT_ATTEMPTS_PER_IMPORT_BATCH: usize = 1_000;
const MAX_PRINT_ATTEMPT_PARTS: usize = 256;
const MAX_PRINTER_TEXT_CHARS: usize = 128;
const MAX_PRINTER_NOTES_BYTES: usize = 20_000;
const MAX_PART_STOCK_RECORDS: usize = 10_000;
const MAX_PART_STOCK_QUANTITY: u32 = 1_000_000;
const PRINTVENTORY_ORPHAN_PART_ID_PREFIX: &str = "printventory_orphan_";
pub const ROOT_FOLDER_ID: &str = "root";
pub const MIN_SCAN_INTERVAL_MINUTES: u32 = 1;
pub const MAX_SCAN_INTERVAL_MINUTES: u32 = 1_440;
pub const MAX_INDEXED_FILE_BYTES: u64 = stl_core::limits::MAX_FILE_BYTES;

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);
const MAX_CACHED_THUMBNAIL_BYTES: u64 = 4 * 1024 * 1024;

const PRINTER_SELECT_COLUMNS: &str = "p.id,p.metadata_version,p.nickname,p.make,p.model,
    p.printer_type,p.firmware,p.host,p.web_url,p.endpoint_approved,p.bed_x_mm,p.bed_y_mm,
    p.bed_z_mm,p.notes,p.source_kind,p.connection_status,p.job_status,p.last_checked_at,
    p.connection_error_code,
    COALESCE((SELECT SUM(a.quantity) FROM print_attempts a WHERE a.printer_id=p.id AND a.outcome='printed'),0),
    (SELECT MAX(a.printed_at) FROM print_attempts a WHERE a.printer_id=p.id AND a.outcome='printed'),
    p.created_at,p.updated_at";
const PRINTER_REMINDER_SELECT_BY_ID: &str =
    "SELECT id,metadata_version,printer_id,printer_nickname,
    title,notes,due_at,interval_days,status,last_completed_at,created_at,updated_at
    FROM printer_reminders WHERE id=?1";
const PART_STOCK_SELECT_COLUMNS: &str =
    "id,metadata_version,name,category,quantity,unit,notes,low_stock,created_at,updated_at";

/// Generate an ID for a native source-trash transaction before its file hold
/// is created. The prefix lets crash recovery distinguish it from catalog-only
/// removals.
pub fn new_source_trash_snapshot_id() -> String {
    new_id("source-trash")
}

// Internal SQLite projection shared by managed and indexed source resolution.
type SourceResolutionRow = (
    String,
    Option<String>,
    Option<String>,
    bool,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
);

pub struct LibraryDb {
    conn: Connection,
    catalog_vfs: Option<CatalogVfs>,
    root: PathBuf,
    catalog_file: Option<Arc<CatalogPin>>,
}

/// A bounded, consistent snapshot used by library JSON export.
#[derive(Debug)]
pub struct PrinterDataExportSnapshot {
    pub printers: Vec<Printer>,
    pub maintenance_logs: Vec<PrinterMaintenanceLog>,
    pub reminders: Vec<PrinterReminder>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrinterDataExportCounts {
    pub printers: usize,
    pub maintenance_logs: usize,
    pub reminders: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrinterExportAttribution {
    pub id: String,
    pub nickname: String,
    pub make: Option<String>,
    pub model: Option<String>,
    pub printer_type: Option<String>,
}

struct CatalogPin {
    root: PathBuf,
    file: File,
    #[cfg(test)]
    before_commit: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    after_commit_identity_check: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl CatalogPin {
    fn matches_path(&self) -> bool {
        catalog_file_matches_path(&self.root, &self.file)
    }

    #[cfg(test)]
    fn install_before_commit_hook(&self, hook: impl FnOnce() + Send + 'static) {
        *self.before_commit.lock().unwrap() = Some(Box::new(hook));
    }

    #[cfg(test)]
    fn run_before_commit_hook(&self) {
        if let Some(hook) = self.before_commit.lock().unwrap().take() {
            hook();
        }
    }

    #[cfg(test)]
    fn install_after_commit_identity_check_hook(&self, hook: impl FnOnce() + Send + 'static) {
        *self.after_commit_identity_check.lock().unwrap() = Some(Box::new(hook));
    }

    #[cfg(test)]
    fn run_after_commit_identity_check_hook(&self) {
        if let Some(hook) = self.after_commit_identity_check.lock().unwrap().take() {
            hook();
        }
    }
}

fn pin_catalog_file(path: &Path) -> std::io::Result<File> {
    match open_catalog_path(path) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match create_catalog_path(path) {
                Ok(file) => Ok(file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    open_catalog_path(path)
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

fn open_catalog_path(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

fn create_catalog_path(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

fn catalog_file_matches_path(root: &Path, pinned: &File) -> bool {
    let path = root.join("library.sqlite3");
    let Ok(path_metadata) = std::fs::symlink_metadata(&path) else {
        return false;
    };
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return false;
    }
    let Ok(current) = open_catalog_path(&path) else {
        return false;
    };
    let Ok(current_metadata) = current.metadata() else {
        return false;
    };
    if !current_metadata.is_file() || is_reparse_point(&current_metadata) {
        return false;
    }
    crate::catalog_vfs::same_file_identity(pinned, &current).is_ok_and(|matches| matches)
}

#[cfg(unix)]
fn sqlite_file_has_moved(connection: &Connection) -> Option<bool> {
    let mut has_moved: std::ffi::c_int = 0;
    let result = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            connection.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_HAS_MOVED,
            (&mut has_moved as *mut std::ffi::c_int).cast(),
        )
    };
    (result == rusqlite::ffi::SQLITE_OK).then_some(has_moved != 0)
}

#[cfg(unix)]
fn is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(any(unix, windows)))]
fn is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

impl LibraryDb {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, LibraryError> {
        Self::open_inner(root, || {}, || {})
    }

    #[cfg(test)]
    fn open_with_hooks<B, A>(
        root: impl AsRef<Path>,
        before_connection_open: B,
        after_connection_open: A,
    ) -> Result<Self, LibraryError>
    where
        B: FnOnce(),
        A: FnOnce(),
    {
        Self::open_inner(root, before_connection_open, after_connection_open)
    }

    fn open_inner<B, A>(
        root: impl AsRef<Path>,
        before_connection_open: B,
        after_connection_open: A,
    ) -> Result<Self, LibraryError>
    where
        B: FnOnce(),
        A: FnOnce(),
    {
        let root = root.as_ref().to_path_buf();
        match std::fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(LibraryError::InvalidInput(
                    "library root must be a real directory".into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(&root).map_err(|e| io(&root, e))?
            }
            Err(error) => return Err(io(&root, error)),
        }
        let root = root.canonicalize().map_err(|error| io(&root, error))?;
        for dir in ["originals", "manuals", "thumbnails", "staging", "trash"] {
            ensure_owned_directory(&root, dir)?;
        }
        let db_path = root.join("library.sqlite3");
        match std::fs::symlink_metadata(&db_path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(LibraryError::InvalidInput(
                    "library database must be a regular owned file".into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(&db_path, error)),
        }
        let catalog_file = pin_catalog_file(&db_path).map_err(|error| io(&db_path, error))?;
        if !catalog_file
            .metadata()
            .map_err(|error| io(&db_path, error))?
            .is_file()
        {
            return Err(LibraryError::InvalidInput(
                "library database must be a regular owned file".into(),
            ));
        }
        let catalog_pin = Arc::new(CatalogPin {
            root: root.clone(),
            file: catalog_file,
            #[cfg(test)]
            before_commit: Mutex::new(None),
            #[cfg(test)]
            after_commit_identity_check: Mutex::new(None),
        });
        let catalog_vfs =
            CatalogVfs::new(&db_path, &catalog_pin.file).map_err(|error| io(&db_path, error))?;
        before_connection_open();
        let connection_result = Connection::open_with_flags_and_vfs(
            catalog_vfs.sqlite_path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            catalog_vfs.vfs_name(),
        );
        after_connection_open();
        let conn = connection_result?;
        let opened_db = std::fs::symlink_metadata(&db_path).map_err(|error| io(&db_path, error))?;
        if opened_db.file_type().is_symlink()
            || !opened_db.is_file()
            || db_path
                .canonicalize()
                .map_err(|error| io(&db_path, error))?
                .parent()
                != Some(root.as_path())
        {
            return Err(LibraryError::InvalidInput(
                "library database identity changed".into(),
            ));
        }
        if !catalog_pin.matches_path() {
            return Err(LibraryError::InvalidInput(
                "library database identity changed".into(),
            ));
        }
        if catalog_vfs.check_connection(&conn).is_err() {
            return Err(LibraryError::InvalidInput(
                "SQLite opened a different library database file".into(),
            ));
        }
        #[cfg(unix)]
        if sqlite_file_has_moved(&conn) != Some(false) {
            return Err(LibraryError::InvalidInput(
                "SQLite opened a different library database file".into(),
            ));
        }
        let db = Self::initialize(conn, root, Some(catalog_pin), Some(catalog_vfs))?;
        if !db.catalog_path_matches() {
            return Err(LibraryError::InvalidInput(
                "library database identity changed".into(),
            ));
        }
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self, LibraryError> {
        Self::initialize(
            Connection::open_in_memory()?,
            PathBuf::from("."),
            None,
            None,
        )
    }

    fn initialize(
        conn: Connection,
        root: PathBuf,
        catalog_file: Option<Arc<CatalogPin>>,
        catalog_vfs: Option<CatalogVfs>,
    ) -> Result<Self, LibraryError> {
        if let Some(catalog_file) = catalog_file.as_ref() {
            let commit_pin = Arc::clone(catalog_file);
            conn.commit_hook(Some(move || {
                #[cfg(test)]
                commit_pin.run_before_commit_hook();
                let matches_path = commit_pin.matches_path();
                #[cfg(test)]
                commit_pin.run_after_commit_identity_check_hook();
                !matches_path
            }))?;
        }
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > CURRENT_SCHEMA_VERSION {
            return Err(LibraryError::SchemaTooNew {
                found: version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        if version == 0 {
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
        } else if version == 1 {
            create_migration_backup(&conn, &root, version, catalog_vfs.as_ref())?;
            conn.execute_batch(MIGRATE_V1_TO_V2)?;
            conn.execute_batch(MIGRATE_V2_TO_V3)?;
            conn.execute_batch(MIGRATE_V3_TO_V4)?;
            conn.execute_batch(MIGRATE_V4_TO_V5)?;
            conn.execute_batch(MIGRATE_V5_TO_V6)?;
            conn.execute_batch(MIGRATE_V6_TO_V7)?;
            conn.execute_batch(MIGRATE_V7_TO_V8)?;
            conn.execute_batch(MIGRATE_V8_TO_V9)?;
        } else if version == 2 {
            create_migration_backup(&conn, &root, version, catalog_vfs.as_ref())?;
            conn.execute_batch(MIGRATE_V2_TO_V3)?;
            conn.execute_batch(MIGRATE_V3_TO_V4)?;
            conn.execute_batch(MIGRATE_V4_TO_V5)?;
            conn.execute_batch(MIGRATE_V5_TO_V6)?;
            conn.execute_batch(MIGRATE_V6_TO_V7)?;
            conn.execute_batch(MIGRATE_V7_TO_V8)?;
            conn.execute_batch(MIGRATE_V8_TO_V9)?;
        } else if version == 3 {
            create_migration_backup(&conn, &root, version, catalog_vfs.as_ref())?;
            conn.execute_batch(MIGRATE_V3_TO_V4)?;
            conn.execute_batch(MIGRATE_V4_TO_V5)?;
            conn.execute_batch(MIGRATE_V5_TO_V6)?;
            conn.execute_batch(MIGRATE_V6_TO_V7)?;
            conn.execute_batch(MIGRATE_V7_TO_V8)?;
            conn.execute_batch(MIGRATE_V8_TO_V9)?;
        } else if version == 4 {
            create_migration_backup(&conn, &root, version, catalog_vfs.as_ref())?;
            conn.execute_batch(MIGRATE_V4_TO_V5)?;
            conn.execute_batch(MIGRATE_V5_TO_V6)?;
            conn.execute_batch(MIGRATE_V6_TO_V7)?;
            conn.execute_batch(MIGRATE_V7_TO_V8)?;
            conn.execute_batch(MIGRATE_V8_TO_V9)?;
        } else if version == 5 {
            create_migration_backup(&conn, &root, version, catalog_vfs.as_ref())?;
            conn.execute_batch(MIGRATE_V5_TO_V6)?;
            conn.execute_batch(MIGRATE_V6_TO_V7)?;
            conn.execute_batch(MIGRATE_V7_TO_V8)?;
            conn.execute_batch(MIGRATE_V8_TO_V9)?;
        } else if version == 6 {
            create_migration_backup(&conn, &root, version, catalog_vfs.as_ref())?;
            conn.execute_batch(MIGRATE_V6_TO_V7)?;
            conn.execute_batch(MIGRATE_V7_TO_V8)?;
            conn.execute_batch(MIGRATE_V8_TO_V9)?;
        } else if version == 7 {
            create_migration_backup(&conn, &root, version, catalog_vfs.as_ref())?;
            conn.execute_batch(MIGRATE_V7_TO_V8)?;
            conn.execute_batch(MIGRATE_V8_TO_V9)?;
        } else if version == 8 {
            create_migration_backup(&conn, &root, version, catalog_vfs.as_ref())?;
            conn.execute_batch(MIGRATE_V8_TO_V9)?;
        } else if version == 9 {
            create_migration_backup(&conn, &root, version, catalog_vfs.as_ref())?;
        } else if version == 10 {
            create_migration_backup(&conn, &root, version, catalog_vfs.as_ref())?;
        }
        if (1..=9).contains(&version) {
            conn.execute_batch(MIGRATE_V9_TO_V10)?;
        }
        if (1..=10).contains(&version) {
            conn.execute_batch(MIGRATE_V10_TO_V11)?;
        }
        conn.execute_batch(ENSURE_CATALOG_SETTINGS)?;
        conn.execute_batch(ENSURE_PRINT_BATCH_RECEIPTS)?;
        Ok(Self {
            conn,
            catalog_vfs,
            root,
            catalog_file,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Checks that the visible catalog path still names the regular file pinned
    /// when this connection opened. SQLite accepts paths rather than file
    /// handles, so callers that perform multi-step source mutations must refuse
    /// to continue after a same-user path replacement.
    pub fn catalog_path_matches(&self) -> bool {
        let Some(catalog_file) = self.catalog_file.as_ref() else {
            return true;
        };
        if !catalog_file.matches_path() {
            return false;
        }
        if self
            .catalog_vfs
            .as_ref()
            .is_some_and(|vfs| vfs.check_connection(&self.conn).is_err())
        {
            return false;
        }
        #[cfg(unix)]
        if sqlite_file_has_moved(&self.conn) != Some(false) {
            return false;
        }
        true
    }

    pub fn total_changes(&self) -> u64 {
        self.conn.total_changes()
    }

    pub fn create_model(
        &self,
        display_name: &str,
        folder_id: Option<&str>,
    ) -> Result<Model, LibraryError> {
        let folder_id = folder_id.unwrap_or(ROOT_FOLDER_ID);
        self.ensure_folder(folder_id)?;
        let display_name = clean_name(display_name, "Model")?;
        let id = new_id("model");
        let now = timestamp();
        self.conn.execute(
            "INSERT INTO models (id, display_name, description, folder_id, metadata_version, created_at, updated_at)
             VALUES (?1, ?2, '', ?3, 1, ?4, ?4)",
            params![id, display_name, folder_id, now],
        )?;
        self.get_model(&id)?.ok_or(LibraryError::ModelNotFound(id))
    }

    pub fn get_model(&self, id: &str) -> Result<Option<Model>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(&format!("{MODEL_SELECT} WHERE m.id = ?1"))?;
        let model = stmt.query_row(params![id], model_from_row).optional()?;
        if let Some(mut model) = model {
            model.tags = self.tags_for(&model.id)?;
            model.filament_ids = self.filament_ids_for_model(&model.id)?;
            Ok(Some(model))
        } else {
            Ok(None)
        }
    }

    pub fn resolve_parent_model_id(&self, value: &str) -> Result<String, LibraryError> {
        let value = clean_name(value, "Parent model")?;
        let id_match: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM models WHERE id=?1 AND deleted_at IS NULL",
                params![value],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = id_match {
            return Ok(id);
        }
        let ids = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM models WHERE deleted_at IS NULL AND lower(display_name)=lower(?1)
                 ORDER BY id LIMIT 2",
            )?;
            let rows = stmt.query_map(params![value], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        match ids.as_slice() {
            [id] => Ok(id.clone()),
            [] => Err(LibraryError::InvalidInput(
                "embedded parent model does not match an active catalog model".into(),
            )),
            _ => Err(LibraryError::InvalidInput(
                "embedded parent model name is ambiguous".into(),
            )),
        }
    }

    pub fn list_models(
        &self,
        folder_id: Option<&str>,
        query: Option<&str>,
    ) -> Result<Vec<Model>, LibraryError> {
        let query = query.map(str::trim).filter(|q| !q.is_empty());
        let mut stmt = self.conn.prepare(
            &format!(
                "{MODEL_SELECT} WHERE m.deleted_at IS NULL
                 AND (?1 IS NULL OR m.folder_id = ?1)
                 AND (?2 IS NULL OR lower(m.display_name) LIKE '%' || lower(?2) || '%'
                      OR EXISTS (SELECT 1 FROM model_tags mt JOIN tags t ON t.id = mt.tag_id
                                 WHERE mt.model_id = m.id AND lower(t.name) LIKE '%' || lower(?2) || '%'))
                 ORDER BY m.created_at, m.id"
            ),
        )?;
        let rows = stmt.query_map(params![folder_id, query], model_from_row)?;
        let mut models = Vec::new();
        for row in rows {
            models.push(row?);
        }
        if models.is_empty() {
            return Ok(models);
        }
        let mut tags = self.tags_by_model()?;
        let mut filaments = self.filaments_by_model()?;
        for model in &mut models {
            model.tags = tags.remove(&model.id).unwrap_or_default();
            model.filament_ids = filaments.remove(&model.id).unwrap_or_default();
        }
        Ok(models)
    }

    /// Lists the full library JSON model set only when it stays within
    /// the importer limit. Counting and materialization share one SQLite
    /// snapshot so a concurrent insert cannot bypass the limit.
    pub fn list_models_for_json_export(&self) -> Result<Vec<Model>, LibraryError> {
        if self.conn.is_autocommit() {
            self.with_json_export_snapshot(|db| db.list_models_for_json_export_in_snapshot())
        } else {
            self.list_models_for_json_export_in_snapshot()
        }
    }

    fn list_models_for_json_export_in_snapshot(&self) -> Result<Vec<Model>, LibraryError> {
        let count = self.conn.query_row(
            "SELECT COUNT(*) FROM models WHERE deleted_at IS NULL",
            [],
            |row| row.get(0),
        )?;
        let count = checked_export_count(count, "models", MAX_LIBRARY_JSON_EXPORT_MODELS)?;
        let models = self.list_models(None, None)?;
        if models.len() != count {
            return Err(LibraryError::InvalidInput(
                "JSON export snapshot did not read every counted model".into(),
            ));
        }
        Ok(models)
    }

    /// Runs every library JSON export read on one deferred SQLite snapshot.
    /// The first count read pins the snapshot before the caller begins paging.
    pub fn with_json_export_snapshot<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T, LibraryError>,
    ) -> Result<T, LibraryError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.query_row(
            "SELECT COUNT(*) FROM models WHERE deleted_at IS NULL",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        let result = operation(self)?;
        tx.commit()?;
        Ok(result)
    }

    /// Counts the active model rows permitted in a library JSON export.
    pub fn count_models_for_json_export(&self) -> Result<usize, LibraryError> {
        let count = self.conn.query_row(
            "SELECT COUNT(*) FROM models WHERE deleted_at IS NULL",
            [],
            |row| row.get(0),
        )?;
        checked_export_count(count, "models", MAX_LIBRARY_JSON_EXPORT_MODELS)
    }

    /// Reads a small, stable-order page for streaming library JSON export.
    pub fn list_models_for_json_export_page(
        &self,
        offset: usize,
        limit: u16,
    ) -> Result<Vec<Model>, LibraryError> {
        if limit == 0 || limit > 100 || offset > MAX_LIBRARY_JSON_EXPORT_MODELS {
            return Err(LibraryError::InvalidInput(
                "JSON export model page request is invalid".into(),
            ));
        }
        let count = self.conn.query_row(
            "SELECT COUNT(*) FROM models WHERE deleted_at IS NULL",
            [],
            |row| row.get(0),
        )?;
        checked_export_count(count, "models", MAX_LIBRARY_JSON_EXPORT_MODELS)?;
        let mut statement = self.conn.prepare(&format!(
            "{MODEL_SELECT} WHERE m.deleted_at IS NULL
             ORDER BY m.created_at,m.id LIMIT ?1 OFFSET ?2"
        ))?;
        let rows = statement.query_map(params![i64::from(limit), offset as i64], model_from_row)?;
        let mut models = rows.collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        for model in &mut models {
            model.tags = self.tags_for(&model.id)?;
            model.filament_ids = self.filament_ids_for_model(&model.id)?;
        }
        Ok(models)
    }

    pub fn active_model_display_name_for_json_export(
        &self,
        model_id: &str,
    ) -> Result<Option<String>, LibraryError> {
        self.conn
            .query_row(
                "SELECT display_name FROM models WHERE id=?1 AND deleted_at IS NULL",
                params![model_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn tag_category_for_json_export(
        &self,
        tag_name: &str,
    ) -> Result<Option<String>, LibraryError> {
        self.conn
            .query_row(
                "SELECT category FROM tags WHERE lower(name)=lower(?1)",
                params![tag_name],
                |row| row.get(0),
            )
            .optional()
            .map(|category| category.flatten())
            .map_err(Into::into)
    }

    pub fn list_models_page(
        &self,
        query: &ModelPageQuery,
        after_sort_value: Option<&str>,
        after_id: Option<&str>,
        limit: u16,
    ) -> Result<(Vec<Model>, bool), LibraryError> {
        self.list_models_page_with_offset(query, after_sort_value, after_id, limit, 0)
    }

    pub fn list_models_page_with_offset(
        &self,
        query: &ModelPageQuery,
        after_sort_value: Option<&str>,
        after_id: Option<&str>,
        limit: u16,
        offset: u32,
    ) -> Result<(Vec<Model>, bool), LibraryError> {
        if limit == 0
            || limit > 500
            || offset > MAX_MODEL_PAGE_OFFSET
            || after_sort_value.is_some() != after_id.is_some()
            || (offset > 0 && after_sort_value.is_some())
        {
            return Err(LibraryError::InvalidInput(
                "model page request is invalid".into(),
            ));
        }
        if query
            .thumbnail_mode
            .as_deref()
            .is_some_and(|value| !matches!(value, "missing" | "default" | "custom"))
        {
            return Err(LibraryError::InvalidInput(
                "Thumbnail filter is invalid".into(),
            ));
        }
        if let Some(directory) = query.source_directory.as_deref() {
            if query.root_id.is_none()
                || directory.len() > 1024
                || directory.contains(['\\', '\0'])
                || (!directory.is_empty()
                    && directory
                        .split('/')
                        .any(|part| part.is_empty() || matches!(part, "." | "..")))
            {
                return Err(LibraryError::InvalidInput(
                    "Source directory must be relative to a selected root".into(),
                ));
            }
        }
        if query.minimum_rating.is_some_and(|rating| rating > 5) {
            return Err(LibraryError::InvalidInput(
                "Minimum rating must be between 0 and 5".into(),
            ));
        }
        if query.exclude_model_ids.len() > 2000 {
            return Err(LibraryError::InvalidInput(
                "Thumbnail exclusion list exceeds 2000 model IDs".into(),
            ));
        }
        if query.tag_names.len() > 100 {
            return Err(LibraryError::InvalidInput(
                "Tag filter exceeds 100 names".into(),
            ));
        }
        let tag_names = query
            .tag_names
            .iter()
            .map(|name| name.trim().to_owned())
            .collect::<Vec<_>>();
        let mut unique_tag_names = HashSet::new();
        if tag_names.iter().any(|name| {
            name.is_empty() || name.len() > 256 || !unique_tag_names.insert(name.to_lowercase())
        }) {
            return Err(LibraryError::InvalidInput(
                "Tag filter contains an invalid or duplicate name".into(),
            ));
        }
        let file_type = query.file_type.as_deref().map(|value| {
            value
                .trim()
                .strip_prefix('.')
                .unwrap_or(value.trim())
                .to_ascii_lowercase()
        });
        if query.file_type.as_deref().is_some_and(|value| {
            let extension = value.trim().strip_prefix('.').unwrap_or(value.trim());
            extension.is_empty()
                || extension.len() > 16
                || !extension
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-'))
        }) {
            return Err(LibraryError::InvalidInput(
                "File type must be a short extension".into(),
            ));
        }
        let mut excluded = HashSet::new();
        if query
            .exclude_model_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 128 || !excluded.insert(id.as_str()))
        {
            return Err(LibraryError::InvalidInput(
                "Thumbnail exclusion list contains an invalid or duplicate model ID".into(),
            ));
        }
        let exclude_model_ids = serde_json::to_string(&query.exclude_model_ids)
            .map_err(|_| LibraryError::InvalidInput("Thumbnail exclusions are invalid".into()))?;
        let tag_names = serde_json::to_string(&tag_names)
            .map_err(|_| LibraryError::InvalidInput("Tag filter is invalid".into()))?;
        let search = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let print_status = query.print_status.as_ref().map(|status| match status {
            PrintStatus::Unprinted => "unprinted",
            PrintStatus::Want => "want",
            PrintStatus::Queued => "queued",
            PrintStatus::Printing => "printing",
            PrintStatus::Printed => "printed",
            PrintStatus::Failed => "failed",
        });
        let group = query.group.map(|group| match group {
            ModelGroupFilter::Standalone => 1_i64,
            ModelGroupFilter::Bundle => 2,
            ModelGroupFilter::Archive => 3,
        });
        let (sort_expression, cursor_expression) = match query.sort {
            ModelSort::Name => ("lower(m.display_name)", "?16"),
            ModelSort::Size => ("m.bytes", "CAST(?16 AS INTEGER)"),
            ModelSort::Date => ("m.created_at", "?16"),
        };
        let comparison = match query.direction {
            SortDirection::Asc => ">",
            SortDirection::Desc => "<",
        };
        let direction = match query.direction {
            SortDirection::Asc => "ASC",
            SortDirection::Desc => "DESC",
        };
        let mut stmt = self.conn.prepare(&format!(
            "{MODEL_SELECT} WHERE m.deleted_at IS NULL
             AND (?1 IS NULL OR m.folder_id=?1)
             AND (?2 IS NULL OR lower(m.display_name) LIKE '%' || lower(?2) || '%'
                  OR EXISTS (SELECT 1 FROM model_tags mt JOIN tags t ON t.id=mt.tag_id
                             WHERE mt.model_id=m.id AND lower(t.name) LIKE '%' || lower(?2) || '%'))
             AND (?3 IS NULL OR EXISTS (SELECT 1 FROM model_tags mt
                                         WHERE mt.model_id=m.id AND mt.tag_id=?3))
             AND (?4 IS NULL OR EXISTS (SELECT 1 FROM model_tags mt JOIN tags t ON t.id=mt.tag_id
                                         WHERE mt.model_id=m.id AND t.category=?4))
             AND (?5 IS NULL OR m.designer=?5)
             AND (?6 IS NULL OR m.model_license=?6)
             AND (?7 IS NULL OR m.print_status=?7)
             AND (?8 IS NULL OR (?8=1 AND m.print_count>0) OR (?8=0 AND m.print_count=0))
             AND (?9 IS NULL OR m.bundle_id=?9)
             AND (?10 IS NULL OR m.source_root_id=?10)
             AND (?11 IS NULL OR EXISTS (SELECT 1 FROM model_filaments mf
                                          WHERE mf.model_id=m.id AND mf.filament_id=?11))
             AND (?12 IS NULL OR m.source_missing=?12)
             AND (?13=0 OR (COALESCE(r.blob_hash,d.content_hash) IS NOT NULL AND EXISTS (
                  SELECT 1 FROM models duplicate
                  LEFT JOIN revisions duplicate_revision ON duplicate_revision.id=duplicate.current_revision_id
                  LEFT JOIN indexed_model_data duplicate_data ON duplicate_data.model_id=duplicate.id
                  WHERE duplicate.deleted_at IS NULL AND duplicate.id!=m.id
                  AND COALESCE(duplicate_revision.blob_hash,duplicate_data.content_hash)
                      =COALESCE(r.blob_hash,d.content_hash))))
             AND (?14 IS NULL OR m.source_kind=?14)
             AND (?15 IS NULL OR (?15=1 AND m.bundle_id IS NULL AND m.source_archive_member IS NULL)
                  OR (?15=2 AND m.bundle_id IS NOT NULL)
                  OR (?15=3 AND m.source_archive_member IS NOT NULL))
             AND (?19 IS NULL
                  OR (?19='missing' AND NOT EXISTS (SELECT 1 FROM thumbnails tf
                       WHERE tf.model_id=m.id AND (tf.is_custom=1 OR tf.source_content_hash IS NULL
                       OR tf.source_content_hash=COALESCE(r.blob_hash,d.content_hash))))
                  OR (?19 IN ('default','custom') AND
                       (SELECT tf.is_custom FROM thumbnails tf WHERE tf.model_id=m.id
                        AND (tf.is_custom=1 OR tf.source_content_hash IS NULL
                        OR tf.source_content_hash=COALESCE(r.blob_hash,d.content_hash))
                        ORDER BY tf.is_active DESC,tf.is_custom DESC,tf.ordinal,tf.id LIMIT 1)
                       =CASE WHEN ?19='custom' THEN 1 ELSE 0 END))
             AND (?20 IS NULL OR (m.source_kind='indexed'
                  AND (?20='' OR substr(m.source_path,1,length(?20)+1)=?20||'/')))
             AND (?21 IS NULL OR m.rating>=?21)
             AND (?22 IS NULL OR m.favorite=?22)
             AND NOT EXISTS (
                 SELECT 1 FROM json_each(?23) excluded WHERE excluded.value=m.id
             )
             AND NOT EXISTS (
                 SELECT 1 FROM json_each(?24) requested_tag
                 WHERE NOT EXISTS (
                     SELECT 1 FROM model_tags mt JOIN tags t ON t.id=mt.tag_id
                     WHERE mt.model_id=m.id AND t.name=requested_tag.value
                 )
             )
             AND (?25 IS NULL OR (?25='stl' AND COALESCE(r.format,d.format) IN ('stl-binary','stl-ascii'))
                  OR lower(COALESCE(r.format,d.format))=?25
                  OR lower(COALESCE(r.original_name,d.original_name)) LIKE '%.'||?25
                  OR (m.source_kind='indexed' AND lower(m.source_path) LIKE '%.'||?25))
             AND (?16 IS NULL OR {sort_expression}{comparison}{cursor_expression}
                  OR ({sort_expression}={cursor_expression} AND m.id{comparison}?17))
             ORDER BY {sort_expression} {direction},m.id {direction} LIMIT ?18 OFFSET ?26"
        ))?;
        let rows = stmt.query_map(
            params![
                query.folder_id.as_deref(),
                search,
                query.tag_id.as_deref(),
                query.category.as_deref(),
                query.designer.as_deref(),
                query.model_license.as_deref(),
                print_status,
                query.ever_printed.map(i64::from),
                query.bundle_id.as_deref(),
                query.root_id.as_deref(),
                query.filament_id.as_deref(),
                query.source_missing.map(i64::from),
                i64::from(query.duplicates_only),
                query.source_kind.as_deref(),
                group,
                after_sort_value,
                after_id,
                i64::from(limit) + 1,
                query.thumbnail_mode.as_deref(),
                query.source_directory.as_deref(),
                query.minimum_rating.map(i64::from),
                query.favorite.map(i64::from),
                exclude_model_ids,
                tag_names,
                file_type,
                i64::from(offset)
            ],
            model_from_row,
        )?;
        let mut models = rows.collect::<Result<Vec<_>, _>>()?;
        let has_more = models.len() > usize::from(limit);
        models.truncate(usize::from(limit));
        for model in &mut models {
            model.tags = self.tags_for(&model.id)?;
            model.filament_ids = self.filament_ids_for_model(&model.id)?;
        }
        Ok((models, has_more))
    }

    pub fn list_bundle_models(&self, bundle_id: &str) -> Result<Vec<Model>, LibraryError> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM bundles WHERE id=?1)",
            params![bundle_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(LibraryError::InvalidInput("bundle not found".into()));
        }
        let mut stmt=self.conn.prepare(&format!("{MODEL_SELECT} WHERE m.bundle_id=?1 AND m.deleted_at IS NULL ORDER BY lower(m.display_name),m.id"))?;
        let rows = stmt.query_map(params![bundle_id], model_from_row)?;
        let mut models = rows.collect::<Result<Vec<_>, _>>()?;
        for model in &mut models {
            model.tags = self.tags_for(&model.id)?;
            model.filament_ids = self.filament_ids_for_model(&model.id)?;
        }
        Ok(models)
    }

    pub fn duplicate_groups(
        &self,
        model_ids: &[String],
    ) -> Result<Vec<DuplicateGroup>, LibraryError> {
        if model_ids.len() > 1_000 {
            return Err(LibraryError::InvalidInput(
                "duplicate scope exceeds 1000 models".into(),
            ));
        }
        let mut unique = HashSet::new();
        let mut groups: BTreeMap<String, Vec<Model>> = BTreeMap::new();
        for id in model_ids {
            if !unique.insert(id) {
                return Err(LibraryError::InvalidInput(
                    "duplicate model ID in scope".into(),
                ));
            }
            let active: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM models WHERE id=?1 AND deleted_at IS NULL)",
                params![id],
                |row| row.get(0),
            )?;
            if !active {
                return Err(LibraryError::ModelNotFound(id.clone()));
            }
            let model = self
                .get_model(id)?
                .ok_or_else(|| LibraryError::ModelNotFound(id.clone()))?;
            if let Some(hash) = model.content_hash.clone() {
                groups.entry(hash).or_default().push(model);
            }
        }
        Ok(groups
            .into_iter()
            .filter_map(|(content_hash, models)| {
                (models.len() > 1).then_some(DuplicateGroup {
                    content_hash,
                    models,
                })
            })
            .collect())
    }

    pub fn catalog_hash_counts(&self) -> Result<crate::models::CatalogHashCounts, LibraryError> {
        self.conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(COALESCE(r.blob_hash,d.content_hash) IS NOT NULL),0),
                    COALESCE(SUM(m.catalog_kind!='bookmark' AND COALESCE(r.blob_hash,d.content_hash) IS NULL),0),
                    COALESCE(SUM(m.catalog_kind='bookmark'),0),
                    COALESCE(SUM(m.source_missing=1),0)
             FROM models m LEFT JOIN revisions r ON r.id=m.current_revision_id
             LEFT JOIN indexed_model_data d ON d.model_id=m.id WHERE m.deleted_at IS NULL",
            [], |row| {
                let count = |index: usize| -> rusqlite::Result<u64> {
                    let value: i64 = row.get(index)?;
                    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
                };
                Ok(crate::models::CatalogHashCounts { total: count(0)?, hashed: count(1)?,
                    unhashed: count(2)?, bookmarks: count(3)?, missing: count(4)? })
            },
        ).map_err(LibraryError::from)
    }

    pub fn catalog_stats(&self) -> Result<CatalogStats, LibraryError> {
        let (total, available, missing, bookmarks, bytes): (i64, i64, i64, i64, i64) =
            self.conn.query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(catalog_kind='file' AND source_missing=0),0),
                        COALESCE(SUM(catalog_kind='file' AND source_missing=1),0),
                        COALESCE(SUM(catalog_kind='bookmark'),0),
                        COALESCE(SUM(CASE WHEN catalog_kind='file' THEN COALESCE(r.bytes,d.bytes,0) ELSE 0 END),0)
                 FROM models m
                 LEFT JOIN revisions r ON r.id=m.current_revision_id
                 LEFT JOIN indexed_model_data d ON d.model_id=m.id
                 WHERE m.deleted_at IS NULL",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )?;
        let bundle_count: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT bundle_id) FROM models
             WHERE deleted_at IS NULL AND bundle_id IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        let duplicate_group_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM (
                 SELECT COALESCE(r.blob_hash,d.content_hash) hash
                 FROM models m
                 LEFT JOIN revisions r ON r.id=m.current_revision_id
                 LEFT JOIN indexed_model_data d ON d.model_id=m.id
                 WHERE m.deleted_at IS NULL AND COALESCE(r.blob_hash,d.content_hash) IS NOT NULL
                 GROUP BY hash HAVING COUNT(*)>1)",
            [],
            |row| row.get(0),
        )?;
        Ok(CatalogStats {
            total_models: total as u64,
            available_models: available as u64,
            missing_models: missing as u64,
            bookmarks: bookmarks as u64,
            total_bytes: bytes as u64,
            bundle_count: bundle_count as u64,
            duplicate_group_count: duplicate_group_count as u64,
        })
    }

    pub fn catalog_facets(&self) -> Result<CatalogFacets, LibraryError> {
        fn strings(conn: &Connection, sql: &str) -> Result<Vec<String>, rusqlite::Error> {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |row| row.get(0))?;
            rows.collect()
        }
        let designers = strings(
            &self.conn,
            "SELECT DISTINCT designer FROM models WHERE deleted_at IS NULL AND designer IS NOT NULL
             ORDER BY lower(designer) LIMIT 5000",
        )?;
        let licenses = strings(
            &self.conn,
            "SELECT DISTINCT model_license FROM models WHERE deleted_at IS NULL AND model_license IS NOT NULL
             ORDER BY lower(model_license) LIMIT 5000",
        )?;
        let categories = strings(
            &self.conn,
            "SELECT DISTINCT category FROM tags WHERE category IS NOT NULL
             ORDER BY lower(category) LIMIT 5000",
        )?;
        let folder_ids = strings(
            &self.conn,
            "SELECT id FROM folders ORDER BY lower(name),id LIMIT 5000",
        )?;
        let root_ids = strings(
            &self.conn,
            "SELECT id FROM index_roots ORDER BY id LIMIT 5000",
        )?;
        let mut stmt = self.conn.prepare(
            "SELECT id,display_name FROM models WHERE deleted_at IS NULL
             ORDER BY lower(display_name),id LIMIT 5000",
        )?;
        let parents = stmt
            .query_map([], |row| {
                Ok(CatalogFacetValue {
                    id: row.get(0)?,
                    label: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CatalogFacets {
            designers,
            licenses,
            categories,
            parents,
            folder_ids,
            root_ids,
        })
    }

    pub fn remove_models_recoverably(
        &self,
        targets: &[VersionedTarget],
    ) -> Result<RemovalResult, LibraryError> {
        self.remove_models_recoverably_with_snapshot_id(targets, &new_id("removal"))
    }

    pub fn remove_models_recoverably_with_snapshot_id(
        &self,
        targets: &[VersionedTarget],
        snapshot_id: &str,
    ) -> Result<RemovalResult, LibraryError> {
        if targets.is_empty() || targets.len() > 50_000 {
            return Err(LibraryError::InvalidInput(
                "removal target count is invalid".into(),
            ));
        }
        if snapshot_id.is_empty()
            || snapshot_id.len() > 128
            || !snapshot_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(LibraryError::InvalidInput(
                "removal snapshot id is invalid".into(),
            ));
        }
        let mut unique = HashSet::new();
        if targets
            .iter()
            .any(|target| !unique.insert(&target.model_id))
        {
            return Err(LibraryError::InvalidInput(
                "duplicate removal target".into(),
            ));
        }
        let now = timestamp();
        let tx = self.conn.unchecked_transaction()?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM jobs WHERE kind='removal' AND status=?1 LIMIT 1",
                params![snapshot_id],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_some() {
            return Err(LibraryError::InvalidInput(
                "removal snapshot id is already in use".into(),
            ));
        }
        for target in targets {
            let row: Option<(i64, Option<String>)> = tx
                .query_row(
                    "SELECT metadata_version,deleted_at FROM models WHERE id=?1",
                    params![target.model_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let (actual, deleted) =
                row.ok_or_else(|| LibraryError::ModelNotFound(target.model_id.clone()))?;
            if actual as u64 != target.expected_version {
                return Err(LibraryError::MetadataConflict {
                    model_id: target.model_id.clone(),
                    expected: target.expected_version,
                    actual: actual as u64,
                });
            }
            if deleted.is_some() {
                return Err(LibraryError::InvalidInput(
                    "model is already removed".into(),
                ));
            }
            tx.execute("UPDATE models SET deleted_at=?1,metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",params![now,target.model_id])?;
            tx.execute(
                "DELETE FROM jobs WHERE model_id=?1 AND kind='removal'",
                params![target.model_id],
            )?;
            tx.execute("INSERT INTO jobs(id,model_id,kind,status,retry_count,created_at,updated_at) VALUES(?1,?2,'removal',?3,0,?4,?4)",params![new_id("removal-item"),target.model_id,snapshot_id,now])?;
        }
        tx.commit()?;
        Ok(RemovalResult {
            snapshot_id: snapshot_id.to_owned(),
            removed_ids: targets
                .iter()
                .map(|target| target.model_id.clone())
                .collect(),
        })
    }

    pub fn list_removed_models(&self) -> Result<Vec<RemovedModel>, LibraryError> {
        let rows = {
            let mut stmt=self.conn.prepare("SELECT m.id,m.deleted_at,j.status FROM models m JOIN jobs j ON j.model_id=m.id AND j.kind='removal' WHERE m.deleted_at IS NOT NULL ORDER BY m.deleted_at DESC,m.id")?;
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        rows.into_iter()
            .map(|(id, removed_at, snapshot_id)| {
                Ok(RemovedModel {
                    model: self
                        .get_model(&id)?
                        .ok_or_else(|| LibraryError::ModelNotFound(id.clone()))?,
                    removed_at,
                    snapshot_id,
                })
            })
            .collect()
    }

    pub fn restore_removal(&self, snapshot_id: &str) -> Result<Vec<Model>, LibraryError> {
        let ids = {
            let mut stmt = self.conn.prepare(
                "SELECT model_id FROM jobs WHERE kind='removal' AND status=?1 ORDER BY model_id",
            )?;
            let rows = stmt.query_map(params![snapshot_id], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if ids.is_empty() {
            return Err(LibraryError::InvalidInput(
                "removal snapshot not found".into(),
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        for id in &ids {
            let deleted: Option<String> = tx
                .query_row(
                    "SELECT deleted_at FROM models WHERE id=?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            if deleted.is_none() {
                return Err(LibraryError::InvalidInput(
                    "removal snapshot conflicts with active model".into(),
                ));
            }
            tx.execute("UPDATE models SET deleted_at=NULL,metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",params![timestamp(),id])?;
        }
        tx.execute(
            "DELETE FROM jobs WHERE kind='removal' AND status=?1",
            params![snapshot_id],
        )?;
        tx.commit()?;
        ids.iter()
            .map(|id| {
                self.get_model(id)?
                    .ok_or_else(|| LibraryError::ModelNotFound(id.clone()))
            })
            .collect()
    }

    pub fn restore_removal_with_owned_source(
        &self,
        snapshot_id: &str,
        model_id: &str,
        content_hash: &str,
        bytes: u64,
        modified_at: u64,
    ) -> Result<Model, LibraryError> {
        validate_hash(content_hash)?;
        let stored_bytes = i64::try_from(bytes)
            .map_err(|_| LibraryError::InvalidInput("source byte count is too large".into()))?;
        let stored_modified_at = i64::try_from(modified_at)
            .map_err(|_| LibraryError::InvalidInput("source timestamp is too large".into()))?;
        let ids = {
            let mut stmt = self.conn.prepare(
                "SELECT model_id FROM jobs WHERE kind='removal' AND status=?1 ORDER BY model_id",
            )?;
            let rows = stmt.query_map(params![snapshot_id], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if ids.len() != 1 || ids[0] != model_id {
            return Err(LibraryError::InvalidInput(
                "owned source recovery does not match removal snapshot".into(),
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        type RecoverySourceRow = (Option<String>, String, Option<String>, String, i64);
        let source: Option<RecoverySourceRow> = tx
            .query_row(
                "SELECT m.deleted_at,m.source_kind,m.source_archive_member,d.content_hash,d.bytes
                 FROM models m JOIN indexed_model_data d ON d.model_id=m.id WHERE m.id=?1",
                params![model_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((deleted_at, source_kind, archive_member, stored_hash, catalog_bytes)) = source
        else {
            return Err(LibraryError::ModelNotFound(model_id.to_owned()));
        };
        if deleted_at.is_none()
            || source_kind != "indexed"
            || archive_member.is_some()
            || stored_hash != content_hash
            || catalog_bytes != stored_bytes
        {
            return Err(LibraryError::InvalidInput(
                "owned source recovery conflicts with catalog".into(),
            ));
        }
        tx.execute(
            "UPDATE models SET deleted_at=NULL,source_missing=0,source_modified_at=?1,
                    metadata_version=metadata_version+1,updated_at=?2 WHERE id=?3",
            params![stored_modified_at, timestamp(), model_id],
        )?;
        tx.execute(
            "DELETE FROM jobs WHERE kind='removal' AND status=?1",
            params![snapshot_id],
        )?;
        tx.commit()?;
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.to_owned()))
    }

    pub fn create_folder(
        &self,
        name: &str,
        parent_id: Option<&str>,
    ) -> Result<Folder, LibraryError> {
        let parent_id = parent_id.unwrap_or(ROOT_FOLDER_ID);
        self.ensure_folder(parent_id)?;
        let name = clean_name(name, "Folder")?;
        let id = new_id("folder");
        self.conn.execute(
            "INSERT INTO folders (id, name, parent_id) VALUES (?1, ?2, ?3)",
            params![id, name, parent_id],
        )?;
        self.get_folder(&id)?
            .ok_or(LibraryError::FolderNotFound(id))
    }

    pub fn rename_folder(&self, id: &str, name: &str) -> Result<Folder, LibraryError> {
        if id == ROOT_FOLDER_ID {
            return Err(LibraryError::RootFolderMutation);
        }
        self.ensure_folder(id)?;
        let name = clean_name(name, "Folder")?;
        self.conn.execute(
            "UPDATE folders SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
        self.get_folder(id)?
            .ok_or_else(|| LibraryError::FolderNotFound(id.to_string()))
    }

    pub fn move_folder(&self, id: &str, new_parent_id: &str) -> Result<Folder, LibraryError> {
        if id == ROOT_FOLDER_ID {
            return Err(LibraryError::RootFolderMutation);
        }
        self.ensure_folder(id)?;
        self.ensure_folder(new_parent_id)?;
        let mut ancestor = Some(new_parent_id.to_owned());
        while let Some(current) = ancestor {
            if current == id {
                return Err(LibraryError::FolderCycle);
            }
            ancestor = self.conn.query_row(
                "SELECT parent_id FROM folders WHERE id = ?1",
                params![current],
                |row| row.get::<_, Option<String>>(0),
            )?;
        }
        self.conn.execute(
            "UPDATE folders SET parent_id = ?1 WHERE id = ?2",
            params![new_parent_id, id],
        )?;
        self.get_folder(id)?
            .ok_or_else(|| LibraryError::FolderNotFound(id.to_string()))
    }

    pub fn delete_folder(&self, id: &str) -> Result<(), LibraryError> {
        if id == ROOT_FOLDER_ID {
            return Err(LibraryError::RootFolderMutation);
        }
        self.ensure_folder(id)?;
        let children: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM folders WHERE parent_id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        let models: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM models WHERE folder_id = ?1 AND deleted_at IS NULL",
            params![id],
            |row| row.get(0),
        )?;
        if children > 0 || models > 0 {
            return Err(LibraryError::FolderNotEmpty);
        }
        self.conn
            .execute("DELETE FROM folders WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn list_folders(&self) -> Result<Vec<Folder>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.name, f.parent_id,
                    (SELECT COUNT(*) FROM models m WHERE m.folder_id = f.id AND m.deleted_at IS NULL)
             FROM folders f ORDER BY f.parent_id IS NOT NULL, lower(f.name), f.id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Folder {
                id: row.get(0)?,
                name: row.get(1)?,
                parent_id: row.get(2)?,
                model_count: row.get::<_, i64>(3)? as u64,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn import_file(
        &self,
        source: impl AsRef<Path>,
        folder_id: Option<&str>,
        display_name: Option<&str>,
    ) -> Result<ImportResult, LibraryError> {
        let source = source.as_ref();
        let folder_id = folder_id.unwrap_or(ROOT_FOLDER_ID);
        self.ensure_folder(folder_id)?;
        let display_name = display_name
            .map(str::to_owned)
            .or_else(|| {
                source
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "Model".into());
        let display_name = clean_name(&display_name, "Model")?;
        let metadata = std::fs::symlink_metadata(source).map_err(|e| io(source, e))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(LibraryError::InvalidInput("source is not a file".into()));
        }
        if metadata.len() > MAX_INDEXED_FILE_BYTES {
            return Err(LibraryError::InvalidInput(
                "source exceeds the native file size limit".into(),
            ));
        }
        let mut file = std::fs::File::open(source).map_err(|error| io(source, error))?;
        let opened = file.metadata().map_err(|error| io(source, error))?;
        if !opened.is_file()
            || opened.len() != metadata.len()
            || opened.len() > MAX_INDEXED_FILE_BYTES
        {
            return Err(LibraryError::SourceChanged);
        }
        let mut bytes = Vec::with_capacity(opened.len() as usize);
        std::io::Read::by_ref(&mut file)
            .take(MAX_INDEXED_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| io(source, error))?;
        let after = file.metadata().map_err(|error| io(source, error))?;
        if metadata.len() != bytes.len() as u64
            || bytes.len() as u64 > MAX_INDEXED_FILE_BYTES
            || after.len() != opened.len()
        {
            return Err(LibraryError::SourceChanged);
        }
        let parsed = stl_core::parse_model(&bytes)?;
        let (format, extension) = match parsed.format {
            stl_core::StlFormat::Binary => ("stl-binary", "stl"),
            stl_core::StlFormat::Ascii => ("stl-ascii", "stl"),
            stl_core::StlFormat::ThreeMf => ("3mf", "3mf"),
        };
        let hash = hex_digest(&bytes);
        let relative_path = format!("originals/{hash}.{extension}");
        let originals = ensure_owned_directory(&self.root, "originals")?;
        let destination = originals.join(format!("{hash}.{extension}"));
        let blob = self
            .conn
            .query_row(
                "SELECT relative_path,bytes FROM blobs WHERE hash = ?1",
                params![hash],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let mut reused_blob = blob.is_some();
        if let Some((stored, stored_bytes)) = blob {
            if stored != relative_path || stored_bytes != bytes.len() as i64 {
                return Err(LibraryError::InvalidInput(
                    "blob catalog metadata is inconsistent".into(),
                ));
            }
            validate_managed_blob(&destination, &hash, bytes.len() as u64)?;
        } else {
            let staging_root = ensure_owned_directory(&self.root, "staging")?;
            let staging = staging_root.join(format!("{}.part", Uuid::new_v4()));
            let mut staged = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staging)
                .map_err(|error| io(&staging, error))?;
            staged
                .write_all(&bytes)
                .map_err(|error| io(&staging, error))?;
            staged.sync_all().map_err(|error| io(&staging, error))?;
            drop(staged);

            // Keep the staging name. A path-based unlink here could remove a
            // same-user replacement installed after the file was staged. On
            // the common path this remains a hard-link alias of the managed
            // blob, so it does not duplicate the model's data blocks.
            match std::fs::hard_link(&staging, &destination) {
                Ok(()) => {
                    validate_managed_blob(&destination, &hash, bytes.len() as u64)
                        .map_err(|error| retained_import_staging_error(&staging, error))?;
                    retain_import_staging(&staging)
                        .map_err(|error| retained_import_staging_error(&staging, error))?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Err(error) =
                        validate_managed_blob(&destination, &hash, bytes.len() as u64)
                    {
                        return Err(retained_import_staging_error(&staging, error));
                    }
                    retain_import_staging(&staging)
                        .map_err(|error| retained_import_staging_error(&staging, error))?;
                    reused_blob = true;
                }
                Err(error) => {
                    return Err(retained_import_staging_error(
                        &staging,
                        io(&destination, error),
                    ));
                }
            }
        }

        let original_name = source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("model.stl");
        let model_id = new_id("model");
        let revision_id = new_id("revision");
        let now = timestamp();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO blobs (hash, relative_path, bytes) VALUES (?1, ?2, ?3)",
            params![hash, relative_path, bytes.len() as i64],
        )?;
        tx.execute(
            "INSERT INTO models (id, display_name, description, folder_id, current_revision_id, metadata_version, created_at, updated_at)
             VALUES (?1, ?2, '', ?3, ?4, 1, ?5, ?5)",
            params![model_id, display_name, folder_id, revision_id, now],
        )?;
        tx.execute(
            "INSERT INTO revisions (id, model_id, blob_hash, original_name, format, bytes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![revision_id, model_id, hash, original_name, format, bytes.len() as i64, now],
        )?;
        tx.commit()?;
        let model = self
            .get_model(&model_id)?
            .ok_or(LibraryError::ModelNotFound(model_id))?;
        Ok(ImportResult { model, reused_blob })
    }

    pub fn add_index_root(
        &self,
        path: impl AsRef<Path>,
        scan_interval_minutes: u32,
        max_file_bytes: u64,
    ) -> Result<IndexRoot, LibraryError> {
        validate_root_limits(scan_interval_minutes, max_file_bytes)?;
        let path = canonical_directory(path.as_ref())?;
        self.ensure_root_does_not_overlap(&path, None)?;
        let root = IndexRoot {
            id: new_id("index-root"),
            path,
            enabled: true,
            last_scanned_at: None,
            scan_interval_minutes,
            max_file_bytes,
            scan_zip: false,
        };
        self.conn.execute(
            "INSERT INTO index_roots
             (id, path, enabled, last_scanned_at, scan_interval_minutes, max_file_bytes, scan_zip)
             VALUES (?1, ?2, 1, NULL, ?3, ?4, 0)",
            params![
                root.id,
                root.path.to_string_lossy(),
                i64::from(root.scan_interval_minutes),
                root.max_file_bytes as i64
            ],
        )?;
        Ok(root)
    }

    pub fn list_index_roots(&self) -> Result<Vec<IndexRoot>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, enabled, last_scanned_at, scan_interval_minutes, max_file_bytes, scan_zip
             FROM index_roots ORDER BY lower(path), id",
        )?;
        let rows = stmt.query_map([], index_root_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Seed an empty replacement catalog with the root mappings and catalog
    /// rendering/scanning settings from an existing database. Purge keeps root
    /// IDs stable so admitted server handles remain mapped after replacement.
    pub fn copy_catalog_configuration_from(&self, source: &LibraryDb) -> Result<(), LibraryError> {
        let roots = source.list_index_roots()?;
        let settings = source.get_catalog_settings()?;
        validate_catalog_settings(&settings)?;
        let tx = self.conn.unchecked_transaction()?;
        let model_count: i64 = tx.query_row("SELECT COUNT(*) FROM models", [], |row| row.get(0))?;
        let root_count: i64 =
            tx.query_row("SELECT COUNT(*) FROM index_roots", [], |row| row.get(0))?;
        if model_count != 0 || root_count != 0 {
            return Err(LibraryError::InvalidInput(
                "Catalog configuration can only be copied into an empty library".into(),
            ));
        }
        let mut root_ids = HashSet::new();
        for root in roots {
            if root.id.is_empty()
                || root.id.len() > 128
                || !root
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(LibraryError::InvalidInput(
                    "Index root identity is invalid".into(),
                ));
            }
            validate_root_limits(root.scan_interval_minutes, root.max_file_bytes)?;
            if !root_ids.insert(root.id.clone()) {
                return Err(LibraryError::InvalidInput(
                    "Index root identity is duplicated".into(),
                ));
            }
            if !root.path.is_absolute() {
                return Err(LibraryError::InvalidInput(
                    "Index root path must be absolute".into(),
                ));
            }
            tx.execute(
                "INSERT INTO index_roots
                 (id, path, enabled, last_scanned_at, scan_interval_minutes, max_file_bytes, scan_zip)
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)",
                params![
                    root.id,
                    root.path.to_string_lossy(),
                    root.enabled,
                    i64::from(root.scan_interval_minutes),
                    root.max_file_bytes as i64,
                    root.scan_zip,
                ],
            )?;
        }
        tx.execute(
            "UPDATE catalog_settings SET thumbnail_size=?1, thumbnail_background=?2,
                    thumbnail_concurrency=?3, scan_batch_size=?4 WHERE id='local'",
            params![
                i64::from(settings.thumbnail_size),
                settings.thumbnail_background,
                i64::from(settings.thumbnail_concurrency),
                i64::from(settings.scan_batch_size),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_catalog_settings(&self) -> Result<CatalogSettings, LibraryError> {
        let (thumbnail_size, thumbnail_background, thumbnail_concurrency, scan_batch_size): (
            i64,
            String,
            i64,
            i64,
        ) = self.conn.query_row(
            "SELECT thumbnail_size, thumbnail_background, thumbnail_concurrency,
                        scan_batch_size FROM catalog_settings WHERE id = 'local'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        let corrupt = || LibraryError::InvalidInput("catalog settings row is corrupt".into());
        let settings = CatalogSettings {
            thumbnail_size: u32::try_from(thumbnail_size).map_err(|_| corrupt())?,
            thumbnail_background,
            thumbnail_concurrency: u32::try_from(thumbnail_concurrency).map_err(|_| corrupt())?,
            scan_batch_size: u32::try_from(scan_batch_size).map_err(|_| corrupt())?,
        };
        validate_catalog_settings(&settings)?;
        Ok(settings)
    }

    pub fn update_catalog_settings(
        &self,
        settings: &CatalogSettings,
    ) -> Result<CatalogSettings, LibraryError> {
        validate_catalog_settings(settings)?;
        let current = self.get_catalog_settings()?;
        let appearance_changed = current.thumbnail_size != settings.thumbnail_size
            || current.thumbnail_background != settings.thumbnail_background;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE catalog_settings SET thumbnail_size = ?1, thumbnail_background = ?2,
                    thumbnail_concurrency = ?3, scan_batch_size = ?4
             WHERE id = 'local'",
            params![
                i64::from(settings.thumbnail_size),
                settings.thumbnail_background,
                i64::from(settings.thumbnail_concurrency),
                i64::from(settings.scan_batch_size)
            ],
        )?;
        if appearance_changed {
            let mut affected = Vec::new();
            {
                let mut stmt =
                    tx.prepare("SELECT DISTINCT model_id FROM thumbnails WHERE is_custom=0")?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                affected.extend(rows.collect::<Result<Vec<_>, _>>()?);
            }
            tx.execute("DELETE FROM thumbnails WHERE is_custom = 0", [])?;
            for model_id in affected {
                compact_thumbnail_ordinals(&tx, &model_id)?;
                select_thumbnail_fallback(&tx, &model_id)?;
            }
        }
        tx.commit()?;
        self.get_catalog_settings()
    }

    pub fn update_index_root(&self, root: &IndexRoot) -> Result<IndexRoot, LibraryError> {
        validate_root_limits(root.scan_interval_minutes, root.max_file_bytes)?;
        let existing = self
            .get_index_root(&root.id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(root.id.clone()))?;
        if root.path != existing.path {
            return Err(LibraryError::InvalidInput(
                "index root paths are immutable; remove and add the root again".into(),
            ));
        }
        let changed = self.conn.execute(
            "UPDATE index_roots SET path = ?1, enabled = ?2, scan_interval_minutes = ?3,
                    max_file_bytes = ?4, scan_zip = ?5
             WHERE id = ?6",
            params![
                existing.path.to_string_lossy(),
                root.enabled,
                i64::from(root.scan_interval_minutes),
                root.max_file_bytes as i64,
                root.scan_zip,
                root.id
            ],
        )?;
        if changed == 0 {
            return Err(LibraryError::IndexRootNotFound(root.id.clone()));
        }
        self.get_index_root(&root.id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(root.id.clone()))
    }

    pub fn remove_index_root(&self, id: &str) -> Result<(), LibraryError> {
        self.get_index_root(id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(id.to_owned()))?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM models WHERE source_kind = 'indexed' AND source_root_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM index_roots WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn remap_restored_index_roots(
        &self,
        mappings: &[(String, String, PathBuf, bool)],
    ) -> Result<(), LibraryError> {
        let mut backup_ids = HashSet::new();
        let mut target_ids = HashSet::new();
        let mut paths = Vec::new();
        for (backup_id, target_id, path, _) in mappings {
            if !backup_ids.insert(backup_id.clone()) || !target_ids.insert(target_id.clone()) {
                return Err(LibraryError::InvalidInput(
                    "restore root mapping is duplicated".into(),
                ));
            }
            let path = canonical_directory(path)?;
            if paths
                .iter()
                .any(|existing: &PathBuf| path.starts_with(existing) || existing.starts_with(&path))
            {
                return Err(LibraryError::InvalidInput(
                    "restore root mappings overlap".into(),
                ));
            }
            paths.push(path);
        }
        self.conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
        let result = (|| {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute("UPDATE index_roots SET enabled=0", [])?;
            for ((backup_id, target_id, _, enabled), path) in mappings.iter().zip(&paths) {
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM index_roots WHERE id=?1)",
                    params![backup_id],
                    |row| row.get(0),
                )?;
                if !exists {
                    return Err(LibraryError::IndexRootNotFound(backup_id.clone()));
                }
                if backup_id != target_id {
                    let target_exists: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM index_roots WHERE id=?1)",
                        params![target_id],
                        |row| row.get(0),
                    )?;
                    if target_exists {
                        let displaced = new_id("unmapped-root");
                        tx.execute(
                            "UPDATE models SET source_root_id=?1 WHERE source_root_id=?2",
                            params![displaced, target_id],
                        )?;
                        tx.execute(
                            "UPDATE bundles SET source_root_id=?1 WHERE source_root_id=?2",
                            params![displaced, target_id],
                        )?;
                        tx.execute(
                            "UPDATE index_roots SET id=?1,enabled=0 WHERE id=?2",
                            params![displaced, target_id],
                        )?;
                    }
                    tx.execute(
                        "UPDATE models SET source_root_id=?1 WHERE source_root_id=?2",
                        params![target_id, backup_id],
                    )?;
                    tx.execute(
                        "UPDATE bundles SET source_root_id=?1 WHERE source_root_id=?2",
                        params![target_id, backup_id],
                    )?;
                }
                tx.execute(
                    "UPDATE index_roots SET id=?1,path=?2,enabled=?3 WHERE id=?4",
                    params![target_id, path.to_string_lossy(), enabled, backup_id],
                )?;
            }
            tx.commit()?;
            Ok(())
        })();
        self.conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        result?;
        let violation: Option<String> = self
            .conn
            .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
            .optional()?;
        if violation.is_some() {
            return Err(LibraryError::InvalidInput(
                "restore root mapping violates catalog links".into(),
            ));
        }
        Ok(())
    }

    pub fn move_loose_indexed_model(
        &self,
        model_id: &str,
        expected_version: u64,
        destination_root_id: &str,
        destination_relative_path: &Path,
        modified_at: u64,
    ) -> Result<Model, LibraryError> {
        self.move_loose_indexed_models(&[IndexedModelMove {
            model_id: model_id.into(),
            expected_version,
            destination_root_id: destination_root_id.into(),
            destination_relative_path: destination_relative_path.to_path_buf(),
            modified_at,
        }])?
        .into_iter()
        .next()
        .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    pub fn move_loose_indexed_models(
        &self,
        moves: &[IndexedModelMove],
    ) -> Result<Vec<Model>, LibraryError> {
        const MAX_MOVE_TARGETS: usize = 100;
        if moves.is_empty() || moves.len() > MAX_MOVE_TARGETS {
            return Err(LibraryError::InvalidInput(
                "move target count is invalid".into(),
            ));
        }
        let mut model_ids = HashSet::new();
        let mut destinations = HashSet::new();
        let mut normalized = Vec::with_capacity(moves.len());
        for item in moves {
            if !model_ids.insert(item.model_id.as_str()) {
                return Err(LibraryError::InvalidInput("duplicate move target".into()));
            }
            let relative = validated_relative_path(&item.destination_relative_path)?;
            if !destinations.insert((item.destination_root_id.clone(), relative.clone())) {
                return Err(LibraryError::InvalidInput(
                    "duplicate move destination".into(),
                ));
            }
            let modified_at = i64::try_from(item.modified_at)
                .map_err(|_| LibraryError::InvalidInput("modified time is out of range".into()))?;
            normalized.push((item, relative, modified_at));
        }
        let tx = self.conn.unchecked_transaction()?;
        let move_ids = model_ids;
        for (item, _, _) in &normalized {
            tx.query_row(
                "SELECT id FROM index_roots WHERE id=?1",
                params![item.destination_root_id],
                |_| Ok(()),
            )
            .optional()?
            .ok_or_else(|| LibraryError::IndexRootNotFound(item.destination_root_id.clone()))?;
            let row: Option<(i64, String, Option<String>, Option<String>)> = tx
                .query_row(
                    "SELECT metadata_version,source_kind,source_archive_member,deleted_at
                     FROM models WHERE id=?1",
                    params![item.model_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            let Some((actual, source_kind, archive_member, deleted_at)) = row else {
                return Err(LibraryError::ModelNotFound(item.model_id.clone()));
            };
            if actual as u64 != item.expected_version {
                return Err(LibraryError::MetadataConflict {
                    model_id: item.model_id.clone(),
                    expected: item.expected_version,
                    actual: actual as u64,
                });
            }
            if deleted_at.is_some() || source_kind != "indexed" || archive_member.is_some() {
                return Err(LibraryError::InvalidInput(
                    "only active loose indexed models can be moved".into(),
                ));
            }
        }
        for (item, relative, _) in &normalized {
            let collision: Option<String> = tx
                .query_row(
                    "SELECT id FROM models WHERE source_kind='indexed' AND source_root_id=?1
                     AND source_path=?2 AND COALESCE(source_archive_member,'')=''
                     AND id<>?3 LIMIT 1",
                    params![item.destination_root_id, relative, item.model_id],
                    |row| row.get(0),
                )
                .optional()?;
            if collision
                .as_ref()
                .is_some_and(|id| !move_ids.contains(id.as_str()))
            {
                return Err(LibraryError::InvalidInput(
                    "move destination is already cataloged".into(),
                ));
            }
        }
        let now = timestamp();
        for (item, _, _) in &normalized {
            let temporary_path = format!(".gogglelab-move-{}", Uuid::new_v4().simple());
            tx.execute(
                "UPDATE models SET source_path=?1 WHERE id=?2",
                params![temporary_path, item.model_id],
            )?;
        }
        for (item, relative, modified_at) in &normalized {
            tx.execute(
                "UPDATE models SET source_root_id=?1,source_path=?2,source_missing=0,
                        source_modified_at=?3,metadata_version=metadata_version+1,updated_at=?4
                 WHERE id=?5",
                params![
                    item.destination_root_id,
                    relative,
                    modified_at,
                    now,
                    item.model_id
                ],
            )?;
        }
        tx.commit()?;
        moves
            .iter()
            .map(|item| {
                self.get_model(&item.model_id)?
                    .ok_or_else(|| LibraryError::ModelNotFound(item.model_id.clone()))
            })
            .collect()
    }

    pub fn indexed_source_metadata(
        &self,
        root_id: &str,
        relative_path: &Path,
        archive_member: Option<&str>,
    ) -> Result<Option<IndexedSourceMetadata>, LibraryError> {
        let relative_path = validated_relative_path(relative_path)?;
        let archive_member = archive_member.map(validated_archive_member).transpose()?;
        let root = self
            .get_index_root(root_id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(root_id.to_owned()))?;
        let absolute_path = root
            .path
            .join(&relative_path)
            .to_str()
            .map(ToOwned::to_owned);
        self.conn
            .query_row(
                "SELECT d.bytes, m.source_modified_at, d.content_hash,
                        m.source_archive_member, d.outer_fingerprint
                 FROM models m JOIN indexed_model_data d ON d.model_id = m.id
                 WHERE m.source_kind = 'indexed' AND m.source_root_id = ?1
                   AND (m.source_path = ?2 OR (?4 IS NOT NULL AND m.source_path = ?4))
                   AND COALESCE(m.source_archive_member,'') = COALESCE(?3,'')
                 ORDER BY (m.source_path = ?2) DESC LIMIT 1",
                params![root_id, relative_path, archive_member, absolute_path],
                |row| {
                    Ok(IndexedSourceMetadata {
                        bytes: row.get::<_, i64>(0)? as u64,
                        modified_at: row.get::<_, i64>(1)? as u64,
                        content_hash: row.get(2)?,
                        archive_member: row.get(3)?,
                        outer_fingerprint: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn indexed_archive_metadata(
        &self,
        root_id: &str,
        relative_path: &Path,
    ) -> Result<Vec<IndexedSourceMetadata>, LibraryError> {
        let relative = validated_relative_path(relative_path)?;
        let root = self
            .get_index_root(root_id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(root_id.to_owned()))?;
        let absolute = root.path.join(&relative).to_str().map(ToOwned::to_owned);
        let mut stmt = self.conn.prepare("SELECT d.bytes,m.source_modified_at,d.content_hash,m.source_archive_member,d.outer_fingerprint FROM models m JOIN indexed_model_data d ON d.model_id=m.id WHERE m.source_kind='indexed' AND m.source_root_id=?1 AND (m.source_path=?2 OR (?3 IS NOT NULL AND m.source_path=?3)) AND m.source_archive_member IS NOT NULL ORDER BY m.source_archive_member")?;
        let rows = stmt.query_map(params![root_id, relative, absolute], |row| {
            Ok(IndexedSourceMetadata {
                bytes: row.get::<_, i64>(0)? as u64,
                modified_at: row.get::<_, i64>(1)? as u64,
                content_hash: row.get(2)?,
                archive_member: row.get(3)?,
                outer_fingerprint: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn record_scan_entries(
        &self,
        root_id: &str,
        entries: &[ScannedModel],
    ) -> Result<ScanReconcile, LibraryError> {
        let root = self
            .get_index_root(root_id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(root_id.to_owned()))?;
        if !root.enabled {
            return Err(LibraryError::InvalidInput("index root is disabled".into()));
        }
        let mut validated = Vec::with_capacity(entries.len());
        let mut paths = HashSet::with_capacity(entries.len());
        for entry in entries {
            let relative = validated_relative_path(&entry.relative_path)?;
            let archive_member = entry
                .archive_member
                .as_deref()
                .map(validated_archive_member)
                .transpose()?;
            if archive_member.is_some()
                && entry.outer_fingerprint.as_deref().is_none_or(str::is_empty)
            {
                return Err(LibraryError::InvalidInput(
                    "archive entries require an outer fingerprint".into(),
                ));
            }
            if !paths.insert((relative.clone(), archive_member.clone())) {
                return Err(LibraryError::InvalidInput(
                    "scan batch contains duplicate relative paths".into(),
                ));
            }
            validate_hash(&entry.content_hash)?;
            if entry.format.trim().is_empty() || entry.bytes > root.max_file_bytes {
                return Err(LibraryError::InvalidInput(
                    "invalid scanned model metadata".into(),
                ));
            }
            resolve_under_root(&root.path, Path::new(&relative))?;
            let absolute = root.path.join(&relative).to_str().map(ToOwned::to_owned);
            validated.push((relative, absolute, archive_member, entry));
        }

        let tx = self.conn.unchecked_transaction()?;
        let mut result = ScanReconcile {
            seen: validated.len(),
            ..ScanReconcile::default()
        };
        for (relative, absolute, archive_member, entry) in validated {
            let existing: Option<(String, String)> = tx
                .query_row(
                    "SELECT m.id, d.content_hash
                     FROM models m JOIN indexed_model_data d ON d.model_id = m.id
                     WHERE m.source_kind = 'indexed' AND m.source_root_id = ?1
                       AND (m.source_path = ?2 OR (?4 IS NOT NULL AND m.source_path = ?4))
                       AND COALESCE(m.source_archive_member,'')=COALESCE(?3,'')
                     ORDER BY (m.source_path = ?2) DESC LIMIT 1",
                    params![root_id, relative, archive_member, absolute],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let original_name = archive_member.as_deref().unwrap_or(&relative);
            let original_name = Path::new(original_name)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("model")
                .to_owned();
            if let Some((model_id, existing_hash)) = existing {
                let content_changed = existing_hash != entry.content_hash;
                tx.execute(
                    "UPDATE indexed_model_data SET content_hash = ?1, original_name = ?2,
                            format = ?3, bytes = ?4, outer_fingerprint=?5 WHERE model_id = ?6",
                    params![
                        entry.content_hash,
                        original_name,
                        entry.format,
                        entry.bytes as i64,
                        entry.outer_fingerprint,
                        model_id
                    ],
                )?;
                tx.execute(
                    "UPDATE models SET source_path = ?1, source_missing = 0, source_modified_at = ?2,
                            metadata_version = metadata_version + ?3, updated_at = ?4 WHERE id = ?5",
                    params![
                        relative,
                        entry.modified_at as i64,
                        if content_changed { 1_i64 } else { 0_i64 },
                        timestamp(),
                        model_id
                    ],
                )?;
                if archive_member.is_some() {
                    tx.execute(
                        "UPDATE bundles SET source_path=?1
                         WHERE id=(SELECT bundle_id FROM models WHERE id=?2)
                           AND source_root_id=?3",
                        params![relative, model_id, root_id],
                    )?;
                }
                result.updated += 1;
            } else {
                let model_id = new_id("model");
                let bundle_id = if archive_member.is_some() {
                    Some(ensure_archive_bundle(&tx, root_id, &relative)?)
                } else {
                    None
                };
                let display_name = entry
                    .display_name
                    .as_deref()
                    .map(|value| clean_name(value, "Model"))
                    .transpose()?
                    .or_else(|| {
                        Path::new(&relative)
                            .file_stem()
                            .and_then(|value| value.to_str())
                            .map(ToOwned::to_owned)
                    })
                    .unwrap_or_else(|| "Model".into());
                let now = timestamp();
                tx.execute(
                    "INSERT INTO models
                     (id, display_name, description, folder_id, metadata_version, created_at,
                      updated_at, source_kind, source_path, source_root_id, source_missing,
                      source_modified_at, source_archive_member, bundle_id)
                     VALUES (?1, ?2, '', ?3, 1, ?4, ?4, 'indexed', ?5, ?6, 0, ?7, ?8, ?9)",
                    params![
                        model_id,
                        display_name,
                        ROOT_FOLDER_ID,
                        now,
                        relative,
                        root_id,
                        entry.modified_at as i64,
                        archive_member,
                        bundle_id
                    ],
                )?;
                tx.execute(
                    "INSERT INTO indexed_model_data
                     (model_id, content_hash, original_name, format, bytes, outer_fingerprint)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        model_id,
                        entry.content_hash,
                        original_name,
                        entry.format,
                        entry.bytes as i64,
                        entry.outer_fingerprint
                    ],
                )?;
                result.added_model_ids.push(model_id);
                result.added += 1;
            }
        }
        tx.commit()?;
        Ok(result)
    }

    pub fn complete_scan(
        &self,
        root_id: &str,
        seen_relative_paths: &[PathBuf],
        completed_at: &str,
    ) -> Result<ScanReconcile, LibraryError> {
        self.complete_scan_with_new_models(root_id, seen_relative_paths, &[], completed_at)
    }

    pub fn complete_scan_with_new_models(
        &self,
        root_id: &str,
        seen_relative_paths: &[PathBuf],
        newly_added_model_ids: &[String],
        completed_at: &str,
    ) -> Result<ScanReconcile, LibraryError> {
        let seen = seen_relative_paths
            .iter()
            .cloned()
            .map(|relative_path| SourceRef {
                relative_path,
                archive_member: None,
            })
            .collect::<Vec<_>>();
        self.complete_scan_sources_with_new_models(
            root_id,
            &seen,
            newly_added_model_ids,
            completed_at,
        )
    }

    pub fn complete_scan_sources_with_new_models(
        &self,
        root_id: &str,
        seen_sources: &[SourceRef],
        newly_added_model_ids: &[String],
        completed_at: &str,
    ) -> Result<ScanReconcile, LibraryError> {
        self.complete_scan_sources_in_scope(
            root_id,
            None,
            seen_sources,
            newly_added_model_ids,
            Some(completed_at),
        )
    }

    pub fn indexed_archive_source_refs(
        &self,
        root_id: &str,
        scope: Option<&Path>,
    ) -> Result<Vec<SourceRef>, LibraryError> {
        let root = self
            .get_index_root(root_id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(root_id.to_owned()))?;
        let mut stmt = self.conn.prepare(
            "SELECT source_path, source_archive_member FROM models
             WHERE source_kind='indexed' AND source_root_id=?1 AND deleted_at IS NULL
               AND source_archive_member IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![root_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        let mut sources = Vec::new();
        for row in rows {
            let (path, archive_member) = row?;
            let relative_path = normalize_indexed_source_path(&root.path, Path::new(&path))?;
            if scope.is_some_and(|scope| !relative_path.starts_with(scope)) {
                continue;
            }
            sources.push(SourceRef {
                relative_path,
                archive_member: Some(validated_archive_member(
                    archive_member.as_deref().ok_or_else(|| {
                        LibraryError::InvalidInput("archive member path is missing".into())
                    })?,
                )?),
            });
        }
        Ok(sources)
    }

    pub fn complete_subdirectory_scan_sources_with_new_models(
        &self,
        root_id: &str,
        relative_directory: &Path,
        seen_sources: &[SourceRef],
        newly_added_model_ids: &[String],
    ) -> Result<ScanReconcile, LibraryError> {
        let relative_directory = PathBuf::from(validated_relative_path(relative_directory)?);
        self.complete_scan_sources_in_scope(
            root_id,
            Some(&relative_directory),
            seen_sources,
            newly_added_model_ids,
            None,
        )
    }

    fn complete_scan_sources_in_scope(
        &self,
        root_id: &str,
        scope: Option<&Path>,
        seen_sources: &[SourceRef],
        newly_added_model_ids: &[String],
        completed_at: Option<&str>,
    ) -> Result<ScanReconcile, LibraryError> {
        let root = self
            .get_index_root(root_id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(root_id.to_owned()))?;
        if !root.enabled {
            return Err(LibraryError::InvalidInput("index root is disabled".into()));
        }
        if canonical_directory(&root.path)? != root.path {
            return Err(LibraryError::InvalidInput(
                "configured root identity changed".into(),
            ));
        }
        let scope_relative = scope.and_then(|scope| scope.to_str().map(ToOwned::to_owned));
        let scope_absolute =
            scope.and_then(|scope| root.path.join(scope).to_str().map(ToOwned::to_owned));
        if scope.is_some() && scope_relative.is_none() {
            return Err(LibraryError::InvalidInput(
                "scan directory is not valid UTF-8".into(),
            ));
        }
        let separator = std::path::MAIN_SEPARATOR.to_string();
        let seen = seen_sources
            .iter()
            .map(|source| -> Result<SourceRef, LibraryError> {
                let source = SourceRef {
                    relative_path: PathBuf::from(validated_relative_path(&source.relative_path)?),
                    archive_member: source
                        .archive_member
                        .as_deref()
                        .map(validated_archive_member)
                        .transpose()?,
                };
                if scope.is_some_and(|scope| !source.relative_path.starts_with(scope)) {
                    return Err(LibraryError::InvalidInput(
                        "scanned source is outside the selected directory".into(),
                    ));
                }
                Ok(source)
            })
            .collect::<Result<HashSet<_>, _>>()?;
        let tx = self.conn.unchecked_transaction()?;
        let mut indexed = Vec::new();
        {
            let mut stmt = tx.prepare(
                "SELECT m.id, m.source_path, m.source_archive_member, d.content_hash
                 FROM models m JOIN indexed_model_data d ON d.model_id = m.id
                 WHERE m.source_kind = 'indexed' AND m.source_root_id = ?1
                   AND (?2 IS NULL OR m.source_path = ?2
                        OR substr(m.source_path, 1, length(?2) + 1) = ?2 || ?3
                        OR (?4 IS NOT NULL AND (m.source_path = ?4
                            OR substr(m.source_path, 1, length(?4) + 1) = ?4 || ?3)))",
            )?;
            let rows = stmt.query_map(
                params![root_id, scope_relative, separator, scope_absolute],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )?;
            for row in rows {
                let (id, path, member, hash) = row?;
                let path = normalize_indexed_source_path(&root.path, Path::new(&path))?;
                if scope.is_some_and(|scope| !path.starts_with(scope)) {
                    continue;
                }
                indexed.push((id, validated_relative_path(&path)?, member, hash));
            }
        }

        let newly_added = newly_added_model_ids.iter().collect::<HashSet<_>>();
        let mut absent_by_hash: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut present_by_hash: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (id, path, member, hash) in &indexed {
            let source = SourceRef {
                relative_path: PathBuf::from(path),
                archive_member: member.clone(),
            };
            let target = if seen.contains(&source) {
                &mut present_by_hash
            } else {
                &mut absent_by_hash
            };
            target.entry(hash.clone()).or_default().push((
                id.clone(),
                format!("{}\0{}", path, member.as_deref().unwrap_or("")),
            ));
        }

        let mut result = ScanReconcile {
            seen: seen.len(),
            ..ScanReconcile::default()
        };
        let mut rename_targets = HashMap::<String, (String, String, String)>::new();
        for (hash, absent) in &absent_by_hash {
            let Some(present) = present_by_hash.get(hash) else {
                continue;
            };
            if absent.len() != 1 || present.len() != 1 {
                continue;
            }
            let (old_id, _) = &absent[0];
            let (new_id, new_key) = &present[0];
            if old_id == new_id || !newly_added.contains(new_id) {
                continue;
            }
            let (new_path, new_member) = new_key.split_once('\0').unwrap();
            rename_targets.insert(
                old_id.clone(),
                (new_id.clone(), new_path.to_owned(), new_member.to_owned()),
            );
        }
        for (hash, absent) in absent_by_hash.iter() {
            let Some(present) = present_by_hash.get(hash) else {
                continue;
            };
            if absent.len() != 1 || present.len() != 1 {
                continue;
            }
            let (old_id, _) = &absent[0];
            let (new_id, new_key) = &present[0];
            if old_id == new_id || !newly_added.contains(new_id) {
                continue;
            }
            let (new_path, new_member) = new_key.split_once('\0').unwrap();
            let (modified_at, original_name, format, bytes, outer_fingerprint): (i64, String, String, i64, Option<String>) = tx
                .query_row(
                    "SELECT m.source_modified_at, d.original_name, d.format, d.bytes,d.outer_fingerprint
                     FROM models m JOIN indexed_model_data d ON d.model_id = m.id WHERE m.id = ?1",
                    params![new_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,row.get(4)?)),
                )?;
            let old_archive_member: Option<String> = tx.query_row(
                "SELECT source_archive_member FROM models WHERE id=?1",
                params![old_id],
                |row| row.get(0),
            )?;
            let new_is_archive_member = !new_member.is_empty();
            let archive_kind_changed = old_archive_member.is_some() != new_is_archive_member;
            if archive_kind_changed {
                let old_bundle:Option<(Option<String>,Option<String>)>=tx.query_row("SELECT m.bundle_id,b.source_root_id FROM models m LEFT JOIN bundles b ON b.id=m.bundle_id WHERE m.id=?1",params![old_id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
                if let Some((Some(_), Some(_))) = old_bundle {
                    if new_member.is_empty() {
                        tx.execute(
                            "UPDATE models SET bundle_id=NULL WHERE id=?1",
                            params![old_id],
                        )?;
                    } else {
                        let new_bundle: Option<String> = tx
                            .query_row(
                                "SELECT bundle_id FROM models WHERE id=?1",
                                params![new_id],
                                |row| row.get(0),
                            )
                            .optional()?
                            .flatten();
                        tx.execute(
                            "UPDATE models SET bundle_id=?1 WHERE id=?2",
                            params![new_bundle, old_id],
                        )?;
                    }
                }
            }
            if !archive_kind_changed && !new_member.is_empty() {
                let old_bundle:Option<(Option<String>,Option<String>)>=tx.query_row("SELECT m.bundle_id,b.source_root_id FROM models m LEFT JOIN bundles b ON b.id=m.bundle_id WHERE m.id=?1",params![old_id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
                let new_bundle: Option<String> = tx
                    .query_row(
                        "SELECT bundle_id FROM models WHERE id=?1",
                        params![new_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();
                if let Some((Some(old_bundle_id), Some(_))) = old_bundle {
                    if let Some(new_bundle_id) = new_bundle {
                        if new_bundle_id != old_bundle_id {
                            let mut refs = Vec::new();
                            {
                                let mut stmt=tx.prepare("SELECT id,source_path,source_archive_member FROM models WHERE bundle_id=?1")?;
                                let rows = stmt.query_map(params![old_bundle_id], |row| {
                                    Ok((
                                        row.get::<_, String>(0)?,
                                        row.get::<_, String>(1)?,
                                        row.get::<_, Option<String>>(2)?,
                                    ))
                                })?;
                                refs.extend(rows.collect::<Result<Vec<_>, _>>()?);
                            }
                            let all_moving = refs.iter().all(|(id, _, _)| {
                                rename_targets
                                    .get(id)
                                    .is_some_and(|(_, target_path, _)| target_path == new_path)
                            });
                            if all_moving {
                                tx.execute(
                                    "UPDATE models SET bundle_id=?1 WHERE bundle_id=?2",
                                    params![old_bundle_id, new_bundle_id],
                                )?;
                                tx.execute(
                                    "DELETE FROM bundles WHERE id=?1",
                                    params![new_bundle_id],
                                )?;
                                tx.execute("UPDATE bundles SET source_root_id=?1,source_path=?2 WHERE id=?3",params![root_id,new_path,old_bundle_id])?;
                            } else {
                                tx.execute(
                                    "UPDATE models SET bundle_id=?1 WHERE id=?2",
                                    params![new_bundle_id, old_id],
                                )?;
                            }
                        }
                    }
                }
            }
            tx.execute("DELETE FROM models WHERE id = ?1", params![new_id])?;
            tx.execute(
                "UPDATE models SET source_path = ?1, source_archive_member=?2, source_missing = 0,
                        source_modified_at = ?3, metadata_version=metadata_version+1,
                        updated_at = ?4 WHERE id = ?5",
                params![
                    new_path,
                    if new_member.is_empty() {
                        None
                    } else {
                        Some(new_member)
                    },
                    modified_at,
                    timestamp(),
                    old_id
                ],
            )?;
            tx.execute(
                "UPDATE indexed_model_data SET original_name = ?1, format = ?2, bytes = ?3,outer_fingerprint=?4
                 WHERE model_id = ?5",
                params![original_name, format, bytes, outer_fingerprint, old_id],
            )?;
            result.renamed += 1;
        }

        let mut ids_and_paths = Vec::new();
        {
            let mut stmt = tx.prepare(
                "SELECT id, source_path,source_archive_member FROM models
                 WHERE source_kind = 'indexed' AND source_root_id = ?1
                   AND (?2 IS NULL OR source_path = ?2
                        OR substr(source_path, 1, length(?2) + 1) = ?2 || ?3
                        OR (?4 IS NOT NULL AND (source_path = ?4
                            OR substr(source_path, 1, length(?4) + 1) = ?4 || ?3)))",
            )?;
            let rows = stmt.query_map(
                params![root_id, scope_relative, separator, scope_absolute],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )?;
            for row in rows {
                let (id, path, member) = row?;
                let path = normalize_indexed_source_path(&root.path, Path::new(&path))?;
                if scope.is_some_and(|scope| !path.starts_with(scope)) {
                    continue;
                }
                ids_and_paths.push((id, validated_relative_path(&path)?, member));
            }
        }
        for (id, path, member) in ids_and_paths {
            let missing = !seen.contains(&SourceRef {
                relative_path: PathBuf::from(path),
                archive_member: member,
            });
            tx.execute(
                "UPDATE models SET source_missing = ?1 WHERE id = ?2",
                params![missing, id],
            )?;
            result.missing += usize::from(missing);
        }
        if scope.is_none() {
            reconcile_folder_bundles(&tx, root_id)?;
            let changed = tx.execute(
                "UPDATE index_roots SET last_scanned_at = ?1 WHERE id = ?2",
                params![
                    completed_at.ok_or_else(|| {
                        LibraryError::InvalidInput(
                            "full scan is missing its completion time".into(),
                        )
                    })?,
                    root_id
                ],
            )?;
            if changed == 0 {
                return Err(LibraryError::IndexRootNotFound(root_id.to_owned()));
            }
        }
        tx.commit()?;
        Ok(result)
    }

    pub fn resolve_model_source(&self, model_id: &str) -> Result<PathBuf, LibraryError> {
        Ok(self.resolve_model_source_ref(model_id)?.outer_path)
    }

    pub fn resolve_model_source_ref(
        &self,
        model_id: &str,
    ) -> Result<ResolvedModelSource, LibraryError> {
        let source: Option<SourceResolutionRow> = self
            .conn
            .query_row(
                "SELECT m.source_kind, m.source_path, m.source_root_id, m.source_missing,
                        b.relative_path,m.source_archive_member,d.outer_fingerprint,
                        COALESCE(r.blob_hash,d.content_hash),COALESCE(r.bytes,d.bytes,0)
                 FROM models m
                 LEFT JOIN revisions r ON r.id = m.current_revision_id
                 LEFT JOIN blobs b ON b.hash = r.blob_hash
                 LEFT JOIN indexed_model_data d ON d.model_id=m.id
                 WHERE m.id = ?1",
                params![model_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                },
            )
            .optional()?;
        let (
            source_kind,
            source_path,
            source_root_id,
            source_missing,
            managed_path,
            archive_member,
            outer_fingerprint,
            content_hash,
            bytes,
        ) = source.ok_or_else(|| LibraryError::ModelNotFound(model_id.to_owned()))?;
        let content_hash = content_hash
            .ok_or_else(|| LibraryError::InvalidInput("model source has no content hash".into()))?;
        if source_missing {
            return Err(LibraryError::InvalidInput("model source is missing".into()));
        }
        match source_kind.as_str() {
            "managed" => {
                let relative = managed_path.ok_or_else(|| {
                    LibraryError::InvalidInput("managed model has no storage path".into())
                })?;
                if Path::new(&relative).components().next()
                    != Some(Component::Normal("originals".as_ref()))
                {
                    return Err(LibraryError::InvalidInput(
                        "managed model source must be stored under originals".into(),
                    ));
                }
                Ok(ResolvedModelSource {
                    outer_path: resolve_under_root(&self.root, Path::new(&relative))?,
                    archive_member: None,
                    outer_fingerprint: None,
                    content_hash,
                    bytes: bytes as u64,
                })
            }
            "indexed" => {
                let root_id = source_root_id.ok_or_else(|| {
                    LibraryError::InvalidInput("indexed model has no source root".into())
                })?;
                let root = self
                    .get_index_root(&root_id)?
                    .ok_or(LibraryError::IndexRootNotFound(root_id))?;
                let relative = source_path.ok_or_else(|| {
                    LibraryError::InvalidInput("indexed model has no source path".into())
                })?;
                let relative = normalize_indexed_source_path(&root.path, Path::new(&relative))?;
                Ok(ResolvedModelSource {
                    outer_path: resolve_under_root(&root.path, &relative)?,
                    archive_member,
                    outer_fingerprint,
                    content_hash,
                    bytes: bytes as u64,
                })
            }
            _ => Err(LibraryError::InvalidInput(
                "unknown model source kind".into(),
            )),
        }
    }

    pub fn set_thumbnail(
        &self,
        model_id: &str,
        content_hash: &str,
        thumbnail_relative_path: &Path,
    ) -> Result<(), LibraryError> {
        let model = self
            .get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.to_owned()))?;
        let source_content_hash = model
            .content_hash
            .ok_or_else(|| LibraryError::InvalidInput("model has no source content hash".into()))?;
        self.set_thumbnail_for_source(
            model_id,
            &source_content_hash,
            content_hash,
            thumbnail_relative_path,
        )
    }

    pub fn set_thumbnail_for_source(
        &self,
        model_id: &str,
        source_content_hash: &str,
        content_hash: &str,
        thumbnail_relative_path: &Path,
    ) -> Result<(), LibraryError> {
        self.set_thumbnail_for_source_with_expected_settings(
            model_id,
            source_content_hash,
            None,
            content_hash,
            thumbnail_relative_path,
        )
    }

    pub fn set_thumbnail_for_source_and_settings(
        &self,
        model_id: &str,
        source_content_hash: &str,
        expected_settings: &CatalogSettings,
        content_hash: &str,
        thumbnail_relative_path: &Path,
    ) -> Result<(), LibraryError> {
        validate_catalog_settings(expected_settings)?;
        self.set_thumbnail_for_source_with_expected_settings(
            model_id,
            source_content_hash,
            Some(expected_settings),
            content_hash,
            thumbnail_relative_path,
        )
    }

    pub fn set_custom_thumbnail(
        &self,
        model_id: &str,
        expected_version: u64,
        png_hash: &str,
        thumbnail_relative_path: &Path,
    ) -> Result<Model, LibraryError> {
        let relative = validated_relative_path(thumbnail_relative_path)?;
        if Path::new(&relative).components().next()
            != Some(Component::Normal("thumbnails".as_ref()))
        {
            return Err(LibraryError::InvalidInput(
                "custom thumbnail must be stored under thumbnails".into(),
            ));
        }
        validated_thumbnail_cache_path(&self.root, Path::new(&relative), png_hash)?;
        let tx = self.conn.unchecked_transaction()?;
        let row: Option<(i64, Option<String>)> = tx
            .query_row(
                "SELECT metadata_version,deleted_at FROM models WHERE id=?1",
                params![model_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (actual, deleted) = row.ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))?;
        if actual as u64 != expected_version {
            return Err(LibraryError::MetadataConflict {
                model_id: model_id.into(),
                expected: expected_version,
                actual: actual as u64,
            });
        }
        if deleted.is_some() {
            return Err(LibraryError::InvalidInput(
                "removed model cannot change preview".into(),
            ));
        }
        tx.execute(
            "DELETE FROM thumbnails WHERE model_id=?1 AND is_custom=1",
            params![model_id],
        )?;
        tx.execute(
            "UPDATE thumbnails SET is_active=0 WHERE model_id=?1",
            params![model_id],
        )?;
        let ordinal: i64 = tx.query_row(
            "SELECT COALESCE(MAX(ordinal)+1,0) FROM thumbnails WHERE model_id=?1",
            params![model_id],
            |row| row.get(0),
        )?;
        tx.execute("INSERT INTO thumbnails(id,model_id,relative_path,content_hash,is_custom,source_content_hash,ordinal,is_active) VALUES(?1,?2,?3,?4,1,NULL,?5,1)",params![new_id("custom-thumbnail"),model_id,relative,png_hash,ordinal])?;
        tx.execute(
            "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
            params![timestamp(), model_id],
        )?;
        tx.commit()?;
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    pub fn clear_custom_thumbnail(
        &self,
        model_id: &str,
        expected_version: u64,
    ) -> Result<Model, LibraryError> {
        let tx = self.conn.unchecked_transaction()?;
        let row: Option<(i64, Option<String>)> = tx
            .query_row(
                "SELECT metadata_version,deleted_at FROM models WHERE id=?1",
                params![model_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (actual, deleted) = row.ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))?;
        if actual as u64 != expected_version {
            return Err(LibraryError::MetadataConflict {
                model_id: model_id.into(),
                expected: expected_version,
                actual: actual as u64,
            });
        }
        if deleted.is_some() {
            return Err(LibraryError::InvalidInput(
                "removed model cannot change preview".into(),
            ));
        }
        tx.execute(
            "DELETE FROM thumbnails WHERE model_id=?1 AND is_custom=1",
            params![model_id],
        )?;
        select_thumbnail_fallback(&tx, model_id)?;
        tx.execute(
            "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
            params![timestamp(), model_id],
        )?;
        tx.commit()?;
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    pub fn list_thumbnail_gallery(
        &self,
        model_id: &str,
    ) -> Result<Vec<ThumbnailGalleryItem>, LibraryError> {
        self.ensure_model(model_id)?;
        let mut stmt = self.conn.prepare(
            "SELECT t.id,t.model_id,t.relative_path,t.content_hash,t.source_content_hash,
                    t.is_custom,t.ordinal,t.is_active
             FROM thumbnails t JOIN models m ON m.id=t.model_id
             LEFT JOIN revisions r ON r.id=m.current_revision_id
             LEFT JOIN indexed_model_data d ON d.model_id=m.id
             WHERE t.model_id=?1
               AND (t.is_custom=1 OR t.source_content_hash IS NULL
                    OR t.source_content_hash=COALESCE(r.blob_hash,d.content_hash))
             ORDER BY t.ordinal,t.id",
        )?;
        let rows = stmt.query_map(params![model_id], thumbnail_gallery_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn add_custom_thumbnail(
        &self,
        model_id: &str,
        expected_version: u64,
        png_hash: &str,
        thumbnail_relative_path: &Path,
    ) -> Result<(Model, ThumbnailGalleryItem), LibraryError> {
        let relative =
            validate_thumbnail_record_path(&self.root, thumbnail_relative_path, png_hash)?;
        let tx = self.conn.unchecked_transaction()?;
        validate_active_model_version(&tx, model_id, expected_version)?;
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM thumbnails WHERE model_id=?1",
            params![model_id],
            |row| row.get(0),
        )?;
        if count >= 12 {
            return Err(LibraryError::InvalidInput(
                "thumbnail gallery is limited to 12 images".into(),
            ));
        }
        let ordinal: i64 = tx.query_row(
            "SELECT COALESCE(MAX(ordinal)+1,0) FROM thumbnails WHERE model_id=?1",
            params![model_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "UPDATE thumbnails SET is_active=0 WHERE model_id=?1",
            params![model_id],
        )?;
        let id = new_id("custom-thumbnail");
        tx.execute(
            "INSERT INTO thumbnails(id,model_id,relative_path,content_hash,is_custom,
             source_content_hash,ordinal,is_active) VALUES(?1,?2,?3,?4,1,NULL,?5,1)",
            params![id, model_id, relative, png_hash, ordinal],
        )?;
        tx.execute(
            "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
            params![timestamp(), model_id],
        )?;
        tx.commit()?;
        let model = self
            .get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))?;
        let item = self
            .list_thumbnail_gallery(model_id)?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| LibraryError::InvalidInput("thumbnail gallery item missing".into()))?;
        Ok((model, item))
    }

    pub fn set_active_thumbnail(
        &self,
        model_id: &str,
        expected_version: u64,
        thumbnail_id: &str,
    ) -> Result<Model, LibraryError> {
        let tx = self.conn.unchecked_transaction()?;
        validate_active_model_version(&tx, model_id, expected_version)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM thumbnails t JOIN models m ON m.id=t.model_id
             LEFT JOIN revisions r ON r.id=m.current_revision_id
             LEFT JOIN indexed_model_data d ON d.model_id=m.id
             WHERE t.id=?1 AND t.model_id=?2
               AND (t.is_custom=1 OR t.source_content_hash IS NULL
                    OR t.source_content_hash=COALESCE(r.blob_hash,d.content_hash)))",
            params![thumbnail_id, model_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(LibraryError::InvalidInput(
                "thumbnail gallery item not found".into(),
            ));
        }
        tx.execute(
            "UPDATE thumbnails SET is_active=0 WHERE model_id=?1",
            params![model_id],
        )?;
        tx.execute(
            "UPDATE thumbnails SET is_active=1 WHERE id=?1 AND model_id=?2",
            params![thumbnail_id, model_id],
        )?;
        tx.execute(
            "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
            params![timestamp(), model_id],
        )?;
        tx.commit()?;
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    pub fn delete_thumbnail_gallery_item(
        &self,
        model_id: &str,
        expected_version: u64,
        thumbnail_id: &str,
    ) -> Result<(Model, PathBuf), LibraryError> {
        let tx = self.conn.unchecked_transaction()?;
        validate_active_model_version(&tx, model_id, expected_version)?;
        let item: Option<(String, String, bool)> = tx
            .query_row(
                "SELECT relative_path,content_hash,is_active FROM thumbnails
                 WHERE id=?1 AND model_id=?2",
                params![thumbnail_id, model_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((relative_path, content_hash, active)) = item else {
            return Err(LibraryError::InvalidInput(
                "thumbnail gallery item not found".into(),
            ));
        };
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM thumbnails WHERE model_id=?1",
            params![model_id],
            |row| row.get(0),
        )?;
        if active || count <= 1 {
            return Err(LibraryError::InvalidInput(
                "active or last thumbnail cannot be deleted".into(),
            ));
        }
        let absolute =
            validated_thumbnail_cache_path(&self.root, Path::new(&relative_path), &content_hash)?;
        tx.execute("DELETE FROM thumbnails WHERE id=?1", params![thumbnail_id])?;
        compact_thumbnail_ordinals(&tx, model_id)?;
        tx.execute(
            "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
            params![timestamp(), model_id],
        )?;
        tx.commit()?;
        let model = self
            .get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))?;
        Ok((model, absolute))
    }

    fn set_thumbnail_for_source_with_expected_settings(
        &self,
        model_id: &str,
        source_content_hash: &str,
        expected_settings: Option<&CatalogSettings>,
        content_hash: &str,
        thumbnail_relative_path: &Path,
    ) -> Result<(), LibraryError> {
        let relative = validated_relative_path(thumbnail_relative_path)?;
        if Path::new(&relative).components().next()
            != Some(Component::Normal("thumbnails".as_ref()))
        {
            return Err(LibraryError::InvalidInput(
                "thumbnail must be stored in the managed thumbnails directory".into(),
            ));
        }
        resolve_under_root(&self.root, Path::new(&relative))?;
        validate_hash(content_hash)?;
        validate_hash(source_content_hash)?;
        let tx = self.conn.unchecked_transaction()?;
        let current_source_hash: Option<String> = tx
            .query_row(
                "SELECT COALESCE(r.blob_hash, d.content_hash)
                 FROM models m
                 LEFT JOIN revisions r ON r.id = m.current_revision_id
                 LEFT JOIN indexed_model_data d ON d.model_id = m.id
                 WHERE m.id = ?1",
                params![model_id],
                |row| row.get(0),
            )
            .optional()?;
        let current_source_hash =
            current_source_hash.ok_or_else(|| LibraryError::ModelNotFound(model_id.to_owned()))?;
        if current_source_hash != source_content_hash {
            return Err(LibraryError::SourceChanged);
        }
        if let Some(expected) = expected_settings {
            let current: (i64, String) = tx.query_row(
                "SELECT thumbnail_size, thumbnail_background
                 FROM catalog_settings WHERE id = 'local'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if current.0 != i64::from(expected.thumbnail_size)
                || current.1 != expected.thumbnail_background
            {
                return Err(LibraryError::InvalidInput(
                    "catalog thumbnail settings changed while rendering".into(),
                ));
            }
        }
        let unchanged: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM thumbnails
             WHERE model_id=?1 AND is_custom=0 AND relative_path=?2
               AND content_hash=?3 AND source_content_hash=?4)",
            params![model_id, relative, content_hash, source_content_hash],
            |row| row.get(0),
        )?;
        if unchanged {
            tx.commit()?;
            return Ok(());
        }
        tx.execute(
            "DELETE FROM thumbnails WHERE model_id = ?1 AND is_custom = 0",
            params![model_id],
        )?;
        let ordinal: i64 = tx.query_row(
            "SELECT COALESCE(MAX(ordinal)+1,0) FROM thumbnails WHERE model_id=?1",
            params![model_id],
            |row| row.get(0),
        )?;
        let has_active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM thumbnails WHERE model_id=?1 AND is_active=1)",
            params![model_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO thumbnails
             (id, model_id, relative_path, content_hash, is_custom, source_content_hash,ordinal,is_active)
             VALUES (?1, ?2, ?3, ?4, 0, ?5,?6,?7)",
            params![
                new_id("thumbnail"),
                model_id,
                relative,
                content_hash,
                source_content_hash,
                ordinal,
                !has_active
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn thumbnail_for_model(&self, model_id: &str) -> Result<Option<PathBuf>, LibraryError> {
        Ok(self
            .thumbnail_for_model_with_hash(model_id)?
            .map(|(path, _)| path))
    }

    pub fn thumbnail_for_model_with_hash(
        &self,
        model_id: &str,
    ) -> Result<Option<(PathBuf, String)>, LibraryError> {
        Ok(self
            .thumbnail_cache_entry_for_model(model_id)?
            .map(|entry| (entry.path, entry.png_hash)))
    }

    pub fn thumbnail_cache_entry_for_model(
        &self,
        model_id: &str,
    ) -> Result<Option<ThumbnailCacheEntry>, LibraryError> {
        let thumbnail: Option<(String, String, bool)> = self
            .conn
            .query_row(
                "SELECT t.relative_path, t.content_hash, t.is_custom FROM thumbnails t
                 LEFT JOIN revisions r ON r.id = (SELECT current_revision_id FROM models WHERE id = ?1)
                 LEFT JOIN indexed_model_data d ON d.model_id = ?1
                 WHERE t.model_id = ?1
                   AND (t.is_custom = 1 OR t.source_content_hash IS NULL
                        OR t.source_content_hash = COALESCE(r.blob_hash, d.content_hash))
                 ORDER BY t.is_custom DESC, t.id LIMIT 1",
                params![model_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        thumbnail
            .map(|(path, png_hash, is_custom)| {
                resolve_under_root(&self.root, Path::new(&path)).map(|absolute| {
                    ThumbnailCacheEntry {
                        path: absolute,
                        png_hash,
                        is_custom,
                    }
                })
            })
            .transpose()
    }

    pub fn thumbnail_relative_paths(&self) -> Result<HashSet<String>, LibraryError> {
        let mut stmt = self.conn.prepare("SELECT relative_path FROM thumbnails")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<Result<HashSet<_>, _>>()?)
    }
    pub fn catalog_content_hashes(&self) -> Result<HashSet<String>, LibraryError> {
        let mut hashes = HashSet::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT blob_hash FROM revisions UNION SELECT content_hash FROM indexed_model_data",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                hashes.insert(row?);
            }
        }
        Ok(hashes)
    }
    pub fn max_metadata_version(&self) -> Result<u64, LibraryError> {
        let value: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(metadata_version),0) FROM models",
            [],
            |row| row.get(0),
        )?;
        u64::try_from(value)
            .map_err(|_| LibraryError::InvalidInput("catalog metadata version is invalid".into()))
    }
    pub fn bump_all_metadata_versions_above(&self, floor: u64) -> Result<(), LibraryError> {
        let next = floor
            .checked_add(1)
            .ok_or_else(|| LibraryError::InvalidInput("metadata version exhausted".into()))?;
        let next = i64::try_from(next)
            .map_err(|_| LibraryError::InvalidInput("metadata version exhausted".into()))?;
        self.conn.execute(
            "UPDATE models SET metadata_version=?1,updated_at=?2",
            params![next, timestamp()],
        )?;
        Ok(())
    }

    pub fn purge_stale_thumbnails(&self) -> Result<Vec<PathBuf>, LibraryError> {
        self.purge_thumbnail_records(true)
    }

    pub fn purge_thumbnails(&self) -> Result<Vec<PathBuf>, LibraryError> {
        self.purge_thumbnail_records(false)
    }

    fn purge_thumbnail_records(&self, stale_only: bool) -> Result<Vec<PathBuf>, LibraryError> {
        let predicate = if stale_only {
            "t.is_custom = 0 AND t.source_content_hash IS NOT NULL
             AND t.source_content_hash != COALESCE(r.blob_hash, d.content_hash, '')"
        } else {
            "t.is_custom = 0"
        };
        let sql = format!(
            "SELECT t.id, t.model_id, t.relative_path, t.content_hash FROM thumbnails t
             JOIN models m ON m.id = t.model_id
             LEFT JOIN revisions r ON r.id = m.current_revision_id
             LEFT JOIN indexed_model_data d ON d.model_id = m.id
             WHERE {predicate}"
        );
        let mut records = Vec::new();
        {
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            records.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }
        let preserved_paths = {
            let mut stmt = self
                .conn
                .prepare("SELECT relative_path FROM thumbnails WHERE is_custom = 1")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<HashSet<_>, _>>()?
        };
        let validated_paths = records
            .iter()
            .map(|(_, _, path, hash)| {
                validated_thumbnail_cache_path(&self.root, Path::new(path), hash)
                    .map(|absolute| (path, absolute))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let tx = self.conn.unchecked_transaction()?;
        let affected = records
            .iter()
            .map(|(_, model_id, _, _)| model_id.clone())
            .collect::<BTreeSet<_>>();
        for (id, _, _, _) in &records {
            tx.execute("DELETE FROM thumbnails WHERE id = ?1", params![id])?;
        }
        for model_id in affected {
            select_thumbnail_fallback(&tx, &model_id)?;
        }
        tx.commit()?;
        Ok(validated_paths
            .into_iter()
            .filter_map(|(relative, absolute)| {
                (!preserved_paths.contains(relative)).then_some(absolute)
            })
            .collect())
    }

    fn get_index_root(&self, id: &str) -> Result<Option<IndexRoot>, LibraryError> {
        self.conn
            .query_row(
                "SELECT id, path, enabled, last_scanned_at, scan_interval_minutes, max_file_bytes, scan_zip
                 FROM index_roots WHERE id = ?1",
                params![id],
                index_root_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn ensure_root_does_not_overlap(
        &self,
        path: &Path,
        excluding_id: Option<&str>,
    ) -> Result<(), LibraryError> {
        for existing in self.list_index_roots()? {
            if excluding_id == Some(existing.id.as_str()) {
                continue;
            }
            if path.starts_with(&existing.path) || existing.path.starts_with(path) {
                return Err(LibraryError::InvalidInput(
                    "index roots cannot duplicate or overlap".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn list_tags(&self) -> Result<Vec<Tag>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT id,name,category FROM tags ORDER BY COALESCE(category,''),lower(name),id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Tag {
                id: r.get(0)?,
                name: r.get(1)?,
                category: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn create_tag(&self, name: &str, category: Option<&str>) -> Result<Tag, LibraryError> {
        let name = clean_tag_name(name)?;
        let category = clean_optional(category, 120)?;
        let id = new_id("tag");
        self.conn.execute(
            "INSERT INTO tags(id,name,category) VALUES(?1,?2,?3)",
            params![id, name, category],
        )?;
        Ok(Tag { id, name, category })
    }

    pub fn rename_tag(
        &self,
        id: &str,
        name: &str,
        category: Option<&str>,
    ) -> Result<Tag, LibraryError> {
        let name = clean_tag_name(name)?;
        let category = clean_optional(category, 120)?;
        let tx = self.conn.unchecked_transaction()?;
        let affected = tag_model_ids(&tx, id)?;
        if tx.execute(
            "UPDATE tags SET name=?1,category=?2 WHERE id=?3",
            params![name, category, id],
        )? == 0
        {
            return Err(LibraryError::InvalidInput("tag not found".into()));
        }
        bump_models(&tx, &affected)?;
        tx.commit()?;
        Ok(Tag {
            id: id.into(),
            name,
            category,
        })
    }

    pub fn delete_tag(&self, id: &str) -> Result<(), LibraryError> {
        let tx = self.conn.unchecked_transaction()?;
        let affected = tag_model_ids(&tx, id)?;
        if tx.execute("DELETE FROM tags WHERE id=?1", params![id])? == 0 {
            return Err(LibraryError::InvalidInput("tag not found".into()));
        }
        bump_models(&tx, &affected)?;
        tx.commit()?;
        Ok(())
    }

    pub fn add_model_tags_by_name(
        &self,
        model_id: &str,
        expected_version: u64,
        names: &[String],
    ) -> Result<Model, LibraryError> {
        let names = clean_model_tag_names(names)?;
        let tx = self.conn.unchecked_transaction()?;
        validate_active_model_version(&tx, model_id, expected_version)?;
        let mut changed = false;
        for name in names {
            let tag_id = tx
                .query_row("SELECT id FROM tags WHERE name=?1", params![name], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?;
            let tag_id = if let Some(id) = tag_id {
                id
            } else {
                let id = new_id("tag");
                tx.execute(
                    "INSERT INTO tags(id,name,category) VALUES(?1,?2,NULL)",
                    params![id, name],
                )?;
                id
            };
            changed |= tx.execute(
                "INSERT OR IGNORE INTO model_tags(model_id,tag_id) VALUES(?1,?2)",
                params![model_id, tag_id],
            )? > 0;
        }
        if changed {
            tx.execute(
                "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
                params![timestamp(), model_id],
            )?;
        }
        tx.commit()?;
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    pub fn remove_model_tags_by_name(
        &self,
        model_id: &str,
        expected_version: u64,
        names: &[String],
    ) -> Result<Model, LibraryError> {
        let names = clean_model_tag_names(names)?;
        let tx = self.conn.unchecked_transaction()?;
        validate_active_model_version(&tx, model_id, expected_version)?;
        let mut changed = false;
        for name in names {
            let tag_id = tx
                .query_row("SELECT id FROM tags WHERE name=?1", params![name], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?;
            if let Some(tag_id) = tag_id {
                changed |= tx.execute(
                    "DELETE FROM model_tags WHERE model_id=?1 AND tag_id=?2",
                    params![model_id, tag_id],
                )? > 0;
            }
        }
        if changed {
            tx.execute(
                "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
                params![timestamp(), model_id],
            )?;
        }
        tx.commit()?;
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    pub fn apply_reviewed_tags(
        &self,
        targets: &[ReviewedTagTarget],
        strategy: ReviewedTagStrategy,
    ) -> Result<Vec<Model>, LibraryError> {
        if targets.is_empty() || targets.len() > 50_000 {
            return Err(LibraryError::InvalidInput(
                "reviewed tag target count is invalid".into(),
            ));
        }
        let mut model_ids = HashSet::new();
        let mut accepted_count = 0_usize;
        let mut validated = Vec::with_capacity(targets.len());
        for target in targets {
            accepted_count = accepted_count
                .checked_add(target.tags.len())
                .ok_or_else(|| LibraryError::InvalidInput("reviewed tag count overflow".into()))?;
            if !model_ids.insert(target.model_id.clone())
                || target.tags.len() > 200
                || accepted_count > 100_000
            {
                return Err(LibraryError::InvalidInput(
                    "reviewed tag target is duplicated or too large".into(),
                ));
            }
            let mut names = HashSet::new();
            let mut tags = Vec::with_capacity(target.tags.len());
            for tag in &target.tags {
                let name = clean_tag_name(&tag.name)?;
                let category = clean_optional(tag.category.as_deref(), 120)?;
                if !names.insert(name.to_lowercase()) {
                    return Err(LibraryError::InvalidInput(
                        "reviewed tags contain duplicate names".into(),
                    ));
                }
                tags.push((name, category));
            }
            validated.push((target, tags));
        }

        let tx = self.conn.unchecked_transaction()?;
        for (target, _) in &validated {
            validate_active_model_version(&tx, &target.model_id, target.expected_version)?;
        }
        for (target, tags) in &validated {
            let mut accepted = BTreeMap::<String, Option<String>>::new();
            let mut represented_categories = BTreeSet::<String>::new();
            for (name, requested_category) in tags {
                let existing: Option<(String, Option<String>)> = tx
                    .query_row(
                        "SELECT id,category FROM tags WHERE lower(name)=lower(?1)",
                        params![name],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                let (id, actual_category) = if let Some((id, actual_category)) = existing {
                    if requested_category.is_some() && requested_category != &actual_category {
                        return Err(LibraryError::InvalidInput(format!(
                            "reviewed tag category conflicts with existing tag: {name}"
                        )));
                    }
                    (id, actual_category)
                } else {
                    let id = new_id("tag");
                    tx.execute(
                        "INSERT INTO tags(id,name,category) VALUES(?1,?2,?3)",
                        params![id, name, requested_category],
                    )?;
                    (id, requested_category.clone())
                };
                if let Some(category) = requested_category {
                    represented_categories.insert(category.clone());
                }
                accepted.insert(id, actual_category);
            }

            let mut current = BTreeMap::<String, Option<String>>::new();
            {
                let mut stmt = tx.prepare(
                    "SELECT t.id,t.category FROM tags t JOIN model_tags mt ON mt.tag_id=t.id
                     WHERE mt.model_id=?1",
                )?;
                let rows = stmt.query_map(params![target.model_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })?;
                current.extend(rows.collect::<Result<Vec<_>, _>>()?);
            }
            let final_tags = match strategy {
                ReviewedTagStrategy::Append => {
                    let mut result = current.clone();
                    result.extend(accepted);
                    result
                }
                ReviewedTagStrategy::Replace => accepted,
                ReviewedTagStrategy::Merge => {
                    let mut result = current
                        .iter()
                        .filter(|(_, category)| {
                            category
                                .as_ref()
                                .is_none_or(|category| !represented_categories.contains(category))
                        })
                        .map(|(id, category)| (id.clone(), category.clone()))
                        .collect::<BTreeMap<_, _>>();
                    result.extend(accepted);
                    result
                }
            };
            if final_tags != current {
                tx.execute(
                    "DELETE FROM model_tags WHERE model_id=?1",
                    params![target.model_id],
                )?;
                for id in final_tags.keys() {
                    tx.execute(
                        "INSERT INTO model_tags(model_id,tag_id) VALUES(?1,?2)",
                        params![target.model_id, id],
                    )?;
                }
                tx.execute(
                    "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
                    params![timestamp(),target.model_id],
                )?;
            }
        }
        tx.commit()?;
        targets
            .iter()
            .map(|target| {
                self.get_model(&target.model_id)?
                    .ok_or_else(|| LibraryError::ModelNotFound(target.model_id.clone()))
            })
            .collect()
    }

    pub fn update_model_metadata(
        &self,
        model_id: &str,
        expected_version: u64,
        patch: &MetadataPatch,
    ) -> Result<Model, LibraryError> {
        self.update_models_metadata(&[VersionedModelUpdate {
            model_id: model_id.into(),
            expected_version,
            patch: patch.clone(),
            tags: None,
            filament_ids: None,
        }])?
        .into_iter()
        .next()
        .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    /// Renames one designer, license, or parent-model name atomically. Parent links
    /// keep their stable model IDs when their referenced display name changes.
    pub fn rename_metadata_value(
        &self,
        kind: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<u64, LibraryError> {
        let (old_name, new_name, count_sql, update_sql) = match kind {
            "designer" | "license" => {
                let old_name = clean_optional(Some(old_name), 200)?
                    .ok_or_else(|| LibraryError::InvalidInput("oldName is required".into()))?;
                let new_name = clean_optional(Some(new_name), 200)?
                    .ok_or_else(|| LibraryError::InvalidInput("newName is required".into()))?;
                let column = if kind == "designer" {
                    "designer"
                } else {
                    "model_license"
                };
                (
                    old_name,
                    new_name,
                    format!("SELECT COUNT(*) FROM models WHERE deleted_at IS NULL AND {column}=?1"),
                    format!(
                        "UPDATE models SET {column}=?1,metadata_version=metadata_version+1,updated_at=?2 WHERE deleted_at IS NULL AND {column}=?3"
                    ),
                )
            }
            "parentModel" => {
                let old_name = clean_name(old_name, "Parent model")?;
                let new_name = clean_name(new_name, "Parent model")?;
                if old_name.len() > 200 || new_name.len() > 200 {
                    return Err(LibraryError::InvalidInput(
                        "Parent model name exceeds 200 characters".into(),
                    ));
                }
                (
                    old_name,
                    new_name,
                    "SELECT COUNT(*) FROM models WHERE deleted_at IS NULL AND display_name=?1".into(),
                    "UPDATE models SET display_name=?1,metadata_version=metadata_version+1,updated_at=?2 WHERE deleted_at IS NULL AND display_name=?3".into(),
                )
            }
            _ => {
                return Err(LibraryError::InvalidInput(
                    "Metadata kind must be designer, parentModel, or license".into(),
                ));
            }
        };
        if old_name == new_name {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        let count: i64 = tx.query_row(&count_sql, params![old_name], |row| row.get(0))?;
        if count > MAX_GLOBAL_METADATA_TARGETS {
            return Err(LibraryError::InvalidInput(
                "Global metadata rename exceeds 50000 models".into(),
            ));
        }
        let changed = tx.execute(&update_sql, params![new_name, timestamp(), old_name])?;
        tx.commit()?;
        Ok(changed as u64)
    }

    /// Clears one designer, license, or parent-model reference atomically.
    pub fn delete_metadata_value(&self, kind: &str, name: &str) -> Result<u64, LibraryError> {
        let name = match kind {
            "designer" | "license" => clean_optional(Some(name), 200)?
                .ok_or_else(|| LibraryError::InvalidInput("name is required".into()))?,
            "parentModel" => clean_name(name, "Parent model")?,
            _ => {
                return Err(LibraryError::InvalidInput(
                    "Metadata kind must be designer, parentModel, or license".into(),
                ));
            }
        };
        let (count_sql, update_sql) = match kind {
            "designer" => (
                "SELECT COUNT(*) FROM models WHERE deleted_at IS NULL AND designer=?1",
                "UPDATE models SET designer=NULL,metadata_version=metadata_version+1,updated_at=?1 WHERE deleted_at IS NULL AND designer=?2",
            ),
            "license" => (
                "SELECT COUNT(*) FROM models WHERE deleted_at IS NULL AND model_license=?1",
                "UPDATE models SET model_license=NULL,metadata_version=metadata_version+1,updated_at=?1 WHERE deleted_at IS NULL AND model_license=?2",
            ),
            "parentModel" => (
                "SELECT COUNT(*) FROM models WHERE deleted_at IS NULL AND parent_model_id IN
                   (SELECT id FROM models WHERE deleted_at IS NULL AND display_name=?1)",
                "UPDATE models SET parent_model_id=NULL,metadata_version=metadata_version+1,updated_at=?1
                 WHERE deleted_at IS NULL AND parent_model_id IN
                   (SELECT id FROM models WHERE deleted_at IS NULL AND display_name=?2)",
            ),
            _ => unreachable!("Metadata kind was validated above"),
        };
        let tx = self.conn.unchecked_transaction()?;
        let count: i64 = tx.query_row(count_sql, params![name], |row| row.get(0))?;
        if count > MAX_GLOBAL_METADATA_TARGETS {
            return Err(LibraryError::InvalidInput(
                "Global metadata delete exceeds 50000 models".into(),
            ));
        }
        let changed = tx.execute(update_sql, params![timestamp(), name])?;
        tx.commit()?;
        Ok(changed as u64)
    }

    pub fn update_models_metadata(
        &self,
        updates: &[VersionedModelUpdate],
    ) -> Result<Vec<Model>, LibraryError> {
        let mut unique = HashSet::new();
        for update in updates {
            if !unique.insert(update.model_id.clone()) {
                return Err(LibraryError::InvalidInput(
                    "duplicate model in metadata batch".into(),
                ));
            }
            validate_metadata_patch(&update.patch)?;
            if let Some(tags) = &update.tags {
                if !tags.is_empty() {
                    clean_model_tag_names(tags)?;
                } else if tags.len() > 200 {
                    return Err(LibraryError::InvalidInput("too many tags".into()));
                }
            }
            if let Some(filament_ids) = &update.filament_ids {
                validate_filament_ids(filament_ids)?;
            }
        }
        let tx = self.conn.unchecked_transaction()?;
        let filament_ids = updates
            .iter()
            .filter_map(|update| update.filament_ids.as_ref())
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        validate_catalog_filament_ids(&tx, &filament_ids)?;
        for update in updates {
            apply_metadata_patch(
                &tx,
                &update.model_id,
                update.expected_version,
                &update.patch,
            )?;
            if let Some(tags) = &update.tags {
                let names = if tags.is_empty() {
                    Vec::new()
                } else {
                    clean_model_tag_names(tags)?
                };
                replace_model_tags_by_name_tx(&tx, &update.model_id, &names)?;
            }
            if let Some(filament_ids) = &update.filament_ids {
                replace_model_filaments_tx(&tx, &update.model_id, filament_ids)?;
            }
        }
        tx.commit()?;
        updates
            .iter()
            .map(|u| {
                self.get_model(&u.model_id)?
                    .ok_or_else(|| LibraryError::ModelNotFound(u.model_id.clone()))
            })
            .collect()
    }
    pub fn bulk_update_metadata(
        &self,
        targets: &[VersionedTarget],
        patch: &MetadataPatch,
    ) -> Result<Vec<Model>, LibraryError> {
        if targets.len() > 50_000 {
            return Err(LibraryError::InvalidInput(
                "metadata batch exceeds 50000 models".into(),
            ));
        }
        validate_metadata_patch(patch)?;
        let mut unique = HashSet::new();
        if targets.iter().any(|t| !unique.insert(t.model_id.clone())) {
            return Err(LibraryError::InvalidInput(
                "duplicate model in metadata batch".into(),
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        for target in targets {
            apply_metadata_patch(&tx, &target.model_id, target.expected_version, patch)?;
        }
        tx.commit()?;
        targets
            .iter()
            .map(|t| {
                self.get_model(&t.model_id)?
                    .ok_or_else(|| LibraryError::ModelNotFound(t.model_id.clone()))
            })
            .collect()
    }

    pub fn create_part_stock(&self, input: &PartStockInput) -> Result<PartStock, LibraryError> {
        let input = normalize_part_stock_input(input)?;
        let id = new_id("part");
        let now = timestamp();
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM parts_stock", [], |row| row.get(0))?;
        if count >= MAX_PART_STOCK_RECORDS as i64 {
            return Err(LibraryError::InvalidInput(
                "parts stock exceeds its record limit".into(),
            ));
        }
        tx.execute(
            "INSERT INTO parts_stock
             (id,metadata_version,name,category,quantity,unit,notes,low_stock,created_at,updated_at)
             VALUES(?1,1,?2,?3,?4,?5,?6,?7,?8,?8)",
            params![
                id,
                input.name,
                input.category,
                input.quantity,
                input.unit,
                input.notes,
                input.low_stock,
                now,
            ],
        )?;
        tx.commit()?;
        self.get_part_stock(&id)?
            .ok_or_else(|| LibraryError::InvalidInput("created part is missing".into()))
    }

    pub fn get_part_stock(&self, id: &str) -> Result<Option<PartStock>, LibraryError> {
        validate_record_id(id, "part ID")?;
        self.conn
            .query_row(
                &format!("SELECT {PART_STOCK_SELECT_COLUMNS} FROM parts_stock WHERE id=?1"),
                params![id],
                part_stock_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_part_stock(&self) -> Result<Vec<PartStock>, LibraryError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {PART_STOCK_SELECT_COLUMNS} FROM parts_stock ORDER BY name COLLATE NOCASE,id"
        ))?;
        let rows = statement.query_map([], part_stock_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn update_part_stock(
        &self,
        id: &str,
        expected_version: u64,
        input: &PartStockInput,
    ) -> Result<PartStock, LibraryError> {
        validate_record_id(id, "part ID")?;
        let input = normalize_part_stock_input(input)?;
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let actual_version: i64 = tx
            .query_row(
                "SELECT metadata_version FROM parts_stock WHERE id=?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("part not found".into()))?;
        ensure_part_stock_version(id, expected_version, actual_version)?;
        tx.execute(
            "UPDATE parts_stock SET metadata_version=metadata_version+1,name=?1,category=?2,
             quantity=?3,unit=?4,notes=?5,low_stock=?6,updated_at=?7 WHERE id=?8",
            params![
                input.name,
                input.category,
                input.quantity,
                input.unit,
                input.notes,
                input.low_stock,
                timestamp(),
                id,
            ],
        )?;
        tx.commit()?;
        self.get_part_stock(id)?
            .ok_or_else(|| LibraryError::InvalidInput("updated part is missing".into()))
    }

    pub fn delete_part_stock(&self, id: &str, expected_version: u64) -> Result<(), LibraryError> {
        validate_record_id(id, "part ID")?;
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let actual_version: i64 = tx
            .query_row(
                "SELECT metadata_version FROM parts_stock WHERE id=?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("part not found".into()))?;
        ensure_part_stock_version(id, expected_version, actual_version)?;
        // Event rows keep the part ID and name snapshot; deleting a part does
        // not restore stock, and later event deletion cannot credit a new part.
        tx.execute("DELETE FROM parts_stock WHERE id=?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Creates a user-managed printer. Approval is a separate explicit input
    /// from the local Add/Edit flow; compatibility imports always pass false.
    pub fn create_printer(
        &self,
        input: &PrinterInput,
        endpoint_approved: bool,
    ) -> Result<Printer, LibraryError> {
        let input = normalize_printer_input(input)?;
        validate_endpoint_approval(&input, endpoint_approved)?;
        let id = new_id("printer");
        let now = timestamp();
        let (bed_x, bed_y, bed_z) = printer_bed_columns(input.bed_mm);
        let tx = self.conn.unchecked_transaction()?;
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM printers", [], |row| row.get(0))?;
        if count >= MAX_PRINTERS as i64 {
            return Err(LibraryError::InvalidInput(
                "printer catalog exceeds its record limit".into(),
            ));
        }
        tx.execute(
            "INSERT INTO printers (
                id,metadata_version,nickname,make,model,printer_type,firmware,host,web_url,
                endpoint_approved,bed_x_mm,bed_y_mm,bed_z_mm,notes,source_kind,
                connection_status,job_status,last_checked_at,connection_error_code,created_at,updated_at
             ) VALUES (?1,1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'manual',
                      'unknown','unknown',NULL,NULL,?14,?14)",
            params![
                id,
                input.nickname,
                input.make,
                input.model,
                input.printer_type,
                input.firmware,
                input.host,
                input.web_url,
                endpoint_approved,
                bed_x,
                bed_y,
                bed_z,
                input.notes,
                now,
            ],
        )?;
        tx.commit()?;
        self.get_printer(&id)?
            .ok_or_else(|| LibraryError::InvalidInput("created printer is missing".into()))
    }

    /// Saves a reviewed set of discovery suggestions in one transaction.
    /// Endpoint approval remains an explicit per-printer choice.
    pub fn create_discovered_printers(
        &self,
        items: &[(PrinterInput, bool)],
    ) -> Result<Vec<Printer>, LibraryError> {
        if items.is_empty() || items.len() > MAX_DISCOVERED_PRINTER_BATCH {
            return Err(LibraryError::InvalidInput(
                "discovered printer batch must contain 1 to 32 records".into(),
            ));
        }

        let now = timestamp();
        let mut prepared = Vec::with_capacity(items.len());
        for (input, endpoint_approved) in items {
            let input = normalize_printer_input(input)?;
            validate_endpoint_approval(&input, *endpoint_approved)?;
            let bed = printer_bed_columns(input.bed_mm);
            prepared.push((new_id("printer"), input, *endpoint_approved, bed));
        }
        let ids = prepared
            .iter()
            .map(|(id, _, _, _)| id.clone())
            .collect::<Vec<_>>();

        let tx = self.conn.unchecked_transaction()?;
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM printers", [], |row| row.get(0))?;
        if count + items.len() as i64 > MAX_PRINTERS as i64 {
            return Err(LibraryError::InvalidInput(
                "printer catalog exceeds its record limit".into(),
            ));
        }
        for (id, input, endpoint_approved, (bed_x, bed_y, bed_z)) in prepared {
            tx.execute(
                "INSERT INTO printers (
                    id,metadata_version,nickname,make,model,printer_type,firmware,host,web_url,
                    endpoint_approved,bed_x_mm,bed_y_mm,bed_z_mm,notes,source_kind,
                    connection_status,job_status,last_checked_at,connection_error_code,created_at,updated_at
                 ) VALUES (?1,1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'discovered',
                          'unknown','unknown',NULL,NULL,?14,?14)",
                params![
                    id,
                    input.nickname,
                    input.make,
                    input.model,
                    input.printer_type,
                    input.firmware,
                    input.host,
                    input.web_url,
                    endpoint_approved,
                    bed_x,
                    bed_y,
                    bed_z,
                    input.notes,
                    now,
                ],
            )?;
        }
        tx.commit()?;
        ids.iter()
            .map(|id| {
                self.get_printer(id)?
                    .ok_or_else(|| LibraryError::InvalidInput("created printer is missing".into()))
            })
            .collect()
    }

    /// Updates editable printer metadata using a version check. Omitting the
    /// approval change preserves the existing confirmation state.
    pub fn update_printer(
        &self,
        id: &str,
        expected_version: u64,
        input: &PrinterInput,
        endpoint_approved: Option<bool>,
    ) -> Result<Printer, LibraryError> {
        validate_record_id(id, "printer ID")?;
        let input = normalize_printer_input(input)?;
        let (bed_x, bed_y, bed_z) = printer_bed_columns(input.bed_mm);
        let tx = self.conn.unchecked_transaction()?;
        let (actual_version, prior_approval, prior_host, prior_web_url): (
            i64,
            bool,
            Option<String>,
            Option<String>,
        ) = tx
            .query_row(
                "SELECT metadata_version,endpoint_approved,host,web_url FROM printers WHERE id=?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("printer not found".into()))?;
        ensure_record_version("printer", expected_version, actual_version)?;
        let same_endpoint = prior_host == input.host && prior_web_url == input.web_url;
        let approved = endpoint_approved.unwrap_or(prior_approval && same_endpoint);
        validate_endpoint_approval(&input, approved)?;
        let reset_status = !same_endpoint || !approved;
        tx.execute(
            "UPDATE printers SET
                metadata_version=metadata_version+1,nickname=?1,make=?2,model=?3,
                printer_type=?4,firmware=?5,host=?6,web_url=?7,endpoint_approved=?8,
                bed_x_mm=?9,bed_y_mm=?10,bed_z_mm=?11,notes=?12,
                connection_status=CASE WHEN ?13 THEN 'unknown' ELSE connection_status END,
                job_status=CASE WHEN ?13 THEN 'unknown' ELSE job_status END,
                last_checked_at=CASE WHEN ?13 THEN NULL ELSE last_checked_at END,
                connection_error_code=CASE WHEN ?13 THEN NULL ELSE connection_error_code END,
                updated_at=?14 WHERE id=?15",
            params![
                input.nickname,
                input.make,
                input.model,
                input.printer_type,
                input.firmware,
                input.host,
                input.web_url,
                approved,
                bed_x,
                bed_y,
                bed_z,
                input.notes,
                reset_status,
                timestamp(),
                id,
            ],
        )?;
        tx.commit()?;
        self.get_printer(id)?
            .ok_or_else(|| LibraryError::InvalidInput("updated printer is missing".into()))
    }

    pub fn get_printer(&self, id: &str) -> Result<Option<Printer>, LibraryError> {
        validate_record_id(id, "printer ID")?;
        let query = format!("SELECT {PRINTER_SELECT_COLUMNS} FROM printers p WHERE p.id=?1");
        self.conn
            .query_row(&query, params![id], printer_from_row)
            .optional()
            .map_err(Into::into)
    }

    pub fn list_printers(&self) -> Result<Vec<Printer>, LibraryError> {
        let query = format!(
            "SELECT {PRINTER_SELECT_COLUMNS} FROM printers p ORDER BY lower(p.nickname),p.id"
        );
        let mut statement = self.conn.prepare(&query)?;
        let rows = statement.query_map([], printer_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Returns every printer profile and related history row allowed by the
    /// JSON interchange limits. UI list methods intentionally remain capped
    /// separately. A single deferred transaction holds the same SQLite read
    /// snapshot across all count checks and keyset pages.
    pub fn export_printer_data_snapshot(&self) -> Result<PrinterDataExportSnapshot, LibraryError> {
        if self.conn.is_autocommit() {
            self.with_json_export_snapshot(|db| db.export_printer_data_snapshot_in_snapshot())
        } else {
            self.export_printer_data_snapshot_in_snapshot()
        }
    }

    fn export_printer_data_snapshot_in_snapshot(
        &self,
    ) -> Result<PrinterDataExportSnapshot, LibraryError> {
        let printer_count = self
            .conn
            .query_row("SELECT COUNT(*) FROM printers", [], |row| row.get(0))?;
        let maintenance_count =
            self.conn
                .query_row("SELECT COUNT(*) FROM printer_maintenance_logs", [], |row| {
                    row.get(0)
                })?;
        let reminder_count =
            self.conn
                .query_row("SELECT COUNT(*) FROM printer_reminders", [], |row| {
                    row.get(0)
                })?;
        let printer_count =
            checked_export_count(printer_count, "printers", MAX_PRINTER_JSON_EXPORT_PRINTERS)?;
        let maintenance_count = checked_export_count(
            maintenance_count,
            "maintenance logs",
            MAX_PRINTER_JSON_EXPORT_MAINTENANCE_LOGS,
        )?;
        let reminder_count = checked_export_count(
            reminder_count,
            "maintenance reminders",
            MAX_PRINTER_JSON_EXPORT_REMINDERS,
        )?;
        let record_count = printer_count
            .checked_add(maintenance_count)
            .and_then(|count| count.checked_add(reminder_count))
            .ok_or_else(|| {
                LibraryError::InvalidInput("printer export record count overflowed".into())
            })?;
        if record_count > MAX_PRINTER_JSON_EXPORT_RECORDS {
            return Err(LibraryError::InvalidInput(
                "printer export exceeds its total record limit".into(),
            ));
        }

        let mut printers = Vec::with_capacity(printer_count);
        let mut after_id: Option<String> = None;
        loop {
            let page = {
                let mut statement = self.conn.prepare(&format!(
                    "SELECT {PRINTER_SELECT_COLUMNS} FROM printers p
                     WHERE (?1 IS NULL OR p.id>?1) ORDER BY p.id ASC LIMIT ?2"
                ))?;
                let rows = statement.query_map(
                    params![after_id.as_deref(), PRINTER_JSON_EXPORT_PAGE_ROWS as i64],
                    printer_from_row,
                )?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            if page.is_empty() {
                break;
            }
            after_id = page.last().map(|printer| printer.id.clone());
            printers.extend(page);
        }
        if printers.len() != printer_count {
            return Err(LibraryError::InvalidInput(
                "printer export snapshot did not read every counted profile".into(),
            ));
        }
        for printer in &mut printers {
            printer.web_url = normalize_printer_web_url(printer.web_url.as_deref())?;
        }

        let mut maintenance_logs = Vec::with_capacity(maintenance_count);
        after_id = None;
        loop {
            let page = {
                let mut statement = self.conn.prepare(
                    "SELECT id,printer_id,printer_nickname,title,performed_at,notes,created_at,metadata_version
                     FROM printer_maintenance_logs
                     WHERE (?1 IS NULL OR id>?1) ORDER BY id ASC LIMIT ?2",
                )?;
                let rows = statement.query_map(
                    params![after_id.as_deref(), PRINTER_JSON_EXPORT_PAGE_ROWS as i64],
                    maintenance_log_from_row,
                )?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            if page.is_empty() {
                break;
            }
            after_id = page.last().map(|log| log.id.clone());
            maintenance_logs.extend(page);
        }
        if maintenance_logs.len() != maintenance_count {
            return Err(LibraryError::InvalidInput(
                "printer export snapshot did not read every counted maintenance log".into(),
            ));
        }

        let mut reminders = Vec::with_capacity(reminder_count);
        after_id = None;
        loop {
            let page = {
                let mut statement = self.conn.prepare(
                    "SELECT id,metadata_version,printer_id,printer_nickname,title,notes,due_at,
                            interval_days,status,last_completed_at,created_at,updated_at
                     FROM printer_reminders
                     WHERE (?1 IS NULL OR id>?1) ORDER BY id ASC LIMIT ?2",
                )?;
                let rows = statement.query_map(
                    params![after_id.as_deref(), PRINTER_JSON_EXPORT_PAGE_ROWS as i64],
                    printer_reminder_from_row,
                )?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            if page.is_empty() {
                break;
            }
            after_id = page.last().map(|reminder| reminder.id.clone());
            reminders.extend(page);
        }
        if reminders.len() != reminder_count {
            return Err(LibraryError::InvalidInput(
                "printer export snapshot did not read every counted reminder".into(),
            ));
        }

        Ok(PrinterDataExportSnapshot {
            printers,
            maintenance_logs,
            reminders,
        })
    }

    /// Counts the rows permitted in the streaming printer portion of an
    /// interchange export without materializing any of those rows.
    pub fn printer_data_export_counts(&self) -> Result<PrinterDataExportCounts, LibraryError> {
        let printer_count = self
            .conn
            .query_row("SELECT COUNT(*) FROM printers", [], |row| row.get(0))?;
        let maintenance_count =
            self.conn
                .query_row("SELECT COUNT(*) FROM printer_maintenance_logs", [], |row| {
                    row.get(0)
                })?;
        let reminder_count =
            self.conn
                .query_row("SELECT COUNT(*) FROM printer_reminders", [], |row| {
                    row.get(0)
                })?;
        let counts = PrinterDataExportCounts {
            printers: checked_export_count(
                printer_count,
                "printers",
                MAX_PRINTER_JSON_EXPORT_PRINTERS,
            )?,
            maintenance_logs: checked_export_count(
                maintenance_count,
                "maintenance logs",
                MAX_PRINTER_JSON_EXPORT_MAINTENANCE_LOGS,
            )?,
            reminders: checked_export_count(
                reminder_count,
                "maintenance reminders",
                MAX_PRINTER_JSON_EXPORT_REMINDERS,
            )?,
        };
        let record_count = counts
            .printers
            .checked_add(counts.maintenance_logs)
            .and_then(|value| value.checked_add(counts.reminders))
            .ok_or_else(|| {
                LibraryError::InvalidInput("printer export record count overflowed".into())
            })?;
        if record_count > MAX_PRINTER_JSON_EXPORT_RECORDS {
            return Err(LibraryError::InvalidInput(
                "printer export exceeds its total record limit".into(),
            ));
        }
        Ok(counts)
    }

    pub fn list_printers_for_json_export_page(
        &self,
        after_id: Option<&str>,
        limit: u16,
    ) -> Result<Vec<Printer>, LibraryError> {
        if limit == 0 || limit > 100 {
            return Err(LibraryError::InvalidInput(
                "JSON export printer page request is invalid".into(),
            ));
        }
        let mut statement = self.conn.prepare(&format!(
            "SELECT {PRINTER_SELECT_COLUMNS} FROM printers p
             WHERE (?1 IS NULL OR p.id>?1) ORDER BY p.id ASC LIMIT ?2"
        ))?;
        let rows = statement.query_map(params![after_id, i64::from(limit)], printer_from_row)?;
        let mut printers = rows.collect::<Result<Vec<_>, _>>()?;
        for printer in &mut printers {
            printer.web_url = normalize_printer_web_url(printer.web_url.as_deref())?;
        }
        Ok(printers)
    }

    pub fn list_printer_export_attributions(
        &self,
    ) -> Result<Vec<PrinterExportAttribution>, LibraryError> {
        let count = self
            .conn
            .query_row("SELECT COUNT(*) FROM printers", [], |row| row.get(0))?;
        checked_export_count(count, "printers", MAX_PRINTER_JSON_EXPORT_PRINTERS)?;
        let mut statement = self
            .conn
            .prepare("SELECT id,nickname,make,model,printer_type FROM printers ORDER BY id ASC")?;
        let rows = statement.query_map([], |row| {
            Ok(PrinterExportAttribution {
                id: row.get(0)?,
                nickname: row.get(1)?,
                make: row.get(2)?,
                model: row.get(3)?,
                printer_type: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_printer_maintenance_logs_for_json_export_page(
        &self,
        after_id: Option<&str>,
        limit: u16,
    ) -> Result<Vec<PrinterMaintenanceLog>, LibraryError> {
        if limit == 0 || limit > 100 {
            return Err(LibraryError::InvalidInput(
                "JSON export maintenance page request is invalid".into(),
            ));
        }
        let mut statement = self.conn.prepare(
            "SELECT id,printer_id,printer_nickname,title,performed_at,notes,created_at,metadata_version
             FROM printer_maintenance_logs
             WHERE (?1 IS NULL OR id>?1) ORDER BY id ASC LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![after_id, i64::from(limit)],
            maintenance_log_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_printer_reminders_for_json_export_page(
        &self,
        after_id: Option<&str>,
        limit: u16,
    ) -> Result<Vec<PrinterReminder>, LibraryError> {
        if limit == 0 || limit > 100 {
            return Err(LibraryError::InvalidInput(
                "JSON export reminder page request is invalid".into(),
            ));
        }
        let mut statement = self.conn.prepare(
            "SELECT id,metadata_version,printer_id,printer_nickname,title,notes,due_at,
                    interval_days,status,last_completed_at,created_at,updated_at
             FROM printer_reminders
             WHERE (?1 IS NULL OR id>?1) ORDER BY id ASC LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![after_id, i64::from(limit)],
            printer_reminder_from_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Imports prior settings.json entries by stable ID. Existing rows are
    /// never overwritten, making repeated startup imports safe after edits.
    pub fn import_legacy_printers(
        &self,
        legacy: &[LegacyPrinter],
    ) -> Result<LegacyPrinterImportSummary, LibraryError> {
        if legacy.len() > MAX_PRINTER_IMPORT_COUNT {
            return Err(LibraryError::InvalidInput(
                "legacy printer import exceeds its record limit".into(),
            ));
        }
        let mut ids = HashSet::with_capacity(legacy.len());
        let mut normalized = Vec::with_capacity(legacy.len());
        for value in legacy {
            let id = validate_record_id(&value.id, "printer ID")?;
            if !ids.insert(id.clone()) {
                return Err(LibraryError::InvalidInput(
                    "legacy printer IDs must be unique".into(),
                ));
            }
            let input = normalize_printer_input(&PrinterInput {
                nickname: value.name.clone(),
                make: None,
                model: None,
                printer_type: None,
                firmware: None,
                host: None,
                web_url: None,
                bed_mm: Some(value.bed_mm),
                notes: String::new(),
            })?;
            normalized.push((id, input));
        }

        let tx = self.conn.unchecked_transaction()?;
        let mut summary = LegacyPrinterImportSummary::default();
        for (id, input) in normalized {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM printers WHERE id=?1)",
                params![id],
                |row| row.get(0),
            )?;
            if exists {
                summary.already_present += 1;
                continue;
            }
            let count: i64 = tx.query_row("SELECT COUNT(*) FROM printers", [], |row| row.get(0))?;
            if count >= MAX_PRINTERS as i64 {
                return Err(LibraryError::InvalidInput(
                    "printer catalog exceeds its record limit".into(),
                ));
            }
            let (bed_x, bed_y, bed_z) = printer_bed_columns(input.bed_mm);
            let now = timestamp();
            tx.execute(
                "INSERT INTO printers (
                    id,metadata_version,nickname,make,model,printer_type,firmware,host,web_url,
                    endpoint_approved,bed_x_mm,bed_y_mm,bed_z_mm,notes,source_kind,
                    connection_status,job_status,last_checked_at,connection_error_code,created_at,updated_at
                 ) VALUES (?1,1,?2,NULL,NULL,NULL,NULL,NULL,NULL,0,?3,?4,?5,'','manual',
                          'unknown','unknown',NULL,NULL,?6,?6)",
                params![id, input.nickname, bed_x, bed_y, bed_z, now],
            )?;
            summary.inserted += 1;
        }
        tx.commit()?;
        Ok(summary)
    }

    /// Imports printer profiles and their history atomically. External refs
    /// only connect rows inside this batch; all persisted IDs are generated
    /// locally or selected from existing local printers.
    pub fn import_printer_data(
        &self,
        batch: &PrinterDataImport,
    ) -> Result<PrinterDataImportResult, LibraryError> {
        if batch.printers.len() > MAX_PRINTER_DATA_IMPORT_PRINTERS
            || batch.maintenance_logs.len() > MAX_PRINTER_JSON_EXPORT_MAINTENANCE_LOGS
            || batch.reminders.len() > MAX_PRINTER_JSON_EXPORT_REMINDERS
        {
            return Err(LibraryError::InvalidInput(
                "printer data import exceeds a record limit".into(),
            ));
        }
        let total_records = batch
            .printers
            .len()
            .checked_add(batch.maintenance_logs.len())
            .and_then(|total| total.checked_add(batch.reminders.len()));
        if total_records.is_none_or(|total| total > MAX_PRINTER_DATA_IMPORT_RECORDS) {
            return Err(LibraryError::InvalidInput(
                "printer data import exceeds its total record limit".into(),
            ));
        }

        struct PreparedPrinter {
            external_ref: String,
            map_to_local_printer_id: Option<String>,
            created_at: String,
            updated_at: String,
            input: PrinterInput,
        }
        struct PreparedMaintenanceLog {
            printer_external_ref: Option<String>,
            printer_nickname: Option<String>,
            input: PrinterMaintenanceInput,
            created_at: String,
        }
        struct PreparedReminder {
            printer_external_ref: Option<String>,
            printer_nickname: Option<String>,
            input: PrinterReminderInput,
            status: PrinterReminderStatus,
            last_completed_at: Option<String>,
            created_at: String,
            updated_at: String,
        }

        let import_timestamp = timestamp();
        let mut prepared_printers = Vec::with_capacity(batch.printers.len());
        let mut printer_refs = HashSet::with_capacity(batch.printers.len());
        for record in &batch.printers {
            let external_ref =
                validate_import_external_ref(&record.external_ref, "printer external reference")?;
            if !printer_refs.insert(external_ref.clone()) {
                return Err(LibraryError::InvalidInput(
                    "printer external references must be unique".into(),
                ));
            }
            let map_to_local_printer_id = record
                .map_to_local_printer_id
                .as_deref()
                .map(|id| validate_record_id(id, "local printer ID"))
                .transpose()?;
            prepared_printers.push(PreparedPrinter {
                external_ref,
                map_to_local_printer_id,
                created_at: record
                    .created_at
                    .as_deref()
                    .map(|value| validate_import_timestamp(value, "printer created_at"))
                    .transpose()?
                    .unwrap_or_else(|| import_timestamp.clone()),
                updated_at: record
                    .updated_at
                    .as_deref()
                    .map(|value| validate_import_timestamp(value, "printer updated_at"))
                    .transpose()?
                    .unwrap_or_else(|| import_timestamp.clone()),
                input: normalize_printer_input(&record.input)?,
            });
        }

        let mut prepared_logs = Vec::with_capacity(batch.maintenance_logs.len());
        let mut maintenance_refs = HashSet::with_capacity(batch.maintenance_logs.len());
        for record in &batch.maintenance_logs {
            let external_ref = validate_import_external_ref(
                &record.external_ref,
                "maintenance external reference",
            )?;
            if !maintenance_refs.insert(external_ref) {
                return Err(LibraryError::InvalidInput(
                    "maintenance log external references must be unique".into(),
                ));
            }
            let printer_external_ref = record
                .printer_external_ref
                .as_deref()
                .map(|value| {
                    validate_import_external_ref(value, "maintenance printer external reference")
                })
                .transpose()?;
            prepared_logs.push(PreparedMaintenanceLog {
                printer_external_ref,
                printer_nickname: validate_optional_import_snapshot_name(
                    record.printer_nickname.as_deref(),
                )?,
                input: normalize_maintenance_input(&record.input)?,
                created_at: record
                    .created_at
                    .as_deref()
                    .map(|value| validate_import_timestamp(value, "maintenance created_at"))
                    .transpose()?
                    .unwrap_or_else(|| import_timestamp.clone()),
            });
        }

        let mut prepared_reminders = Vec::with_capacity(batch.reminders.len());
        let mut reminder_refs = HashSet::with_capacity(batch.reminders.len());
        for record in &batch.reminders {
            let external_ref =
                validate_import_external_ref(&record.external_ref, "reminder external reference")?;
            if !reminder_refs.insert(external_ref) {
                return Err(LibraryError::InvalidInput(
                    "reminder external references must be unique".into(),
                ));
            }
            let printer_external_ref = record
                .printer_external_ref
                .as_deref()
                .map(|value| {
                    validate_import_external_ref(value, "reminder printer external reference")
                })
                .transpose()?;
            prepared_reminders.push(PreparedReminder {
                printer_external_ref,
                printer_nickname: validate_optional_import_snapshot_name(
                    record.printer_nickname.as_deref(),
                )?,
                input: normalize_printer_reminder_input(&record.input)?,
                status: record.status,
                last_completed_at: record
                    .last_completed_at
                    .as_deref()
                    .map(|value| normalize_rfc3339_field(value, "last_completed_at"))
                    .transpose()?,
                created_at: validate_import_timestamp(&record.created_at, "reminder created_at")?,
                updated_at: validate_import_timestamp(&record.updated_at, "reminder updated_at")?,
            });
        }

        let tx = self.conn.unchecked_transaction()?;
        let maintenance_count =
            tx.query_row("SELECT COUNT(*) FROM printer_maintenance_logs", [], |row| {
                row.get(0)
            })?;
        ensure_printer_history_capacity(
            maintenance_count,
            prepared_logs.len(),
            "maintenance log",
            MAX_PRINTER_JSON_EXPORT_MAINTENANCE_LOGS,
        )?;
        let reminder_count = tx.query_row("SELECT COUNT(*) FROM printer_reminders", [], |row| {
            row.get(0)
        })?;
        ensure_printer_history_capacity(
            reminder_count,
            prepared_reminders.len(),
            "reminder",
            MAX_PRINTER_JSON_EXPORT_REMINDERS,
        )?;
        let mut local_id_by_printer_ref = HashMap::with_capacity(prepared_printers.len());
        let mut new_printers = Vec::new();
        let mut result = PrinterDataImportResult::default();
        for printer in prepared_printers {
            let local_printer_id = if let Some(id) = printer.map_to_local_printer_id {
                let existing_id: Option<String> = tx
                    .query_row("SELECT id FROM printers WHERE id=?1", params![id], |row| {
                        row.get(0)
                    })
                    .optional()?;
                let existing_id = existing_id.ok_or_else(|| {
                    LibraryError::InvalidInput("mapped local printer was not found".into())
                })?;
                result.mapped_printer_count += 1;
                existing_id
            } else {
                let id = new_id("printer");
                new_printers.push((
                    id.clone(),
                    printer.input,
                    printer.created_at,
                    printer.updated_at,
                ));
                result.imported_printer_count += 1;
                id
            };
            local_id_by_printer_ref.insert(printer.external_ref.clone(), local_printer_id.clone());
            result.printer_ref_map.push(PrinterImportRefMapEntry {
                external_ref: printer.external_ref,
                local_printer_id,
            });
        }

        let printer_count: i64 =
            tx.query_row("SELECT COUNT(*) FROM printers", [], |row| row.get(0))?;
        if printer_count + new_printers.len() as i64 > MAX_PRINTERS as i64 {
            return Err(LibraryError::InvalidInput(
                "printer catalog exceeds its record limit".into(),
            ));
        }
        for (id, input, created_at, updated_at) in new_printers {
            let (bed_x, bed_y, bed_z) = printer_bed_columns(input.bed_mm);
            tx.execute(
                "INSERT INTO printers (
                    id,metadata_version,nickname,make,model,printer_type,firmware,host,web_url,
                    endpoint_approved,bed_x_mm,bed_y_mm,bed_z_mm,notes,source_kind,
                    connection_status,job_status,last_checked_at,connection_error_code,created_at,updated_at
                 ) VALUES (?1,1,?2,?3,?4,?5,?6,?7,?8,0,?9,?10,?11,?12,'imported',
                          'unknown','unknown',NULL,NULL,?13,?14)",
                params![
                    id,
                    input.nickname,
                    input.make,
                    input.model,
                    input.printer_type,
                    input.firmware,
                    input.host,
                    input.web_url,
                    bed_x,
                    bed_y,
                    bed_z,
                    input.notes,
                    created_at,
                    updated_at,
                ],
            )?;
        }

        for log in prepared_logs {
            let printer_id = log
                .printer_external_ref
                .as_deref()
                .and_then(|external_ref| local_id_by_printer_ref.get(external_ref));
            let printer_nickname = match log.printer_nickname {
                Some(snapshot) => snapshot,
                None => match printer_id {
                    Some(printer_id) => tx
                        .query_row(
                            "SELECT nickname FROM printers WHERE id=?1",
                            params![printer_id],
                            |row| row.get(0),
                        )
                        .optional()?
                        .ok_or_else(|| LibraryError::InvalidInput("printer not found".into()))?,
                    None => String::new(),
                },
            };
            tx.execute(
                "INSERT INTO printer_maintenance_logs
                 (id,printer_id,printer_nickname,title,performed_at,notes,created_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    new_id("maintenance"),
                    printer_id,
                    printer_nickname,
                    log.input.title,
                    log.input.performed_at,
                    log.input.notes,
                    log.created_at,
                ],
            )?;
            result.imported_maintenance_log_count += 1;
        }

        for reminder in prepared_reminders {
            let printer_id = reminder
                .printer_external_ref
                .as_deref()
                .and_then(|external_ref| local_id_by_printer_ref.get(external_ref));
            let printer_nickname = match reminder.printer_nickname {
                Some(snapshot) => snapshot,
                None => match printer_id {
                    Some(printer_id) => tx
                        .query_row(
                            "SELECT nickname FROM printers WHERE id=?1",
                            params![printer_id],
                            |row| row.get(0),
                        )
                        .optional()?
                        .ok_or_else(|| LibraryError::InvalidInput("printer not found".into()))?,
                    None => String::new(),
                },
            };
            tx.execute(
                "INSERT INTO printer_reminders
                 (id,metadata_version,printer_id,printer_nickname,title,notes,due_at,interval_days,
                  status,last_completed_at,created_at,updated_at)
                 VALUES(?1,1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    new_id("reminder"),
                    printer_id,
                    printer_nickname,
                    reminder.input.title,
                    reminder.input.notes,
                    reminder.input.due_at,
                    reminder.input.interval_days,
                    printer_reminder_status_value(&reminder.status),
                    reminder.last_completed_at,
                    reminder.created_at,
                    reminder.updated_at,
                ],
            )?;
            result.imported_reminder_count += 1;
        }

        tx.commit()?;
        Ok(result)
    }

    /// Source provenance is not part of ordinary editable metadata. Changing
    /// it requires this explicit operation so imports cannot impersonate
    /// discovered devices.
    pub fn set_printer_source_kind(
        &self,
        id: &str,
        expected_version: u64,
        source_kind: PrinterSourceKind,
    ) -> Result<Printer, LibraryError> {
        validate_record_id(id, "printer ID")?;
        let tx = self.conn.unchecked_transaction()?;
        let actual: i64 = tx
            .query_row(
                "SELECT metadata_version FROM printers WHERE id=?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("printer not found".into()))?;
        ensure_record_version("printer", expected_version, actual)?;
        tx.execute(
            "UPDATE printers SET source_kind=?1,metadata_version=metadata_version+1,updated_at=?2 WHERE id=?3",
            params![printer_source_kind_value(&source_kind), timestamp(), id],
        )?;
        tx.commit()?;
        self.get_printer(id)?
            .ok_or_else(|| LibraryError::InvalidInput("updated printer is missing".into()))
    }

    /// Persists only the bounded state code returned by an explicit local
    /// connection check. Raw device responses and credentials are not stored.
    pub fn record_printer_status(
        &self,
        id: &str,
        connection_status: PrinterConnectionStatus,
        job_status: PrinterJobStatus,
        last_checked_at: &str,
        connection_error_code: Option<&str>,
    ) -> Result<Printer, LibraryError> {
        validate_record_id(id, "printer ID")?;
        let expected = self
            .get_printer(id)?
            .ok_or_else(|| LibraryError::InvalidInput("printer not found".into()))?;
        self.record_printer_status_if_current(
            &expected,
            connection_status,
            job_status,
            last_checked_at,
            connection_error_code,
        )
    }

    /// Commits a check only if the record and approved endpoint still match
    /// the snapshot used for the network request.
    pub fn record_printer_status_if_current(
        &self,
        expected: &Printer,
        connection_status: PrinterConnectionStatus,
        job_status: PrinterJobStatus,
        last_checked_at: &str,
        connection_error_code: Option<&str>,
    ) -> Result<Printer, LibraryError> {
        validate_record_id(&expected.id, "printer ID")?;
        let checked_at = normalize_rfc3339_field(last_checked_at, "last_checked_at")?;
        validate_printer_status(connection_status, job_status, connection_error_code)?;
        if !expected.endpoint_approved {
            return Err(LibraryError::InvalidInput(
                "printer endpoint requires explicit local approval before checking".into(),
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        let current: Option<(i64, bool, Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT metadata_version,endpoint_approved,host,web_url FROM printers WHERE id=?1",
                params![expected.id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let (actual_version, approved, host, web_url) =
            current.ok_or_else(|| LibraryError::InvalidInput("printer not found".into()))?;
        ensure_record_version("printer", expected.metadata_version, actual_version)?;
        if host.as_deref() != expected.host.as_deref()
            || web_url.as_deref() != expected.web_url.as_deref()
        {
            return Err(LibraryError::InvalidInput(
                "printer endpoint changed during the connection check".into(),
            ));
        }
        let has_endpoint = host.is_some() || web_url.is_some();
        if !approved || !has_endpoint {
            return Err(LibraryError::InvalidInput(
                "printer endpoint requires explicit local approval before checking".into(),
            ));
        }
        tx.execute(
            "UPDATE printers SET connection_status=?1,job_status=?2,last_checked_at=?3,
                connection_error_code=?4,metadata_version=metadata_version+1,updated_at=?5
             WHERE id=?6",
            params![
                printer_connection_status_value(&connection_status),
                printer_job_status_value(&job_status),
                checked_at,
                connection_error_code,
                timestamp(),
                expected.id.as_str(),
            ],
        )?;
        tx.commit()?;
        self.get_printer(&expected.id)?
            .ok_or_else(|| LibraryError::InvalidInput("updated printer is missing".into()))
    }

    pub fn delete_printer(&self, id: &str, expected_version: u64) -> Result<(), LibraryError> {
        validate_record_id(id, "printer ID")?;
        let tx = self.conn.unchecked_transaction()?;
        let actual: i64 = tx
            .query_row(
                "SELECT metadata_version FROM printers WHERE id=?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("printer not found".into()))?;
        ensure_record_version("printer", expected_version, actual)?;
        let now = timestamp();
        tx.execute(
            "UPDATE printer_reminders SET printer_id=NULL,status='paused',
                metadata_version=metadata_version+1,updated_at=?1 WHERE printer_id=?2",
            params![now, id],
        )?;
        tx.execute(
            "UPDATE printer_maintenance_logs
             SET printer_id=NULL,metadata_version=metadata_version+1 WHERE printer_id=?1",
            params![id],
        )?;
        tx.execute("DELETE FROM printers WHERE id=?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn create_printer_maintenance_log(
        &self,
        printer_id: &str,
        input: &PrinterMaintenanceInput,
    ) -> Result<PrinterMaintenanceLog, LibraryError> {
        validate_record_id(printer_id, "printer ID")?;
        let input = normalize_maintenance_input(input)?;
        let id = new_id("maintenance");
        let created_at = timestamp();
        let tx = self.conn.unchecked_transaction()?;
        let printer_nickname: String = tx
            .query_row(
                "SELECT nickname FROM printers WHERE id=?1",
                params![printer_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("printer not found".into()))?;
        let maintenance_count =
            tx.query_row("SELECT COUNT(*) FROM printer_maintenance_logs", [], |row| {
                row.get(0)
            })?;
        ensure_printer_history_capacity(
            maintenance_count,
            1,
            "maintenance log",
            MAX_PRINTER_JSON_EXPORT_MAINTENANCE_LOGS,
        )?;
        tx.execute(
            "INSERT INTO printer_maintenance_logs
             (id,printer_id,printer_nickname,title,performed_at,notes,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                id,
                printer_id,
                printer_nickname,
                input.title,
                input.performed_at,
                input.notes,
                created_at,
            ],
        )?;
        tx.commit()?;
        self.get_printer_maintenance_log(&id)?
            .ok_or_else(|| LibraryError::InvalidInput("created maintenance log is missing".into()))
    }

    pub fn get_printer_maintenance_log(
        &self,
        id: &str,
    ) -> Result<Option<PrinterMaintenanceLog>, LibraryError> {
        validate_record_id(id, "maintenance log ID")?;
        self.conn
            .query_row(
                "SELECT id,printer_id,printer_nickname,title,performed_at,notes,created_at,metadata_version
                 FROM printer_maintenance_logs WHERE id=?1",
                params![id],
                maintenance_log_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_printer_maintenance_logs(
        &self,
        printer_id: Option<&str>,
    ) -> Result<Vec<PrinterMaintenanceLog>, LibraryError> {
        if let Some(id) = printer_id {
            validate_record_id(id, "printer ID")?;
        }
        let tx = self.conn.unchecked_transaction()?;
        let count = tx.query_row(
            "SELECT COUNT(*) FROM printer_maintenance_logs WHERE (?1 IS NULL OR printer_id=?1)",
            params![printer_id],
            |row| row.get(0),
        )?;
        let count = checked_history_list_count(
            count,
            "maintenance log",
            MAX_PRINTER_UI_MAINTENANCE_LOG_ROWS,
        )?;
        let mut statement = tx.prepare(
            "SELECT id,printer_id,printer_nickname,title,performed_at,notes,created_at,metadata_version
             FROM printer_maintenance_logs WHERE (?1 IS NULL OR printer_id=?1)
             ORDER BY performed_at DESC,id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![printer_id, MAX_PRINTER_UI_MAINTENANCE_LOG_ROWS as i64],
            maintenance_log_from_row,
        )?;
        let records = rows.collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if records.len() != count {
            return Err(LibraryError::InvalidInput(
                "printer maintenance log list did not read every counted record".into(),
            ));
        }
        tx.commit()?;
        Ok(records)
    }

    pub fn update_printer_maintenance_log(
        &self,
        id: &str,
        expected_version: u64,
        input: &PrinterMaintenanceInput,
    ) -> Result<PrinterMaintenanceLog, LibraryError> {
        validate_record_id(id, "maintenance log ID")?;
        let input = normalize_maintenance_input(input)?;
        let expected_version = i64::try_from(expected_version).map_err(|_| {
            LibraryError::InvalidInput("maintenance log version is out of range".into())
        })?;
        let changed = self.conn.execute(
            "UPDATE printer_maintenance_logs
             SET title=?1,performed_at=?2,notes=?3,metadata_version=metadata_version+1
             WHERE id=?4 AND metadata_version=?5",
            params![
                input.title,
                input.performed_at,
                input.notes,
                id,
                expected_version
            ],
        )?;
        if changed == 0 {
            ensure_printer_maintenance_log_version(&self.conn, id, expected_version as u64)?;
        }
        self.get_printer_maintenance_log(id)?
            .ok_or_else(|| LibraryError::InvalidInput("maintenance log not found".into()))
    }

    pub fn delete_printer_maintenance_log(
        &self,
        id: &str,
        expected_version: u64,
    ) -> Result<(), LibraryError> {
        validate_record_id(id, "maintenance log ID")?;
        let expected_version = i64::try_from(expected_version).map_err(|_| {
            LibraryError::InvalidInput("maintenance log version is out of range".into())
        })?;
        let deleted = self.conn.execute(
            "DELETE FROM printer_maintenance_logs WHERE id=?1 AND metadata_version=?2",
            params![id, expected_version],
        )?;
        if deleted == 0 {
            ensure_printer_maintenance_log_version(&self.conn, id, expected_version as u64)?;
        }
        Ok(())
    }

    pub fn create_printer_reminder(
        &self,
        printer_id: &str,
        input: &PrinterReminderInput,
    ) -> Result<PrinterReminder, LibraryError> {
        validate_record_id(printer_id, "printer ID")?;
        let input = normalize_printer_reminder_input(input)?;
        let id = new_id("reminder");
        let now = timestamp();
        let tx = self.conn.unchecked_transaction()?;
        let printer_nickname: String = tx
            .query_row(
                "SELECT nickname FROM printers WHERE id=?1",
                params![printer_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("printer not found".into()))?;
        let reminder_count = tx.query_row("SELECT COUNT(*) FROM printer_reminders", [], |row| {
            row.get(0)
        })?;
        ensure_printer_history_capacity(
            reminder_count,
            1,
            "reminder",
            MAX_PRINTER_JSON_EXPORT_REMINDERS,
        )?;
        tx.execute(
            "INSERT INTO printer_reminders
             (id,metadata_version,printer_id,printer_nickname,title,notes,due_at,interval_days,
              status,last_completed_at,created_at,updated_at)
             VALUES(?1,1,?2,?3,?4,?5,?6,?7,'active',NULL,?8,?8)",
            params![
                id,
                printer_id,
                printer_nickname,
                input.title,
                input.notes,
                input.due_at,
                input.interval_days,
                now,
            ],
        )?;
        tx.commit()?;
        self.get_printer_reminder(&id)?
            .ok_or_else(|| LibraryError::InvalidInput("created printer reminder is missing".into()))
    }

    pub fn get_printer_reminder(&self, id: &str) -> Result<Option<PrinterReminder>, LibraryError> {
        validate_record_id(id, "printer reminder ID")?;
        self.conn
            .query_row(
                PRINTER_REMINDER_SELECT_BY_ID,
                params![id],
                printer_reminder_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_printer_reminders(
        &self,
        printer_id: Option<&str>,
        include_completed: bool,
    ) -> Result<Vec<PrinterReminder>, LibraryError> {
        if let Some(id) = printer_id {
            validate_record_id(id, "printer ID")?;
        }
        let tx = self.conn.unchecked_transaction()?;
        let count = tx.query_row(
            "SELECT COUNT(*) FROM printer_reminders
             WHERE (?1 IS NULL OR printer_id=?1) AND (?2=1 OR status!='completed')",
            params![printer_id, include_completed],
            |row| row.get(0),
        )?;
        let count = checked_history_list_count(count, "reminder", MAX_PRINTER_UI_REMINDER_ROWS)?;
        let mut statement = tx.prepare(
            "SELECT id,metadata_version,printer_id,printer_nickname,title,notes,due_at,
                    interval_days,status,last_completed_at,created_at,updated_at
             FROM printer_reminders
             WHERE (?1 IS NULL OR printer_id=?1) AND (?2=1 OR status!='completed')
             ORDER BY due_at,id LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![
                printer_id,
                include_completed,
                MAX_PRINTER_UI_REMINDER_ROWS as i64
            ],
            printer_reminder_from_row,
        )?;
        let records = rows.collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if records.len() != count {
            return Err(LibraryError::InvalidInput(
                "printer reminder list did not read every counted record".into(),
            ));
        }
        tx.commit()?;
        Ok(records)
    }

    pub fn update_printer_reminder(
        &self,
        id: &str,
        expected_version: u64,
        input: &PrinterReminderInput,
    ) -> Result<PrinterReminder, LibraryError> {
        validate_record_id(id, "printer reminder ID")?;
        let input = normalize_printer_reminder_input(input)?;
        let tx = self.conn.unchecked_transaction()?;
        let (actual, status): (i64, String) = tx
            .query_row(
                "SELECT metadata_version,status FROM printer_reminders WHERE id=?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("printer reminder not found".into()))?;
        ensure_record_version("printer reminder", expected_version, actual)?;
        if status == "completed" {
            return Err(LibraryError::InvalidInput(
                "completed printer reminder cannot be edited".into(),
            ));
        }
        tx.execute(
            "UPDATE printer_reminders SET title=?1,notes=?2,due_at=?3,interval_days=?4,
                metadata_version=metadata_version+1,updated_at=?5 WHERE id=?6",
            params![
                input.title,
                input.notes,
                input.due_at,
                input.interval_days,
                timestamp(),
                id,
            ],
        )?;
        tx.commit()?;
        self.get_printer_reminder(id)?
            .ok_or_else(|| LibraryError::InvalidInput("printer reminder not found".into()))
    }

    pub fn reschedule_printer_reminder(
        &self,
        id: &str,
        expected_version: u64,
        due_at: &str,
    ) -> Result<PrinterReminder, LibraryError> {
        validate_record_id(id, "printer reminder ID")?;
        let due_at = normalize_rfc3339_field(due_at, "due_at")?;
        let tx = self.conn.unchecked_transaction()?;
        let (actual, status): (i64, String) = tx
            .query_row(
                "SELECT metadata_version,status FROM printer_reminders WHERE id=?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("printer reminder not found".into()))?;
        ensure_record_version("printer reminder", expected_version, actual)?;
        if status == "completed" {
            return Err(LibraryError::InvalidInput(
                "completed printer reminder cannot be rescheduled".into(),
            ));
        }
        tx.execute(
            "UPDATE printer_reminders SET due_at=?1,status='active',
                metadata_version=metadata_version+1,updated_at=?2 WHERE id=?3",
            params![due_at, timestamp(), id],
        )?;
        tx.commit()?;
        self.get_printer_reminder(id)?
            .ok_or_else(|| LibraryError::InvalidInput("printer reminder not found".into()))
    }

    pub fn set_printer_reminder_paused(
        &self,
        id: &str,
        expected_version: u64,
        paused: bool,
    ) -> Result<PrinterReminder, LibraryError> {
        validate_record_id(id, "printer reminder ID")?;
        let tx = self.conn.unchecked_transaction()?;
        let (actual, status): (i64, String) = tx
            .query_row(
                "SELECT metadata_version,status FROM printer_reminders WHERE id=?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("printer reminder not found".into()))?;
        ensure_record_version("printer reminder", expected_version, actual)?;
        if status == "completed" {
            return Err(LibraryError::InvalidInput(
                "completed printer reminder cannot be paused".into(),
            ));
        }
        tx.execute(
            "UPDATE printer_reminders SET status=?1,metadata_version=metadata_version+1,
                updated_at=?2 WHERE id=?3",
            params![if paused { "paused" } else { "active" }, timestamp(), id],
        )?;
        tx.commit()?;
        self.get_printer_reminder(id)?
            .ok_or_else(|| LibraryError::InvalidInput("printer reminder not found".into()))
    }

    pub fn complete_printer_reminder(
        &self,
        id: &str,
        expected_version: u64,
        completion: &PrinterReminderCompletionInput,
    ) -> Result<PrinterReminderCompletion, LibraryError> {
        validate_record_id(id, "printer reminder ID")?;
        let completed_at = normalize_rfc3339_field(&completion.completed_at, "completed_at")?;
        let notes = clean_bounded_notes(&completion.notes, "maintenance notes")?;
        let tx = self.conn.unchecked_transaction()?;
        let reminder = tx
            .query_row(
                PRINTER_REMINDER_SELECT_BY_ID,
                params![id],
                printer_reminder_from_row,
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("printer reminder not found".into()))?;
        ensure_record_version(
            "printer reminder",
            expected_version,
            reminder.metadata_version as i64,
        )?;
        if reminder.status != PrinterReminderStatus::Active {
            return Err(LibraryError::InvalidInput(
                "only an active printer reminder can be completed".into(),
            ));
        }
        let maintenance_count =
            tx.query_row("SELECT COUNT(*) FROM printer_maintenance_logs", [], |row| {
                row.get(0)
            })?;
        ensure_printer_history_capacity(
            maintenance_count,
            1,
            "maintenance log",
            MAX_PRINTER_JSON_EXPORT_MAINTENANCE_LOGS,
        )?;
        let (due_at, status) = if let Some(interval_days) = reminder.interval_days {
            (
                add_rfc3339_days(&completed_at, interval_days)?,
                PrinterReminderStatus::Active,
            )
        } else {
            (reminder.due_at.clone(), PrinterReminderStatus::Completed)
        };
        let now = timestamp();
        tx.execute(
            "UPDATE printer_reminders SET due_at=?1,status=?2,last_completed_at=?3,
                metadata_version=metadata_version+1,updated_at=?4 WHERE id=?5",
            params![
                due_at,
                printer_reminder_status_value(&status),
                completed_at,
                now,
                id,
            ],
        )?;
        let log_id = new_id("maintenance");
        tx.execute(
            "INSERT INTO printer_maintenance_logs
             (id,printer_id,printer_nickname,title,performed_at,notes,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                log_id,
                reminder.printer_id,
                reminder.printer_nickname,
                reminder.title,
                completed_at,
                notes,
                now,
            ],
        )?;
        tx.commit()?;
        let reminder = self
            .get_printer_reminder(id)?
            .ok_or_else(|| LibraryError::InvalidInput("printer reminder disappeared".into()))?;
        let maintenance_log = self
            .get_printer_maintenance_log(&log_id)?
            .ok_or_else(|| LibraryError::InvalidInput("maintenance log disappeared".into()))?;
        Ok(PrinterReminderCompletion {
            reminder,
            maintenance_log,
        })
    }

    pub fn delete_printer_reminder(
        &self,
        id: &str,
        expected_version: u64,
    ) -> Result<(), LibraryError> {
        validate_record_id(id, "printer reminder ID")?;
        let tx = self.conn.unchecked_transaction()?;
        let actual: i64 = tx
            .query_row(
                "SELECT metadata_version FROM printer_reminders WHERE id=?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("printer reminder not found".into()))?;
        ensure_record_version("printer reminder", expected_version, actual)?;
        tx.execute("DELETE FROM printer_reminders WHERE id=?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn create_print_attempt(
        &self,
        model_id: &str,
        input: &PrintAttemptInput,
    ) -> Result<PrintAttempt, LibraryError> {
        self.create_print_attempt_with_creation_time(model_id, input, None)
    }

    pub fn create_print_attempt_with_printer(
        &self,
        model_id: &str,
        input: &PrintAttemptInputWithPrinter,
    ) -> Result<PrintAttempt, LibraryError> {
        validate_print_attempt(&input.input)?;
        self.ensure_model(model_id)?;
        let id = new_id("print");
        let now = timestamp();
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        write_print_attempt(&tx, &id, model_id, &input.input, &now, true, None, true)?;
        if let Some(Some(printer_id)) = input.printer_id.as_ref() {
            write_print_attempt_printer(&tx, &id, Some(printer_id))?;
        }
        recompute_print_summary(&tx, model_id)?;
        tx.commit()?;
        self.get_print_attempt(&id)?
            .ok_or_else(|| LibraryError::InvalidInput("print attempt missing".into()))
    }

    pub fn import_print_attempt(
        &self,
        model_id: &str,
        input: &PrintAttemptInput,
        created_at: &str,
    ) -> Result<PrintAttempt, LibraryError> {
        let created_at = normalize_rfc3339(created_at)?;
        self.create_print_attempt_with_creation_time(model_id, input, Some(&created_at))
    }

    /// Imports a print event with already-resolved local printer attribution.
    /// The optional name is a historical snapshot and may be supplied without
    /// a local printer link when the source printer was not imported or mapped.
    pub fn import_print_attempt_with_printer(
        &self,
        model_id: &str,
        input: &PrintAttemptInput,
        created_at: Option<&str>,
        printer_id: Option<&str>,
        printer_name: Option<&str>,
    ) -> Result<PrintAttempt, LibraryError> {
        validate_print_attempt(input)?;
        self.ensure_model(model_id)?;
        let imported_created_at = created_at
            .map(|value| validate_import_timestamp(value, "print attempt created_at"))
            .transpose()?;
        let printer_name_snapshot = printer_name
            .map(validate_import_snapshot_name)
            .transpose()?;
        let mapped_printer_id = printer_id
            .map(|value| validate_record_id(value, "printer ID"))
            .transpose()?;

        let id = new_id("print");
        let now = timestamp();
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let printer_attribution = if let Some(local_printer_id) = mapped_printer_id {
            let current_name: Option<String> = tx
                .query_row(
                    "SELECT nickname FROM printers WHERE id=?1",
                    params![local_printer_id],
                    |row| row.get(0),
                )
                .optional()?;
            let current_name = current_name
                .ok_or_else(|| LibraryError::InvalidInput("printer not found".into()))?;
            Some((
                local_printer_id,
                printer_name_snapshot.clone().unwrap_or(current_name),
            ))
        } else {
            None
        };

        write_print_attempt(
            &tx,
            &id,
            model_id,
            input,
            &now,
            true,
            imported_created_at.as_deref(),
            false,
        )?;
        if let Some((local_printer_id, snapshot)) = printer_attribution {
            tx.execute(
                "UPDATE print_attempts SET printer_id=?1,printer_name=?2 WHERE id=?3",
                params![local_printer_id, snapshot, id],
            )?;
        } else if let Some(snapshot) = printer_name_snapshot {
            tx.execute(
                "UPDATE print_attempts SET printer_name=?1 WHERE id=?2",
                params![snapshot, id],
            )?;
        }
        recompute_print_summary(&tx, model_id)?;
        tx.commit()?;
        self.get_print_attempt(&id)?
            .ok_or_else(|| LibraryError::InvalidInput("print attempt missing".into()))
    }

    /// Imports a reviewed history batch as one serialized database mutation.
    /// Existing matching events are counted before inserts, preserving duplicate
    /// event multiplicity within the batch while making concurrent replays safe.
    pub fn import_print_attempts_deduplicated(
        &self,
        model_id: &str,
        attempts: &[PrintAttemptImportItem],
    ) -> Result<PrintAttemptImportResult, LibraryError> {
        let model_id = validate_record_id(model_id, "model ID")?;
        if attempts.len() > MAX_PRINT_ATTEMPTS_PER_IMPORT_BATCH {
            return Err(LibraryError::InvalidInput(format!(
                "print history import exceeds the {MAX_PRINT_ATTEMPTS_PER_IMPORT_BATCH}-event limit"
            )));
        }
        for item in attempts {
            validate_print_attempt(&item.input)?;
            if let Some(created_at) = item.created_at.as_deref() {
                validate_import_timestamp(created_at, "print attempt created_at")?;
            }
            if let Some(printer_name) = item.printer_name.as_deref() {
                validate_print_attempt_printer_snapshot(printer_name)?;
            }
            if let Some(printer_id) = item.printer_id.as_deref() {
                validate_record_id(printer_id, "printer ID")?;
            }
        }

        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let model_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM models WHERE id=?1)",
            params![model_id],
            |row| row.get(0),
        )?;
        if !model_exists {
            return Err(LibraryError::ModelNotFound(model_id));
        }

        let existing_rows = {
            let mut stmt = tx.prepare(
                "SELECT id,printed_at,quantity,notes,outcome,printer_id,printer_name \
                 FROM print_attempts WHERE model_id=?1",
            )?;
            let rows = stmt.query_map(params![model_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? as u32,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut remaining_existing = HashMap::new();
        for (id, printed_at, quantity, notes, outcome, printer_id, printer_name) in existing_rows {
            let filament_ids = attempt_filaments(&tx, &id)?;
            let parts = attempt_parts(&tx, &id)?
                .into_iter()
                .filter_map(|part| {
                    part.id.map(|id| PartUsageInput {
                        id,
                        quantity: part.quantity_per_copy,
                        snapshot: None,
                    })
                })
                .collect::<Vec<_>>();
            let identity = print_attempt_import_identity(
                &printed_at,
                quantity,
                &notes,
                &filament_ids,
                &parts,
                &outcome,
                printer_id,
                printer_name,
            )?;
            *remaining_existing.entry(identity).or_insert(0usize) += 1;
        }

        let mut added_count = 0usize;
        let mut skipped_count = 0usize;
        for item in attempts {
            let imported_created_at = item
                .created_at
                .as_deref()
                .map(|value| validate_import_timestamp(value, "print attempt created_at"))
                .transpose()?;
            let supplied_printer_name = item
                .printer_name
                .as_deref()
                .map(validate_print_attempt_printer_snapshot)
                .transpose()?;
            let printer_id = item
                .printer_id
                .as_deref()
                .map(|value| validate_record_id(value, "printer ID"))
                .transpose()?;
            let (printer_id, printer_name) = if let Some(printer_id) = printer_id {
                let current_name: Option<String> = tx
                    .query_row(
                        "SELECT nickname FROM printers WHERE id=?1",
                        params![printer_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                let current_name = current_name
                    .ok_or_else(|| LibraryError::InvalidInput("printer not found".into()))?;
                let printer_name = supplied_printer_name.unwrap_or(current_name);
                (Some(printer_id), Some(printer_name))
            } else {
                (None, supplied_printer_name)
            };
            let identity = print_attempt_import_identity(
                &item.input.printed_at,
                item.input.quantity,
                &item.input.notes,
                &item.input.filament_ids,
                item.input.parts.as_deref().unwrap_or_default(),
                print_outcome_value(&item.input.outcome),
                printer_id.clone(),
                printer_name.clone(),
            )?;
            if let Some(remaining) = remaining_existing.get_mut(&identity) {
                if *remaining > 0 {
                    *remaining -= 1;
                    skipped_count += 1;
                    continue;
                }
            }

            let id = new_id("print");
            let now = timestamp();
            write_print_attempt(
                &tx,
                &id,
                &model_id,
                &item.input,
                &now,
                true,
                imported_created_at.as_deref(),
                false,
            )?;
            if let Some(printer_id) = printer_id {
                tx.execute(
                    "UPDATE print_attempts SET printer_id=?1,printer_name=?2 WHERE id=?3",
                    params![printer_id, printer_name, id],
                )?;
            } else if let Some(printer_name) = printer_name {
                tx.execute(
                    "UPDATE print_attempts SET printer_name=?1 WHERE id=?2",
                    params![printer_name, id],
                )?;
            }
            added_count += 1;
        }

        if added_count > 0 {
            recompute_print_summary(&tx, &model_id)?;
        }
        let metadata_version: i64 = tx.query_row(
            "SELECT metadata_version FROM models WHERE id=?1",
            params![model_id],
            |row| row.get(0),
        )?;
        let metadata_version = u64::try_from(metadata_version)
            .map_err(|_| LibraryError::InvalidInput("model version is invalid".into()))?;
        tx.commit()?;
        Ok(PrintAttemptImportResult {
            added_count,
            skipped_count,
            metadata_version,
        })
    }

    fn create_print_attempt_with_creation_time(
        &self,
        model_id: &str,
        input: &PrintAttemptInput,
        imported_created_at: Option<&str>,
    ) -> Result<PrintAttempt, LibraryError> {
        validate_print_attempt(input)?;
        self.ensure_model(model_id)?;
        let id = new_id("print");
        let now = timestamp();
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        write_print_attempt(
            &tx,
            &id,
            model_id,
            input,
            &now,
            true,
            imported_created_at,
            imported_created_at.is_none(),
        )?;
        recompute_print_summary(&tx, model_id)?;
        tx.commit()?;
        self.get_print_attempt(&id)?
            .ok_or_else(|| LibraryError::InvalidInput("print attempt missing".into()))
    }

    pub fn create_print_attempts_idempotent(
        &self,
        idempotency_key: &str,
        items: &[PrintAttemptBatchItem],
    ) -> Result<Vec<PrintAttempt>, LibraryError> {
        validate_plain_capture_id(idempotency_key, "print batch idempotency key")?;
        if items.is_empty() || items.len() > 10_000 {
            return Err(LibraryError::InvalidInput(
                "print attempt batch count is invalid".into(),
            ));
        }
        for item in items {
            validate_print_attempt(&item.input)?;
        }
        let payload = serde_json::to_vec(items)
            .map_err(|_| LibraryError::InvalidInput("print batch payload is invalid".into()))?;
        let payload_hash = format!("{:x}", Sha256::digest(&payload));
        if let Some((stored_hash, attempt_ids_json)) = self
            .conn
            .query_row(
                "SELECT payload_hash,attempt_ids_json FROM print_batch_receipts
                 WHERE idempotency_key=?1",
                params![idempotency_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        {
            if stored_hash != payload_hash {
                return Err(LibraryError::IdempotencyConflict(idempotency_key.into()));
            }
            let ids: Vec<String> = serde_json::from_str(&attempt_ids_json)
                .map_err(|_| LibraryError::InvalidInput("print batch receipt is corrupt".into()))?;
            return ids
                .into_iter()
                .map(|id| {
                    self.get_print_attempt(&id)?.ok_or_else(|| {
                        LibraryError::InvalidInput("print batch receipt target is missing".into())
                    })
                })
                .collect();
        }

        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        // A competing request may have committed after the fast preflight
        // read above. Recheck under the write reservation before charging stock.
        if let Some((stored_hash, attempt_ids_json)) = tx
            .query_row(
                "SELECT payload_hash,attempt_ids_json FROM print_batch_receipts
                 WHERE idempotency_key=?1",
                params![idempotency_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        {
            if stored_hash != payload_hash {
                return Err(LibraryError::IdempotencyConflict(idempotency_key.into()));
            }
            let ids: Vec<String> = serde_json::from_str(&attempt_ids_json)
                .map_err(|_| LibraryError::InvalidInput("print batch receipt is corrupt".into()))?;
            tx.commit()?;
            return ids
                .into_iter()
                .map(|id| {
                    self.get_print_attempt(&id)?.ok_or_else(|| {
                        LibraryError::InvalidInput("print batch receipt target is missing".into())
                    })
                })
                .collect();
        }
        let mut affected = BTreeSet::new();
        for item in items {
            let active: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM models WHERE id=?1 AND deleted_at IS NULL)",
                params![item.model_id],
                |row| row.get(0),
            )?;
            if !active {
                return Err(LibraryError::ModelNotFound(item.model_id.clone()));
            }
            affected.insert(item.model_id.clone());
        }
        let now = timestamp();
        let mut ids = Vec::with_capacity(items.len());
        for item in items {
            let id = new_id("print");
            write_print_attempt(
                &tx,
                &id,
                &item.model_id,
                &item.input,
                &now,
                true,
                None,
                true,
            )?;
            ids.push(id);
        }
        for model_id in affected {
            recompute_print_summary(&tx, &model_id)?;
        }
        tx.execute(
            "INSERT INTO print_batch_receipts
             (idempotency_key,payload_hash,attempt_ids_json,created_at) VALUES(?1,?2,?3,?4)",
            params![
                idempotency_key,
                payload_hash,
                serde_json::to_string(&ids).map_err(|_| LibraryError::InvalidInput(
                    "print batch receipt could not be encoded".into()
                ))?,
                now
            ],
        )?;
        tx.commit()?;
        ids.into_iter()
            .map(|id| {
                self.get_print_attempt(&id)?.ok_or_else(|| {
                    LibraryError::InvalidInput("created print attempt is missing".into())
                })
            })
            .collect()
    }
    pub fn list_print_attempts(&self, model_id: &str) -> Result<Vec<PrintAttempt>, LibraryError> {
        let mut stmt=self.conn.prepare("SELECT id,model_id,printed_at,quantity,notes,outcome,printer_id,printer_name,created_at,updated_at,stock_adjusted FROM print_attempts WHERE model_id=?1 ORDER BY printed_at DESC,id DESC")?;
        let rows = stmt.query_map(params![model_id], attempt_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            let mut a = row?;
            a.filament_ids = attempt_filaments(&self.conn, &a.id)?;
            a.parts = attempt_parts(&self.conn, &a.id)?;
            out.push(a);
        }
        Ok(out)
    }

    pub fn list_print_attempts_for_json_export_page(
        &self,
        model_id: &str,
        after_printed_at: Option<&str>,
        after_id: Option<&str>,
        limit: u16,
    ) -> Result<Vec<PrintAttempt>, LibraryError> {
        if limit == 0 || limit > 100 || after_printed_at.is_some() != after_id.is_some() {
            return Err(LibraryError::InvalidInput(
                "JSON export print-event page request is invalid".into(),
            ));
        }
        let mut statement = self.conn.prepare(
            "SELECT id,model_id,printed_at,quantity,notes,outcome,printer_id,printer_name,created_at,updated_at,stock_adjusted
             FROM print_attempts WHERE model_id=?1
             AND (?2 IS NULL OR printed_at<?2 OR (printed_at=?2 AND id<?3))
             ORDER BY printed_at DESC,id DESC LIMIT ?4",
        )?;
        let rows = statement.query_map(
            params![model_id, after_printed_at, after_id, i64::from(limit)],
            attempt_from_row,
        )?;
        let mut attempts = rows.collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        for attempt in &mut attempts {
            attempt.filament_ids = attempt_filaments(&self.conn, &attempt.id)?;
            attempt.parts = attempt_parts(&self.conn, &attempt.id)?;
        }
        Ok(attempts)
    }
    pub fn get_print_attempt(&self, id: &str) -> Result<Option<PrintAttempt>, LibraryError> {
        let mut a=self.conn.query_row("SELECT id,model_id,printed_at,quantity,notes,outcome,printer_id,printer_name,created_at,updated_at,stock_adjusted FROM print_attempts WHERE id=?1",params![id],attempt_from_row).optional()?;
        if let Some(ref mut value) = a {
            value.filament_ids = attempt_filaments(&self.conn, id)?;
            value.parts = attempt_parts(&self.conn, id)?;
        }
        Ok(a)
    }
    pub fn update_print_attempt(
        &self,
        id: &str,
        input: &PrintAttemptInput,
    ) -> Result<PrintAttempt, LibraryError> {
        validate_print_attempt(input)?;
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let model_id: String = tx
            .query_row(
                "SELECT model_id FROM print_attempts WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("print attempt not found".into()))?;
        write_print_attempt(&tx, id, &model_id, input, &timestamp(), false, None, true)?;
        recompute_print_summary(&tx, &model_id)?;
        tx.commit()?;
        self.get_print_attempt(id)?
            .ok_or_else(|| LibraryError::InvalidInput("print attempt not found".into()))
    }

    pub fn update_print_attempt_with_printer(
        &self,
        id: &str,
        input: &PrintAttemptInputWithPrinter,
    ) -> Result<PrintAttempt, LibraryError> {
        validate_record_id(id, "print attempt ID")?;
        validate_print_attempt(&input.input)?;
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let model_id: String = tx
            .query_row(
                "SELECT model_id FROM print_attempts WHERE id=?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("print attempt not found".into()))?;
        write_print_attempt(
            &tx,
            id,
            &model_id,
            &input.input,
            &timestamp(),
            false,
            None,
            true,
        )?;
        if let Some(printer_id) = input.printer_id.as_ref() {
            write_print_attempt_printer(&tx, id, printer_id.as_deref())?;
        }
        recompute_print_summary(&tx, &model_id)?;
        tx.commit()?;
        self.get_print_attempt(id)?
            .ok_or_else(|| LibraryError::InvalidInput("print attempt not found".into()))
    }
    pub fn delete_print_attempt(&self, id: &str) -> Result<(), LibraryError> {
        validate_record_id(id, "print attempt ID")?;
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let model_id: String = tx
            .query_row(
                "SELECT model_id FROM print_attempts WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("print attempt not found".into()))?;
        restore_print_attempt_parts(&tx, id)?;
        tx.execute("DELETE FROM print_attempts WHERE id=?1", params![id])?;
        recompute_print_summary(&tx, &model_id)?;
        tx.commit()?;
        Ok(())
    }

    pub fn create_or_get_capture_receipt(
        &self,
        input: &CaptureReceiptInput,
    ) -> Result<CaptureReceipt, LibraryError> {
        validate_capture_receipt(input)?;
        if let Some(existing_hash) = self
            .conn
            .query_row(
                "SELECT payload_hash FROM capture_items WHERE item_id=?1",
                params![input.item_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            if existing_hash != input.payload_hash {
                return Err(LibraryError::InvalidInput(
                    "capture item ID conflicts with a different payload".into(),
                ));
            }
            return self
                .get_capture_receipt(&input.item_id)?
                .ok_or_else(|| LibraryError::InvalidInput("capture receipt is missing".into()));
        }
        let tx = self.conn.unchecked_transaction()?;
        for download in &input.downloads {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM capture_downloads WHERE capture_id=?1)",
                params![download.capture_id],
                |row| row.get(0),
            )?;
            if exists {
                return Err(LibraryError::InvalidInput(
                    "capture download ID is already owned by another item".into(),
                ));
            }
        }
        let parent_model_id = if let Some(parent) = &input.page.parent_external_id {
            tx.query_row(
                "SELECT page_model_id FROM capture_items WHERE item_id=?1",
                params![parent],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .ok_or_else(|| LibraryError::InvalidInput("capture parent item is unknown".into()))?
            .into()
        } else {
            None
        };
        let model_id = new_id("bookmark");
        let now = timestamp();
        tx.execute(
            "INSERT INTO models(id,display_name,description,folder_id,metadata_version,
             created_at,updated_at,source_kind,source_missing,designer,source_url,notes,
             model_license,parent_model_id,catalog_kind)
             VALUES(?1,?2,'','root',1,?3,?3,'managed',0,?4,?5,?6,?7,?8,'bookmark')",
            params![
                model_id,
                input.page.title.trim(),
                now,
                clean_optional(input.page.designer.as_deref(), 200)?,
                input.page.url.trim(),
                clean_optional(input.page.notes.as_deref(), 20_000)?,
                clean_optional(input.page.model_license.as_deref(), 200)?,
                parent_model_id
            ],
        )?;
        let status = if input.downloads.is_empty() {
            "imported"
        } else {
            "partial"
        };
        tx.execute(
            "INSERT INTO capture_items(item_id,payload_hash,status,page_model_id,created_at,
             received_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?6)",
            params![
                input.item_id,
                input.payload_hash,
                status,
                model_id,
                input.created_at,
                now
            ],
        )?;
        for (ordinal, download) in input.downloads.iter().enumerate() {
            tx.execute(
                "INSERT INTO capture_downloads(capture_id,item_id,ordinal,file_name,expected_hash,
                 expected_bytes,status) VALUES(?1,?2,?3,?4,?5,?6,'needs_file')",
                params![
                    download.capture_id,
                    input.item_id,
                    ordinal as i64,
                    download.file_name,
                    download.sha256,
                    download.bytes.map(|value| value as i64)
                ],
            )?;
        }
        tx.commit()?;
        self.get_capture_receipt(&input.item_id)?
            .ok_or_else(|| LibraryError::InvalidInput("capture receipt is missing".into()))
    }

    pub fn get_capture_receipt(
        &self,
        item_id: &str,
    ) -> Result<Option<CaptureReceipt>, LibraryError> {
        let item: Option<(String, String, String, Option<String>)> = self
            .conn
            .query_row(
                "SELECT item_id,payload_hash,status,page_model_id FROM capture_items WHERE item_id=?1",
                params![item_id],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            )
            .optional()?;
        let Some((item_id, payload_hash, status, page_model_id)) = item else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare(
            "SELECT d.capture_id,d.status,d.model_id,d.error_code,m.deleted_at
             FROM capture_downloads d LEFT JOIN models m ON m.id=d.model_id
             WHERE d.item_id=?1 ORDER BY d.ordinal,d.capture_id",
        )?;
        let rows = stmt.query_map(params![item_id], |row| {
            let status: String = row.get(1)?;
            let deleted_at: Option<String> = row.get(4)?;
            Ok(CaptureDownloadRecord {
                capture_id: row.get(0)?,
                status: if status == "imported" && deleted_at.is_some() {
                    "imported_removed".into()
                } else {
                    status
                },
                model_id: row.get(2)?,
                error_code: row.get(3)?,
            })
        })?;
        Ok(Some(CaptureReceipt {
            item_id,
            payload_hash,
            status,
            page_model_id,
            downloads: rows.collect::<Result<Vec<_>, _>>()?,
        }))
    }

    pub fn list_capture_summaries_page(
        &self,
        after_created_at: Option<&str>,
        after_item_id: Option<&str>,
        limit: u16,
    ) -> Result<(Vec<CaptureSummary>, bool), LibraryError> {
        if limit == 0 || limit > 500 || after_created_at.is_some() != after_item_id.is_some() {
            return Err(LibraryError::InvalidInput(
                "capture page cursor or limit is invalid".into(),
            ));
        }
        let mut stmt = self.conn.prepare(
            "SELECT ci.item_id,ci.created_at,ci.status,ci.page_model_id,
                    COALESCE(m.display_name,''),COALESCE(m.source_url,''),
                    COUNT(cd.capture_id),
                    COALESCE(SUM(cd.status='failed'),0),
                    COALESCE(SUM(cd.status='needs_file'),0)
             FROM capture_items ci
             LEFT JOIN models m ON m.id=ci.page_model_id
             LEFT JOIN capture_downloads cd ON cd.item_id=ci.item_id
             WHERE ?1 IS NULL OR ci.created_at < ?1
                OR (ci.created_at = ?1 AND ci.item_id < ?2)
             GROUP BY ci.item_id,ci.created_at,ci.status,ci.page_model_id,
                      m.display_name,m.source_url
             ORDER BY ci.created_at DESC,ci.item_id DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![after_created_at, after_item_id, i64::from(limit) + 1],
            |row| {
                Ok(CaptureSummary {
                    item_id: row.get(0)?,
                    created_at: row.get(1)?,
                    status: row.get(2)?,
                    page_model_id: row.get(3)?,
                    title: row.get(4)?,
                    url: row.get(5)?,
                    download_count: row.get::<_, i64>(6)? as u64,
                    failed_count: row.get::<_, i64>(7)? as u64,
                    needs_file_count: row.get::<_, i64>(8)? as u64,
                })
            },
        )?;
        let mut summaries = rows.collect::<Result<Vec<_>, _>>()?;
        let has_more = summaries.len() > usize::from(limit);
        summaries.truncate(usize::from(limit));
        Ok((summaries, has_more))
    }

    pub fn update_capture_download_result(
        &self,
        item_id: &str,
        capture_id: &str,
        model_id: Option<&str>,
        error_code: Option<&str>,
    ) -> Result<CaptureReceipt, LibraryError> {
        if model_id.is_some() == error_code.is_some() {
            return Err(LibraryError::InvalidInput(
                "capture result must be either imported or failed".into(),
            ));
        }
        if let Some(code) = error_code {
            validate_plain_capture_id(code, "capture error code")?;
        }
        let tx = self.conn.unchecked_transaction()?;
        if let Some(model_id) = model_id {
            let active: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM models WHERE id=?1 AND deleted_at IS NULL)",
                params![model_id],
                |row| row.get(0),
            )?;
            if !active {
                return Err(LibraryError::ModelNotFound(model_id.into()));
            }
        }
        let changed = tx.execute(
            "UPDATE capture_downloads SET status=?1,model_id=?2,error_code=?3
             WHERE item_id=?4 AND capture_id=?5 AND status!='imported'",
            params![
                if model_id.is_some() {
                    "imported"
                } else {
                    "failed"
                },
                model_id,
                error_code,
                item_id,
                capture_id
            ],
        )?;
        if changed == 0 {
            let existing: Option<Option<String>> = tx
                .query_row(
                    "SELECT model_id FROM capture_downloads WHERE item_id=?1 AND capture_id=?2",
                    params![item_id, capture_id],
                    |row| row.get(0),
                )
                .optional()?;
            if existing.flatten().as_deref() != model_id {
                return Err(LibraryError::InvalidInput(
                    "capture result conflicts with an existing import".into(),
                ));
            }
        }
        let (remaining, failed): (i64, i64) = tx.query_row(
            "SELECT SUM(status!='imported'),SUM(status='failed') FROM capture_downloads WHERE item_id=?1",
            params![item_id],
            |row| Ok((row.get::<_,Option<i64>>(0)?.unwrap_or(0),row.get::<_,Option<i64>>(1)?.unwrap_or(0))),
        )?;
        let item_status = if remaining == 0 {
            "imported"
        } else if failed > 0 {
            "failed"
        } else {
            "partial"
        };
        tx.execute(
            "UPDATE capture_items SET status=?1,updated_at=?2 WHERE item_id=?3",
            params![item_status, timestamp(), item_id],
        )?;
        tx.commit()?;
        self.get_capture_receipt(item_id)?
            .ok_or_else(|| LibraryError::InvalidInput("capture receipt is missing".into()))
    }

    pub fn create_filament(&self, input: &FilamentInput) -> Result<Filament, LibraryError> {
        let input = validate_filament_input(input)?;
        let id = new_id("filament");
        let now = timestamp();
        self.conn.execute(
            "INSERT INTO filaments(id,version,brand,material,color,name,notes,origin,resource_kind,
             archived,source_missing,created_at,updated_at)
             VALUES(?1,1,?2,?3,?4,?5,?6,'manual','manual',0,0,?7,?7)",
            params![
                id,
                input.brand,
                input.material,
                input.color,
                input.name,
                input.notes,
                now
            ],
        )?;
        self.get_filament(&id)?
            .ok_or_else(|| LibraryError::InvalidInput("created filament is missing".into()))
    }

    pub fn get_filament(&self, id: &str) -> Result<Option<Filament>, LibraryError> {
        self.conn
            .query_row(
                &format!("{FILAMENT_SELECT} WHERE f.id=?1"),
                params![id],
                filament_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_filaments(&self, include_unavailable: bool) -> Result<Vec<Filament>, LibraryError> {
        let mut stmt = self.conn.prepare(&format!(
            "{FILAMENT_SELECT} WHERE ?1 OR (f.archived=0 AND f.source_missing=0)
             ORDER BY lower(f.brand),lower(f.material),lower(f.name),f.resource_kind,f.external_id,f.id"
        ))?;
        let rows = stmt.query_map(params![include_unavailable], filament_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn update_filament(
        &self,
        id: &str,
        expected_version: u64,
        input: &FilamentInput,
    ) -> Result<Filament, LibraryError> {
        let input = validate_filament_input(input)?;
        let actual: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT version,origin FROM filaments WHERE id=?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((actual, origin)) = actual else {
            return Err(LibraryError::InvalidInput("filament not found".into()));
        };
        if actual as u64 != expected_version {
            return Err(LibraryError::FilamentVersionConflict {
                filament_id: id.into(),
                expected: expected_version,
                actual: actual as u64,
            });
        }
        if origin != "manual" {
            return Err(LibraryError::InvalidInput(
                "Spoolman filaments are updated by synchronization".into(),
            ));
        }
        self.conn.execute(
            "UPDATE filaments SET brand=?1,material=?2,color=?3,name=?4,notes=?5,
             version=version+1,updated_at=?6 WHERE id=?7",
            params![
                input.brand,
                input.material,
                input.color,
                input.name,
                input.notes,
                timestamp(),
                id
            ],
        )?;
        self.get_filament(id)?
            .ok_or_else(|| LibraryError::InvalidInput("filament not found".into()))
    }

    pub fn delete_filament(
        &self,
        id: &str,
        expected_version: u64,
    ) -> Result<Option<Filament>, LibraryError> {
        let tx = self.conn.unchecked_transaction()?;
        let actual: Option<(i64, String)> = tx
            .query_row(
                "SELECT version,origin FROM filaments WHERE id=?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((actual, origin)) = actual else {
            return Err(LibraryError::InvalidInput("filament not found".into()));
        };
        if actual as u64 != expected_version {
            return Err(LibraryError::FilamentVersionConflict {
                filament_id: id.into(),
                expected: expected_version,
                actual: actual as u64,
            });
        }
        if origin != "manual" {
            return Err(LibraryError::InvalidInput(
                "Spoolman filaments cannot be deleted locally".into(),
            ));
        }
        let referenced: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM model_filaments WHERE filament_id=?1)
                 OR EXISTS(SELECT 1 FROM print_attempt_filaments WHERE filament_id=?1)",
            params![id],
            |row| row.get(0),
        )?;
        if referenced {
            tx.execute(
                "UPDATE filaments SET archived=1,version=version+1,updated_at=?1 WHERE id=?2",
                params![timestamp(), id],
            )?;
        } else {
            tx.execute("DELETE FROM filaments WHERE id=?1", params![id])?;
        }
        tx.commit()?;
        self.get_filament(id)
    }

    pub fn set_model_filaments(
        &self,
        model_id: &str,
        expected_version: u64,
        filament_ids: &[String],
    ) -> Result<Model, LibraryError> {
        let tx = self.conn.unchecked_transaction()?;
        set_model_filaments_tx(&tx, model_id, expected_version, filament_ids)?;
        tx.commit()?;
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    pub fn bulk_set_model_filaments(
        &self,
        targets: &[VersionedTarget],
        filament_ids: &[String],
    ) -> Result<Vec<Model>, LibraryError> {
        if targets.is_empty() || targets.len() > 50_000 {
            return Err(LibraryError::InvalidInput(
                "filament assignment target count is invalid".into(),
            ));
        }
        validate_filament_ids(filament_ids)?;
        let mut unique = HashSet::new();
        if targets
            .iter()
            .any(|target| !unique.insert(&target.model_id))
        {
            return Err(LibraryError::InvalidInput(
                "duplicate filament assignment target".into(),
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        validate_catalog_filament_ids(&tx, filament_ids)?;
        for target in targets {
            validate_active_model_version(&tx, &target.model_id, target.expected_version)?;
        }
        for target in targets {
            write_model_filaments_tx(&tx, &target.model_id, filament_ids)?;
        }
        tx.commit()?;
        targets
            .iter()
            .map(|target| {
                self.get_model(&target.model_id)?
                    .ok_or_else(|| LibraryError::ModelNotFound(target.model_id.clone()))
            })
            .collect()
    }

    pub fn bulk_assign_model_filaments(
        &self,
        assignments: &[ModelFilamentAssignment],
    ) -> Result<Vec<Model>, LibraryError> {
        if assignments.is_empty() || assignments.len() > 50_000 {
            return Err(LibraryError::InvalidInput(
                "filament assignment count is invalid".into(),
            ));
        }
        let mut unique_models = HashSet::new();
        for assignment in assignments {
            if !unique_models.insert(&assignment.target.model_id) {
                return Err(LibraryError::InvalidInput(
                    "duplicate filament assignment target".into(),
                ));
            }
            validate_filament_ids(&assignment.filament_ids)?;
        }
        let tx = self.conn.unchecked_transaction()?;
        let all_filament_ids = assignments
            .iter()
            .flat_map(|assignment| assignment.filament_ids.iter())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        validate_catalog_filament_ids(&tx, &all_filament_ids)?;
        for assignment in assignments {
            validate_active_model_version(
                &tx,
                &assignment.target.model_id,
                assignment.target.expected_version,
            )?;
        }
        for assignment in assignments {
            write_model_filaments_tx(&tx, &assignment.target.model_id, &assignment.filament_ids)?;
        }
        tx.commit()?;
        assignments
            .iter()
            .map(|assignment| {
                self.get_model(&assignment.target.model_id)?
                    .ok_or_else(|| LibraryError::ModelNotFound(assignment.target.model_id.clone()))
            })
            .collect()
    }

    pub fn upsert_spoolman_source(&self, base_url: &str) -> Result<SpoolmanSource, LibraryError> {
        let base_url = validate_spoolman_base_url(base_url)?;
        if let Some(source) = self
            .conn
            .query_row(
                "SELECT id,base_url,last_synced_at FROM filament_sources WHERE base_url=?1",
                params![base_url],
                spoolman_source_from_row,
            )
            .optional()?
        {
            return Ok(source);
        }
        let id = new_id("spoolman");
        let now = timestamp();
        self.conn.execute(
            "INSERT INTO filament_sources(id,kind,base_url,created_at,updated_at)
             VALUES(?1,'spoolman',?2,?3,?3)",
            params![id, base_url, now],
        )?;
        self.get_spoolman_source(&id)?
            .ok_or_else(|| LibraryError::InvalidInput("Spoolman source is missing".into()))
    }

    pub fn get_spoolman_source(&self, id: &str) -> Result<Option<SpoolmanSource>, LibraryError> {
        self.conn
            .query_row(
                "SELECT id,base_url,last_synced_at FROM filament_sources WHERE id=?1 AND kind='spoolman'",
                params![id],
                spoolman_source_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_spoolman_sources(&self) -> Result<Vec<SpoolmanSource>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT id,base_url,last_synced_at FROM filament_sources WHERE kind='spoolman' ORDER BY lower(base_url),id",
        )?;
        let rows = stmt.query_map([], spoolman_source_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn sync_spoolman_snapshot(
        &self,
        source_id: &str,
        filaments: &[SpoolmanFilamentRecord],
        spools: &[SpoolmanSpoolRecord],
        completed_at: &str,
    ) -> Result<FilamentSyncSummary, LibraryError> {
        if filaments.len().saturating_add(spools.len()) > 100_000 {
            return Err(LibraryError::InvalidInput(
                "Spoolman snapshot exceeds 100000 records".into(),
            ));
        }
        let completed_at = normalize_rfc3339(completed_at)?;
        let mut filament_records = Vec::with_capacity(filaments.len());
        let mut filament_external_ids = HashSet::new();
        for record in filaments {
            let external_id = validate_external_id(&record.external_id)?;
            if !filament_external_ids.insert(external_id.clone()) {
                return Err(LibraryError::InvalidInput(
                    "duplicate Spoolman filament ID".into(),
                ));
            }
            filament_records.push((external_id, validate_spoolman_filament(record)?));
        }
        let mut spool_records = Vec::with_capacity(spools.len());
        let mut spool_external_ids = HashSet::new();
        for record in spools {
            let external_id = validate_external_id(&record.external_id)?;
            let parent_external_id = validate_external_id(&record.filament_external_id)?;
            if !spool_external_ids.insert(external_id.clone()) {
                return Err(LibraryError::InvalidInput(
                    "duplicate Spoolman spool ID".into(),
                ));
            }
            if !filament_external_ids.contains(&parent_external_id) {
                return Err(LibraryError::InvalidInput(
                    "Spoolman spool references an unknown filament".into(),
                ));
            }
            spool_records.push((
                external_id,
                parent_external_id,
                validate_spoolman_spool(record)?,
                record.archived,
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        let source_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM filament_sources WHERE id=?1 AND kind='spoolman')",
            params![source_id],
            |row| row.get(0),
        )?;
        if !source_exists {
            return Err(LibraryError::InvalidInput(
                "Spoolman source not found".into(),
            ));
        }
        let mut summary = FilamentSyncSummary {
            source_id: source_id.into(),
            seen: filament_records.len() + spool_records.len(),
            completed_at: completed_at.clone(),
            ..FilamentSyncSummary::default()
        };
        let mut local_filament_ids = HashMap::new();
        let mut seen = HashSet::new();
        for (external_id, input) in filament_records {
            let (id, added, updated) = upsert_remote_filament(
                &tx,
                source_id,
                "filament",
                &external_id,
                None,
                &input,
                false,
            )?;
            summary.added += usize::from(added);
            summary.updated += usize::from(updated);
            local_filament_ids.insert(external_id.clone(), id);
            seen.insert(("filament".to_owned(), external_id));
        }
        for (external_id, parent_external_id, input, archived) in spool_records {
            let parent_id = local_filament_ids.get(&parent_external_id).ok_or_else(|| {
                LibraryError::InvalidInput("Spoolman filament identity is missing".into())
            })?;
            let (_id, added, updated) = upsert_remote_filament(
                &tx,
                source_id,
                "spool",
                &external_id,
                Some(parent_id),
                &input,
                archived,
            )?;
            summary.added += usize::from(added);
            summary.updated += usize::from(updated);
            seen.insert(("spool".to_owned(), external_id));
        }
        let mut existing = Vec::new();
        {
            let mut stmt = tx.prepare(
                "SELECT id,resource_kind,external_id,source_missing FROM filaments WHERE source_id=?1",
            )?;
            let rows = stmt.query_map(params![source_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            })?;
            existing.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }
        for (id, kind, external_id, was_missing) in existing {
            if !seen.contains(&(kind, external_id)) && !was_missing {
                tx.execute(
                    "UPDATE filaments SET source_missing=1,version=version+1,updated_at=?1 WHERE id=?2",
                    params![timestamp(),id],
                )?;
                summary.missing += 1;
            }
        }
        tx.execute(
            "UPDATE filament_sources SET last_synced_at=?1,updated_at=?2 WHERE id=?3",
            params![completed_at, timestamp(), source_id],
        )?;
        tx.commit()?;
        Ok(summary)
    }

    pub fn create_bundle(&self, model_ids: &[String]) -> Result<String, LibraryError> {
        if model_ids.is_empty() {
            return Err(LibraryError::InvalidInput("bundle requires models".into()));
        }
        let id = new_id("bundle");
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO bundles(id,created_at) VALUES(?1,?2)",
            params![id, timestamp()],
        )?;
        for model in model_ids {
            if tx.execute("UPDATE models SET bundle_id=?1,metadata_version=metadata_version+1,updated_at=?2 WHERE id=?3",params![id,timestamp(),model])?==0{return Err(LibraryError::ModelNotFound(model.clone()));}
        }
        tx.commit()?;
        Ok(id)
    }
    pub fn set_model_bundle(
        &self,
        model_id: &str,
        bundle_id: Option<&str>,
    ) -> Result<Model, LibraryError> {
        if let Some(id) = bundle_id {
            let source_root: Option<Option<String>> = self
                .conn
                .query_row(
                    "SELECT source_root_id FROM bundles WHERE id=?1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()?;
            match source_root {
                None => return Err(LibraryError::InvalidInput("bundle not found".into())),
                Some(Some(_)) => {
                    return Err(LibraryError::InvalidInput(
                        "automatic source bundles are managed by scanning".into(),
                    ));
                }
                Some(None) => {}
            }
        }
        if self.conn.execute("UPDATE models SET bundle_id=?1,metadata_version=metadata_version+1,updated_at=?2 WHERE id=?3",params![bundle_id,timestamp(),model_id])?==0{return Err(LibraryError::ModelNotFound(model_id.into()));}
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    fn ensure_model(&self, id: &str) -> Result<(), LibraryError> {
        if self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM models WHERE id=?1)",
            params![id],
            |r| r.get(0),
        )? {
            Ok(())
        } else {
            Err(LibraryError::ModelNotFound(id.into()))
        }
    }

    fn ensure_folder(&self, id: &str) -> Result<(), LibraryError> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
            params![id],
            |row| row.get(0),
        )?;
        if exists {
            Ok(())
        } else {
            Err(LibraryError::FolderNotFound(id.to_string()))
        }
    }

    fn get_folder(&self, id: &str) -> Result<Option<Folder>, LibraryError> {
        self.conn.query_row(
            "SELECT f.id, f.name, f.parent_id,
                    (SELECT COUNT(*) FROM models m WHERE m.folder_id = f.id AND m.deleted_at IS NULL)
             FROM folders f WHERE f.id = ?1",
            params![id],
            |row| Ok(Folder {
                id: row.get(0)?, name: row.get(1)?, parent_id: row.get(2)?, model_count: row.get::<_, i64>(3)? as u64,
            }),
        ).optional().map_err(Into::into)
    }

    fn tags_for(&self, model_id: &str) -> Result<Vec<String>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name FROM tags t JOIN model_tags mt ON mt.tag_id = t.id WHERE mt.model_id = ?1 ORDER BY lower(t.name), t.name",
        )?;
        let rows = stmt.query_map(params![model_id], |row| row.get(0))?;
        Ok(rows.collect::<Result<Vec<String>, _>>()?)
    }

    fn tags_by_model(&self) -> Result<HashMap<String, Vec<String>>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT mt.model_id, t.name FROM model_tags mt
             JOIN tags t ON t.id = mt.tag_id
             ORDER BY mt.model_id, lower(t.name), t.name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut tags = HashMap::<String, Vec<String>>::new();
        for row in rows {
            let (model_id, tag) = row?;
            tags.entry(model_id).or_default().push(tag);
        }
        Ok(tags)
    }

    fn filament_ids_for_model(&self, model_id: &str) -> Result<Vec<String>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT filament_id FROM model_filaments WHERE model_id=?1 ORDER BY ordinal",
        )?;
        let rows = stmt.query_map(params![model_id], |row| row.get(0))?;
        Ok(rows.collect::<Result<Vec<String>, _>>()?)
    }

    fn filaments_by_model(&self) -> Result<HashMap<String, Vec<String>>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT model_id,filament_id FROM model_filaments ORDER BY model_id,ordinal",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut filaments = HashMap::<String, Vec<String>>::new();
        for row in rows {
            let (model_id, filament_id) = row?;
            filaments.entry(model_id).or_default().push(filament_id);
        }
        Ok(filaments)
    }
}

const MODEL_SELECT: &str = "SELECT m.id, m.display_name, m.description, m.folder_id,
            COALESCE(r.original_name, d.original_name), COALESCE(r.format, d.format),
            COALESCE(r.bytes, d.bytes, 0), COALESCE(r.blob_hash, d.content_hash),
            b.relative_path, m.metadata_version, m.created_at, m.updated_at,
            m.source_kind,
            CASE WHEN m.source_kind = 'indexed'
                 THEN CASE WHEN (ir.path = '/' AND substr(m.source_path, 1, 1) = '/')
                                OR (ir.path <> '/' AND (
                                    m.source_path = ir.path
                                    OR substr(m.source_path, 1, length(ir.path) + 1) = ir.path || '/'
                                    OR substr(m.source_path, 1, length(ir.path) + 1) = ir.path || char(92)
                                ))
                           THEN m.source_path
                           ELSE rtrim(ir.path, '/') || '/' || m.source_path
                      END
                 ELSE b.relative_path END,
            m.source_root_id, m.source_missing, m.source_modified_at,
            (SELECT t.relative_path FROM thumbnails t
             WHERE t.model_id = m.id
               AND (t.is_custom = 1 OR t.source_content_hash IS NULL
                    OR t.source_content_hash = COALESCE(r.blob_hash, d.content_hash))
             ORDER BY t.is_active DESC, t.is_custom DESC, t.ordinal, t.id LIMIT 1),
            m.designer, m.source_url, m.notes, m.model_license, m.print_status,
            m.parent_model_id, m.last_printed_at, m.print_count, m.bundle_id,
            m.source_archive_member,m.catalog_kind,m.rating,m.favorite
     FROM models m
     LEFT JOIN revisions r ON r.id = m.current_revision_id
     LEFT JOIN blobs b ON b.hash = r.blob_hash
     LEFT JOIN indexed_model_data d ON d.model_id = m.id
     LEFT JOIN index_roots ir ON ir.id = m.source_root_id";

const FILAMENT_SELECT: &str = "SELECT f.id,f.version,f.brand,f.material,f.color,f.name,f.notes,
       f.origin,f.resource_kind,f.source_id,f.external_id,f.parent_filament_id,
       f.archived,f.source_missing,f.created_at,f.updated_at FROM filaments f";

const SCHEMA: &str = r#"
CREATE TABLE libraries (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL);
INSERT INTO libraries (id, name, created_at) VALUES ('local', 'Local Library', '1970-01-01T00:00:00Z');
CREATE TABLE catalog_settings (
    id TEXT PRIMARY KEY CHECK(id = 'local'),
    thumbnail_size INTEGER NOT NULL CHECK(thumbnail_size IN (128, 256, 512)),
    thumbnail_background TEXT NOT NULL CHECK(
        length(thumbnail_background) = 7
        AND substr(thumbnail_background, 1, 1) = '#'
        AND substr(thumbnail_background, 2) NOT GLOB '*[^0-9A-Fa-f]*'
    ),
    thumbnail_concurrency INTEGER NOT NULL CHECK(thumbnail_concurrency BETWEEN 1 AND 2),
    scan_batch_size INTEGER NOT NULL CHECK(scan_batch_size BETWEEN 16 AND 256)
);
INSERT INTO catalog_settings VALUES ('local', 256, '#12171F', 2, 128);
CREATE TABLE folders (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    parent_id TEXT REFERENCES folders(id) ON DELETE RESTRICT,
    UNIQUE(parent_id, name)
);
INSERT INTO folders (id, name, parent_id) VALUES ('root', 'All Models', NULL);
CREATE TABLE blobs (
    hash TEXT PRIMARY KEY,
    relative_path TEXT NOT NULL UNIQUE,
    bytes INTEGER NOT NULL CHECK(bytes >= 0)
);
CREATE TABLE models (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    folder_id TEXT NOT NULL REFERENCES folders(id) ON DELETE RESTRICT,
    current_revision_id TEXT,
    metadata_version INTEGER NOT NULL DEFAULT 1 CHECK(metadata_version > 0),
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    source_kind TEXT NOT NULL DEFAULT 'managed' CHECK(source_kind IN ('managed', 'indexed')),
    source_path TEXT,
    source_root_id TEXT,
    source_missing INTEGER NOT NULL DEFAULT 0 CHECK(source_missing IN (0, 1)),
    source_modified_at INTEGER,
    source_archive_member TEXT,
    designer TEXT, source_url TEXT, notes TEXT, model_license TEXT,
    print_status TEXT NOT NULL DEFAULT 'unprinted',
    parent_model_id TEXT,
    last_printed_at TEXT, print_count INTEGER NOT NULL DEFAULT 0,
    bundle_id TEXT,
    catalog_kind TEXT NOT NULL DEFAULT 'file' CHECK(catalog_kind IN ('file','bookmark')),
    rating INTEGER NOT NULL DEFAULT 0 CHECK(rating BETWEEN 0 AND 5),
    favorite INTEGER NOT NULL DEFAULT 0 CHECK(favorite IN (0,1))
);
CREATE INDEX models_folder_idx ON models(folder_id, deleted_at, created_at, id);
CREATE TABLE revisions (
    id TEXT PRIMARY KEY,
    model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    blob_hash TEXT NOT NULL REFERENCES blobs(hash) ON DELETE RESTRICT,
    original_name TEXT NOT NULL,
    format TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK(bytes >= 0),
    created_at TEXT NOT NULL
);
CREATE INDEX revisions_model_idx ON revisions(model_id, created_at);
CREATE TABLE tags (id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE, category TEXT);
CREATE TABLE model_tags (
    model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY(model_id, tag_id)
);
CREATE TABLE manuals (model_id TEXT PRIMARY KEY REFERENCES models(id) ON DELETE CASCADE, relative_path TEXT NOT NULL, content_hash TEXT NOT NULL);
CREATE TABLE thumbnails (id TEXT PRIMARY KEY, model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE, relative_path TEXT NOT NULL, content_hash TEXT NOT NULL, is_custom INTEGER NOT NULL DEFAULT 0, source_content_hash TEXT, ordinal INTEGER NOT NULL DEFAULT 0 CHECK(ordinal >= 0), is_active INTEGER NOT NULL DEFAULT 0 CHECK(is_active IN (0,1)));
CREATE INDEX thumbnails_model_idx ON thumbnails(model_id, is_active DESC, ordinal, id);
CREATE UNIQUE INDEX thumbnails_active_model_idx ON thumbnails(model_id) WHERE is_active=1;
CREATE TABLE jobs (id TEXT PRIMARY KEY, model_id TEXT REFERENCES models(id) ON DELETE CASCADE, kind TEXT NOT NULL, status TEXT NOT NULL, retry_count INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE index_roots (
    id TEXT PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0, 1)),
    last_scanned_at TEXT,
    scan_interval_minutes INTEGER NOT NULL CHECK(scan_interval_minutes BETWEEN 1 AND 1440),
    max_file_bytes INTEGER NOT NULL CHECK(max_file_bytes BETWEEN 1 AND 104857600),
    scan_zip INTEGER NOT NULL DEFAULT 0 CHECK(scan_zip IN (0,1))
);
CREATE TABLE indexed_model_data (
    model_id TEXT PRIMARY KEY REFERENCES models(id) ON DELETE CASCADE,
    content_hash TEXT NOT NULL,
    original_name TEXT NOT NULL,
    format TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK(bytes >= 0),
    outer_fingerprint TEXT
);
CREATE INDEX indexed_model_hash_idx ON indexed_model_data(content_hash, model_id);
CREATE UNIQUE INDEX indexed_model_source_idx ON models(source_root_id, source_path, COALESCE(source_archive_member,''))
    WHERE source_kind = 'indexed';
CREATE TABLE bundles (id TEXT PRIMARY KEY, source_root_id TEXT, source_path TEXT, created_at TEXT NOT NULL, UNIQUE(source_root_id,source_path));
CREATE TABLE printers (
    id TEXT PRIMARY KEY,
    metadata_version INTEGER NOT NULL DEFAULT 1 CHECK(metadata_version > 0),
    nickname TEXT NOT NULL CHECK(length(nickname) BETWEEN 1 AND 128),
    make TEXT CHECK(make IS NULL OR length(make) <= 128),
    model TEXT CHECK(model IS NULL OR length(model) <= 128),
    printer_type TEXT CHECK(printer_type IS NULL OR length(printer_type) <= 128),
    firmware TEXT CHECK(firmware IS NULL OR length(firmware) <= 128),
    host TEXT CHECK(host IS NULL OR length(host) <= 255),
    web_url TEXT CHECK(web_url IS NULL OR length(web_url) <= 2048),
    endpoint_approved INTEGER NOT NULL DEFAULT 0 CHECK(endpoint_approved IN (0,1)),
    bed_x_mm REAL, bed_y_mm REAL, bed_z_mm REAL,
    notes TEXT NOT NULL DEFAULT '' CHECK(length(notes) <= 20000),
    source_kind TEXT NOT NULL DEFAULT 'manual' CHECK(source_kind IN ('manual','discovered','imported')),
    connection_status TEXT NOT NULL DEFAULT 'unknown'
        CHECK(connection_status IN ('unknown','online','offline','auth_required','error')),
    job_status TEXT NOT NULL DEFAULT 'unknown'
        CHECK(job_status IN ('unknown','idle','printing','paused','complete')),
    last_checked_at TEXT,
    connection_error_code TEXT CHECK(connection_error_code IS NULL OR length(connection_error_code) <= 64),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK((bed_x_mm IS NULL AND bed_y_mm IS NULL AND bed_z_mm IS NULL)
       OR (bed_x_mm BETWEEN 0.1 AND 10000 AND bed_y_mm BETWEEN 0.1 AND 10000 AND bed_z_mm BETWEEN 0.1 AND 10000)),
    CHECK(connection_status='online' OR job_status='unknown')
);
CREATE INDEX printers_name_idx ON printers(nickname COLLATE NOCASE,id);
CREATE TABLE printer_maintenance_logs (
    id TEXT PRIMARY KEY,
    metadata_version INTEGER NOT NULL DEFAULT 1 CHECK(metadata_version > 0),
    printer_id TEXT REFERENCES printers(id) ON DELETE SET NULL,
    printer_nickname TEXT NOT NULL CHECK(length(printer_nickname) BETWEEN 0 AND 128),
    title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 128),
    performed_at TEXT NOT NULL,
    notes TEXT NOT NULL CHECK(length(notes) <= 20000),
    created_at TEXT NOT NULL
);
CREATE INDEX printer_maintenance_printer_idx ON printer_maintenance_logs(printer_id,performed_at DESC,id);
CREATE TABLE printer_reminders (
    id TEXT PRIMARY KEY,
    metadata_version INTEGER NOT NULL DEFAULT 1 CHECK(metadata_version > 0),
    printer_id TEXT REFERENCES printers(id) ON DELETE SET NULL,
    printer_nickname TEXT NOT NULL CHECK(length(printer_nickname) BETWEEN 0 AND 128),
    title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 128),
    notes TEXT NOT NULL CHECK(length(notes) <= 20000),
    due_at TEXT NOT NULL,
    interval_days INTEGER CHECK(interval_days IS NULL OR interval_days BETWEEN 1 AND 36500),
    status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active','paused','completed')),
    last_completed_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX printer_reminders_due_idx ON printer_reminders(status,due_at,id);
CREATE INDEX printer_reminders_printer_idx ON printer_reminders(printer_id,status,due_at,id);
CREATE TABLE print_attempts (
    id TEXT PRIMARY KEY,
    model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    printed_at TEXT NOT NULL,
    quantity INTEGER NOT NULL,
    notes TEXT NOT NULL,
    outcome TEXT NOT NULL,
    stock_adjusted INTEGER NOT NULL DEFAULT 1 CHECK(stock_adjusted IN (0,1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    printer_id TEXT REFERENCES printers(id) ON DELETE SET NULL,
    printer_name TEXT
);
CREATE INDEX print_attempt_printer_idx ON print_attempts(printer_id,printed_at DESC,id);
CREATE TABLE print_attempt_filaments (attempt_id TEXT NOT NULL REFERENCES print_attempts(id) ON DELETE CASCADE, filament_id TEXT NOT NULL, ordinal INTEGER NOT NULL, PRIMARY KEY(attempt_id,filament_id));
CREATE TABLE parts_stock (
    id TEXT PRIMARY KEY,
    metadata_version INTEGER NOT NULL DEFAULT 1 CHECK(metadata_version > 0),
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 128),
    category TEXT CHECK(category IS NULL OR length(category) <= 128),
    quantity INTEGER NOT NULL DEFAULT 0 CHECK(quantity >= 0),
    unit TEXT NOT NULL DEFAULT 'pcs' CHECK(length(unit) BETWEEN 1 AND 128),
    notes TEXT NOT NULL DEFAULT '' CHECK(length(notes) <= 20000),
    low_stock INTEGER NOT NULL DEFAULT 0 CHECK(low_stock BETWEEN 0 AND 1000000),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX parts_stock_name_idx ON parts_stock(name COLLATE NOCASE,id);
CREATE TABLE print_attempt_parts (
    attempt_id TEXT NOT NULL REFERENCES print_attempts(id) ON DELETE CASCADE,
    part_id TEXT NOT NULL,
    part_name TEXT NOT NULL CHECK(length(part_name) BETWEEN 1 AND 128),
    category TEXT,
    unit TEXT NOT NULL,
    quantity_per_copy INTEGER NOT NULL CHECK(quantity_per_copy BETWEEN 1 AND 9999),
    quantity INTEGER NOT NULL CHECK(quantity BETWEEN 1 AND 100000000),
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    PRIMARY KEY(attempt_id,ordinal)
);
CREATE UNIQUE INDEX print_attempt_parts_unique_part_idx
    ON print_attempt_parts(attempt_id,part_id) WHERE part_id IS NOT NULL;
CREATE INDEX print_attempt_parts_part_idx ON print_attempt_parts(part_id,attempt_id);
CREATE TABLE filament_sources (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind='spoolman'),
    base_url TEXT NOT NULL UNIQUE,
    last_synced_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE filaments (
    id TEXT PRIMARY KEY,
    version INTEGER NOT NULL DEFAULT 1 CHECK(version > 0),
    brand TEXT NOT NULL DEFAULT '', material TEXT NOT NULL DEFAULT '',
    color TEXT NOT NULL DEFAULT '', name TEXT NOT NULL, notes TEXT NOT NULL DEFAULT '',
    origin TEXT NOT NULL CHECK(origin IN ('manual','spoolman')),
    resource_kind TEXT NOT NULL CHECK(resource_kind IN ('manual','filament','spool')),
    source_id TEXT REFERENCES filament_sources(id) ON DELETE RESTRICT,
    external_id TEXT,
    parent_filament_id TEXT REFERENCES filaments(id) ON DELETE RESTRICT,
    archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1)),
    source_missing INTEGER NOT NULL DEFAULT 0 CHECK(source_missing IN (0,1)),
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
    CHECK((origin='manual' AND resource_kind='manual' AND source_id IS NULL AND external_id IS NULL AND parent_filament_id IS NULL AND source_missing=0)
       OR (origin='spoolman' AND resource_kind IN ('filament','spool') AND source_id IS NOT NULL AND external_id IS NOT NULL))
);
CREATE UNIQUE INDEX filaments_remote_identity_idx ON filaments(source_id,resource_kind,external_id)
    WHERE source_id IS NOT NULL;
CREATE TABLE model_filaments (
    model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    filament_id TEXT NOT NULL REFERENCES filaments(id) ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 4),
    PRIMARY KEY(model_id,filament_id), UNIQUE(model_id,ordinal)
);
CREATE INDEX model_filaments_filament_idx ON model_filaments(filament_id,model_id);
CREATE TABLE capture_items (
    item_id TEXT PRIMARY KEY,
    payload_hash TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('imported','partial','failed')),
    page_model_id TEXT REFERENCES models(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    received_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    error_code TEXT
);
CREATE TABLE capture_downloads (
    capture_id TEXT PRIMARY KEY,
    item_id TEXT NOT NULL REFERENCES capture_items(item_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    file_name TEXT NOT NULL,
    expected_hash TEXT,
    expected_bytes INTEGER CHECK(expected_bytes IS NULL OR expected_bytes >= 0),
    status TEXT NOT NULL CHECK(status IN ('needs_file','imported','failed')),
    model_id TEXT REFERENCES models(id) ON DELETE RESTRICT,
    error_code TEXT,
    UNIQUE(item_id,ordinal)
);
CREATE INDEX capture_downloads_item_idx ON capture_downloads(item_id,ordinal);
"#;

const MIGRATE_V1_TO_V2: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE models ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'managed'
    CHECK(source_kind IN ('managed', 'indexed'));
ALTER TABLE models ADD COLUMN source_path TEXT;
ALTER TABLE models ADD COLUMN source_root_id TEXT;
ALTER TABLE models ADD COLUMN source_missing INTEGER NOT NULL DEFAULT 0
    CHECK(source_missing IN (0, 1));
ALTER TABLE models ADD COLUMN source_modified_at INTEGER;
ALTER TABLE thumbnails ADD COLUMN source_content_hash TEXT;
CREATE INDEX thumbnails_model_idx ON thumbnails(model_id, is_custom);
CREATE TABLE catalog_settings (
    id TEXT PRIMARY KEY CHECK(id = 'local'),
    thumbnail_size INTEGER NOT NULL CHECK(thumbnail_size IN (128, 256, 512)),
    thumbnail_background TEXT NOT NULL CHECK(
        length(thumbnail_background) = 7
        AND substr(thumbnail_background, 1, 1) = '#'
        AND substr(thumbnail_background, 2) NOT GLOB '*[^0-9A-Fa-f]*'
    ),
    thumbnail_concurrency INTEGER NOT NULL CHECK(thumbnail_concurrency BETWEEN 1 AND 2),
    scan_batch_size INTEGER NOT NULL CHECK(scan_batch_size BETWEEN 16 AND 256)
);
INSERT INTO catalog_settings VALUES ('local', 256, '#12171F', 2, 128);
CREATE TABLE index_roots (
    id TEXT PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0, 1)),
    last_scanned_at TEXT,
    scan_interval_minutes INTEGER NOT NULL CHECK(scan_interval_minutes BETWEEN 1 AND 1440),
    max_file_bytes INTEGER NOT NULL CHECK(max_file_bytes BETWEEN 1 AND 104857600)
);
CREATE TABLE indexed_model_data (
    model_id TEXT PRIMARY KEY REFERENCES models(id) ON DELETE CASCADE,
    content_hash TEXT NOT NULL,
    original_name TEXT NOT NULL,
    format TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK(bytes >= 0)
);
CREATE INDEX indexed_model_hash_idx ON indexed_model_data(content_hash, model_id);
CREATE UNIQUE INDEX indexed_model_source_idx ON models(source_root_id, source_path)
    WHERE source_kind = 'indexed';
PRAGMA user_version = 2;
COMMIT;
"#;

const MIGRATE_V2_TO_V3: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE index_roots ADD COLUMN scan_zip INTEGER NOT NULL DEFAULT 0 CHECK(scan_zip IN (0,1));
ALTER TABLE models ADD COLUMN source_archive_member TEXT;
ALTER TABLE models ADD COLUMN designer TEXT;
ALTER TABLE models ADD COLUMN source_url TEXT;
ALTER TABLE models ADD COLUMN notes TEXT;
UPDATE models SET notes = description WHERE description != '';
ALTER TABLE models ADD COLUMN model_license TEXT;
ALTER TABLE models ADD COLUMN print_status TEXT NOT NULL DEFAULT 'unprinted';
ALTER TABLE models ADD COLUMN parent_model_id TEXT;
ALTER TABLE models ADD COLUMN last_printed_at TEXT;
ALTER TABLE models ADD COLUMN print_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE models ADD COLUMN bundle_id TEXT;
ALTER TABLE tags ADD COLUMN category TEXT;
ALTER TABLE indexed_model_data ADD COLUMN outer_fingerprint TEXT;
DROP INDEX indexed_model_source_idx;
CREATE UNIQUE INDEX indexed_model_source_idx ON models(source_root_id, source_path, COALESCE(source_archive_member,'')) WHERE source_kind='indexed';
CREATE TABLE bundles (id TEXT PRIMARY KEY, source_root_id TEXT, source_path TEXT, created_at TEXT NOT NULL, UNIQUE(source_root_id,source_path));
CREATE TABLE print_attempts (id TEXT PRIMARY KEY, model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE, printed_at TEXT NOT NULL, quantity INTEGER NOT NULL, notes TEXT NOT NULL, outcome TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE print_attempt_filaments (attempt_id TEXT NOT NULL REFERENCES print_attempts(id) ON DELETE CASCADE, filament_id TEXT NOT NULL, ordinal INTEGER NOT NULL, PRIMARY KEY(attempt_id,filament_id));
PRAGMA user_version=3;
COMMIT;
"#;

const MIGRATE_V3_TO_V4: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE filament_sources (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind='spoolman'),
    base_url TEXT NOT NULL UNIQUE,
    last_synced_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE filaments (
    id TEXT PRIMARY KEY,
    version INTEGER NOT NULL DEFAULT 1 CHECK(version > 0),
    brand TEXT NOT NULL DEFAULT '', material TEXT NOT NULL DEFAULT '',
    color TEXT NOT NULL DEFAULT '', name TEXT NOT NULL, notes TEXT NOT NULL DEFAULT '',
    origin TEXT NOT NULL CHECK(origin IN ('manual','spoolman')),
    resource_kind TEXT NOT NULL CHECK(resource_kind IN ('manual','filament','spool')),
    source_id TEXT REFERENCES filament_sources(id) ON DELETE RESTRICT,
    external_id TEXT,
    parent_filament_id TEXT REFERENCES filaments(id) ON DELETE RESTRICT,
    archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1)),
    source_missing INTEGER NOT NULL DEFAULT 0 CHECK(source_missing IN (0,1)),
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
    CHECK((origin='manual' AND resource_kind='manual' AND source_id IS NULL AND external_id IS NULL AND parent_filament_id IS NULL AND source_missing=0)
       OR (origin='spoolman' AND resource_kind IN ('filament','spool') AND source_id IS NOT NULL AND external_id IS NOT NULL))
);
CREATE UNIQUE INDEX filaments_remote_identity_idx ON filaments(source_id,resource_kind,external_id)
    WHERE source_id IS NOT NULL;
CREATE TABLE model_filaments (
    model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    filament_id TEXT NOT NULL REFERENCES filaments(id) ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 4),
    PRIMARY KEY(model_id,filament_id), UNIQUE(model_id,ordinal)
);
CREATE INDEX model_filaments_filament_idx ON model_filaments(filament_id,model_id);
PRAGMA user_version=4;
COMMIT;
"#;

const MIGRATE_V4_TO_V5: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE models ADD COLUMN catalog_kind TEXT NOT NULL DEFAULT 'file'
    CHECK(catalog_kind IN ('file','bookmark'));
ALTER TABLE thumbnails ADD COLUMN ordinal INTEGER NOT NULL DEFAULT 0 CHECK(ordinal >= 0);
ALTER TABLE thumbnails ADD COLUMN is_active INTEGER NOT NULL DEFAULT 0 CHECK(is_active IN (0,1));
UPDATE thumbnails AS target
SET ordinal=(SELECT COUNT(*) FROM thumbnails prior
             WHERE prior.model_id=target.model_id AND prior.id < target.id);
UPDATE thumbnails SET is_active=1
WHERE id=(SELECT candidate.id FROM thumbnails candidate
          WHERE candidate.model_id=thumbnails.model_id
          ORDER BY candidate.is_custom DESC,candidate.id LIMIT 1);
DROP INDEX IF EXISTS thumbnails_model_idx;
CREATE INDEX thumbnails_model_idx ON thumbnails(model_id,is_active DESC,ordinal,id);
CREATE UNIQUE INDEX thumbnails_active_model_idx ON thumbnails(model_id) WHERE is_active=1;
CREATE TABLE capture_items (
    item_id TEXT PRIMARY KEY,
    payload_hash TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('imported','partial','failed')),
    page_model_id TEXT REFERENCES models(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    received_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    error_code TEXT
);
CREATE TABLE capture_downloads (
    capture_id TEXT PRIMARY KEY,
    item_id TEXT NOT NULL REFERENCES capture_items(item_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    file_name TEXT NOT NULL,
    expected_hash TEXT,
    expected_bytes INTEGER CHECK(expected_bytes IS NULL OR expected_bytes >= 0),
    status TEXT NOT NULL CHECK(status IN ('needs_file','imported','failed')),
    model_id TEXT REFERENCES models(id) ON DELETE RESTRICT,
    error_code TEXT,
    UNIQUE(item_id,ordinal)
);
CREATE INDEX capture_downloads_item_idx ON capture_downloads(item_id,ordinal);
PRAGMA user_version=5;
COMMIT;
"#;

const MIGRATE_V5_TO_V6: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE models ADD COLUMN rating INTEGER NOT NULL DEFAULT 0 CHECK(rating BETWEEN 0 AND 5);
ALTER TABLE models ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0 CHECK(favorite IN (0,1));
PRAGMA user_version=6;
COMMIT;
"#;

const MIGRATE_V6_TO_V7: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE printers (
    id TEXT PRIMARY KEY,
    metadata_version INTEGER NOT NULL DEFAULT 1 CHECK(metadata_version > 0),
    nickname TEXT NOT NULL CHECK(length(nickname) BETWEEN 1 AND 128),
    make TEXT CHECK(make IS NULL OR length(make) <= 128),
    model TEXT CHECK(model IS NULL OR length(model) <= 128),
    printer_type TEXT CHECK(printer_type IS NULL OR length(printer_type) <= 128),
    firmware TEXT CHECK(firmware IS NULL OR length(firmware) <= 128),
    host TEXT CHECK(host IS NULL OR length(host) <= 255),
    web_url TEXT CHECK(web_url IS NULL OR length(web_url) <= 2048),
    endpoint_approved INTEGER NOT NULL DEFAULT 0 CHECK(endpoint_approved IN (0,1)),
    bed_x_mm REAL, bed_y_mm REAL, bed_z_mm REAL,
    notes TEXT NOT NULL DEFAULT '' CHECK(length(notes) <= 20000),
    source_kind TEXT NOT NULL DEFAULT 'manual' CHECK(source_kind IN ('manual','discovered','imported')),
    connection_status TEXT NOT NULL DEFAULT 'unknown'
        CHECK(connection_status IN ('unknown','online','offline','auth_required','error')),
    job_status TEXT NOT NULL DEFAULT 'unknown'
        CHECK(job_status IN ('unknown','idle','printing','paused','complete')),
    last_checked_at TEXT,
    connection_error_code TEXT CHECK(connection_error_code IS NULL OR length(connection_error_code) <= 64),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK((bed_x_mm IS NULL AND bed_y_mm IS NULL AND bed_z_mm IS NULL)
       OR (bed_x_mm BETWEEN 0.1 AND 10000 AND bed_y_mm BETWEEN 0.1 AND 10000 AND bed_z_mm BETWEEN 0.1 AND 10000)),
    CHECK(connection_status='online' OR job_status='unknown')
);
CREATE INDEX printers_name_idx ON printers(nickname COLLATE NOCASE,id);
CREATE TABLE printer_maintenance_logs (
    id TEXT PRIMARY KEY,
    printer_id TEXT REFERENCES printers(id) ON DELETE SET NULL,
    printer_nickname TEXT NOT NULL CHECK(length(printer_nickname) BETWEEN 1 AND 128),
    title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 128),
    performed_at TEXT NOT NULL,
    notes TEXT NOT NULL CHECK(length(notes) <= 20000),
    created_at TEXT NOT NULL
);
CREATE INDEX printer_maintenance_printer_idx ON printer_maintenance_logs(printer_id,performed_at DESC,id);
CREATE TABLE printer_reminders (
    id TEXT PRIMARY KEY,
    metadata_version INTEGER NOT NULL DEFAULT 1 CHECK(metadata_version > 0),
    printer_id TEXT REFERENCES printers(id) ON DELETE SET NULL,
    printer_nickname TEXT NOT NULL CHECK(length(printer_nickname) BETWEEN 1 AND 128),
    title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 128),
    notes TEXT NOT NULL CHECK(length(notes) <= 20000),
    due_at TEXT NOT NULL,
    interval_days INTEGER CHECK(interval_days IS NULL OR interval_days BETWEEN 1 AND 36500),
    status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active','paused','completed')),
    last_completed_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX printer_reminders_due_idx ON printer_reminders(status,due_at,id);
CREATE INDEX printer_reminders_printer_idx ON printer_reminders(printer_id,status,due_at,id);
ALTER TABLE print_attempts ADD COLUMN printer_id TEXT REFERENCES printers(id) ON DELETE SET NULL;
ALTER TABLE print_attempts ADD COLUMN printer_name TEXT;
CREATE INDEX print_attempt_printer_idx ON print_attempts(printer_id,printed_at DESC,id);
PRAGMA user_version=7;
COMMIT;
"#;

const MIGRATE_V7_TO_V8: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE printer_maintenance_logs
    ADD COLUMN metadata_version INTEGER NOT NULL DEFAULT 1 CHECK(metadata_version > 0);
PRAGMA user_version=8;
COMMIT;
"#;

const MIGRATE_V8_TO_V9: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE parts_stock (
    id TEXT PRIMARY KEY,
    metadata_version INTEGER NOT NULL DEFAULT 1 CHECK(metadata_version > 0),
    name TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 128),
    category TEXT CHECK(category IS NULL OR length(category) <= 128),
    quantity INTEGER NOT NULL DEFAULT 0 CHECK(quantity >= 0),
    unit TEXT NOT NULL DEFAULT 'pcs' CHECK(length(unit) BETWEEN 1 AND 128),
    notes TEXT NOT NULL DEFAULT '' CHECK(length(notes) <= 20000),
    low_stock INTEGER NOT NULL DEFAULT 0 CHECK(low_stock BETWEEN 0 AND 1000000),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX parts_stock_name_idx ON parts_stock(name COLLATE NOCASE,id);
CREATE TABLE print_attempt_parts (
    attempt_id TEXT NOT NULL REFERENCES print_attempts(id) ON DELETE CASCADE,
    part_id TEXT NOT NULL,
    part_name TEXT NOT NULL CHECK(length(part_name) BETWEEN 1 AND 128),
    category TEXT,
    unit TEXT NOT NULL,
    quantity_per_copy INTEGER NOT NULL CHECK(quantity_per_copy BETWEEN 1 AND 9999),
    quantity INTEGER NOT NULL CHECK(quantity BETWEEN 1 AND 100000000),
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    PRIMARY KEY(attempt_id,ordinal)
);
CREATE UNIQUE INDEX print_attempt_parts_unique_part_idx
    ON print_attempt_parts(attempt_id,part_id) WHERE part_id IS NOT NULL;
CREATE INDEX print_attempt_parts_part_idx ON print_attempt_parts(part_id,attempt_id);
PRAGMA user_version=9;
COMMIT;
"#;

const MIGRATE_V9_TO_V10: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE print_attempts
    ADD COLUMN stock_adjusted INTEGER CHECK(stock_adjusted IN (0,1));
PRAGMA user_version=10;
COMMIT;
"#;

const MIGRATE_V10_TO_V11: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE printer_maintenance_logs_v11 (
    id TEXT PRIMARY KEY,
    metadata_version INTEGER NOT NULL DEFAULT 1 CHECK(metadata_version > 0),
    printer_id TEXT REFERENCES printers(id) ON DELETE SET NULL,
    printer_nickname TEXT NOT NULL CHECK(length(printer_nickname) BETWEEN 0 AND 128),
    title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 128),
    performed_at TEXT NOT NULL,
    notes TEXT NOT NULL CHECK(length(notes) <= 20000),
    created_at TEXT NOT NULL
);
INSERT INTO printer_maintenance_logs_v11
    (id,metadata_version,printer_id,printer_nickname,title,performed_at,notes,created_at)
SELECT id,metadata_version,printer_id,printer_nickname,title,performed_at,notes,created_at
FROM printer_maintenance_logs;
DROP TABLE printer_maintenance_logs;
ALTER TABLE printer_maintenance_logs_v11 RENAME TO printer_maintenance_logs;
CREATE INDEX printer_maintenance_printer_idx
    ON printer_maintenance_logs(printer_id,performed_at DESC,id);

CREATE TABLE printer_reminders_v11 (
    id TEXT PRIMARY KEY,
    metadata_version INTEGER NOT NULL DEFAULT 1 CHECK(metadata_version > 0),
    printer_id TEXT REFERENCES printers(id) ON DELETE SET NULL,
    printer_nickname TEXT NOT NULL CHECK(length(printer_nickname) BETWEEN 0 AND 128),
    title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 128),
    notes TEXT NOT NULL CHECK(length(notes) <= 20000),
    due_at TEXT NOT NULL,
    interval_days INTEGER CHECK(interval_days IS NULL OR interval_days BETWEEN 1 AND 36500),
    status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active','paused','completed')),
    last_completed_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
INSERT INTO printer_reminders_v11
    (id,metadata_version,printer_id,printer_nickname,title,notes,due_at,interval_days,
     status,last_completed_at,created_at,updated_at)
SELECT id,metadata_version,printer_id,printer_nickname,title,notes,due_at,interval_days,
       status,last_completed_at,created_at,updated_at
FROM printer_reminders;
DROP TABLE printer_reminders;
ALTER TABLE printer_reminders_v11 RENAME TO printer_reminders;
CREATE INDEX printer_reminders_due_idx ON printer_reminders(status,due_at,id);
CREATE INDEX printer_reminders_printer_idx
    ON printer_reminders(printer_id,status,due_at,id);
PRAGMA user_version=11;
COMMIT;
"#;

const ENSURE_CATALOG_SETTINGS: &str = r#"
CREATE TABLE IF NOT EXISTS catalog_settings (
    id TEXT PRIMARY KEY CHECK(id = 'local'),
    thumbnail_size INTEGER NOT NULL CHECK(thumbnail_size IN (128, 256, 512)),
    thumbnail_background TEXT NOT NULL CHECK(
        length(thumbnail_background) = 7
        AND substr(thumbnail_background, 1, 1) = '#'
        AND substr(thumbnail_background, 2) NOT GLOB '*[^0-9A-Fa-f]*'
    ),
    thumbnail_concurrency INTEGER NOT NULL CHECK(thumbnail_concurrency BETWEEN 1 AND 2),
    scan_batch_size INTEGER NOT NULL CHECK(scan_batch_size BETWEEN 16 AND 256)
);
INSERT OR IGNORE INTO catalog_settings
    (id, thumbnail_size, thumbnail_background, thumbnail_concurrency, scan_batch_size)
VALUES ('local', 256, '#12171F', 2, 128);
"#;

const ENSURE_PRINT_BATCH_RECEIPTS: &str = r#"
CREATE TABLE IF NOT EXISTS print_batch_receipts (
    idempotency_key TEXT PRIMARY KEY,
    payload_hash TEXT NOT NULL,
    attempt_ids_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
"#;

fn model_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Model> {
    Ok(Model {
        id: row.get(0)?,
        display_name: row.get(1)?,
        description: row.get(2)?,
        folder_id: row.get(3)?,
        original_name: row.get(4)?,
        format: row.get(5)?,
        bytes: row.get::<_, i64>(6)? as u64,
        content_hash: row.get(7)?,
        storage_path: row.get(8)?,
        metadata_version: row.get::<_, i64>(9)? as u64,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        tags: Vec::new(),
        filament_ids: Vec::new(),
        source_kind: row.get(12)?,
        source_path: row.get(13)?,
        source_root_id: row.get(14)?,
        source_missing: row.get(15)?,
        source_modified_at: row.get::<_, Option<i64>>(16)?.map(|value| value as u64),
        thumbnail_path: row.get(17)?,
        metadata: ModelMetadata {
            designer: row.get(18)?,
            source_url: row.get(19)?,
            notes: row.get(20)?,
            model_license: row.get(21)?,
            print_status: parse_print_status(row.get::<_, String>(22)?)?,
            parent_model_id: row.get(23)?,
            last_printed_at: row.get(24)?,
            print_count: row.get::<_, i64>(25)? as u64,
            rating: u8::try_from(row.get::<_, i64>(29)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            favorite: row.get(30)?,
        },
        bundle_id: row.get(26)?,
        source_archive_member: row.get(27)?,
        catalog_kind: row.get(28)?,
    })
}

fn filament_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Filament> {
    let origin = match row.get::<_, String>(7)?.as_str() {
        "manual" => FilamentOrigin::Manual,
        "spoolman" => FilamentOrigin::Spoolman,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let resource_kind = match row.get::<_, String>(8)?.as_str() {
        "manual" => FilamentResourceKind::Manual,
        "filament" => FilamentResourceKind::Filament,
        "spool" => FilamentResourceKind::Spool,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let version = row.get::<_, i64>(1)?;
    Ok(Filament {
        id: row.get(0)?,
        version: u64::try_from(version)
            .map_err(|_| invalid_persisted_integer(1, "filament version is negative"))?,
        brand: row.get(2)?,
        material: row.get(3)?,
        color: row.get(4)?,
        name: row.get(5)?,
        notes: row.get(6)?,
        origin,
        resource_kind,
        source_id: row.get(9)?,
        external_id: row.get(10)?,
        parent_filament_id: row.get(11)?,
        archived: row.get(12)?,
        source_missing: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

fn spoolman_source_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SpoolmanSource> {
    Ok(SpoolmanSource {
        id: row.get(0)?,
        base_url: row.get(1)?,
        last_synced_at: row.get(2)?,
    })
}

fn thumbnail_gallery_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ThumbnailGalleryItem> {
    let ordinal = row.get::<_, i64>(6)?;
    let is_custom = row.get::<_, i64>(5)?;
    let is_active = row.get::<_, i64>(7)?;
    if ordinal < 0 || !matches!(is_custom, 0 | 1) || !matches!(is_active, 0 | 1) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(ThumbnailGalleryItem {
        id: row.get(0)?,
        model_id: row.get(1)?,
        relative_path: row.get(2)?,
        content_hash: row.get(3)?,
        source_content_hash: row.get(4)?,
        is_custom: is_custom == 1,
        ordinal: u16::try_from(ordinal)
            .map_err(|_| invalid_persisted_integer(6, "thumbnail ordinal exceeds u16"))?,
        is_active: is_active == 1,
    })
}

fn parse_print_status(value: String) -> rusqlite::Result<PrintStatus> {
    match value.as_str() {
        "unprinted" => Ok(PrintStatus::Unprinted),
        "want" => Ok(PrintStatus::Want),
        "queued" => Ok(PrintStatus::Queued),
        "printing" => Ok(PrintStatus::Printing),
        "printed" => Ok(PrintStatus::Printed),
        "failed" => Ok(PrintStatus::Failed),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn index_root_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IndexRoot> {
    let enabled: i64 = row.get(2)?;
    let scan_interval: i64 = row.get(4)?;
    let max_file_bytes: i64 = row.get(5)?;
    let scan_zip: i64 = row.get(6)?;
    let scan_interval_minutes = u32::try_from(scan_interval)
        .map_err(|_| invalid_persisted_integer(4, "scan interval is outside u32"))?;
    let max_file_bytes = u64::try_from(max_file_bytes)
        .map_err(|_| invalid_persisted_integer(5, "maximum file size is negative"))?;
    if !(0..=1).contains(&enabled) {
        return Err(invalid_persisted_integer(2, "enabled must be zero or one"));
    }
    if !(0..=1).contains(&scan_zip) {
        return Err(invalid_persisted_integer(6, "scan_zip must be zero or one"));
    }
    if !(MIN_SCAN_INTERVAL_MINUTES..=MAX_SCAN_INTERVAL_MINUTES).contains(&scan_interval_minutes) {
        return Err(invalid_persisted_integer(
            4,
            "scan interval is outside the supported range",
        ));
    }
    if max_file_bytes == 0 || max_file_bytes > MAX_INDEXED_FILE_BYTES {
        return Err(invalid_persisted_integer(
            5,
            "maximum file size is outside the supported range",
        ));
    }
    Ok(IndexRoot {
        id: row.get(0)?,
        path: PathBuf::from(row.get::<_, String>(1)?),
        enabled: enabled == 1,
        last_scanned_at: row.get(3)?,
        scan_interval_minutes,
        max_file_bytes,
        scan_zip: scan_zip == 1,
    })
}

fn invalid_persisted_integer(index: usize, message: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Integer,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message.to_owned(),
        )),
    )
}

fn clean_name(value: &str, kind: &str) -> Result<String, LibraryError> {
    let name = value.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(LibraryError::InvalidInput(format!(
            "{kind} name is invalid"
        )));
    }
    Ok(name.to_owned())
}

fn clean_tag_name(value: &str) -> Result<String, LibraryError> {
    if value.len() > 256 || value.chars().any(char::is_control) {
        return Err(LibraryError::InvalidInput(
            "Tag name exceeds its limit or contains control characters".into(),
        ));
    }
    clean_name(value, "Tag")
}

fn clean_model_tag_names(values: &[String]) -> Result<Vec<String>, LibraryError> {
    if values.is_empty() || values.len() > 200 {
        return Err(LibraryError::InvalidInput(
            "Model tag count is outside its limit".into(),
        ));
    }
    let mut names = Vec::with_capacity(values.len());
    let mut seen = HashSet::with_capacity(values.len());
    for value in values {
        let name = clean_tag_name(value)?;
        if !seen.insert(name.to_lowercase()) {
            return Err(LibraryError::InvalidInput(
                "Model tag names contain duplicates".into(),
            ));
        }
        names.push(name);
    }
    Ok(names)
}

fn replace_model_tags_by_name_tx(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
    names: &[String],
) -> Result<(), LibraryError> {
    tx.execute(
        "DELETE FROM model_tags WHERE model_id=?1",
        params![model_id],
    )?;
    for name in names {
        let tag_id = tx
            .query_row("SELECT id FROM tags WHERE name=?1", params![name], |row| {
                row.get::<_, String>(0)
            })
            .optional()?;
        let tag_id = if let Some(id) = tag_id {
            id
        } else {
            let id = new_id("tag");
            tx.execute(
                "INSERT INTO tags(id,name,category) VALUES(?1,?2,NULL)",
                params![id, name],
            )?;
            id
        };
        tx.execute(
            "INSERT INTO model_tags(model_id,tag_id) VALUES(?1,?2)",
            params![model_id, tag_id],
        )?;
    }
    Ok(())
}

fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{seconds}")
}

fn new_id(prefix: &str) -> String {
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{counter}", Uuid::new_v4().simple())
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn create_migration_backup(
    conn: &Connection,
    root: &Path,
    version: i64,
    catalog_vfs: Option<&CatalogVfs>,
) -> Result<(), LibraryError> {
    let mut backup = root.join(format!("library.sqlite3.schema-v{version}.backup"));
    if backup.exists() {
        backup = root.join(format!(
            "library.sqlite3.schema-v{version}.{}.backup",
            Uuid::new_v4().simple()
        ));
    }
    catalog_vfs
        .ok_or_else(|| {
            LibraryError::InvalidInput(
                "schema migration backup requires a pinned catalog VFS".into(),
            )
        })?
        .allow_output_path(&backup)
        .map_err(|error| io(&backup, error))?;
    conn.execute("VACUUM INTO ?1", params![backup.to_string_lossy()])?;
    Ok(())
}

fn canonical_directory(path: &Path) -> Result<PathBuf, LibraryError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| io(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(LibraryError::InvalidInput(
            "index root must be an existing non-symlink directory".into(),
        ));
    }
    path.canonicalize().map_err(|error| io(path, error))
}

fn ensure_owned_directory(root: &Path, name: &str) -> Result<PathBuf, LibraryError> {
    let path = root.join(name);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(LibraryError::InvalidInput(format!(
                "managed {name} namespace is unsafe"
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&path).map_err(|error| io(&path, error))?
        }
        Err(error) => return Err(io(&path, error)),
    }
    let canonical = path.canonicalize().map_err(|error| io(&path, error))?;
    if canonical.parent() != Some(root) || canonical != path {
        return Err(LibraryError::InvalidInput(format!(
            "managed {name} namespace identity changed"
        )));
    }
    Ok(path)
}
fn validate_managed_blob(
    path: &Path,
    expected_hash: &str,
    expected_bytes: u64,
) -> Result<(), LibraryError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| io(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != expected_bytes
    {
        return Err(LibraryError::InvalidInput(
            "managed blob target is unsafe or inconsistent".into(),
        ));
    }
    let mut file = std::fs::File::open(path).map_err(|error| io(path, error))?;
    let mut digest = Sha256::new();
    let copied = std::io::copy(
        &mut (&mut file).take(expected_bytes.saturating_add(1)),
        &mut digest,
    )
    .map_err(|error| io(path, error))?;
    if copied != expected_bytes || format!("{:x}", digest.finalize()) != expected_hash {
        return Err(LibraryError::InvalidInput(
            "managed blob target hash mismatch".into(),
        ));
    }
    Ok(())
}

fn retained_import_staging_error(staging: &Path, error: LibraryError) -> LibraryError {
    LibraryError::InvalidInput(format!(
        "{error}; import staging preserved at {}",
        staging.display()
    ))
}

fn retain_import_staging(staging: &Path) -> Result<(), LibraryError> {
    // Observe that the staged entry remains available, but never remove it by
    // path: another same-user process can replace it between any identity
    // check and an unlink. For new blobs this is normally a hard-link alias.
    std::fs::symlink_metadata(staging)
        .map(|_| ())
        .map_err(|error| io(staging, error))
}

fn validated_relative_path(path: &Path) -> Result<String, LibraryError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(LibraryError::InvalidInput(
            "source path must be a safe relative path".into(),
        ));
    }
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| LibraryError::InvalidInput("source path is not valid UTF-8".into()))
}

fn normalize_indexed_source_path(root: &Path, stored_path: &Path) -> Result<PathBuf, LibraryError> {
    let relative_path = if stored_path.is_absolute() {
        stored_path.strip_prefix(root).map_err(|_| {
            LibraryError::InvalidInput("indexed source path is outside its configured root".into())
        })?
    } else {
        stored_path
    };
    Ok(PathBuf::from(validated_relative_path(relative_path)?))
}

fn resolve_under_root(root: &Path, relative: &Path) -> Result<PathBuf, LibraryError> {
    let relative = validated_relative_path(relative)?;
    let canonical_root = root.canonicalize().map_err(|error| io(root, error))?;
    if canonical_root != root {
        return Err(LibraryError::InvalidInput(
            "configured root identity changed".into(),
        ));
    }
    let candidate = canonical_root.join(relative);
    let canonical_candidate = candidate
        .canonicalize()
        .map_err(|error| io(&candidate, error))?;
    if !canonical_candidate.starts_with(&canonical_root) {
        return Err(LibraryError::InvalidInput(
            "source path escapes its configured root".into(),
        ));
    }
    let metadata =
        std::fs::metadata(&canonical_candidate).map_err(|error| io(&canonical_candidate, error))?;
    if !metadata.is_file() {
        return Err(LibraryError::InvalidInput("source is not a file".into()));
    }
    Ok(canonical_candidate)
}

fn validated_thumbnail_cache_path(
    root: &Path,
    relative: &Path,
    expected_hash: &str,
) -> Result<PathBuf, LibraryError> {
    let relative = validated_relative_path(relative)?;
    validate_hash(expected_hash)?;
    let components = Path::new(&relative).components().collect::<Vec<_>>();
    if components.len() < 2 || components[0] != Component::Normal("thumbnails".as_ref()) {
        return Err(LibraryError::InvalidInput(
            "generated thumbnail path is outside the thumbnails directory".into(),
        ));
    }
    let canonical_root = root.canonicalize().map_err(|error| io(root, error))?;
    if canonical_root != root {
        return Err(LibraryError::InvalidInput(
            "library root identity changed".into(),
        ));
    }
    let mut current = canonical_root.clone();
    let mut missing = false;
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            return Err(LibraryError::InvalidInput(
                "generated thumbnail path is unsafe".into(),
            ));
        };
        current.push(component);
        if missing {
            continue;
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(LibraryError::InvalidInput(
                        "generated thumbnail path contains a symlink".into(),
                    ));
                }
                let is_leaf = index + 1 == components.len();
                if (is_leaf && !metadata.is_file()) || (!is_leaf && !metadata.is_dir()) {
                    return Err(LibraryError::InvalidInput(
                        "generated thumbnail path has an unexpected file type".into(),
                    ));
                }
                if is_leaf {
                    if metadata.len() > MAX_CACHED_THUMBNAIL_BYTES {
                        return Err(LibraryError::InvalidInput(
                            "generated thumbnail exceeds the cache size limit".into(),
                        ));
                    }
                    let mut file =
                        std::fs::File::open(&current).map_err(|error| io(&current, error))?;
                    let mut bytes = Vec::with_capacity(metadata.len() as usize);
                    std::io::Read::by_ref(&mut file)
                        .take(MAX_CACHED_THUMBNAIL_BYTES + 1)
                        .read_to_end(&mut bytes)
                        .map_err(|error| io(&current, error))?;
                    if bytes.len() as u64 > MAX_CACHED_THUMBNAIL_BYTES
                        || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                        || hex_digest(&bytes) != expected_hash
                    {
                        return Err(LibraryError::InvalidInput(
                            "generated thumbnail does not match its catalog hash".into(),
                        ));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => missing = true,
            Err(error) => return Err(io(&current, error)),
        }
    }
    if !current.starts_with(&canonical_root) {
        return Err(LibraryError::InvalidInput(
            "generated thumbnail path escapes the library root".into(),
        ));
    }
    Ok(current)
}

fn validate_hash(hash: &str) -> Result<(), LibraryError> {
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(LibraryError::InvalidInput(
            "content hash must be a 64-character hexadecimal SHA-256 digest".into(),
        ));
    }
    Ok(())
}

fn validate_capture_receipt(input: &CaptureReceiptInput) -> Result<(), LibraryError> {
    validate_plain_capture_id(&input.item_id, "capture item ID")?;
    validate_hash(&input.payload_hash)?;
    normalize_rfc3339(&input.created_at)?;
    let url = input.page.url.trim();
    if url.len() > 2048
        || !(url.starts_with("http://") || url.starts_with("https://"))
        || url.chars().any(char::is_control)
    {
        return Err(LibraryError::InvalidInput(
            "capture page URL is invalid".into(),
        ));
    }
    let title = input.page.title.trim();
    if title.is_empty() || title.len() > 512 || title.chars().any(char::is_control) {
        return Err(LibraryError::InvalidInput(
            "capture page title is invalid".into(),
        ));
    }
    clean_optional(input.page.designer.as_deref(), 200)?;
    clean_optional(input.page.model_license.as_deref(), 200)?;
    clean_optional(input.page.notes.as_deref(), 20_000)?;
    if let Some(parent) = &input.page.parent_external_id {
        validate_plain_capture_id(parent, "capture parent ID")?;
    }
    if input.downloads.len() > 128 {
        return Err(LibraryError::InvalidInput(
            "capture item has too many downloads".into(),
        ));
    }
    let mut ids = HashSet::new();
    for download in &input.downloads {
        validate_plain_capture_id(&download.capture_id, "capture download ID")?;
        if !ids.insert(&download.capture_id) {
            return Err(LibraryError::InvalidInput(
                "capture item repeats a download ID".into(),
            ));
        }
        if download.file_name.is_empty()
            || download.file_name.len() > 255
            || download.file_name.contains(['/', '\\', '\0'])
            || Path::new(&download.file_name).file_name() != Some(download.file_name.as_ref())
        {
            return Err(LibraryError::InvalidInput(
                "capture download filename is invalid".into(),
            ));
        }
        if let Some(hash) = &download.sha256 {
            validate_hash(hash)?;
        }
        if download
            .bytes
            .is_some_and(|bytes| bytes == 0 || bytes > MAX_INDEXED_FILE_BYTES)
        {
            return Err(LibraryError::InvalidInput(
                "capture download byte count is invalid".into(),
            ));
        }
    }
    Ok(())
}

fn validate_plain_capture_id(value: &str, label: &str) -> Result<(), LibraryError> {
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        })
    {
        return Err(LibraryError::InvalidInput(format!("{label} is invalid")));
    }
    Ok(())
}

fn select_thumbnail_fallback(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
) -> Result<(), LibraryError> {
    let has_active: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM thumbnails WHERE model_id=?1 AND is_active=1)",
        params![model_id],
        |row| row.get(0),
    )?;
    if !has_active {
        let fallback: Option<String> = tx
            .query_row(
                "SELECT id FROM thumbnails WHERE model_id=?1
                 ORDER BY is_custom DESC,ordinal,id LIMIT 1",
                params![model_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = fallback {
            tx.execute("UPDATE thumbnails SET is_active=1 WHERE id=?1", params![id])?;
        }
    }
    Ok(())
}

fn compact_thumbnail_ordinals(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
) -> Result<(), LibraryError> {
    let mut ids = Vec::new();
    {
        let mut stmt =
            tx.prepare("SELECT id FROM thumbnails WHERE model_id=?1 ORDER BY ordinal,id")?;
        let rows = stmt.query_map(params![model_id], |row| row.get::<_, String>(0))?;
        ids.extend(rows.collect::<Result<Vec<_>, _>>()?);
    }
    for (ordinal, id) in ids.into_iter().enumerate() {
        tx.execute(
            "UPDATE thumbnails SET ordinal=?1 WHERE id=?2",
            params![ordinal as i64, id],
        )?;
    }
    Ok(())
}

fn validate_thumbnail_record_path(
    root: &Path,
    thumbnail_relative_path: &Path,
    png_hash: &str,
) -> Result<String, LibraryError> {
    let relative = validated_relative_path(thumbnail_relative_path)?;
    if Path::new(&relative).components().next() != Some(Component::Normal("thumbnails".as_ref())) {
        return Err(LibraryError::InvalidInput(
            "custom thumbnail must be stored under thumbnails".into(),
        ));
    }
    validated_thumbnail_cache_path(root, Path::new(&relative), png_hash)?;
    Ok(relative)
}

fn validate_root_limits(
    scan_interval_minutes: u32,
    max_file_bytes: u64,
) -> Result<(), LibraryError> {
    if !(MIN_SCAN_INTERVAL_MINUTES..=MAX_SCAN_INTERVAL_MINUTES).contains(&scan_interval_minutes) {
        return Err(LibraryError::InvalidInput(format!(
            "scan interval must be between {MIN_SCAN_INTERVAL_MINUTES} and {MAX_SCAN_INTERVAL_MINUTES} minutes"
        )));
    }
    if max_file_bytes == 0 || max_file_bytes > MAX_INDEXED_FILE_BYTES {
        return Err(LibraryError::InvalidInput(format!(
            "maximum file size must be between 1 and {MAX_INDEXED_FILE_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_catalog_settings(settings: &CatalogSettings) -> Result<(), LibraryError> {
    if !matches!(settings.thumbnail_size, 128 | 256 | 512) {
        return Err(LibraryError::InvalidInput(
            "thumbnail size must be 128, 256, or 512 pixels".into(),
        ));
    }
    let background = settings.thumbnail_background.as_bytes();
    if background.len() != 7
        || background[0] != b'#'
        || !background[1..].iter().all(u8::is_ascii_hexdigit)
    {
        return Err(LibraryError::InvalidInput(
            "thumbnail background must be a #RRGGBB color".into(),
        ));
    }
    if !(1..=2).contains(&settings.thumbnail_concurrency) {
        return Err(LibraryError::InvalidInput(
            "thumbnail concurrency must be between 1 and 2".into(),
        ));
    }
    if !(16..=256).contains(&settings.scan_batch_size) {
        return Err(LibraryError::InvalidInput(
            "scan batch size must be between 16 and 256".into(),
        ));
    }
    Ok(())
}

fn clean_optional(value: Option<&str>, max: usize) -> Result<Option<String>, LibraryError> {
    match value {
        Some(v) => {
            let v = v.trim();
            if v.is_empty() {
                Ok(None)
            } else if v.len() > max {
                Err(LibraryError::InvalidInput(
                    "metadata value is too long".into(),
                ))
            } else {
                Ok(Some(v.into()))
            }
        }
        None => Ok(None),
    }
}

fn validate_filament_input(input: &FilamentInput) -> Result<FilamentInput, LibraryError> {
    Ok(FilamentInput {
        brand: clean_filament_text(&input.brand, 200, false, false)?,
        material: clean_filament_text(&input.material, 200, false, false)?,
        color: clean_filament_text(&input.color, 256, false, false)?,
        name: clean_filament_text(&input.name, 200, true, false)?,
        notes: clean_filament_text(&input.notes, 20_000, false, true)?,
    })
}

fn validate_spoolman_filament(
    record: &SpoolmanFilamentRecord,
) -> Result<FilamentInput, LibraryError> {
    validate_filament_input(&FilamentInput {
        brand: record.brand.clone(),
        material: record.material.clone(),
        color: record.color.clone(),
        name: record.name.clone(),
        notes: record.notes.clone(),
    })
}

fn validate_spoolman_spool(record: &SpoolmanSpoolRecord) -> Result<FilamentInput, LibraryError> {
    validate_filament_input(&FilamentInput {
        brand: record.brand.clone(),
        material: record.material.clone(),
        color: record.color.clone(),
        name: record.name.clone(),
        notes: record.notes.clone(),
    })
}

fn clean_filament_text(
    value: &str,
    max: usize,
    required: bool,
    multiline: bool,
) -> Result<String, LibraryError> {
    let cleaned = value.trim();
    let invalid_control = cleaned.chars().any(|character| {
        character == '\0'
            || (character.is_control() && (!multiline || !matches!(character, '\n' | '\r' | '\t')))
    });
    if cleaned.len() > max || invalid_control || (required && cleaned.is_empty()) {
        return Err(LibraryError::InvalidInput(
            "filament text field is invalid".into(),
        ));
    }
    Ok(cleaned.into())
}

fn validate_external_id(value: &str) -> Result<String, LibraryError> {
    let cleaned = value.trim();
    if cleaned.is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
        || value != cleaned
    {
        return Err(LibraryError::InvalidInput(
            "Spoolman external ID is invalid".into(),
        ));
    }
    Ok(cleaned.into())
}

fn validate_spoolman_base_url(value: &str) -> Result<String, LibraryError> {
    let value = value.trim().trim_end_matches('/');
    if value.len() > 2048
        || !(value.starts_with("http://") || value.starts_with("https://"))
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || value.contains(['@', '?', '#'])
        || value
            .split_once("://")
            .is_none_or(|(_, remainder)| remainder.is_empty() || remainder.starts_with('/'))
    {
        return Err(LibraryError::InvalidInput(
            "Spoolman base URL must be a bounded HTTP or HTTPS URL".into(),
        ));
    }
    Ok(value.into())
}

fn validate_filament_ids(ids: &[String]) -> Result<(), LibraryError> {
    let mut unique = HashSet::new();
    if ids.len() > 5
        || ids.iter().any(|id| {
            id.is_empty()
                || id.len() > 256
                || id.trim() != id
                || id.chars().any(char::is_control)
                || !unique.insert(id)
        })
    {
        return Err(LibraryError::InvalidInput(
            "model filament assignments are invalid".into(),
        ));
    }
    Ok(())
}

fn validate_catalog_filament_ids(
    tx: &rusqlite::Transaction<'_>,
    ids: &[String],
) -> Result<(), LibraryError> {
    for id in ids {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM filaments WHERE id=?1)",
            params![id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(LibraryError::InvalidInput(format!(
                "filament not found: {id}"
            )));
        }
    }
    Ok(())
}

fn set_model_filaments_tx(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
    expected_version: u64,
    filament_ids: &[String],
) -> Result<(), LibraryError> {
    validate_filament_ids(filament_ids)?;
    validate_catalog_filament_ids(tx, filament_ids)?;
    set_model_filaments_tx_validated(tx, model_id, expected_version, filament_ids)
}

fn set_model_filaments_tx_validated(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
    expected_version: u64,
    filament_ids: &[String],
) -> Result<(), LibraryError> {
    validate_active_model_version(tx, model_id, expected_version)?;
    write_model_filaments_tx(tx, model_id, filament_ids)
}

fn write_model_filaments_tx(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
    filament_ids: &[String],
) -> Result<(), LibraryError> {
    replace_model_filaments_tx(tx, model_id, filament_ids)?;
    tx.execute(
        "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
        params![timestamp(), model_id],
    )?;
    Ok(())
}

fn replace_model_filaments_tx(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
    filament_ids: &[String],
) -> Result<(), LibraryError> {
    tx.execute(
        "DELETE FROM model_filaments WHERE model_id=?1",
        params![model_id],
    )?;
    for (ordinal, filament_id) in filament_ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO model_filaments(model_id,filament_id,ordinal) VALUES(?1,?2,?3)",
            params![model_id, filament_id, ordinal as i64],
        )?;
    }
    Ok(())
}

fn validate_active_model_version(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
    expected_version: u64,
) -> Result<(), LibraryError> {
    let current: Option<(i64, Option<String>)> = tx
        .query_row(
            "SELECT metadata_version,deleted_at FROM models WHERE id=?1",
            params![model_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((actual, deleted_at)) = current else {
        return Err(LibraryError::ModelNotFound(model_id.into()));
    };
    if deleted_at.is_some() {
        return Err(LibraryError::ModelNotFound(model_id.into()));
    }
    if actual as u64 != expected_version {
        return Err(LibraryError::MetadataConflict {
            model_id: model_id.into(),
            expected: expected_version,
            actual: actual as u64,
        });
    }
    Ok(())
}

fn upsert_remote_filament(
    tx: &rusqlite::Transaction<'_>,
    source_id: &str,
    resource_kind: &str,
    external_id: &str,
    parent_filament_id: Option<&String>,
    input: &FilamentInput,
    archived: bool,
) -> Result<(String, bool, bool), LibraryError> {
    type ExistingRemoteFilament = (
        String,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        bool,
        bool,
    );
    let existing: Option<ExistingRemoteFilament> = tx
        .query_row(
            "SELECT id,brand,material,color,name,notes,parent_filament_id,archived,source_missing
             FROM filaments WHERE source_id=?1 AND resource_kind=?2 AND external_id=?3",
            params![source_id, resource_kind, external_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()?;
    if let Some((id, brand, material, color, name, notes, parent, old_archived, missing)) = existing
    {
        let changed = brand != input.brand
            || material != input.material
            || color != input.color
            || name != input.name
            || notes != input.notes
            || parent.as_deref() != parent_filament_id.map(String::as_str)
            || old_archived != archived
            || missing;
        if changed {
            tx.execute(
                "UPDATE filaments SET brand=?1,material=?2,color=?3,name=?4,notes=?5,
                 parent_filament_id=?6,archived=?7,source_missing=0,version=version+1,updated_at=?8
                 WHERE id=?9",
                params![
                    input.brand,
                    input.material,
                    input.color,
                    input.name,
                    input.notes,
                    parent_filament_id,
                    archived,
                    timestamp(),
                    id
                ],
            )?;
        }
        Ok((id, false, changed))
    } else {
        let id = new_id("filament");
        let now = timestamp();
        tx.execute(
            "INSERT INTO filaments(id,version,brand,material,color,name,notes,origin,resource_kind,
             source_id,external_id,parent_filament_id,archived,source_missing,created_at,updated_at)
             VALUES(?1,1,?2,?3,?4,?5,?6,'spoolman',?7,?8,?9,?10,?11,0,?12,?12)",
            params![
                id,
                input.brand,
                input.material,
                input.color,
                input.name,
                input.notes,
                resource_kind,
                source_id,
                external_id,
                parent_filament_id,
                archived,
                now
            ],
        )?;
        Ok((id, true, false))
    }
}
fn validated_archive_member(value: &str) -> Result<String, LibraryError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        Err(LibraryError::InvalidInput("unsafe archive member".into()))
    } else {
        Ok(value.into())
    }
}
fn ensure_archive_bundle(
    tx: &rusqlite::Transaction<'_>,
    root: &str,
    path: &str,
) -> Result<String, LibraryError> {
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM bundles WHERE source_root_id=?1 AND source_path=?2",
            params![root, path],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    let id = new_id("bundle");
    tx.execute(
        "INSERT INTO bundles(id,source_root_id,source_path,created_at) VALUES(?1,?2,?3,?4)",
        params![id, root, path, timestamp()],
    )?;
    Ok(id)
}
fn reconcile_folder_bundles(
    tx: &rusqlite::Transaction<'_>,
    root_id: &str,
) -> Result<(), LibraryError> {
    #[derive(Clone)]
    struct Item {
        id: String,
        bundle_id: Option<String>,
        auto_bundle: Option<String>,
        manual: bool,
    }
    let mut groups: BTreeMap<Option<String>, Vec<Item>> = BTreeMap::new();
    {
        let mut stmt = tx.prepare(
            "SELECT m.id,m.source_path,m.bundle_id,b.id,b.source_root_id
             FROM models m LEFT JOIN bundles b ON b.id=m.bundle_id
             WHERE m.source_kind='indexed' AND m.source_root_id=?1
               AND m.source_archive_member IS NULL AND m.source_missing=0",
        )?;
        let rows = stmt.query_map(params![root_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        for row in rows {
            let (id, path, bundle_id, bundle_exists, bundle_root) = row?;
            let parent = Path::new(&path)
                .parent()
                .and_then(Path::to_str)
                .filter(|parent| !parent.is_empty())
                .map(str::to_owned);
            let manual = bundle_id.is_some() && bundle_exists.is_some() && bundle_root.is_none();
            let auto_bundle = bundle_root.is_some().then_some(bundle_id.clone()).flatten();
            groups.entry(parent).or_default().push(Item {
                id,
                bundle_id,
                auto_bundle,
                manual,
            });
        }
    }
    let mut bundle_folders: BTreeMap<String, BTreeSet<Option<String>>> = BTreeMap::new();
    for (folder, items) in &groups {
        for item in items {
            if !item.manual {
                if let Some(bundle) = &item.auto_bundle {
                    bundle_folders
                        .entry(bundle.clone())
                        .or_default()
                        .insert(folder.clone());
                }
            }
        }
    }
    for (folder, items) in groups {
        let eligible = items
            .into_iter()
            .filter(|item| !item.manual)
            .collect::<Vec<_>>();
        if folder.is_none() || eligible.len() < 2 {
            for item in eligible {
                if item.auto_bundle.is_some() {
                    tx.execute("UPDATE models SET bundle_id=NULL,metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",params![timestamp(),item.id])?;
                }
            }
            continue;
        }
        let marker = format!("{}/", folder.unwrap().trim_end_matches('/'));
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM bundles WHERE source_root_id=?1 AND source_path=?2",
                params![root_id, marker],
                |row| row.get(0),
            )
            .optional()?;
        let bundle_id = if let Some(id) = existing {
            id
        } else {
            let candidates = eligible
                .iter()
                .filter_map(|item| item.auto_bundle.clone())
                .collect::<BTreeSet<_>>();
            if candidates.len() == 1
                && bundle_folders
                    .get(candidates.iter().next().unwrap())
                    .is_some_and(|folders| folders.len() == 1)
            {
                let id = candidates.into_iter().next().unwrap();
                tx.execute(
                    "UPDATE bundles SET source_root_id=?1,source_path=?2 WHERE id=?3",
                    params![root_id, marker, id],
                )?;
                id
            } else {
                let id = new_id("bundle");
                tx.execute("INSERT INTO bundles(id,source_root_id,source_path,created_at) VALUES(?1,?2,?3,?4)",params![id,root_id,marker,timestamp()])?;
                id
            }
        };
        for item in eligible {
            if item.bundle_id.as_deref() != Some(&bundle_id) {
                tx.execute("UPDATE models SET bundle_id=?1,metadata_version=metadata_version+1,updated_at=?2 WHERE id=?3",params![bundle_id,timestamp(),item.id])?;
            }
        }
    }
    Ok(())
}
fn print_status_value(value: &PrintStatus) -> &'static str {
    match value {
        PrintStatus::Unprinted => "unprinted",
        PrintStatus::Want => "want",
        PrintStatus::Queued => "queued",
        PrintStatus::Printing => "printing",
        PrintStatus::Printed => "printed",
        PrintStatus::Failed => "failed",
    }
}
fn print_outcome_value(value: &PrintOutcome) -> &'static str {
    match value {
        PrintOutcome::Printed => "printed",
        PrintOutcome::Failed => "failed",
        PrintOutcome::Cancelled => "cancelled",
    }
}
fn print_status_for_outcome(value: &PrintOutcome) -> &'static str {
    match value {
        PrintOutcome::Printed => "printed",
        PrintOutcome::Failed => "failed",
        // A cancelled job is queued again without counting it as printed.
        PrintOutcome::Cancelled => "queued",
    }
}
fn validate_metadata_patch(p: &MetadataPatch) -> Result<(), LibraryError> {
    if p.rating.is_some_and(|rating| rating > 5) {
        return Err(LibraryError::InvalidInput(
            "rating must be between 0 and 5".into(),
        ));
    }
    clean_optional(p.designer.as_deref(), 200)?;
    if p.source_url
        .as_deref()
        .is_some_and(|source| source.chars().any(char::is_control))
    {
        return Err(LibraryError::InvalidInput(
            "source must be an http or https URL or a safe origin label".into(),
        ));
    }
    let cleaned_source = clean_optional(p.source_url.as_deref(), 2048)?;
    clean_optional(p.notes.as_deref(), 20_000)?;
    clean_optional(p.model_license.as_deref(), 200)?;
    if cleaned_source.is_some() {
        let is_http_url = p.source_url.as_deref().is_some_and(is_safe_http_source_url);
        let is_origin_label = p
            .source_url
            .as_deref()
            .is_some_and(is_safe_source_origin_label);
        if !is_http_url && !is_origin_label {
            return Err(LibraryError::InvalidInput(
                "source must be an http or https URL or a safe origin label".into(),
            ));
        }
    }
    if let Some(tags) = &p.tags {
        if tags.len() > 200 {
            return Err(LibraryError::InvalidInput("too many tags".into()));
        }
    }
    Ok(())
}

fn is_safe_http_source_url(value: &str) -> bool {
    let value = value.trim();
    let Some((_, after_scheme)) = value.split_once("://") else {
        return false;
    };
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if authority.contains('@') {
        return false;
    }
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

fn is_safe_source_origin_label(value: &str) -> bool {
    !value.trim().is_empty()
        && !value.chars().any(char::is_control)
        && !value
            .chars()
            .any(|character| matches!(character, ':' | '/' | '\\' | '?' | '#'))
}

fn apply_metadata_patch(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
    expected_version: u64,
    patch: &MetadataPatch,
) -> Result<(), LibraryError> {
    let actual: i64 = tx
        .query_row(
            "SELECT metadata_version FROM models WHERE id=?1",
            params![model_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| LibraryError::ModelNotFound(model_id.to_owned()))?;
    if actual as u64 != expected_version {
        return Err(LibraryError::MetadataConflict {
            model_id: model_id.to_owned(),
            expected: expected_version,
            actual: actual as u64,
        });
    }
    if let Some(folder) = &patch.folder_id {
        let ok: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM folders WHERE id=?1)",
            params![folder],
            |r| r.get(0),
        )?;
        if !ok {
            return Err(LibraryError::FolderNotFound(folder.clone()));
        }
    }
    if let Some(parent) = &patch.parent_model_id {
        let parent = parent.trim();
        if !parent.is_empty() {
            ensure_no_parent_cycle(tx, model_id, parent)?;
        }
    }
    let designer = patch
        .designer
        .as_deref()
        .map(|v| clean_optional(Some(v), 200))
        .transpose()?
        .flatten();
    let source_url = patch
        .source_url
        .as_deref()
        .map(|v| clean_optional(Some(v), 2048))
        .transpose()?
        .flatten();
    let notes = patch
        .notes
        .as_deref()
        .map(|v| clean_optional(Some(v), 20_000))
        .transpose()?
        .flatten();
    let license = patch
        .model_license
        .as_deref()
        .map(|v| clean_optional(Some(v), 200))
        .transpose()?
        .flatten();
    let parent = patch.parent_model_id.as_deref().map(str::trim).map(|v| {
        if v.is_empty() {
            None
        } else {
            Some(v.to_owned())
        }
    });
    tx.execute("UPDATE models SET designer=CASE WHEN ?1 THEN ?2 ELSE designer END,source_url=CASE WHEN ?3 THEN ?4 ELSE source_url END,notes=CASE WHEN ?5 THEN ?6 ELSE notes END,description=CASE WHEN ?5 THEN COALESCE(?6,'') ELSE description END,model_license=CASE WHEN ?7 THEN ?8 ELSE model_license END,print_status=COALESCE(?9,print_status),folder_id=COALESCE(?10,folder_id),parent_model_id=CASE WHEN ?11 THEN ?12 ELSE parent_model_id END,rating=COALESCE(?13,rating),favorite=COALESCE(?14,favorite),metadata_version=metadata_version+1,updated_at=?15 WHERE id=?16",params![patch.designer.is_some(),designer,patch.source_url.is_some(),source_url,patch.notes.is_some(),notes,patch.model_license.is_some(),license,patch.print_status.as_ref().map(print_status_value),patch.folder_id,patch.parent_model_id.is_some(),parent.flatten(),patch.rating.map(i64::from),patch.favorite.map(i64::from),timestamp(),model_id])?;
    if let Some(tags) = &patch.tags {
        for id in tags {
            let ok: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM tags WHERE id=?1)",
                params![id],
                |r| r.get(0),
            )?;
            if !ok {
                return Err(LibraryError::InvalidInput(format!("tag not found: {id}")));
            }
        }
        if !matches!(patch.tag_mode, Some(TagMode::Merge)) {
            tx.execute(
                "DELETE FROM model_tags WHERE model_id=?1",
                params![model_id],
            )?;
        }
        for id in tags {
            tx.execute(
                "INSERT OR IGNORE INTO model_tags(model_id,tag_id) VALUES(?1,?2)",
                params![model_id, id],
            )?;
        }
    }
    Ok(())
}
fn ensure_no_parent_cycle(
    tx: &rusqlite::Transaction<'_>,
    model: &str,
    parent: &str,
) -> Result<(), LibraryError> {
    let mut cur = Some(parent.to_owned());
    while let Some(id) = cur {
        if id == model {
            return Err(LibraryError::InvalidInput("model parent cycle".into()));
        }
        cur = tx
            .query_row(
                "SELECT parent_model_id FROM models WHERE id=?1",
                params![id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .ok_or_else(|| LibraryError::ModelNotFound(id.clone()))?;
    }
    Ok(())
}
fn tag_model_ids(tx: &rusqlite::Transaction<'_>, tag: &str) -> Result<Vec<String>, LibraryError> {
    let mut s = tx.prepare("SELECT model_id FROM model_tags WHERE tag_id=?1")?;
    let r = s.query_map(params![tag], |row| row.get(0))?;
    Ok(r.collect::<Result<Vec<_>, _>>()?)
}
fn bump_models(tx: &rusqlite::Transaction<'_>, ids: &[String]) -> Result<(), LibraryError> {
    for id in ids {
        tx.execute(
            "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
            params![timestamp(), id],
        )?;
    }
    Ok(())
}
fn validate_print_attempt(i: &PrintAttemptInput) -> Result<(), LibraryError> {
    if i.quantity == 0
        || i.quantity > 10_000
        || i.notes.len() > 20_000
        || i.filament_ids.len() > 5
        || i.filament_ids.iter().any(|id| {
            id.is_empty() || id.len() > 256 || id.trim() != id || id.chars().any(char::is_control)
        })
        || normalize_rfc3339(&i.printed_at).is_err()
    {
        return Err(LibraryError::InvalidInput("invalid print attempt".into()));
    }
    if let Some(parts) = &i.parts {
        if parts.len() > MAX_PRINT_ATTEMPT_PARTS {
            return Err(LibraryError::InvalidInput(
                "print attempt has too many stock parts".into(),
            ));
        }
        let mut seen_parts = HashSet::new();
        for part in parts {
            validate_record_id(&part.id, "part ID")?;
            if part.quantity == 0 || part.quantity > 9_999 || !seen_parts.insert(part.id.as_str()) {
                return Err(LibraryError::InvalidInput(
                    "invalid or duplicate print attempt part usage".into(),
                ));
            }
            if let Some(snapshot) = &part.snapshot {
                normalize_part_usage_snapshot(snapshot)?;
            }
        }
    }
    Ok(())
}

type PrintAttemptImportIdentity = (
    String,
    u32,
    String,
    Vec<String>,
    String,
    Option<String>,
    Option<String>,
    Vec<(String, u32)>,
);

fn print_attempt_import_identity(
    printed_at: &str,
    quantity: u32,
    notes: &str,
    filament_ids: &[String],
    parts: &[PartUsageInput],
    outcome: &str,
    printer_id: Option<String>,
    printer_name: Option<String>,
) -> Result<PrintAttemptImportIdentity, LibraryError> {
    let mut part_identity = parts
        .iter()
        .map(|part| (part.id.clone(), part.quantity))
        .collect::<Vec<_>>();
    part_identity.sort();
    Ok((
        normalize_rfc3339(printed_at)?,
        quantity,
        notes.to_owned(),
        filament_ids.to_vec(),
        outcome.to_owned(),
        printer_id,
        printer_name,
        part_identity,
    ))
}

fn write_print_attempt(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    model: &str,
    i: &PrintAttemptInput,
    now: &str,
    create: bool,
    imported_created_at: Option<&str>,
    adjust_stock: bool,
) -> Result<(), LibraryError> {
    let printed_at = normalize_rfc3339(&i.printed_at)?;
    if adjust_stock
        && i.parts
            .as_ref()
            .is_some_and(|parts| parts.iter().any(|part| part.snapshot.is_some()))
    {
        return Err(LibraryError::InvalidInput(
            "historical part snapshots are only accepted during history import".into(),
        ));
    }
    if create {
        let created_at = imported_created_at.unwrap_or(now);
        tx.execute(
            "INSERT INTO print_attempts
             (id,model_id,printed_at,quantity,notes,outcome,stock_adjusted,created_at,updated_at,printer_id,printer_name)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,NULL,NULL)",
            params![
                id,
                model,
                printed_at,
                i.quantity,
                i.notes,
                print_outcome_value(&i.outcome),
                adjust_stock,
                created_at,
                now
            ],
        )?;
        apply_print_attempt_parts(tx, id, i, true, adjust_stock)?;
    } else {
        apply_print_attempt_parts(tx, id, i, false, adjust_stock)?;
        tx.execute("UPDATE print_attempts SET printed_at=?1,quantity=?2,notes=?3,outcome=?4,updated_at=?5 WHERE id=?6",params![printed_at,i.quantity,i.notes,print_outcome_value(&i.outcome),now,id])?;
        tx.execute(
            "DELETE FROM print_attempt_filaments WHERE attempt_id=?1",
            params![id],
        )?;
    }
    tx.execute(
        "UPDATE models SET print_status=?1 WHERE id=?2",
        params![print_status_for_outcome(&i.outcome), model],
    )?;
    let mut seen = HashSet::new();
    for (n, f) in i.filament_ids.iter().enumerate() {
        if !seen.insert(f) {
            return Err(LibraryError::InvalidInput("duplicate filament".into()));
        }
        tx.execute(
            "INSERT INTO print_attempt_filaments VALUES(?1,?2,?3)",
            params![id, f, n as i64],
        )?;
    }
    Ok(())
}

fn print_attempt_part_total(quantity_per_copy: u32, copies: u32) -> Result<i64, LibraryError> {
    let total = u64::from(quantity_per_copy)
        .checked_mul(u64::from(copies))
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| LibraryError::InvalidInput("print part quantity is too large".into()))?;
    Ok(total)
}

fn apply_print_attempt_parts(
    tx: &rusqlite::Transaction<'_>,
    attempt_id: &str,
    input: &PrintAttemptInput,
    create: bool,
    adjust_stock: bool,
) -> Result<(), LibraryError> {
    let preserve_existing = !create && input.parts.is_none();
    let old_stock_adjusted = if create {
        Some(adjust_stock)
    } else {
        tx.query_row(
            "SELECT stock_adjusted FROM print_attempts WHERE id=?1",
            params![attempt_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?
        .ok_or_else(|| LibraryError::InvalidInput("print attempt not found".into()))?
        .map(|value| value != 0)
    };
    let should_adjust_stock = adjust_stock && old_stock_adjusted == Some(true);
    let mut old_parts = HashMap::new();
    let mut existing_usages = Vec::new();
    if !create {
        let mut statement = tx.prepare(
            "SELECT part_id,part_name,category,unit,quantity_per_copy,quantity
             FROM print_attempt_parts WHERE attempt_id=?1 ORDER BY ordinal",
        )?;
        let rows = statement.query_map(params![attempt_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)? as u32,
                row.get::<_, i64>(5)?,
            ))
        })?;
        for row in rows {
            let (id, name, category, unit, quantity_per_copy, total) = row?;
            old_parts.insert(id.clone(), (quantity_per_copy, name, category, unit, total));
            existing_usages.push(PartUsageInput {
                id,
                quantity: quantity_per_copy,
                snapshot: None,
            });
        }
    }
    let input_parts = if preserve_existing {
        existing_usages
    } else {
        input.parts.clone().unwrap_or_default()
    };

    let new_usages = input_parts
        .iter()
        .map(|usage| (usage.id.clone(), usage.quantity))
        .collect::<HashMap<_, _>>();
    if new_usages.len() != input_parts.len() {
        return Err(LibraryError::InvalidInput("duplicate part".into()));
    }
    let mut snapshots = HashMap::new();
    let mut stock_records = HashMap::new();
    for usage in &input_parts {
        let is_orphan_id = usage.id.starts_with(PRINTVENTORY_ORPHAN_PART_ID_PREFIX);
        if is_orphan_id && should_adjust_stock {
            return Err(LibraryError::InvalidInput(
                "an unlinked historical part cannot be used as stock".into(),
            ));
        }
        let part = if is_orphan_id {
            None
        } else {
            tx.query_row(
                &format!("SELECT {PART_STOCK_SELECT_COLUMNS} FROM parts_stock WHERE id=?1"),
                params![usage.id],
                part_stock_from_row,
            )
            .optional()?
        };
        if let Some(part) = part {
            stock_records.insert(usage.id.clone(), part);
        }
        if preserve_existing {
            if let Some((_, name, category, unit, _)) = old_parts.get(&usage.id) {
                snapshots.insert(
                    usage.id.clone(),
                    (name.clone(), category.clone(), unit.clone()),
                );
                continue;
            }
        }
        if let Some(part) = stock_records.get(&usage.id) {
            snapshots.insert(
                usage.id.clone(),
                (part.name.clone(), part.category.clone(), part.unit.clone()),
            );
        } else if let Some((_, name, category, unit, _)) = old_parts.get(&usage.id) {
            // A previously logged part may have been deleted. Keep its
            // historical usage when editing the rest of the event.
            snapshots.insert(
                usage.id.clone(),
                (name.clone(), category.clone(), unit.clone()),
            );
        } else if !adjust_stock {
            let imported_snapshot = usage
                .snapshot
                .as_ref()
                .ok_or_else(|| LibraryError::InvalidInput("part snapshot is missing".into()))?;
            let normalized = normalize_part_usage_snapshot(imported_snapshot)?;
            snapshots.insert(
                usage.id.clone(),
                (normalized.name, normalized.category, normalized.unit),
            );
        } else {
            return Err(LibraryError::InvalidInput("part not found".into()));
        }
    }

    let mut all_ids = BTreeSet::new();
    all_ids.extend(old_parts.keys().cloned());
    all_ids.extend(new_usages.keys().cloned());
    if should_adjust_stock {
        for part_id in all_ids {
            let old_total = old_parts
                .get(&part_id)
                .map(|(_, _, _, _, total)| *total)
                .unwrap_or(0);
            let new_per_copy = new_usages.get(&part_id).copied().unwrap_or(0);
            let new_total = print_attempt_part_total(new_per_copy, input.quantity)?;
            let delta = new_total - old_total;
            if delta == 0 {
                continue;
            }

            let part = match stock_records.get(&part_id) {
                Some(part) => Some(part.clone()),
                None => tx
                    .query_row(
                        &format!("SELECT {PART_STOCK_SELECT_COLUMNS} FROM parts_stock WHERE id=?1"),
                        params![part_id],
                        part_stock_from_row,
                    )
                    .optional()?,
            };
            let Some(part) = part else {
                if old_parts.contains_key(&part_id) {
                    // The part was deleted. Its stock no longer exists to restore
                    // or deduct, but its event snapshot remains in the link row.
                    continue;
                }
                return Err(LibraryError::InvalidInput("part not found".into()));
            };
            apply_part_stock_delta(tx, &part, delta)?;
        }
    }

    if preserve_existing {
        for usage in &input_parts {
            let total = print_attempt_part_total(usage.quantity, input.quantity)?;
            tx.execute(
                "UPDATE print_attempt_parts SET quantity=?1 WHERE attempt_id=?2 AND part_id=?3",
                params![total, attempt_id, usage.id],
            )?;
        }
        return Ok(());
    }

    if !create {
        tx.execute(
            "DELETE FROM print_attempt_parts WHERE attempt_id=?1",
            params![attempt_id],
        )?;
    }
    for (ordinal, usage) in input_parts.iter().enumerate() {
        let (name, category, unit) = snapshots
            .get(&usage.id)
            .ok_or_else(|| LibraryError::InvalidInput("part snapshot is missing".into()))?;
        let total = print_attempt_part_total(usage.quantity, input.quantity)?;
        tx.execute(
            "INSERT INTO print_attempt_parts
             (attempt_id,part_id,part_name,category,unit,quantity_per_copy,quantity,ordinal)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                attempt_id,
                usage.id,
                name,
                category,
                unit,
                usage.quantity,
                total,
                ordinal as i64,
            ],
        )?;
    }
    Ok(())
}

fn apply_part_stock_delta(
    tx: &rusqlite::Transaction<'_>,
    part: &PartStock,
    delta: i64,
) -> Result<(), LibraryError> {
    if delta == 0 {
        return Ok(());
    }
    let current = i64::try_from(part.quantity)
        .map_err(|_| LibraryError::InvalidInput("part stock quantity is invalid".into()))?;
    if delta > current {
        return Err(LibraryError::InvalidInput(format!(
            "Not enough \"{}\" in stock ({} available, {} needed).",
            part.name, current, delta
        )));
    }
    let updated = current
        .checked_sub(delta)
        .filter(|value| *value >= 0)
        .ok_or_else(|| LibraryError::InvalidInput("part stock quantity overflowed".into()))?;
    let expected_version = i64::try_from(part.metadata_version)
        .map_err(|_| LibraryError::InvalidInput("part stock version is invalid".into()))?;
    let changed = tx.execute(
        "UPDATE parts_stock SET quantity=?1,metadata_version=metadata_version+1,updated_at=?2
         WHERE id=?3 AND metadata_version=?4",
        params![updated, timestamp(), part.id, expected_version],
    )?;
    if changed != 1 {
        return Err(LibraryError::InvalidInput(
            "part changed during print event; retry the operation".into(),
        ));
    }
    Ok(())
}

fn restore_print_attempt_parts(
    tx: &rusqlite::Transaction<'_>,
    attempt_id: &str,
) -> Result<(), LibraryError> {
    let stock_adjusted: Option<bool> = tx
        .query_row(
            "SELECT stock_adjusted FROM print_attempts WHERE id=?1",
            params![attempt_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?
        .ok_or_else(|| LibraryError::InvalidInput("print attempt not found".into()))?
        .map(|value| value != 0);
    if stock_adjusted != Some(true) {
        return Ok(());
    }
    let rows = {
        let mut statement =
            tx.prepare("SELECT part_id,quantity FROM print_attempt_parts WHERE attempt_id=?1")?;
        let rows = statement.query_map(params![attempt_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (part_id, quantity) in rows {
        if quantity <= 0 {
            return Err(LibraryError::InvalidInput(
                "persisted print part quantity is invalid".into(),
            ));
        }
        let part = tx
            .query_row(
                &format!("SELECT {PART_STOCK_SELECT_COLUMNS} FROM parts_stock WHERE id=?1"),
                params![part_id],
                part_stock_from_row,
            )
            .optional()?;
        if let Some(part) = part {
            apply_part_stock_delta(
                tx,
                &part,
                quantity
                    .checked_neg()
                    .ok_or_else(|| LibraryError::InvalidInput("part quantity overflowed".into()))?,
            )?;
        }
    }
    Ok(())
}

fn write_print_attempt_printer(
    tx: &rusqlite::Transaction<'_>,
    attempt_id: &str,
    printer_id: Option<&str>,
) -> Result<(), LibraryError> {
    match printer_id {
        Some(printer_id) => {
            let printer_id = validate_record_id(printer_id, "printer ID")?;
            let printer_name: String = tx
                .query_row(
                    "SELECT nickname FROM printers WHERE id=?1",
                    params![printer_id],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| LibraryError::InvalidInput("printer not found".into()))?;
            tx.execute(
                "UPDATE print_attempts SET printer_id=?1,printer_name=?2 WHERE id=?3",
                params![printer_id, printer_name, attempt_id],
            )?;
        }
        None => {
            tx.execute(
                "UPDATE print_attempts SET printer_id=NULL,printer_name=NULL WHERE id=?1",
                params![attempt_id],
            )?;
        }
    }
    Ok(())
}

fn recompute_print_summary(
    tx: &rusqlite::Transaction<'_>,
    model: &str,
) -> Result<(), LibraryError> {
    let (count,last):(i64,Option<String>)=tx.query_row("SELECT COALESCE(SUM(CASE WHEN outcome='printed' THEN quantity ELSE 0 END),0),MAX(CASE WHEN outcome='printed' THEN printed_at END) FROM print_attempts WHERE model_id=?1",params![model],|r|Ok((r.get(0)?,r.get(1)?)))?;
    tx.execute("UPDATE models SET print_count=?1,last_printed_at=?2,metadata_version=metadata_version+1,updated_at=?3 WHERE id=?4",params![count,last,timestamp(),model])?;
    Ok(())
}
fn attempt_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PrintAttempt> {
    let o: String = r.get(5)?;
    let outcome = match o.as_str() {
        "printed" => PrintOutcome::Printed,
        "failed" => PrintOutcome::Failed,
        "cancelled" => PrintOutcome::Cancelled,
        _ => return Err(invalid_persisted_text(5, "unknown print outcome")),
    };
    Ok(PrintAttempt {
        id: r.get(0)?,
        model_id: r.get(1)?,
        printed_at: r.get(2)?,
        quantity: r.get::<_, i64>(3)? as u32,
        notes: r.get(4)?,
        filament_ids: Vec::new(),
        parts: Vec::new(),
        outcome,
        stock_adjusted: r.get(10)?,
        printer_id: r.get(6)?,
        printer_name: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

fn printer_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Printer> {
    let bed_values = (
        row.get::<_, Option<f64>>(10)?,
        row.get::<_, Option<f64>>(11)?,
        row.get::<_, Option<f64>>(12)?,
    );
    let bed_mm = match bed_values {
        (Some(x), Some(y), Some(z)) => Some([x as f32, y as f32, z as f32]),
        (None, None, None) => None,
        _ => {
            return Err(invalid_persisted_text(
                10,
                "printer bed dimensions are incomplete",
            ));
        }
    };
    let source: String = row.get(14)?;
    let connection: String = row.get(15)?;
    let job: String = row.get(16)?;
    Ok(Printer {
        id: row.get(0)?,
        metadata_version: persisted_version(row.get(1)?, 1)?,
        nickname: row.get(2)?,
        make: row.get(3)?,
        model: row.get(4)?,
        printer_type: row.get(5)?,
        firmware: row.get(6)?,
        host: row.get(7)?,
        web_url: row.get(8)?,
        endpoint_approved: row.get(9)?,
        bed_mm,
        notes: row.get(13)?,
        source_kind: parse_printer_source_kind(&source)
            .ok_or_else(|| invalid_persisted_text(14, "unknown printer source kind"))?,
        connection_status: parse_printer_connection_status(&connection)
            .ok_or_else(|| invalid_persisted_text(15, "unknown printer connection status"))?,
        job_status: parse_printer_job_status(&job)
            .ok_or_else(|| invalid_persisted_text(16, "unknown printer job status"))?,
        last_checked_at: row.get(17)?,
        connection_error_code: row.get(18)?,
        print_count: persisted_version(row.get(19)?, 19)?,
        last_printed_at: row.get(20)?,
        created_at: row.get(21)?,
        updated_at: row.get(22)?,
    })
}

fn part_stock_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PartStock> {
    Ok(PartStock {
        id: row.get(0)?,
        metadata_version: persisted_version(row.get(1)?, 1)?,
        name: row.get(2)?,
        category: row.get(3)?,
        quantity: persisted_version(row.get(4)?, 4)?,
        unit: row.get(5)?,
        notes: row.get(6)?,
        low_stock: row.get::<_, i64>(7)? as u32,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn maintenance_log_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PrinterMaintenanceLog> {
    Ok(PrinterMaintenanceLog {
        id: row.get(0)?,
        printer_id: row.get(1)?,
        printer_nickname: row.get(2)?,
        title: row.get(3)?,
        performed_at: row.get(4)?,
        notes: row.get(5)?,
        created_at: row.get(6)?,
        metadata_version: persisted_version(row.get(7)?, 7)?,
    })
}

fn printer_reminder_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PrinterReminder> {
    let status: String = row.get(8)?;
    let interval: Option<i64> = row.get(7)?;
    let interval_days = interval
        .map(|value| {
            u32::try_from(value)
                .map_err(|_| invalid_persisted_integer(7, "printer reminder interval is invalid"))
        })
        .transpose()?;
    Ok(PrinterReminder {
        id: row.get(0)?,
        metadata_version: persisted_version(row.get(1)?, 1)?,
        printer_id: row.get(2)?,
        printer_nickname: row.get(3)?,
        title: row.get(4)?,
        notes: row.get(5)?,
        due_at: row.get(6)?,
        interval_days,
        status: parse_printer_reminder_status(&status)
            .ok_or_else(|| invalid_persisted_text(8, "unknown printer reminder status"))?,
        last_completed_at: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn invalid_persisted_text(index: usize, message: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message.to_owned(),
        )),
    )
}

fn persisted_version(value: i64, index: usize) -> rusqlite::Result<u64> {
    u64::try_from(value)
        .map_err(|_| invalid_persisted_integer(index, "persisted version is invalid"))
}

fn parse_printer_source_kind(value: &str) -> Option<PrinterSourceKind> {
    match value {
        "manual" => Some(PrinterSourceKind::Manual),
        "discovered" => Some(PrinterSourceKind::Discovered),
        "imported" => Some(PrinterSourceKind::Imported),
        _ => None,
    }
}

fn printer_source_kind_value(value: &PrinterSourceKind) -> &'static str {
    match value {
        PrinterSourceKind::Manual => "manual",
        PrinterSourceKind::Discovered => "discovered",
        PrinterSourceKind::Imported => "imported",
    }
}

fn parse_printer_connection_status(value: &str) -> Option<PrinterConnectionStatus> {
    match value {
        "unknown" => Some(PrinterConnectionStatus::Unknown),
        "online" => Some(PrinterConnectionStatus::Online),
        "offline" => Some(PrinterConnectionStatus::Offline),
        "auth_required" => Some(PrinterConnectionStatus::AuthRequired),
        "error" => Some(PrinterConnectionStatus::Error),
        _ => None,
    }
}

fn printer_connection_status_value(value: &PrinterConnectionStatus) -> &'static str {
    match value {
        PrinterConnectionStatus::Unknown => "unknown",
        PrinterConnectionStatus::Online => "online",
        PrinterConnectionStatus::Offline => "offline",
        PrinterConnectionStatus::AuthRequired => "auth_required",
        PrinterConnectionStatus::Error => "error",
    }
}

fn parse_printer_job_status(value: &str) -> Option<PrinterJobStatus> {
    match value {
        "unknown" => Some(PrinterJobStatus::Unknown),
        "idle" => Some(PrinterJobStatus::Idle),
        "printing" => Some(PrinterJobStatus::Printing),
        "paused" => Some(PrinterJobStatus::Paused),
        "complete" => Some(PrinterJobStatus::Complete),
        _ => None,
    }
}

fn printer_job_status_value(value: &PrinterJobStatus) -> &'static str {
    match value {
        PrinterJobStatus::Unknown => "unknown",
        PrinterJobStatus::Idle => "idle",
        PrinterJobStatus::Printing => "printing",
        PrinterJobStatus::Paused => "paused",
        PrinterJobStatus::Complete => "complete",
    }
}

fn parse_printer_reminder_status(value: &str) -> Option<PrinterReminderStatus> {
    match value {
        "active" => Some(PrinterReminderStatus::Active),
        "paused" => Some(PrinterReminderStatus::Paused),
        "completed" => Some(PrinterReminderStatus::Completed),
        _ => None,
    }
}

fn printer_reminder_status_value(value: &PrinterReminderStatus) -> &'static str {
    match value {
        PrinterReminderStatus::Active => "active",
        PrinterReminderStatus::Paused => "paused",
        PrinterReminderStatus::Completed => "completed",
    }
}

fn printer_bed_columns(bed_mm: Option<[f32; 3]>) -> (Option<f32>, Option<f32>, Option<f32>) {
    match bed_mm {
        Some([x, y, z]) => (Some(x), Some(y), Some(z)),
        None => (None, None, None),
    }
}
fn attempt_filaments(conn: &Connection, id: &str) -> Result<Vec<String>, LibraryError> {
    let mut s = conn.prepare(
        "SELECT filament_id FROM print_attempt_filaments WHERE attempt_id=?1 ORDER BY ordinal",
    )?;
    let r = s.query_map(params![id], |row| row.get(0))?;
    Ok(r.collect::<Result<Vec<_>, _>>()?)
}

fn attempt_parts(conn: &Connection, id: &str) -> Result<Vec<PrintAttemptPart>, LibraryError> {
    let mut statement = conn.prepare(
        "SELECT pap.part_id,COALESCE(parts.name,pap.part_name),
                CASE WHEN parts.id IS NULL THEN pap.category ELSE parts.category END,
                CASE WHEN parts.id IS NULL THEN pap.unit ELSE parts.unit END,
                pap.quantity_per_copy,pap.quantity
         FROM print_attempt_parts pap
         LEFT JOIN parts_stock parts ON parts.id=pap.part_id
         WHERE pap.attempt_id=?1
         ORDER BY lower(COALESCE(parts.name,pap.part_name)),pap.ordinal",
    )?;
    let rows = statement.query_map(params![id], |row| {
        Ok(PrintAttemptPart {
            id: Some(row.get(0)?),
            name: row.get(1)?,
            category: row.get(2)?,
            unit: row.get(3)?,
            quantity_per_copy: row.get::<_, i64>(4)? as u32,
            quantity: row.get::<_, i64>(5)? as u32,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn normalize_rfc3339(value: &str) -> Result<String, LibraryError> {
    normalize_rfc3339_field(value, "printed_at")
}

fn normalize_rfc3339_field(value: &str, field: &str) -> Result<String, LibraryError> {
    use time::format_description::well_known::Rfc3339;
    use time::{OffsetDateTime, UtcOffset};
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| LibraryError::InvalidInput(format!("{field} must be RFC3339")))?
        .to_offset(UtcOffset::UTC);
    let millis = parsed.unix_timestamp_nanos().div_euclid(1_000_000);
    let normalized = OffsetDateTime::from_unix_timestamp_nanos(millis * 1_000_000)
        .map_err(|_| LibraryError::InvalidInput(format!("{field} is outside supported range")))?;
    let format = time::format_description::parse_borrowed::<2>(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z",
    )
    .map_err(|_| LibraryError::InvalidInput("timestamp format is unavailable".into()))?;
    normalized
        .format(&format)
        .map_err(|_| LibraryError::InvalidInput(format!("{field} is outside supported range")))
}

fn add_rfc3339_days(value: &str, days: u32) -> Result<String, LibraryError> {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| LibraryError::InvalidInput("completed_at must be RFC3339".into()))?;
    let updated = parsed
        .checked_add(time::Duration::days(i64::from(days)))
        .ok_or_else(|| {
            LibraryError::InvalidInput("reminder due date is outside supported range".into())
        })?;
    let formatted = updated.format(&Rfc3339).map_err(|_| {
        LibraryError::InvalidInput("reminder due date is outside supported range".into())
    })?;
    normalize_rfc3339_field(&formatted, "due_at")
}

fn validate_record_id(value: &str, field: &str) -> Result<String, LibraryError> {
    let id = value.trim();
    if id.is_empty()
        || id.len() > 128
        || id.chars().any(char::is_control)
        || id.contains('/')
        || id.contains('\\')
    {
        return Err(LibraryError::InvalidInput(format!("{field} is invalid")));
    }
    Ok(id.to_owned())
}

fn validate_import_external_ref(value: &str, field: &str) -> Result<String, LibraryError> {
    let external_ref = validate_record_id(value, field)?;
    if external_ref != value {
        return Err(LibraryError::InvalidInput(format!(
            "{field} must not have leading or trailing whitespace"
        )));
    }
    Ok(external_ref)
}

fn validate_print_attempt_printer_snapshot(value: &str) -> Result<String, LibraryError> {
    if value.trim().is_empty()
        || value.chars().count() > MAX_PRINTER_TEXT_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(LibraryError::InvalidInput(
            "print attempt printer nickname snapshot is invalid".into(),
        ));
    }
    Ok(value.to_owned())
}

fn validate_import_snapshot_name(value: &str) -> Result<String, LibraryError> {
    if value.trim().is_empty()
        || value.chars().count() > MAX_PRINTER_TEXT_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(LibraryError::InvalidInput(
            "maintenance printer nickname snapshot is invalid".into(),
        ));
    }
    Ok(value.to_owned())
}

fn validate_optional_import_snapshot_name(
    value: Option<&str>,
) -> Result<Option<String>, LibraryError> {
    value
        .map(|name| {
            if name.is_empty() {
                Ok(String::new())
            } else {
                validate_import_snapshot_name(name)
            }
        })
        .transpose()
}

/// Import timestamps are opaque catalog metadata in the existing schema.
/// Preserve their exact representation while bounding the untrusted text.
fn validate_import_timestamp(value: &str, field: &str) -> Result<String, LibraryError> {
    if value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(LibraryError::InvalidInput(format!("{field} is invalid")));
    }
    Ok(value.to_owned())
}

fn ensure_record_version(kind: &str, expected: u64, actual: i64) -> Result<(), LibraryError> {
    let actual = u64::try_from(actual)
        .map_err(|_| LibraryError::InvalidInput(format!("{kind} version is invalid")))?;
    if actual != expected {
        return Err(LibraryError::InvalidInput(format!(
            "{kind} changed since it was read; expected version {expected}, found {actual}"
        )));
    }
    Ok(())
}

fn ensure_part_stock_version(
    part_id: &str,
    expected: u64,
    actual: i64,
) -> Result<(), LibraryError> {
    let actual = u64::try_from(actual)
        .map_err(|_| LibraryError::InvalidInput("part version is invalid".into()))?;
    if actual != expected {
        return Err(LibraryError::PartStockVersionConflict {
            part_id: part_id.into(),
            expected,
            actual,
        });
    }
    Ok(())
}

fn ensure_printer_maintenance_log_version(
    conn: &Connection,
    id: &str,
    expected: u64,
) -> Result<(), LibraryError> {
    let actual = conn
        .query_row(
            "SELECT metadata_version FROM printer_maintenance_logs WHERE id=?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| LibraryError::InvalidInput("maintenance log not found".into()))?;
    ensure_record_version("printer maintenance log", expected, actual)
}

fn clean_bounded_label(value: &str, field: &str) -> Result<String, LibraryError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_PRINTER_TEXT_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(LibraryError::InvalidInput(format!(
            "{field} must be non-empty, at most {MAX_PRINTER_TEXT_CHARS} characters, and contain no control characters"
        )));
    }
    Ok(value.to_owned())
}

fn clean_optional_printer_field(
    value: Option<&str>,
    field: &str,
) -> Result<Option<String>, LibraryError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| clean_bounded_label(value, field))
        .transpose()
}

fn clean_bounded_notes(value: &str, field: &str) -> Result<String, LibraryError> {
    if value.len() > MAX_PRINTER_NOTES_BYTES
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(LibraryError::InvalidInput(format!(
            "{field} exceeds its byte limit or contains invalid control characters"
        )));
    }
    Ok(value.to_owned())
}

fn normalize_part_stock_input(input: &PartStockInput) -> Result<PartStockInput, LibraryError> {
    if input.quantity > MAX_PART_STOCK_QUANTITY || input.low_stock > MAX_PART_STOCK_QUANTITY {
        return Err(LibraryError::InvalidInput(
            "part stock quantities must be between 0 and 1000000".into(),
        ));
    }
    let name = clean_bounded_label(&input.name, "part name")?;
    let category = clean_optional_printer_field(input.category.as_deref(), "part category")?;
    let unit = if input.unit.trim().is_empty() {
        "pcs"
    } else {
        input.unit.trim()
    };
    let unit = clean_bounded_label(unit, "part unit")?;
    let notes = clean_bounded_notes(input.notes.trim(), "part notes")?;
    Ok(PartStockInput {
        name,
        category,
        quantity: input.quantity,
        unit,
        notes,
        low_stock: input.low_stock,
    })
}

fn normalize_part_usage_snapshot(
    snapshot: &PartUsageSnapshot,
) -> Result<PartUsageSnapshot, LibraryError> {
    let normalized = normalize_part_stock_input(&PartStockInput {
        name: snapshot.name.clone(),
        category: snapshot.category.clone(),
        quantity: 0,
        unit: snapshot.unit.clone(),
        notes: String::new(),
        low_stock: 0,
    })?;
    Ok(PartUsageSnapshot {
        name: normalized.name,
        category: normalized.category,
        unit: normalized.unit,
    })
}

fn normalize_printer_input(input: &PrinterInput) -> Result<PrinterInput, LibraryError> {
    let nickname = clean_bounded_label(&input.nickname, "printer nickname")?;
    let make = clean_optional_printer_field(input.make.as_deref(), "printer make")?;
    let model = clean_optional_printer_field(input.model.as_deref(), "printer model")?;
    let printer_type = clean_optional_printer_field(input.printer_type.as_deref(), "printer type")?;
    let firmware = clean_optional_printer_field(input.firmware.as_deref(), "printer firmware")?;
    let host = validate_printer_host(input.host.as_deref())?;
    let web_url = normalize_printer_web_url(input.web_url.as_deref())?;
    let notes = clean_bounded_notes(&input.notes, "printer notes")?;
    if let Some(bed) = input.bed_mm {
        if bed
            .iter()
            .any(|dimension| !dimension.is_finite() || !(0.1..=10_000.0).contains(dimension))
        {
            return Err(LibraryError::InvalidInput(
                "printer bed dimensions must be finite and between 0.1 and 10000 mm".into(),
            ));
        }
    }
    Ok(PrinterInput {
        nickname,
        make,
        model,
        printer_type,
        firmware,
        host,
        web_url,
        bed_mm: input.bed_mm,
        notes,
    })
}

fn validate_printer_host(value: Option<&str>) -> Result<Option<String>, LibraryError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > 255
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || value
            .chars()
            .any(|character| matches!(character, '@' | '/' | '\\' | '?' | '#'))
    {
        return Err(LibraryError::InvalidInput(
            "printer host must be a hostname or IP address without credentials or a URL path"
                .into(),
        ));
    }
    let parsed = url::Url::parse(&format!("http://{value}/"))
        .map_err(|_| LibraryError::InvalidInput("printer host is invalid".into()))?;
    if parsed.host_str().is_none() || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(LibraryError::InvalidInput("printer host is invalid".into()));
    }
    Ok(Some(value.to_owned()))
}

/// Validates a printer's HTTP(S) endpoint and keeps only its origin. Path,
/// query, and fragment values are dropped because imported URLs are inert
/// endpoint hints and may contain tokens or other sensitive values.
pub fn normalize_printer_web_url(value: Option<&str>) -> Result<Option<String>, LibraryError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() > 2048 || value.chars().any(char::is_control) {
        return Err(LibraryError::InvalidInput(
            "printer web URL is invalid".into(),
        ));
    }
    let parsed = url::Url::parse(value)
        .map_err(|_| LibraryError::InvalidInput("printer web URL is invalid".into()))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(LibraryError::InvalidInput(
            "printer web URL must be HTTP(S) without credentials".into(),
        ));
    }
    Ok(Some(parsed.origin().ascii_serialization()))
}

fn checked_export_count(count: i64, label: &str, maximum: usize) -> Result<usize, LibraryError> {
    let count = usize::try_from(count)
        .map_err(|_| LibraryError::InvalidInput(format!("JSON export {label} count is invalid")))?;
    if count > maximum {
        return Err(LibraryError::InvalidInput(format!(
            "JSON export has {count} {label}, exceeding the supported maximum of {maximum}"
        )));
    }
    Ok(count)
}

fn checked_history_list_count(
    count: i64,
    label: &str,
    maximum: usize,
) -> Result<usize, LibraryError> {
    let count = usize::try_from(count).map_err(|_| {
        LibraryError::InvalidInput(format!("printer {label} list count is invalid"))
    })?;
    if count > maximum {
        return Err(LibraryError::InvalidInput(format!(
            "printer {label} list has {count} records, exceeding the supported maximum of {maximum}"
        )));
    }
    Ok(count)
}

fn ensure_printer_history_capacity(
    current_count: i64,
    incoming_count: usize,
    label: &str,
    maximum: usize,
) -> Result<(), LibraryError> {
    let current_count = usize::try_from(current_count)
        .map_err(|_| LibraryError::InvalidInput(format!("printer {label} count is invalid")))?;
    let total = current_count
        .checked_add(incoming_count)
        .ok_or_else(|| LibraryError::InvalidInput(format!("printer {label} count overflowed")))?;
    if total > maximum {
        return Err(LibraryError::InvalidInput(format!(
            "printer {label} limit of {maximum} records would be exceeded"
        )));
    }
    Ok(())
}

fn validate_endpoint_approval(input: &PrinterInput, approved: bool) -> Result<(), LibraryError> {
    if approved && input.host.is_none() && input.web_url.is_none() {
        return Err(LibraryError::InvalidInput(
            "an endpoint must be saved before it can be approved".into(),
        ));
    }
    Ok(())
}

fn validate_printer_status(
    connection: PrinterConnectionStatus,
    job: PrinterJobStatus,
    error_code: Option<&str>,
) -> Result<(), LibraryError> {
    if connection != PrinterConnectionStatus::Online && job != PrinterJobStatus::Unknown {
        return Err(LibraryError::InvalidInput(
            "job status must be unknown unless the printer is online".into(),
        ));
    }
    if let Some(code) = error_code {
        if code.is_empty()
            || code.len() > 64
            || !code.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            })
            || matches!(
                connection,
                PrinterConnectionStatus::Online | PrinterConnectionStatus::Unknown
            )
        {
            return Err(LibraryError::InvalidInput(
                "printer connection error code is invalid or not applicable".into(),
            ));
        }
    }
    Ok(())
}

fn normalize_maintenance_input(
    input: &PrinterMaintenanceInput,
) -> Result<PrinterMaintenanceInput, LibraryError> {
    Ok(PrinterMaintenanceInput {
        title: clean_bounded_label(&input.title, "maintenance title")?,
        performed_at: normalize_rfc3339_field(&input.performed_at, "performed_at")?,
        notes: clean_bounded_notes(&input.notes, "maintenance notes")?,
    })
}

fn normalize_printer_reminder_input(
    input: &PrinterReminderInput,
) -> Result<PrinterReminderInput, LibraryError> {
    if input
        .interval_days
        .is_some_and(|days| !(1..=36_500).contains(&days))
    {
        return Err(LibraryError::InvalidInput(
            "printer reminder interval must be between 1 and 36500 days".into(),
        ));
    }
    Ok(PrinterReminderInput {
        title: clean_bounded_label(&input.title, "reminder title")?,
        notes: clean_bounded_notes(&input.notes, "reminder notes")?,
        due_at: normalize_rfc3339_field(&input.due_at, "due_at")?,
        interval_days: input.interval_days,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        PrinterDataImportMaintenanceLog, PrinterDataImportPrinter, PrinterDataImportReminder,
    };
    use std::fs;

    const TEST_SCHEMA_V1: &str = r#"
    CREATE TABLE libraries (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL);
    INSERT INTO libraries VALUES ('local', 'Local Library', '0');
    CREATE TABLE folders (id TEXT PRIMARY KEY, name TEXT NOT NULL, parent_id TEXT REFERENCES folders(id) ON DELETE RESTRICT, UNIQUE(parent_id, name));
    INSERT INTO folders VALUES ('root', 'All Models', NULL);
    CREATE TABLE blobs (hash TEXT PRIMARY KEY, relative_path TEXT NOT NULL UNIQUE, bytes INTEGER NOT NULL CHECK(bytes >= 0));
    CREATE TABLE models (id TEXT PRIMARY KEY, display_name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', folder_id TEXT NOT NULL REFERENCES folders(id) ON DELETE RESTRICT, current_revision_id TEXT, metadata_version INTEGER NOT NULL DEFAULT 1 CHECK(metadata_version > 0), deleted_at TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
    CREATE TABLE revisions (id TEXT PRIMARY KEY, model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE, blob_hash TEXT NOT NULL REFERENCES blobs(hash) ON DELETE RESTRICT, original_name TEXT NOT NULL, format TEXT NOT NULL, bytes INTEGER NOT NULL CHECK(bytes >= 0), created_at TEXT NOT NULL);
    CREATE TABLE tags (id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE);
    CREATE TABLE model_tags (model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE, tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE, PRIMARY KEY(model_id, tag_id));
    CREATE TABLE manuals (model_id TEXT PRIMARY KEY REFERENCES models(id) ON DELETE CASCADE, relative_path TEXT NOT NULL, content_hash TEXT NOT NULL);
    CREATE TABLE thumbnails (id TEXT PRIMARY KEY, model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE, relative_path TEXT NOT NULL, content_hash TEXT NOT NULL, is_custom INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE jobs (id TEXT PRIMARY KEY, model_id TEXT REFERENCES models(id) ON DELETE CASCADE, kind TEXT NOT NULL, status TEXT NOT NULL, retry_count INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
    "#;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "gogglelab-library-{label}-{}-{}",
            std::process::id(),
            Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    /// Rebuilds only the v6 print-attempt shape so migration fixtures created
    /// from the current base schema exercise the real v6→v7 DDL path.
    fn downgrade_printer_schema_to_v6(conn: &Connection) {
        conn.execute_batch(
            "PRAGMA foreign_keys=OFF;
             DROP TABLE IF EXISTS print_attempt_parts;
             DROP TABLE IF EXISTS parts_stock;
             DROP INDEX IF EXISTS print_attempt_printer_idx;
             DROP INDEX IF EXISTS printer_reminders_due_idx;
             DROP INDEX IF EXISTS printer_reminders_printer_idx;
             DROP INDEX IF EXISTS printer_maintenance_printer_idx;
             DROP INDEX IF EXISTS printers_name_idx;
             DROP TABLE IF EXISTS printer_reminders;
             DROP TABLE IF EXISTS printer_maintenance_logs;
             DROP TABLE IF EXISTS printers;
             ALTER TABLE print_attempts RENAME TO print_attempts_v7;
             DROP TABLE print_attempt_filaments;
             CREATE TABLE print_attempts (
                id TEXT PRIMARY KEY,
                model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
                printed_at TEXT NOT NULL,
                quantity INTEGER NOT NULL,
                notes TEXT NOT NULL,
                outcome TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             INSERT INTO print_attempts
                (id,model_id,printed_at,quantity,notes,outcome,created_at,updated_at)
                SELECT id,model_id,printed_at,quantity,notes,outcome,created_at,updated_at
                FROM print_attempts_v7;
             DROP TABLE print_attempts_v7;
             CREATE TABLE print_attempt_filaments (
                attempt_id TEXT NOT NULL REFERENCES print_attempts(id) ON DELETE CASCADE,
                filament_id TEXT NOT NULL,
                ordinal INTEGER NOT NULL,
                PRIMARY KEY(attempt_id,filament_id)
             );
             PRAGMA user_version=6;",
        )
        .unwrap();
    }

    /// Rebuilds the v7 maintenance table shape so the v7→v8 migration is tested
    /// against a populated database rather than only against a fresh schema.
    fn downgrade_printer_maintenance_logs_to_v7(conn: &Connection) {
        conn.execute_batch(
            "PRAGMA foreign_keys=OFF;
             BEGIN IMMEDIATE;
             DROP TABLE IF EXISTS print_attempt_parts;
             DROP TABLE IF EXISTS parts_stock;
             DROP INDEX IF EXISTS printer_maintenance_printer_idx;
             ALTER TABLE printer_maintenance_logs RENAME TO printer_maintenance_logs_v8;
             ALTER TABLE print_attempts DROP COLUMN stock_adjusted;
             CREATE TABLE printer_maintenance_logs (
                id TEXT PRIMARY KEY,
                printer_id TEXT REFERENCES printers(id) ON DELETE SET NULL,
                printer_nickname TEXT NOT NULL CHECK(length(printer_nickname) BETWEEN 1 AND 128),
                title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 128),
                performed_at TEXT NOT NULL,
                notes TEXT NOT NULL CHECK(length(notes) <= 20000),
                created_at TEXT NOT NULL
             );
             INSERT INTO printer_maintenance_logs
                (id,printer_id,printer_nickname,title,performed_at,notes,created_at)
             SELECT id,printer_id,printer_nickname,title,performed_at,notes,created_at
             FROM printer_maintenance_logs_v8;
             DROP TABLE printer_maintenance_logs_v8;
             CREATE INDEX printer_maintenance_printer_idx
                ON printer_maintenance_logs(printer_id,performed_at DESC,id);
             PRAGMA user_version=7;
             COMMIT;
             PRAGMA foreign_keys=ON;",
        )
        .unwrap();
    }

    fn hash(byte: u8) -> String {
        format!("{byte:02x}").repeat(32)
    }

    fn png_fixture() -> Vec<u8> {
        b"\x89PNG\r\n\x1a\nthumbnail fixture".to_vec()
    }

    fn stl_fixture() -> Vec<u8> {
        let mut bytes = vec![0; 80];
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(
            &[0.0f32; 3]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        for value in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
        bytes
    }

    #[test]
    fn fresh_database_has_root_and_round_trips_model_after_restart() {
        let root = temp_root("restart");
        let db = LibraryDb::open(&root).unwrap();
        let folder = db.create_folder("Projects", None).unwrap();
        let model = db.create_model("Bracket", Some(&folder.id)).unwrap();
        drop(db);
        let reopened = LibraryDb::open(&root).unwrap();
        assert_eq!(
            reopened.get_model(&model.id).unwrap().unwrap().display_name,
            "Bracket"
        );
        assert_eq!(reopened.list_folders().unwrap().len(), 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn catalog_settings_persist_and_existing_schema_v2_gets_defaults() {
        let root = temp_root("catalog-settings");
        let db = LibraryDb::open(&root).unwrap();
        assert_eq!(
            db.get_catalog_settings().unwrap(),
            CatalogSettings::default()
        );
        let updated = CatalogSettings {
            thumbnail_size: 512,
            thumbnail_background: "#A1B2C3".into(),
            thumbnail_concurrency: 1,
            scan_batch_size: 256,
        };
        assert_eq!(db.update_catalog_settings(&updated).unwrap(), updated);
        drop(db);
        assert_eq!(
            LibraryDb::open(&root)
                .unwrap()
                .get_catalog_settings()
                .unwrap(),
            updated
        );

        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute("DROP TABLE catalog_settings", []).unwrap();
        drop(conn);
        assert_eq!(
            LibraryDb::open(&root)
                .unwrap()
                .get_catalog_settings()
                .unwrap(),
            CatalogSettings::default()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn catalog_settings_reject_out_of_range_values_without_persisting() {
        let db = LibraryDb::open_in_memory().unwrap();
        let original = db.get_catalog_settings().unwrap();
        for invalid in [
            CatalogSettings {
                thumbnail_size: 300,
                ..original.clone()
            },
            CatalogSettings {
                thumbnail_background: "12171F".into(),
                ..original.clone()
            },
            CatalogSettings {
                thumbnail_concurrency: 3,
                ..original.clone()
            },
            CatalogSettings {
                scan_batch_size: 15,
                ..original.clone()
            },
        ] {
            assert!(matches!(
                db.update_catalog_settings(&invalid),
                Err(LibraryError::InvalidInput(_))
            ));
            assert_eq!(db.get_catalog_settings().unwrap(), original);
        }

        db.conn
            .execute_batch(
                "DROP TABLE catalog_settings;
                 CREATE TABLE catalog_settings (
                   id TEXT PRIMARY KEY,
                   thumbnail_size INTEGER NOT NULL,
                   thumbnail_background TEXT NOT NULL,
                   thumbnail_concurrency INTEGER NOT NULL,
                   scan_batch_size INTEGER NOT NULL
                 );
                 INSERT INTO catalog_settings VALUES
                   ('local', 4294967552, '#12171F', 4294967297, 128);",
            )
            .unwrap();
        assert!(matches!(
            db.get_catalog_settings(),
            Err(LibraryError::InvalidInput(_))
        ));
    }

    #[test]
    fn metadata_tags_history_and_parent_updates_are_versioned_and_atomic() {
        let db = LibraryDb::open_in_memory().unwrap();
        let a = db.create_model("A", None).unwrap();
        let b = db.create_model("B", None).unwrap();
        let tag = db.create_tag("Mechanical", Some("Type")).unwrap();
        let patch = MetadataPatch {
            designer: Some("Ada".into()),
            notes: Some("notes".into()),
            tags: Some(vec![tag.id.clone()]),
            tag_mode: Some(TagMode::Replace),
            parent_model_id: Some(b.id.clone()),
            ..MetadataPatch::default()
        };
        let a = db.update_model_metadata(&a.id, 1, &patch).unwrap();
        assert_eq!(a.metadata.designer.as_deref(), Some("Ada"));
        assert_eq!(a.tags, vec!["Mechanical"]);
        assert_eq!(a.metadata.parent_model_id.as_deref(), Some(b.id.as_str()));
        assert!(matches!(
            db.update_model_metadata(&a.id, 1, &MetadataPatch::default()),
            Err(LibraryError::MetadataConflict { .. })
        ));
        assert!(db
            .update_model_metadata(
                &b.id,
                1,
                &MetadataPatch {
                    parent_model_id: Some(a.id.clone()),
                    ..MetadataPatch::default()
                }
            )
            .is_err());
        let before = a.metadata_version;
        assert!(db
            .bulk_update_metadata(
                &[
                    VersionedTarget {
                        model_id: a.id.clone(),
                        expected_version: before
                    },
                    VersionedTarget {
                        model_id: b.id.clone(),
                        expected_version: 999
                    }
                ],
                &MetadataPatch {
                    notes: Some("bulk".into()),
                    ..MetadataPatch::default()
                }
            )
            .is_err());
        assert_eq!(
            db.get_model(&a.id).unwrap().unwrap().metadata_version,
            before
        );
        let renamed = db.rename_tag(&tag.id, "Hardware", Some("Type")).unwrap();
        assert_eq!(renamed.name, "Hardware");
        let bumped = db.get_model(&a.id).unwrap().unwrap();
        assert!(bumped.metadata_version > before);
        assert_eq!(bumped.tags, vec!["Hardware"]);
        db.delete_tag(&tag.id).unwrap();
        assert!(db.get_model(&a.id).unwrap().unwrap().tags.is_empty());
        let attempt = db
            .create_print_attempt(
                &a.id,
                &PrintAttemptInput {
                    printed_at: "2026-10-10T12:00:00-05:00".into(),
                    quantity: 2,
                    notes: "good".into(),
                    filament_ids: vec!["pla-red".into()],
                    outcome: PrintOutcome::Printed,
                    parts: None,
                },
            )
            .unwrap();
        assert_eq!(attempt.filament_ids, vec!["pla-red"]);
        assert_eq!(attempt.printed_at, "2026-10-10T17:00:00.000Z");
        let printed = db.get_model(&a.id).unwrap().unwrap();
        assert_eq!(printed.metadata.print_count, 2);
        assert_eq!(
            printed.metadata.last_printed_at.as_deref(),
            Some("2026-10-10T17:00:00.000Z")
        );
        let failed_attempt = db
            .create_print_attempt(
                &a.id,
                &PrintAttemptInput {
                    printed_at: "2026-10-11T00:00:00Z".into(),
                    quantity: 1,
                    notes: "failed".into(),
                    filament_ids: vec![],
                    outcome: PrintOutcome::Failed,
                    parts: None,
                },
            )
            .unwrap();
        db.update_print_attempt(
            &failed_attempt.id,
            &PrintAttemptInput {
                printed_at: "2026-10-11T01:00:00+01:00".into(),
                quantity: 1,
                notes: "failed edited".into(),
                filament_ids: vec![],
                outcome: PrintOutcome::Failed,
                parts: None,
            },
        )
        .unwrap();
        let after_failure = db.get_model(&a.id).unwrap().unwrap();
        assert_eq!(after_failure.metadata.print_status, PrintStatus::Failed);
        assert_eq!(after_failure.metadata.print_count, 2);
        assert_eq!(
            after_failure.metadata.last_printed_at.as_deref(),
            Some("2026-10-10T17:00:00.000Z")
        );
        let current = db.get_model(&a.id).unwrap().unwrap();
        db.update_model_metadata(
            &a.id,
            current.metadata_version,
            &MetadataPatch {
                print_status: Some(PrintStatus::Queued),
                ..MetadataPatch::default()
            },
        )
        .unwrap();
        db.delete_print_attempt(&attempt.id).unwrap();
        assert_eq!(
            db.get_model(&a.id).unwrap().unwrap().metadata.print_status,
            PrintStatus::Queued
        );
        let current = db.get_model(&a.id).unwrap().unwrap();
        let cleared = db
            .update_model_metadata(
                &a.id,
                current.metadata_version,
                &MetadataPatch {
                    notes: Some(String::new()),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        assert_eq!(cleared.metadata.notes, None);
        assert_eq!(cleared.description, "");
    }

    #[test]
    fn source_metadata_accepts_http_urls_and_safe_origin_labels_only() {
        let db = LibraryDb::open_in_memory().unwrap();
        let mut model = db.create_model("Source metadata", None).unwrap();

        for source in [
            "https://example.test/models/42",
            "http://example.test/models/42",
            "HTTPS://example.test/models/42",
            "Thingiverse",
            "my-mini-factory.com",
        ] {
            model = db
                .update_model_metadata(
                    &model.id,
                    model.metadata_version,
                    &MetadataPatch {
                        source_url: Some(source.into()),
                        ..MetadataPatch::default()
                    },
                )
                .unwrap();
            assert_eq!(model.metadata.source_url.as_deref(), Some(source));
        }

        for invalid in [
            "https://",
            "https://user:password@example.test/model",
            "https://@example.test/model",
            "https://example.test/model\nInjected",
            "javascript:alert(1)",
            "file:///tmp/model.stl",
            "//example.test/model",
            "Thingiverse/path",
            "Thingiverse?download=1",
            "Thingiverse#model",
            "Thingiverse\\model",
            "Thingiverse:archive",
            "Thingiverse\n",
            "Thingiverse\u{0001}",
        ] {
            let version = model.metadata_version;
            assert!(
                db.update_model_metadata(
                    &model.id,
                    version,
                    &MetadataPatch {
                        source_url: Some(invalid.into()),
                        ..MetadataPatch::default()
                    },
                )
                .is_err(),
                "unexpectedly accepted {invalid:?}"
            );
            assert_eq!(
                db.get_model(&model.id).unwrap().unwrap().metadata_version,
                version
            );
        }
    }

    #[test]
    fn print_attempt_creation_time_import_is_validated_and_does_not_replace_update_time() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Print history", None).unwrap();
        let imported_value = serde_json::json!({
            "printed_at": "2026-10-10T12:00:00Z",
            "quantity": 1,
            "notes": "Imported history",
            "filament_ids": [],
            "outcome": "printed"
        });
        let imported_input: PrintAttemptInput = serde_json::from_value(imported_value).unwrap();
        let imported = db
            .import_print_attempt(
                &model.id,
                &imported_input,
                "2020-01-02T03:04:05.123456+05:30",
            )
            .unwrap();

        assert_eq!(imported.created_at, "2020-01-01T21:34:05.123Z");
        assert_ne!(imported.updated_at, imported.created_at);

        let invalid_value = serde_json::json!({
            "printed_at": "2026-10-10T12:00:00Z",
            "quantity": 1,
            "notes": "Invalid imported history",
            "filament_ids": [],
            "outcome": "printed"
        });
        let invalid_input: PrintAttemptInput = serde_json::from_value(invalid_value).unwrap();
        assert!(db
            .import_print_attempt(&model.id, &invalid_input, "2020-02-30T00:00:00Z")
            .is_err());
        assert_eq!(db.list_print_attempts(&model.id).unwrap().len(), 1);

        let omitted_input: PrintAttemptInput = serde_json::from_value(serde_json::json!({
            "printed_at": "2026-10-11T12:00:00Z",
            "quantity": 1,
            "notes": "No imported creation time",
            "filament_ids": [],
            "outcome": "failed"
        }))
        .unwrap();
        let omitted = db.create_print_attempt(&model.id, &omitted_input).unwrap();
        assert_eq!(omitted.created_at, omitted.updated_at);
        assert_ne!(omitted.created_at, imported.created_at);
    }

    #[test]
    fn ordinary_print_attempt_creation_does_not_accept_imported_creation_time() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Ordinary print", None).unwrap();
        let requested = "2020-01-01T00:00:00Z";
        let mut value = serde_json::json!({
            "printed_at": "2026-10-10T12:00:00Z",
            "quantity": 1,
            "notes": "Normal user-created event",
            "filament_ids": [],
            "outcome": "printed"
        });
        value["created_at"] = serde_json::Value::String(requested.into());
        let input: PrintAttemptInput = serde_json::from_value(value).unwrap();

        let attempt = db.create_print_attempt(&model.id, &input).unwrap();

        assert_ne!(attempt.created_at, requested);
        assert_eq!(attempt.created_at, attempt.updated_at);

        let mut update_value = serde_json::json!({
            "printed_at": "2026-10-11T12:00:00Z",
            "quantity": 1,
            "notes": "Updated user-created event",
            "filament_ids": [],
            "outcome": "failed"
        });
        update_value["created_at"] = serde_json::Value::String(requested.into());
        let update_input: PrintAttemptInput = serde_json::from_value(update_value).unwrap();
        let updated = db.update_print_attempt(&attempt.id, &update_input).unwrap();
        assert_eq!(updated.created_at, attempt.created_at);
    }

    #[test]
    fn global_metadata_rename_and_delete_are_atomic_and_preserve_parent_identity() {
        let db = LibraryDb::open_in_memory().unwrap();
        let parent_a = db.create_model("Old parent", None).unwrap();
        let parent_b = db.create_model("Old parent", None).unwrap();
        let child = db.create_model("Child", None).unwrap();
        let child = db
            .update_model_metadata(
                &child.id,
                child.metadata_version,
                &MetadataPatch {
                    designer: Some("Old designer".into()),
                    model_license: Some("Old license".into()),
                    parent_model_id: Some(parent_a.id.clone()),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();

        assert_eq!(
            db.rename_metadata_value("designer", "Old designer", "New designer")
                .unwrap(),
            1
        );
        assert_eq!(
            db.rename_metadata_value("license", "Old license", "New license")
                .unwrap(),
            1
        );
        assert_eq!(
            db.rename_metadata_value("parentModel", "Old parent", "New parent")
                .unwrap(),
            2
        );
        let renamed_parent = db.get_model(&parent_a.id).unwrap().unwrap();
        assert_eq!(renamed_parent.display_name, "New parent");
        assert_eq!(
            db.get_model(&parent_b.id).unwrap().unwrap().display_name,
            "New parent"
        );
        let child = db.get_model(&child.id).unwrap().unwrap();
        assert_eq!(
            child.metadata.parent_model_id.as_deref(),
            Some(parent_a.id.as_str())
        );
        assert_eq!(child.metadata.designer.as_deref(), Some("New designer"));
        assert_eq!(child.metadata.model_license.as_deref(), Some("New license"));
        let prior_version = child.metadata_version;

        assert_eq!(
            db.delete_metadata_value("parentModel", "New parent")
                .unwrap(),
            1
        );
        assert_eq!(
            db.delete_metadata_value("designer", "New designer")
                .unwrap(),
            1
        );
        assert_eq!(
            db.delete_metadata_value("license", "New license").unwrap(),
            1
        );
        let child = db.get_model(&child.id).unwrap().unwrap();
        assert!(child.metadata.parent_model_id.is_none());
        assert!(child.metadata.designer.is_none());
        assert!(child.metadata.model_license.is_none());
        assert!(child.metadata_version > prior_version);
        assert!(db.rename_metadata_value("unknown", "x", "y").is_err());
    }

    #[test]
    fn thumbnail_filters_use_active_preview_and_filter_before_cursor_limit() {
        let db = LibraryDb::open_in_memory().unwrap();
        let default = db.create_model("Alpha default", None).unwrap();
        let custom = db.create_model("Beta custom", None).unwrap();
        let missing = db.create_model("Gamma missing", None).unwrap();
        let stale = db.create_model("Delta stale", None).unwrap();
        for (id, model, is_custom, active, source_hash) in [
            ("default", &default, 0, 1, None),
            ("custom-fallback", &custom, 0, 0, None),
            ("custom", &custom, 1, 1, None),
            ("stale", &stale, 0, 1, Some("no-longer-current")),
        ] {
            db.conn.execute(
                "INSERT INTO thumbnails(id,model_id,relative_path,content_hash,is_custom,source_content_hash,ordinal,is_active) VALUES(?1,?2,'thumbnails/fixture.png',?3,?4,?5,0,?6)",
                params![id, model.id, "0".repeat(64), is_custom, source_hash, active],
            ).unwrap();
        }
        for (mode, expected) in [("default", &default.id), ("custom", &custom.id)] {
            let query = ModelPageQuery {
                thumbnail_mode: Some(mode.into()),
                sort: ModelSort::Name,
                ..ModelPageQuery::default()
            };
            let (models, more) = db.list_models_page(&query, None, None, 1).unwrap();
            assert_eq!(models[0].id, *expected);
            assert!(!more);
        }
        let query = ModelPageQuery {
            thumbnail_mode: Some("missing".into()),
            sort: ModelSort::Name,
            ..ModelPageQuery::default()
        };
        let (first, more) = db.list_models_page(&query, None, None, 1).unwrap();
        assert_eq!(first[0].id, stale.id);
        assert!(more);
        let (second, more) = db
            .list_models_page(&query, Some("delta stale"), Some(&stale.id), 1)
            .unwrap();
        assert_eq!(second[0].id, missing.id);
        assert!(!more);
        assert!(db
            .list_models_page(
                &ModelPageQuery {
                    thumbnail_mode: Some("invalid".into()),
                    ..ModelPageQuery::default()
                },
                None,
                None,
                1
            )
            .is_err());
    }

    #[test]
    fn model_tag_name_mutations_are_atomic_versioned_and_keep_removed_tags() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Tagged model", None).unwrap();
        let keep = db.create_tag("Keep", Some("Type")).unwrap();
        let remove = db.create_tag("Remove", Some("Type")).unwrap();
        let model = db
            .update_model_metadata(
                &model.id,
                model.metadata_version,
                &MetadataPatch {
                    tags: Some(vec![keep.id.clone(), remove.id.clone()]),
                    tag_mode: Some(TagMode::Replace),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();

        let added = db
            .add_model_tags_by_name(
                &model.id,
                model.metadata_version,
                &["Keep".into(), "New tag".into()],
            )
            .unwrap();
        assert_eq!(added.metadata_version, model.metadata_version + 1);
        assert_eq!(added.tags, vec!["Keep", "New tag", "Remove"]);
        assert!(db
            .list_tags()
            .unwrap()
            .iter()
            .any(|tag| tag.name == "New tag"));

        let removed = db
            .remove_model_tags_by_name(&model.id, added.metadata_version, &["remove".into()])
            .unwrap();
        assert_eq!(removed.metadata_version, added.metadata_version + 1);
        assert_eq!(removed.tags, vec!["Keep", "New tag"]);
        assert!(db
            .list_tags()
            .unwrap()
            .iter()
            .any(|tag| tag.id == remove.id));

        let unchanged = db
            .remove_model_tags_by_name(&model.id, removed.metadata_version, &["Remove".into()])
            .unwrap();
        assert_eq!(unchanged.metadata_version, removed.metadata_version);

        assert!(db
            .add_model_tags_by_name(&model.id, model.metadata_version, &["Stale tag".into()])
            .is_err());
        assert!(!db
            .list_tags()
            .unwrap()
            .iter()
            .any(|tag| tag.name == "Stale tag"));
        assert!(db
            .add_model_tags_by_name(
                &model.id,
                removed.metadata_version,
                &["Dup".into(), "dup".into()]
            )
            .is_err());
        assert!(!db.list_tags().unwrap().iter().any(|tag| tag.name == "Dup"));
    }

    #[test]
    fn source_directory_filter_is_root_relative_and_applied_before_keyset_limits() {
        let db = LibraryDb::open_in_memory().unwrap();
        let first_root_path = temp_root("source-directory-first-root");
        fs::create_dir_all(&first_root_path).unwrap();
        let root = db
            .add_index_root(&first_root_path, 60, 1024 * 1024)
            .unwrap();
        let entries = [
            ("A in parts", "Parts/a.stl"),
            ("B nested in parts", "Parts/Nested/b.stl"),
            ("C outside parts", "Parts-old/c.stl"),
        ];
        let mut models = Vec::new();
        for (name, relative) in entries {
            let model = db.create_model(name, None).unwrap();
            db.conn.execute(
                "UPDATE models SET source_kind='indexed',source_root_id=?1,source_path=?2,source_missing=0 WHERE id=?3",
                params![root.id, relative, model.id],
            ).unwrap();
            models.push(db.get_model(&model.id).unwrap().unwrap());
        }
        let second_root_path = temp_root("source-directory-second-root");
        fs::create_dir_all(&second_root_path).unwrap();
        let second_root = db
            .add_index_root(&second_root_path, 60, 1024 * 1024)
            .unwrap();
        let second_root_model = db.create_model("Parts in second root", None).unwrap();
        db.conn.execute(
            "UPDATE models SET source_kind='indexed',source_root_id=?1,source_path='Parts/d.stl',source_missing=0 WHERE id=?2",
            params![second_root.id, second_root_model.id],
        ).unwrap();
        let query = ModelPageQuery {
            root_id: Some(root.id),
            source_directory: Some("Parts".into()),
            sort: ModelSort::Name,
            ..ModelPageQuery::default()
        };
        let (first, has_more) = db.list_models_page(&query, None, None, 1).unwrap();
        assert_eq!(first[0].id, models[0].id);
        assert!(has_more);
        let (second, has_more) = db
            .list_models_page(&query, Some("a in parts"), Some(&first[0].id), 1)
            .unwrap();
        assert_eq!(second[0].id, models[1].id);
        assert!(!has_more);
        let (other_root_parts, has_more) = db
            .list_models_page(
                &ModelPageQuery {
                    root_id: Some(second_root.id),
                    source_directory: Some("Parts".into()),
                    ..ModelPageQuery::default()
                },
                None,
                None,
                10,
            )
            .unwrap();
        assert_eq!(other_root_parts.len(), 1);
        assert_eq!(other_root_parts[0].id, second_root_model.id);
        assert!(!has_more);

        for (root_id, source_directory) in [
            (None, Some("Parts".to_owned())),
            (
                Some(models[0].source_root_id.clone().unwrap()),
                Some("../outside".to_owned()),
            ),
        ] {
            assert!(db
                .list_models_page(
                    &ModelPageQuery {
                        root_id,
                        source_directory,
                        ..ModelPageQuery::default()
                    },
                    None,
                    None,
                    10
                )
                .is_err());
        }
        let _ = fs::remove_dir_all(first_root_path);
        let _ = fs::remove_dir_all(second_root_path);
    }

    #[test]
    fn file_type_filter_matches_archive_source_extension_and_member_format() {
        let db = LibraryDb::open_in_memory().unwrap();
        let source_root = temp_root("file-type-archive-source");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(source_root.join("parts.zip"), b"synthetic archive").unwrap();
        let mut root = db.add_index_root(&source_root, 60, 1024 * 1024).unwrap();
        root.scan_zip = true;
        db.update_index_root(&root).unwrap();
        db.record_scan_entries(
            &root.id,
            &[ScannedModel {
                relative_path: "parts.zip".into(),
                content_hash: hash(7),
                bytes: 17,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("Archive member".into()),
                archive_member: Some("member.stl".into()),
                outer_fingerprint: Some("synthetic-archive-fingerprint".into()),
            }],
        )
        .unwrap();

        let (archive_matches, _) = db
            .list_models_page(
                &ModelPageQuery {
                    file_type: Some("zip".into()),
                    ..ModelPageQuery::default()
                },
                None,
                None,
                10,
            )
            .unwrap();
        assert_eq!(archive_matches.len(), 1);
        assert_eq!(archive_matches[0].display_name, "Archive member");

        let (mesh_matches, _) = db
            .list_models_page(
                &ModelPageQuery {
                    file_type: Some(".STL".into()),
                    ..ModelPageQuery::default()
                },
                None,
                None,
                10,
            )
            .unwrap();
        assert_eq!(mesh_matches.len(), 1);
        let _ = fs::remove_dir_all(source_root);
    }

    #[test]
    fn indexed_source_move_batch_is_atomic_and_rejects_duplicate_destinations() {
        let db = LibraryDb::open_in_memory().unwrap();
        let source_root_path = temp_root("move-batch-source");
        let destination_root_path = temp_root("move-batch-destination");
        fs::create_dir_all(&source_root_path).unwrap();
        fs::create_dir_all(&destination_root_path).unwrap();
        fs::write(source_root_path.join("a.stl"), b"synthetic a").unwrap();
        fs::write(source_root_path.join("b.stl"), b"synthetic b").unwrap();
        let source_root = db
            .add_index_root(&source_root_path, 60, 1024 * 1024)
            .unwrap();
        let destination_root = db
            .add_index_root(&destination_root_path, 60, 1024 * 1024)
            .unwrap();
        db.record_scan_entries(
            &source_root.id,
            &[
                ScannedModel {
                    relative_path: "a.stl".into(),
                    content_hash: hash(1),
                    bytes: 11,
                    format: "stl-binary".into(),
                    modified_at: 1,
                    display_name: Some("A".into()),
                    archive_member: None,
                    outer_fingerprint: None,
                },
                ScannedModel {
                    relative_path: "b.stl".into(),
                    content_hash: hash(2),
                    bytes: 11,
                    format: "stl-binary".into(),
                    modified_at: 1,
                    display_name: Some("B".into()),
                    archive_member: None,
                    outer_fingerprint: None,
                },
            ],
        )
        .unwrap();
        let before = db.list_models(None, None).unwrap();
        let moves = vec![
            IndexedModelMove {
                model_id: before[0].id.clone(),
                expected_version: before[0].metadata_version,
                destination_root_id: destination_root.id.clone(),
                destination_relative_path: "moved-a.stl".into(),
                modified_at: 2,
            },
            IndexedModelMove {
                model_id: before[1].id.clone(),
                expected_version: before[1].metadata_version,
                destination_root_id: destination_root.id.clone(),
                destination_relative_path: "moved-b.stl".into(),
                modified_at: 2,
            },
        ];

        let moved = db.move_loose_indexed_models(&moves).unwrap();
        assert_eq!(moved.len(), 2);
        for (index, model) in moved.iter().enumerate() {
            assert_eq!(model.id, before[index].id);
            assert_eq!(model.metadata_version, before[index].metadata_version + 1);
            assert_eq!(
                model.source_root_id.as_deref(),
                Some(destination_root.id.as_str())
            );
            assert!(model
                .source_path
                .as_deref()
                .unwrap()
                .ends_with(if index == 0 {
                    "moved-a.stl"
                } else {
                    "moved-b.stl"
                }));
        }

        let mut stale = moves.clone();
        stale[0].destination_relative_path = "should-not-move-a.stl".into();
        stale[0].expected_version = moved[0].metadata_version;
        stale[1].destination_relative_path = "should-not-move-b.stl".into();
        stale[1].expected_version = moved[1].metadata_version + 1;
        assert!(db.move_loose_indexed_models(&stale).is_err());

        let mut duplicate_destinations = moves.clone();
        duplicate_destinations[0].expected_version = moved[0].metadata_version;
        duplicate_destinations[1].expected_version = moved[1].metadata_version;
        duplicate_destinations[0].destination_relative_path = "same.stl".into();
        duplicate_destinations[1].destination_relative_path = "same.stl".into();
        assert!(db
            .move_loose_indexed_models(&duplicate_destinations)
            .is_err());
        for (index, original) in before.iter().enumerate() {
            let unchanged = db.get_model(&original.id).unwrap().unwrap();
            assert_eq!(unchanged.metadata_version, moved[index].metadata_version);
            assert!(unchanged
                .source_path
                .as_deref()
                .unwrap()
                .ends_with(if index == 0 {
                    "moved-a.stl"
                } else {
                    "moved-b.stl"
                }));
        }
        let _ = fs::remove_dir_all(source_root_path);
        let _ = fs::remove_dir_all(destination_root_path);
    }

    #[test]
    fn paged_catalog_filters_are_applied_before_keyset_limits() {
        let db = LibraryDb::open_in_memory().unwrap();
        let a = db.create_model("Alpha", None).unwrap();
        let b = db.create_model("Beta", None).unwrap();
        let c = db.create_model("Gamma", None).unwrap();
        let tag = db.create_tag("Mechanical", Some("Type")).unwrap();
        let a = db
            .update_model_metadata(
                &a.id,
                a.metadata_version,
                &MetadataPatch {
                    designer: Some("Designer".into()),
                    model_license: Some("CC0".into()),
                    print_status: Some(PrintStatus::Printed),
                    tags: Some(vec![tag.id.clone()]),
                    tag_mode: Some(TagMode::Replace),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        let filament = db
            .create_filament(&FilamentInput {
                brand: "Example".into(),
                material: "PLA".into(),
                color: "#123456".into(),
                name: "Catalog filter".into(),
                notes: String::new(),
            })
            .unwrap();
        db.set_model_filaments(
            &a.id,
            a.metadata_version,
            std::slice::from_ref(&filament.id),
        )
        .unwrap();
        db.create_print_attempt(
            &a.id,
            &PrintAttemptInput {
                printed_at: "2026-10-11T12:00:00Z".into(),
                quantity: 1,
                notes: String::new(),
                filament_ids: vec![filament.id.clone()],
                outcome: PrintOutcome::Printed,
                parts: None,
            },
        )
        .unwrap();
        let bundle_id = db.create_bundle(&[b.id.clone(), c.id.clone()]).unwrap();

        for query in [
            ModelPageQuery {
                category: Some("Type".into()),
                ..ModelPageQuery::default()
            },
            ModelPageQuery {
                designer: Some("Designer".into()),
                ..ModelPageQuery::default()
            },
            ModelPageQuery {
                model_license: Some("CC0".into()),
                ..ModelPageQuery::default()
            },
            ModelPageQuery {
                ever_printed: Some(true),
                ..ModelPageQuery::default()
            },
            ModelPageQuery {
                filament_id: Some(filament.id.clone()),
                ..ModelPageQuery::default()
            },
            ModelPageQuery {
                group: Some(ModelGroupFilter::Standalone),
                ..ModelPageQuery::default()
            },
        ] {
            let (models, more) = db.list_models_page(&query, None, None, 1).unwrap();
            assert!(!more);
            assert_eq!(models[0].id, a.id);
        }
        let (bundled, more) = db
            .list_models_page(
                &ModelPageQuery {
                    bundle_id: Some(bundle_id),
                    sort: ModelSort::Name,
                    direction: SortDirection::Desc,
                    ..ModelPageQuery::default()
                },
                None,
                None,
                1,
            )
            .unwrap();
        assert!(more);
        assert_eq!(bundled[0].display_name, "Gamma");
        let stats = db.catalog_stats().unwrap();
        assert_eq!(stats.total_models, 3);
        assert_eq!(stats.available_models, 3);
        assert_eq!(stats.bundle_count, 1);
        let facets = db.catalog_facets().unwrap();
        assert_eq!(facets.designers, ["Designer"]);
        assert_eq!(facets.licenses, ["CC0"]);
        assert_eq!(facets.categories, ["Type"]);
        assert_eq!(facets.parents.len(), 3);
        assert!(facets.folder_ids.iter().any(|id| id == ROOT_FOLDER_ID));
    }

    #[test]
    fn reviewed_ai_tags_apply_atomically_with_category_aware_merge() {
        use crate::models::ReviewedTag;

        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("AI review", None).unwrap();
        let color = db.create_tag("Red", Some("Color")).unwrap();
        let style = db.create_tag("Cute", Some("Style")).unwrap();
        let model = db
            .update_model_metadata(
                &model.id,
                model.metadata_version,
                &MetadataPatch {
                    tags: Some(vec![color.id, style.id]),
                    tag_mode: Some(TagMode::Replace),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        let merged = db
            .apply_reviewed_tags(
                &[ReviewedTagTarget {
                    model_id: model.id.clone(),
                    expected_version: model.metadata_version,
                    tags: vec![
                        ReviewedTag {
                            name: "Blue".into(),
                            category: Some("Color".into()),
                        },
                        ReviewedTag {
                            name: "Desk toy".into(),
                            category: None,
                        },
                    ],
                }],
                ReviewedTagStrategy::Merge,
            )
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(merged.tags, ["Blue", "Cute", "Desk toy"]);
        assert!(!merged.tags.contains(&"Red".into()));

        let other = db.create_model("Stale", None).unwrap();
        let changed = db
            .update_model_metadata(
                &other.id,
                other.metadata_version,
                &MetadataPatch {
                    notes: Some("changed".into()),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        assert!(matches!(
            db.apply_reviewed_tags(
                &[
                    ReviewedTagTarget {
                        model_id: merged.id.clone(),
                        expected_version: merged.metadata_version,
                        tags: vec![ReviewedTag {
                            name: "Must roll back".into(),
                            category: None,
                        }],
                    },
                    ReviewedTagTarget {
                        model_id: other.id.clone(),
                        expected_version: other.metadata_version,
                        tags: vec![],
                    },
                ],
                ReviewedTagStrategy::Append,
            ),
            Err(LibraryError::MetadataConflict { .. })
        ));
        assert!(!db
            .list_tags()
            .unwrap()
            .iter()
            .any(|tag| tag.name == "Must roll back"));
        assert_eq!(db.get_model(&merged.id).unwrap().unwrap().tags, merged.tags);
        assert!(changed.metadata_version > other.metadata_version);

        assert!(db
            .apply_reviewed_tags(
                &[ReviewedTagTarget {
                    model_id: merged.id.clone(),
                    expected_version: merged.metadata_version,
                    tags: vec![
                        ReviewedTag {
                            name: "Category rollback".into(),
                            category: None,
                        },
                        ReviewedTag {
                            name: "Cute".into(),
                            category: Some("Color".into()),
                        },
                    ],
                }],
                ReviewedTagStrategy::Append,
            )
            .is_err());
        assert!(!db
            .list_tags()
            .unwrap()
            .iter()
            .any(|tag| tag.name == "Category rollback"));
        assert_eq!(db.get_model(&merged.id).unwrap().unwrap().tags, merged.tags);
        let cleared = db
            .apply_reviewed_tags(
                &[ReviewedTagTarget {
                    model_id: merged.id,
                    expected_version: merged.metadata_version,
                    tags: vec![],
                }],
                ReviewedTagStrategy::Replace,
            )
            .unwrap();
        assert!(cleared[0].tags.is_empty());
    }

    #[test]
    fn print_timestamps_use_strict_rfc3339_and_fixed_utc_milliseconds() {
        assert_eq!(
            normalize_rfc3339("2024-02-29T23:59:59.123987+02:30").unwrap(),
            "2024-02-29T21:29:59.123Z"
        );
        for invalid in [
            "2023-02-29T00:00:00Z",
            "2026-01-01T-1:00:00Z",
            "2026-01-01T00:-1:00Z",
            "💥2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00+25:00",
        ] {
            assert!(normalize_rfc3339(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn archive_members_are_distinct_and_auto_grouped_in_a_stable_bundle() {
        let library = temp_root("archive-library");
        let sources = temp_root("archive-source");
        fs::create_dir_all(&sources).unwrap();
        fs::write(sources.join("parts.zip"), b"archive").unwrap();
        let db = LibraryDb::open(&library).unwrap();
        let mut root = db.add_index_root(&sources, 60, 1024).unwrap();
        root.scan_zip = true;
        db.update_index_root(&root).unwrap();
        let entries = [
            ScannedModel {
                relative_path: "parts.zip".into(),
                content_hash: hash(1),
                bytes: 10,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("A".into()),
                archive_member: Some("a.stl".into()),
                outer_fingerprint: Some("fp1".into()),
            },
            ScannedModel {
                relative_path: "parts.zip".into(),
                content_hash: hash(1),
                bytes: 10,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("B".into()),
                archive_member: Some("b.stl".into()),
                outer_fingerprint: Some("fp1".into()),
            },
        ];
        let added = db.record_scan_entries(&root.id, &entries).unwrap();
        assert_eq!(added.added, 2);
        let seen = vec![
            SourceRef {
                relative_path: "parts.zip".into(),
                archive_member: Some("a.stl".into()),
            },
            SourceRef {
                relative_path: "parts.zip".into(),
                archive_member: Some("b.stl".into()),
            },
        ];
        db.complete_scan_sources_with_new_models(&root.id, &seen, &added.added_model_ids, "1")
            .unwrap();
        let models = db.list_models(None, None).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].bundle_id, models[1].bundle_id);
        assert_ne!(models[0].id, models[1].id);
        let resolved = db.resolve_model_source_ref(&models[0].id).unwrap();
        assert_eq!(
            resolved.outer_path,
            sources.join("parts.zip").canonicalize().unwrap()
        );
        assert_eq!(resolved.outer_fingerprint.as_deref(), Some("fp1"));
        assert!(resolved.archive_member.is_some());
        let again = db.record_scan_entries(&root.id, &entries).unwrap();
        assert_eq!(again.added, 0);
        assert_eq!(
            db.list_models(None, None).unwrap()[0].bundle_id,
            models[0].bundle_id
        );
        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn archive_outer_rename_updates_automatic_bundle_path_without_changing_ids() {
        let library = temp_root("archive-rename-library");
        let sources = temp_root("archive-rename-source");
        fs::create_dir_all(&sources).unwrap();
        fs::write(sources.join("pack.zip"), b"first").unwrap();
        let db = LibraryDb::open(&library).unwrap();
        let root = db.add_index_root(&sources, 60, 1024).unwrap();
        let first = ScannedModel {
            relative_path: "pack.zip".into(),
            content_hash: hash(1),
            bytes: 10,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: Some("Part".into()),
            archive_member: Some("part.stl".into()),
            outer_fingerprint: Some("fp1".into()),
        };
        let added = db
            .record_scan_entries(&root.id, std::slice::from_ref(&first))
            .unwrap();
        db.complete_scan_sources_with_new_models(
            &root.id,
            &[SourceRef {
                relative_path: "pack.zip".into(),
                archive_member: Some("part.stl".into()),
            }],
            &added.added_model_ids,
            "1",
        )
        .unwrap();
        let original = db.list_models(None, None).unwrap().pop().unwrap();
        let bundle = original.bundle_id.clone().unwrap();
        fs::rename(sources.join("pack.zip"), sources.join("renamed.zip")).unwrap();
        let renamed = ScannedModel {
            relative_path: "renamed.zip".into(),
            outer_fingerprint: Some("fp2".into()),
            modified_at: 2,
            ..first
        };
        let batch = db
            .record_scan_entries(&root.id, std::slice::from_ref(&renamed))
            .unwrap();
        db.complete_scan_sources_with_new_models(
            &root.id,
            &[SourceRef {
                relative_path: "renamed.zip".into(),
                archive_member: Some("part.stl".into()),
            }],
            &batch.added_model_ids,
            "2",
        )
        .unwrap();
        let current = db.get_model(&original.id).unwrap().unwrap();
        assert_eq!(current.id, original.id);
        assert_eq!(current.bundle_id.as_deref(), Some(bundle.as_str()));
        let bundle_path: String = db
            .conn
            .query_row(
                "SELECT source_path FROM bundles WHERE id=?1",
                params![bundle],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bundle_path, "renamed.zip");
        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn scoped_scan_matches_and_updates_absolute_legacy_source_paths() {
        let library = temp_root("absolute-legacy-scan-library");
        let sources = temp_root("absolute-legacy-scan-source");
        fs::create_dir_all(sources.join("parts")).unwrap();
        fs::write(sources.join("parts/loose.stl"), b"old loose").unwrap();
        fs::write(sources.join("parts/pack.zip"), b"old archive").unwrap();
        let db = LibraryDb::open(&library).unwrap();
        let root = db.add_index_root(&sources, 60, 1024).unwrap();
        let initial = [
            ScannedModel {
                relative_path: "parts/loose.stl".into(),
                content_hash: hash(1),
                bytes: 9,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("Loose".into()),
                archive_member: None,
                outer_fingerprint: None,
            },
            ScannedModel {
                relative_path: "parts/pack.zip".into(),
                content_hash: hash(2),
                bytes: 11,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("Archive member".into()),
                archive_member: Some("member.stl".into()),
                outer_fingerprint: Some("archive-v1".into()),
            },
        ];
        db.record_scan_entries(&root.id, &initial).unwrap();
        let models = db.list_models(None, None).unwrap();
        let loose_id = models
            .iter()
            .find(|model| model.source_archive_member.is_none())
            .unwrap()
            .id
            .clone();
        let archive_id = models
            .iter()
            .find(|model| model.source_archive_member.as_deref() == Some("member.stl"))
            .unwrap()
            .id
            .clone();
        let absolute_loose = root.path.join("parts/loose.stl");
        let absolute_archive = root.path.join("parts/pack.zip");
        db.conn
            .execute(
                "UPDATE models SET source_path=?1 WHERE id=?2",
                params![absolute_loose.to_string_lossy(), loose_id],
            )
            .unwrap();
        db.conn
            .execute(
                "UPDATE models SET source_path=?1 WHERE id=?2",
                params![absolute_archive.to_string_lossy(), archive_id],
            )
            .unwrap();

        assert_eq!(
            db.resolve_model_source_ref(&loose_id).unwrap().outer_path,
            absolute_loose.canonicalize().unwrap()
        );
        let archive_source = db.resolve_model_source_ref(&archive_id).unwrap();
        assert_eq!(
            archive_source.outer_path,
            absolute_archive.canonicalize().unwrap()
        );
        assert_eq!(archive_source.archive_member.as_deref(), Some("member.stl"));
        assert_eq!(
            db.indexed_source_metadata(&root.id, Path::new("parts/loose.stl"), None)
                .unwrap()
                .unwrap()
                .content_hash,
            hash(1)
        );
        assert_eq!(
            db.indexed_archive_metadata(&root.id, Path::new("parts/pack.zip"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            db.indexed_archive_source_refs(&root.id, Some(Path::new("parts")))
                .unwrap(),
            vec![SourceRef {
                relative_path: "parts/pack.zip".into(),
                archive_member: Some("member.stl".into()),
            }]
        );

        let seen = [
            SourceRef {
                relative_path: "parts/loose.stl".into(),
                archive_member: None,
            },
            SourceRef {
                relative_path: "parts/pack.zip".into(),
                archive_member: Some("member.stl".into()),
            },
        ];
        db.complete_subdirectory_scan_sources_with_new_models(
            &root.id,
            Path::new("parts"),
            &seen,
            &[],
        )
        .unwrap();
        for id in [&loose_id, &archive_id] {
            assert!(!db.get_model(id).unwrap().unwrap().source_missing);
        }
        assert_eq!(
            db.get_model(&loose_id)
                .unwrap()
                .unwrap()
                .source_path
                .as_deref(),
            absolute_loose.to_str()
        );

        fs::write(sources.join("parts/loose.stl"), b"new loose bytes").unwrap();
        fs::write(sources.join("parts/pack.zip"), b"new archive bytes").unwrap();
        let changed = [
            ScannedModel {
                content_hash: hash(3),
                bytes: 15,
                modified_at: 2,
                ..initial[0].clone()
            },
            ScannedModel {
                content_hash: hash(4),
                bytes: 16,
                modified_at: 2,
                outer_fingerprint: Some("archive-v2".into()),
                ..initial[1].clone()
            },
        ];
        let updated = db.record_scan_entries(&root.id, &changed).unwrap();
        assert_eq!(updated.added, 0);
        assert_eq!(updated.updated, 2);
        assert!(updated.added_model_ids.is_empty());
        let stored_paths = db
            .conn
            .prepare("SELECT source_path FROM models ORDER BY source_archive_member IS NOT NULL")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(stored_paths, ["parts/loose.stl", "parts/pack.zip"]);
        let current = db.list_models(None, None).unwrap();
        assert_eq!(current.len(), 2);
        assert!(current.iter().any(|model| {
            model.id == loose_id
                && model.source_path.as_deref() == absolute_loose.to_str()
                && model.content_hash.as_deref() == Some(hash(3).as_str())
                && !model.source_missing
        }));
        assert!(current.iter().any(|model| {
            model.id == archive_id
                && model.source_path.as_deref() == absolute_archive.to_str()
                && model.content_hash.as_deref() == Some(hash(4).as_str())
                && !model.source_missing
        }));

        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn moving_one_archive_member_splits_automatic_bundle_without_rewriting_remaining_source() {
        let library = temp_root("archive-split-library");
        let sources = temp_root("archive-split-source");
        fs::create_dir_all(&sources).unwrap();
        fs::write(sources.join("old.zip"), b"old").unwrap();
        fs::write(sources.join("new.zip"), b"new").unwrap();
        let db = LibraryDb::open(&library).unwrap();
        let root = db.add_index_root(&sources, 60, 1024).unwrap();
        let old = [
            ScannedModel {
                relative_path: "old.zip".into(),
                content_hash: hash(1),
                bytes: 84,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("A".into()),
                archive_member: Some("a.stl".into()),
                outer_fingerprint: Some("oldfp".into()),
            },
            ScannedModel {
                relative_path: "old.zip".into(),
                content_hash: hash(2),
                bytes: 84,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("B".into()),
                archive_member: Some("b.stl".into()),
                outer_fingerprint: Some("oldfp".into()),
            },
        ];
        let added = db.record_scan_entries(&root.id, &old).unwrap();
        let initial_seen = old
            .iter()
            .map(|entry| SourceRef {
                relative_path: entry.relative_path.clone(),
                archive_member: entry.archive_member.clone(),
            })
            .collect::<Vec<_>>();
        db.complete_scan_sources_with_new_models(
            &root.id,
            &initial_seen,
            &added.added_model_ids,
            "1",
        )
        .unwrap();
        let before = db.list_models(None, None).unwrap();
        let a = before
            .iter()
            .find(|m| m.source_archive_member.as_deref() == Some("a.stl"))
            .unwrap();
        let b = before
            .iter()
            .find(|m| m.source_archive_member.as_deref() == Some("b.stl"))
            .unwrap();
        let a_id = a.id.clone();
        let b_id = b.id.clone();
        let old_bundle = b.bundle_id.clone().unwrap();
        let moved = ScannedModel {
            relative_path: "new.zip".into(),
            content_hash: hash(1),
            bytes: 84,
            format: "stl-binary".into(),
            modified_at: 2,
            display_name: Some("A".into()),
            archive_member: Some("a.stl".into()),
            outer_fingerprint: Some("newfp".into()),
        };
        let batch = db
            .record_scan_entries(&root.id, std::slice::from_ref(&moved))
            .unwrap();
        let seen = [
            SourceRef {
                relative_path: "new.zip".into(),
                archive_member: Some("a.stl".into()),
            },
            SourceRef {
                relative_path: "old.zip".into(),
                archive_member: Some("b.stl".into()),
            },
        ];
        db.complete_scan_sources_with_new_models(&root.id, &seen, &batch.added_model_ids, "2")
            .unwrap();
        let a = db.get_model(&a_id).unwrap().unwrap();
        let b = db.get_model(&b_id).unwrap().unwrap();
        assert_ne!(a.bundle_id, b.bundle_id);
        assert_eq!(b.bundle_id.as_deref(), Some(old_bundle.as_str()));
        let old_path: String = db
            .conn
            .query_row(
                "SELECT source_path FROM bundles WHERE id=?1",
                params![old_bundle],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_path, "old.zip");
        let new_path: String = db
            .conn
            .query_row(
                "SELECT source_path FROM bundles WHERE id=?1",
                params![a.bundle_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(new_path, "new.zip");
        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn archive_member_to_loose_transition_does_not_reuse_archive_bundle() {
        let library = temp_root("archive-loose-library");
        let sources = temp_root("archive-loose-source");
        fs::create_dir_all(sources.join("folder")).unwrap();
        fs::write(sources.join("old.zip"), b"old").unwrap();
        fs::write(sources.join("folder/a.stl"), b"a").unwrap();
        let db = LibraryDb::open(&library).unwrap();
        let root = db.add_index_root(&sources, 60, 1024).unwrap();
        let old = [
            ScannedModel {
                relative_path: "old.zip".into(),
                content_hash: hash(1),
                bytes: 1,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: None,
                archive_member: Some("a.stl".into()),
                outer_fingerprint: Some("fp".into()),
            },
            ScannedModel {
                relative_path: "old.zip".into(),
                content_hash: hash(2),
                bytes: 1,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: None,
                archive_member: Some("b.stl".into()),
                outer_fingerprint: Some("fp".into()),
            },
        ];
        let added = db.record_scan_entries(&root.id, &old).unwrap();
        let seen = old
            .iter()
            .map(|e| SourceRef {
                relative_path: e.relative_path.clone(),
                archive_member: e.archive_member.clone(),
            })
            .collect::<Vec<_>>();
        db.complete_scan_sources_with_new_models(&root.id, &seen, &added.added_model_ids, "1")
            .unwrap();
        let models = db.list_models(None, None).unwrap();
        let a = models
            .iter()
            .find(|m| m.source_archive_member.as_deref() == Some("a.stl"))
            .unwrap();
        let b = models
            .iter()
            .find(|m| m.source_archive_member.as_deref() == Some("b.stl"))
            .unwrap();
        let a_id = a.id.clone();
        let b_id = b.id.clone();
        let archive_bundle = b.bundle_id.clone().unwrap();
        let loose = ScannedModel {
            relative_path: "folder/a.stl".into(),
            content_hash: hash(1),
            bytes: 1,
            format: "stl-binary".into(),
            modified_at: 2,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        let batch = db
            .record_scan_entries(&root.id, std::slice::from_ref(&loose))
            .unwrap();
        let seen = [
            SourceRef {
                relative_path: "folder/a.stl".into(),
                archive_member: None,
            },
            SourceRef {
                relative_path: "old.zip".into(),
                archive_member: Some("b.stl".into()),
            },
        ];
        db.complete_scan_sources_with_new_models(&root.id, &seen, &batch.added_model_ids, "2")
            .unwrap();
        let a = db.get_model(&a_id).unwrap().unwrap();
        let b = db.get_model(&b_id).unwrap().unwrap();
        assert!(a.source_archive_member.is_none());
        assert!(a.bundle_id.is_none());
        assert_eq!(b.bundle_id.as_deref(), Some(archive_bundle.as_str()));
        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn completed_scan_auto_groups_subfolders_with_stable_bundles_and_preserves_manual_assignments()
    {
        let library = temp_root("folder-bundle-library");
        let sources = temp_root("folder-bundle-source");
        fs::create_dir_all(sources.join("nested")).unwrap();
        fs::create_dir_all(sources.join("solo")).unwrap();
        for path in ["nested/a.stl", "nested/b.stl", "solo/c.stl", "root.stl"] {
            fs::write(sources.join(path), path.as_bytes()).unwrap();
        }
        let db = LibraryDb::open(&library).unwrap();
        let root = db.add_index_root(&sources, 60, 1024).unwrap();
        let make = |path: &str, n: u8| ScannedModel {
            relative_path: path.into(),
            content_hash: hash(n),
            bytes: path.len() as u64,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        let initial = [
            make("nested/a.stl", 1),
            make("nested/b.stl", 2),
            make("solo/c.stl", 3),
            make("root.stl", 4),
        ];
        let added = db.record_scan_entries(&root.id, &initial).unwrap();
        let seen = initial
            .iter()
            .map(|entry| SourceRef {
                relative_path: entry.relative_path.clone(),
                archive_member: None,
            })
            .collect::<Vec<_>>();
        db.complete_scan_sources_with_new_models(&root.id, &seen, &added.added_model_ids, "1")
            .unwrap();
        let models = db.list_models(None, None).unwrap();
        let a = models
            .iter()
            .find(|m| m.source_path.as_deref().unwrap().ends_with("nested/a.stl"))
            .unwrap();
        let b = models
            .iter()
            .find(|m| m.source_path.as_deref().unwrap().ends_with("nested/b.stl"))
            .unwrap();
        let stable_bundle = a.bundle_id.clone().unwrap();
        let a_id = a.id.clone();
        assert_eq!(b.bundle_id.as_deref(), Some(stable_bundle.as_str()));
        assert!(models
            .iter()
            .find(|m| m.source_path.as_deref().unwrap().ends_with("solo/c.stl"))
            .unwrap()
            .bundle_id
            .is_none());
        assert!(models
            .iter()
            .find(|m| m.source_path.as_deref().unwrap().ends_with("root.stl"))
            .unwrap()
            .bundle_id
            .is_none());
        fs::rename(sources.join("nested"), sources.join("renamed")).unwrap();
        let renamed = [
            make("renamed/a.stl", 1),
            make("renamed/b.stl", 2),
            make("solo/c.stl", 3),
            make("root.stl", 4),
        ];
        let batch = db.record_scan_entries(&root.id, &renamed).unwrap();
        let seen = renamed
            .iter()
            .map(|entry| SourceRef {
                relative_path: entry.relative_path.clone(),
                archive_member: None,
            })
            .collect::<Vec<_>>();
        db.complete_scan_sources_with_new_models(&root.id, &seen, &batch.added_model_ids, "2")
            .unwrap();
        let models = db.list_models(None, None).unwrap();
        let renamed_a = models
            .iter()
            .find(|m| m.source_path.as_deref().unwrap().ends_with("renamed/a.stl"))
            .unwrap();
        assert_eq!(renamed_a.id, a_id);
        assert_eq!(renamed_a.bundle_id.as_deref(), Some(stable_bundle.as_str()));
        assert!(db
            .set_model_bundle(&renamed_a.id, Some(&stable_bundle))
            .is_err());
        let manual = db
            .create_bundle(std::slice::from_ref(&renamed_a.id))
            .unwrap();
        db.complete_scan_sources_with_new_models(&root.id, &seen, &[], "3")
            .unwrap();
        let models = db.list_models(None, None).unwrap();
        assert_eq!(
            models
                .iter()
                .find(|m| m.id == renamed_a.id)
                .unwrap()
                .bundle_id
                .as_deref(),
            Some(manual.as_str())
        );
        assert!(models
            .iter()
            .find(|m| m.source_path.as_deref().unwrap().ends_with("renamed/b.stl"))
            .unwrap()
            .bundle_id
            .is_none());
        drop(db);
        let reopened = LibraryDb::open(&library).unwrap();
        assert_eq!(
            reopened
                .get_model(&a_id)
                .unwrap()
                .unwrap()
                .bundle_id
                .as_deref(),
            Some(manual.as_str())
        );
        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn tag_and_filament_inputs_are_bounded_at_the_storage_boundary() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Bounded", None).unwrap();
        for name in [
            "x".repeat(257),
            "line\\nfeed".replace("\\n", "\n"),
            "null\0".to_owned(),
        ] {
            assert!(db.create_tag(&name, None).is_err());
        }
        let tag = db.create_tag("Valid", None).unwrap();
        assert!(db.rename_tag(&tag.id, &"x".repeat(257), None).is_err());
        assert_eq!(db.list_tags().unwrap()[0].name, "Valid");
        for ids in [
            (0..6).map(|n| format!("filament-{n}")).collect::<Vec<_>>(),
            vec!["x".repeat(257)],
            vec!["".into()],
            vec!["bad\nref".into()],
            vec![" padded ".into()],
            vec!["duplicate".into(), "duplicate".into()],
        ] {
            let input = PrintAttemptInput {
                printed_at: "2026-10-10T00:00:00.000Z".into(),
                quantity: 1,
                notes: String::new(),
                filament_ids: ids,
                outcome: PrintOutcome::Printed,
                parts: None,
            };
            assert!(db.create_print_attempt(&model.id, &input).is_err());
            assert!(db.list_print_attempts(&model.id).unwrap().is_empty());
            assert_eq!(
                db.get_model(&model.id)
                    .unwrap()
                    .unwrap()
                    .metadata
                    .print_count,
                0
            );
        }
    }

    #[test]
    fn print_history_persists_across_restart() {
        let root = temp_root("history-restart");
        let db = LibraryDb::open(&root).unwrap();
        let model = db.create_model("History", None).unwrap();
        let attempt = db
            .create_print_attempt(
                &model.id,
                &PrintAttemptInput {
                    printed_at: "2026-10-10T12:34:56.789Z".into(),
                    quantity: 3,
                    notes: "persist".into(),
                    filament_ids: vec!["f1".into(), "f2".into()],
                    outcome: PrintOutcome::Printed,
                    parts: None,
                },
            )
            .unwrap();
        drop(db);
        let reopened = LibraryDb::open(&root).unwrap();
        let loaded = reopened.get_print_attempt(&attempt.id).unwrap().unwrap();
        assert_eq!(loaded.quantity, 3);
        assert_eq!(loaded.filament_ids, vec!["f1", "f2"]);
        assert_eq!(
            reopened
                .get_model(&model.id)
                .unwrap()
                .unwrap()
                .metadata
                .print_count,
            3
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn print_attempt_batches_are_atomic_and_idempotent_without_storing_payloads() {
        let db = LibraryDb::open_in_memory().unwrap();
        let first = db.create_model("First", None).unwrap();
        let second = db.create_model("Second", None).unwrap();
        let part = db
            .create_part_stock(&PartStockInput {
                name: "M3 screw".into(),
                category: Some("Screws".into()),
                quantity: 20,
                unit: "pcs".into(),
                notes: String::new(),
                low_stock: 0,
            })
            .unwrap();
        let input = PrintAttemptInput {
            printed_at: "2026-10-11T12:00:00Z".into(),
            quantity: 2,
            notes: "private batch note".into(),
            filament_ids: vec![],
            outcome: PrintOutcome::Printed,
            parts: Some(vec![PartUsageInput {
                id: part.id.clone(),
                quantity: 2,
                snapshot: None,
            }]),
        };
        let invalid = vec![
            PrintAttemptBatchItem {
                model_id: first.id.clone(),
                input: input.clone(),
            },
            PrintAttemptBatchItem {
                model_id: "missing".into(),
                input: input.clone(),
            },
        ];
        assert!(db
            .create_print_attempts_idempotent("batch-invalid", &invalid)
            .is_err());
        assert!(db.list_print_attempts(&first.id).unwrap().is_empty());

        let items = vec![
            PrintAttemptBatchItem {
                model_id: first.id.clone(),
                input: input.clone(),
            },
            PrintAttemptBatchItem {
                model_id: second.id.clone(),
                input,
            },
        ];
        let created = db
            .create_print_attempts_idempotent("batch-1", &items)
            .unwrap();
        let replayed = db
            .create_print_attempts_idempotent("batch-1", &items)
            .unwrap();
        assert_eq!(created, replayed);
        assert_eq!(db.get_part_stock(&part.id).unwrap().unwrap().quantity, 12);
        assert_eq!(db.list_print_attempts(&first.id).unwrap().len(), 1);
        assert_eq!(db.list_print_attempts(&second.id).unwrap().len(), 1);
        let mut changed = items;
        changed[0].input.parts.as_mut().unwrap()[0].quantity = 3;
        assert!(db
            .create_print_attempts_idempotent("batch-1", &changed)
            .is_err());
        assert_eq!(db.get_part_stock(&part.id).unwrap().unwrap().quantity, 12);
        let receipt: String = db
            .conn
            .query_row(
                "SELECT attempt_ids_json FROM print_batch_receipts WHERE idempotency_key='batch-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!receipt.contains("private batch note"));
    }

    #[test]
    fn concurrent_idempotent_print_batches_charge_parts_only_once() {
        let root = temp_root("concurrent-idempotent-parts");
        let db = LibraryDb::open(&root).unwrap();
        let model = db.create_model("Concurrent inventory", None).unwrap();
        let part = db
            .create_part_stock(&PartStockInput {
                name: "Bearing".into(),
                category: Some("Bearings".into()),
                quantity: 20,
                unit: "pcs".into(),
                notes: String::new(),
                low_stock: 0,
            })
            .unwrap();
        drop(db);

        let mut input = print_attempt_input();
        input.quantity = 2;
        input.parts = Some(vec![PartUsageInput {
            id: part.id.clone(),
            quantity: 2,
            snapshot: None,
        }]);
        let items = vec![PrintAttemptBatchItem {
            model_id: model.id.clone(),
            input,
        }];
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let handles = (0..2)
            .map(|_| {
                let root = root.clone();
                let barrier = Arc::clone(&barrier);
                let items = items.clone();
                std::thread::spawn(move || {
                    let db = LibraryDb::open(&root).unwrap();
                    barrier.wait();
                    db.create_print_attempts_idempotent("same-batch", &items)
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let left = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(left[0], left[1]);

        let db = LibraryDb::open(&root).unwrap();
        assert_eq!(db.get_part_stock(&part.id).unwrap().unwrap().quantity, 16);
        assert_eq!(db.list_print_attempts(&model.id).unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_print_history_imports_preserve_batch_multiplicity_without_duplicates() {
        let root = temp_root("concurrent-print-history-import");
        let db = LibraryDb::open(&root).unwrap();
        let model = db.create_model("Concurrent history", None).unwrap();
        let part = db
            .create_part_stock(&PartStockInput {
                name: "M3 screw".into(),
                category: Some("Screws".into()),
                quantity: 20,
                unit: "pcs".into(),
                notes: String::new(),
                low_stock: 0,
            })
            .unwrap();
        drop(db);

        let item = PrintAttemptImportItem {
            input: PrintAttemptInput {
                printed_at: "2026-10-11T12:00:00Z".into(),
                quantity: 2,
                notes: "same event twice".into(),
                filament_ids: vec!["filament-a".into()],
                outcome: PrintOutcome::Printed,
                parts: Some(vec![PartUsageInput {
                    id: part.id.clone(),
                    quantity: 2,
                    snapshot: None,
                }]),
            },
            created_at: Some("2026-10-11T12:05:00Z".into()),
            printer_id: None,
            printer_name: Some("Workshop printer".into()),
        };
        let attempts = vec![item.clone(), item];
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));

        let left_path = root.clone();
        let left_model = model.id.clone();
        let left_attempts = attempts.clone();
        let left_barrier = std::sync::Arc::clone(&barrier);
        let left = std::thread::spawn(move || {
            let db = LibraryDb::open(&left_path).unwrap();
            left_barrier.wait();
            db.import_print_attempts_deduplicated(&left_model, &left_attempts)
                .unwrap()
        });

        let right_path = root.clone();
        let right_model = model.id.clone();
        let right_barrier = std::sync::Arc::clone(&barrier);
        let right = std::thread::spawn(move || {
            let db = LibraryDb::open(&right_path).unwrap();
            right_barrier.wait();
            db.import_print_attempts_deduplicated(&right_model, &attempts)
                .unwrap()
        });

        barrier.wait();
        let left_result = left.join().unwrap();
        let right_result = right.join().unwrap();
        assert_eq!(left_result.added_count + right_result.added_count, 2);
        assert_eq!(left_result.skipped_count + right_result.skipped_count, 2);

        let db = LibraryDb::open(&root).unwrap();
        let stored = db.list_print_attempts(&model.id).unwrap();
        assert_eq!(stored.len(), 2);
        // Historical JSON already carries the current on-hand quantity, so
        // importing event links must not deduct their past usage again.
        assert_eq!(db.get_part_stock(&part.id).unwrap().unwrap().quantity, 20);
        assert!(stored.iter().all(|attempt| {
            attempt.notes == "same event twice"
                && attempt.printer_name.as_deref() == Some("Workshop printer")
        }));
        assert_eq!(
            db.get_model(&model.id)
                .unwrap()
                .unwrap()
                .metadata
                .print_count,
            4
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn print_history_import_rolls_back_when_a_later_row_is_invalid() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Atomic history", None).unwrap();
        let input = PrintAttemptInput {
            printed_at: "2026-10-11T12:00:00Z".into(),
            quantity: 1,
            notes: "must roll back".into(),
            filament_ids: vec![],
            outcome: PrintOutcome::Printed,
            parts: None,
        };
        let attempts = vec![
            PrintAttemptImportItem {
                input: input.clone(),
                created_at: None,
                printer_id: None,
                printer_name: None,
            },
            PrintAttemptImportItem {
                input,
                created_at: None,
                printer_id: Some("missing-printer".into()),
                printer_name: Some("Missing printer".into()),
            },
        ];

        assert!(db
            .import_print_attempts_deduplicated(&model.id, &attempts)
            .is_err());
        assert!(db.list_print_attempts(&model.id).unwrap().is_empty());
        let unchanged = db.get_model(&model.id).unwrap().unwrap();
        assert_eq!(unchanged.metadata.print_count, 0);
        assert_eq!(unchanged.metadata.print_status, model.metadata.print_status);
        assert_eq!(unchanged.metadata_version, model.metadata_version);
    }

    #[test]
    fn management_duplicates_removal_restore_and_custom_previews_are_scoped_and_versioned() {
        let root = temp_root("management-core");
        let sources = temp_root("management-core-source");
        fs::create_dir_all(&sources).unwrap();
        let source = sources.join("part.stl");
        fs::write(&source, stl_fixture()).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let first = db.import_file(&source, None, Some("First")).unwrap().model;
        let second = db.import_file(&source, None, Some("Second")).unwrap().model;
        assert!(db
            .duplicate_groups(std::slice::from_ref(&first.id))
            .unwrap()
            .is_empty());
        let groups = db
            .duplicate_groups(&[first.id.clone(), second.id.clone()])
            .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].models.len(), 2);
        let managed = db.resolve_model_source(&first.id).unwrap();
        let before = fs::read(&managed).unwrap();
        let removal = db
            .remove_models_recoverably(&[VersionedTarget {
                model_id: first.id.clone(),
                expected_version: first.metadata_version,
            }])
            .unwrap();
        assert_eq!(db.list_models(None, None).unwrap().len(), 1);
        assert_eq!(fs::read(&managed).unwrap(), before);
        assert_eq!(db.list_removed_models().unwrap().len(), 1);
        let restored = db.restore_removal(&removal.snapshot_id).unwrap();
        assert_eq!(restored[0].id, first.id);
        assert_eq!(db.list_models(None, None).unwrap().len(), 2);
        let png = png_fixture();
        let hash = hex_digest(&png);
        let relative = Path::new("thumbnails/custom-management.png");
        fs::write(db.root().join(relative), png).unwrap();
        let current = db.get_model(&first.id).unwrap().unwrap();
        let custom = db
            .set_custom_thumbnail(&first.id, current.metadata_version, &hash, relative)
            .unwrap();
        assert!(
            db.thumbnail_cache_entry_for_model(&first.id)
                .unwrap()
                .unwrap()
                .is_custom
        );
        assert!(matches!(
            db.clear_custom_thumbnail(&first.id, current.metadata_version),
            Err(LibraryError::MetadataConflict { .. })
        ));
        let cleared = db
            .clear_custom_thumbnail(&first.id, custom.metadata_version)
            .unwrap();
        assert!(!db
            .thumbnail_cache_entry_for_model(&first.id)
            .unwrap()
            .is_some_and(|entry| entry.is_custom));
        assert!(cleared.metadata_version > custom.metadata_version);
        let floor = db.max_metadata_version().unwrap();
        db.bump_all_metadata_versions_above(floor).unwrap();
        assert!(db.get_model(&first.id).unwrap().unwrap().metadata_version > floor);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn duplicate_scope_limit_is_checked_before_model_lookups() {
        let root = temp_root("duplicate-scope-limit");
        let db = LibraryDb::open(&root).unwrap();
        let ids = (0..1_001)
            .map(|index| format!("model-{index}"))
            .collect::<Vec<_>>();

        assert!(matches!(
            db.duplicate_groups(&ids),
            Err(LibraryError::InvalidInput(_))
        ));
    }

    #[test]
    fn indexed_tombstones_survive_rescan_and_zip_member_removal_never_changes_archive() {
        let library = temp_root("management-indexed");
        let sources = temp_root("management-indexed-source");
        fs::create_dir_all(&sources).unwrap();
        let archive = sources.join("pack.zip");
        let archive_bytes = b"outer archive unchanged";
        fs::write(&archive, archive_bytes).unwrap();
        let db = LibraryDb::open(&library).unwrap();
        let root = db.add_index_root(&sources, 60, 1024).unwrap();
        let entries = [
            ScannedModel {
                relative_path: "pack.zip".into(),
                content_hash: hash(1),
                bytes: 10,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("A".into()),
                archive_member: Some("a.stl".into()),
                outer_fingerprint: Some("fp".into()),
            },
            ScannedModel {
                relative_path: "pack.zip".into(),
                content_hash: hash(2),
                bytes: 10,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("B".into()),
                archive_member: Some("b.stl".into()),
                outer_fingerprint: Some("fp".into()),
            },
        ];
        let added = db.record_scan_entries(&root.id, &entries).unwrap();
        let seen = entries
            .iter()
            .map(|entry| SourceRef {
                relative_path: entry.relative_path.clone(),
                archive_member: entry.archive_member.clone(),
            })
            .collect::<Vec<_>>();
        db.complete_scan_sources_with_new_models(&root.id, &seen, &added.added_model_ids, "1")
            .unwrap();
        let models = db.list_models(None, None).unwrap();
        let removed = models
            .iter()
            .find(|model| model.source_archive_member.as_deref() == Some("a.stl"))
            .unwrap();
        let removal = db
            .remove_models_recoverably(&[VersionedTarget {
                model_id: removed.id.clone(),
                expected_version: removed.metadata_version,
            }])
            .unwrap();
        db.record_scan_entries(&root.id, &entries).unwrap();
        db.complete_scan_sources_with_new_models(&root.id, &seen, &[], "2")
            .unwrap();
        assert_eq!(db.list_models(None, None).unwrap().len(), 1);
        assert_eq!(fs::read(&archive).unwrap(), archive_bytes);
        let restored = db.restore_removal(&removal.snapshot_id).unwrap();
        assert_eq!(restored[0].id, removed.id);
        assert_eq!(db.list_models(None, None).unwrap().len(), 2);
        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn thumbnail_appearance_settings_invalidate_generated_rows_and_preserve_custom() {
        let root = temp_root("catalog-settings-thumbnails");
        let source_dir = temp_root("catalog-settings-thumbnails-source");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("part.stl");
        fs::write(&source, stl_fixture()).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        fs::write(root.join("thumbnails/generated.png"), b"generated").unwrap();
        fs::write(root.join("thumbnails/custom.png"), b"custom").unwrap();
        db.set_thumbnail(&model.id, &hash(9), Path::new("thumbnails/generated.png"))
            .unwrap();
        db.conn.execute("INSERT INTO thumbnails (id, model_id, relative_path, content_hash, is_custom) VALUES ('settings-custom', ?1, 'thumbnails/custom.png', 'custom-hash', 1)", params![model.id]).unwrap();

        let mut settings = db.get_catalog_settings().unwrap();
        settings.thumbnail_concurrency = 1;
        settings.scan_batch_size = 64;
        db.update_catalog_settings(&settings).unwrap();
        let generated_before_appearance_change: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE is_custom = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generated_before_appearance_change, 1);
        settings.thumbnail_size = 512;
        db.update_catalog_settings(&settings).unwrap();
        let generated: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE is_custom = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let custom: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE is_custom = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generated, 0);
        assert_eq!(custom, 1);
        assert!(db.thumbnail_for_model(&model.id).unwrap().is_some());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_dir);
    }

    #[test]
    fn thumbnail_commit_rejects_stale_appearance_settings_atomically() {
        let root = temp_root("thumbnail-settings-race");
        let source_dir = temp_root("thumbnail-settings-race-source");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("part.stl");
        fs::write(&source, stl_fixture()).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        let png = png_fixture();
        let png_hash = hex_digest(&png);
        let relative = Path::new("thumbnails/settings-race.png");
        fs::write(db.root().join(relative), png).unwrap();
        let stale = db.get_catalog_settings().unwrap();
        let mut current = stale.clone();
        current.thumbnail_background = "#223344".into();
        db.update_catalog_settings(&current).unwrap();

        assert!(matches!(
            db.set_thumbnail_for_source_and_settings(
                &model.id,
                model.content_hash.as_deref().unwrap(),
                &stale,
                &png_hash,
                relative,
            ),
            Err(LibraryError::InvalidInput(_))
        ));
        let generated: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE is_custom = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generated, 0);
        db.set_thumbnail_for_source_and_settings(
            &model.id,
            model.content_hash.as_deref().unwrap(),
            &current,
            &png_hash,
            relative,
        )
        .unwrap();
        let first_id: String = db
            .conn
            .query_row(
                "SELECT id FROM thumbnails WHERE model_id=?1 AND is_custom=0",
                [&model.id],
                |row| row.get(0),
            )
            .unwrap();
        db.set_thumbnail_for_source_and_settings(
            &model.id,
            model.content_hash.as_deref().unwrap(),
            &current,
            &png_hash,
            relative,
        )
        .unwrap();
        let repeated_id: String = db
            .conn
            .query_row(
                "SELECT id FROM thumbnails WHERE model_id=?1 AND is_custom=0",
                [&model.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(repeated_id, first_id);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_dir);
    }

    #[test]
    fn schema_v1_migration_creates_consistent_backup_and_preserves_managed_data() {
        let root = temp_root("existing-v1");
        fs::create_dir_all(root.join("originals")).unwrap();
        fs::create_dir_all(root.join("thumbnails")).unwrap();
        let source_bytes = stl_fixture();
        let content_hash = hex_digest(&source_bytes);
        let storage_path = format!("originals/{content_hash}.stl");
        fs::write(root.join(&storage_path), &source_bytes).unwrap();
        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute_batch(TEST_SCHEMA_V1).unwrap();
        conn.execute(
            "INSERT INTO blobs VALUES (?1, ?2, ?3)",
            params![content_hash, storage_path, source_bytes.len() as i64],
        )
        .unwrap();
        conn.execute("INSERT INTO models (id, display_name, description, folder_id, current_revision_id, metadata_version, created_at, updated_at) VALUES ('model-v1', 'Tagged Bracket', 'legacy notes', 'root', 'revision-v1', 1, '1', '1')", []).unwrap();
        conn.execute("INSERT INTO revisions VALUES ('revision-v1', 'model-v1', ?1, 'Tagged Bracket.stl', 'stl-binary', ?2, '1')", params![content_hash, source_bytes.len() as i64]).unwrap();
        conn.execute("INSERT INTO tags VALUES ('tag-existing', 'Favorite')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO model_tags VALUES ('model-v1', 'tag-existing')",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        drop(conn);

        let db = LibraryDb::open(&root).unwrap();
        let existing = db.get_model("model-v1").unwrap().unwrap();
        assert_eq!(existing.id, "model-v1");
        assert_eq!(existing.display_name, "Tagged Bracket");
        assert_eq!(existing.folder_id, ROOT_FOLDER_ID);
        assert_eq!(
            existing.original_name.as_deref(),
            Some("Tagged Bracket.stl")
        );
        assert_eq!(
            existing.content_hash.as_deref(),
            Some(content_hash.as_str())
        );
        assert_eq!(
            existing.storage_path.as_deref(),
            Some(storage_path.as_str())
        );
        assert_eq!(existing.tags, vec!["Favorite"]);
        assert_eq!(existing.source_kind, "managed");
        assert_eq!(existing.metadata.notes.as_deref(), Some("legacy notes"));
        assert_eq!(fs::read(root.join(storage_path)).unwrap(), source_bytes);
        drop(db);

        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        drop(conn);

        let backup = root.join("library.sqlite3.schema-v1.backup");
        assert!(backup.is_file());
        let backup_conn = Connection::open(backup).unwrap();
        let backup_version: i64 = backup_conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(backup_version, 1);
        let backup_name: String = backup_conn
            .query_row(
                "SELECT display_name FROM models WHERE id = 'model-v1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(backup_name, "Tagged Bracket");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn schema_v2_migration_creates_backup_and_preserves_legacy_notes() {
        let root = temp_root("existing-v2");
        fs::create_dir_all(&root).unwrap();
        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute_batch(TEST_SCHEMA_V1).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute("INSERT INTO models (id,display_name,description,folder_id,metadata_version,created_at,updated_at) VALUES ('legacy','Legacy','keep me','root',1,'1','1')",[]).unwrap();
        conn.execute_batch(MIGRATE_V1_TO_V2).unwrap();
        drop(conn);
        let db = LibraryDb::open(&root).unwrap();
        let model = db.get_model("legacy").unwrap().unwrap();
        assert_eq!(model.metadata.notes.as_deref(), Some("keep me"));
        drop(db);
        let backup = root.join("library.sqlite3.schema-v2.backup");
        assert!(backup.is_file());
        let backup_conn = Connection::open(backup).unwrap();
        let version: i64 = backup_conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn newer_schema_is_rejected_without_migration() {
        let root = temp_root("schema");
        let db = LibraryDb::open(&root).unwrap();
        drop(db);
        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION + 1)
            .unwrap();
        drop(conn);
        let result = LibraryDb::open(&root);
        assert!(matches!(result, Err(LibraryError::SchemaTooNew { .. })));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn import_copies_validated_bytes_and_never_overwrites_same_name() {
        let root = temp_root("import");
        let source_dir = temp_root("source");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("Bracket.stl");
        let bytes = stl_fixture();
        fs::write(&source, &bytes).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let first = db.import_file(&source, None, None).unwrap();
        let second = db.import_file(&source, None, None).unwrap();
        assert_ne!(first.model.id, second.model.id);
        assert_eq!(first.model.display_name, "Bracket");
        assert_eq!(first.model.original_name.as_deref(), Some("Bracket.stl"));
        assert_eq!(first.model.content_hash, second.model.content_hash);
        assert_eq!(first.model.bytes, bytes.len() as u64);
        assert!(first
            .model
            .storage_path
            .as_ref()
            .unwrap()
            .contains("originals/"));
        assert_eq!(fs::read(&source).unwrap(), bytes);
        assert!(!first.reused_blob);
        assert!(second.reused_blob);
        let staged_entries: Vec<_> = fs::read_dir(root.join("staging"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(staged_entries.len(), 1);
        assert_eq!(fs::read(&staged_entries[0]).unwrap(), bytes);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_dir);
    }

    #[test]
    fn import_staging_error_reports_and_preserves_same_user_replacement() {
        let temp = temp_root("import-staging-replacement");
        fs::create_dir_all(&temp).unwrap();
        let staging = temp.join("replacement.part");
        let marker = temp.join(".import-staging-race-test");
        fs::write(&marker, b"test-owned temporary directory").unwrap();
        fs::write(&staging, b"staged model bytes").unwrap();

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "db::tests::import_staging_replacement_child_process",
                "--nocapture",
            ])
            .env("GOGGLELAB_CORE_STAGING_REPLACEMENT_PATH", &staging)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "replacement child failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        retain_import_staging(&staging).unwrap();

        assert_eq!(fs::read(&staging).unwrap(), b"replacement bytes");
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn import_staging_replacement_child_process() {
        let Ok(staging) = std::env::var("GOGGLELAB_CORE_STAGING_REPLACEMENT_PATH") else {
            return;
        };
        let staging = PathBuf::from(staging);
        let parent = staging.parent().unwrap();
        assert!(parent
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("gogglelab-library-import-staging-replacement-"));
        assert_eq!(
            fs::read(parent.join(".import-staging-race-test")).unwrap(),
            b"test-owned temporary directory"
        );
        fs::remove_file(&staging).unwrap();
        fs::write(&staging, b"replacement bytes").unwrap();
    }

    #[test]
    fn import_rejects_oversize_before_read_and_never_overwrites_or_follows_owned_namespace_links() {
        let root = temp_root("import-hardening");
        let sources = temp_root("import-hardening-source");
        fs::create_dir_all(&sources).unwrap();
        let oversized = sources.join("oversized.stl");
        let file = fs::File::create(&oversized).unwrap();
        file.set_len(MAX_INDEXED_FILE_BYTES + 1).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        assert!(db.import_file(&oversized, None, None).is_err());
        assert!(db.list_models(None, None).unwrap().is_empty());
        let source = sources.join("part.stl");
        let bytes = stl_fixture();
        fs::write(&source, &bytes).unwrap();
        let hash = hex_digest(&bytes);
        let destination = db.root().join("originals").join(format!("{hash}.stl"));
        let sentinel = vec![b'x'; bytes.len()];
        fs::write(&destination, &sentinel).unwrap();
        assert!(db.import_file(&source, None, None).is_err());
        assert_eq!(fs::read(&destination).unwrap(), sentinel);
        assert!(db.list_models(None, None).unwrap().is_empty());
        drop(db);
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = temp_root("import-hardening-outside");
            fs::create_dir_all(&outside).unwrap();
            fs::remove_file(&destination).unwrap();
            fs::remove_dir(root.join("originals")).unwrap();
            symlink(&outside, root.join("originals")).unwrap();
            let reopened = LibraryDb::open(&root);
            assert!(reopened.is_err());
            assert!(outside.read_dir().unwrap().next().is_none());
            let _ = fs::remove_file(root.join("originals"));
            let _ = fs::remove_dir_all(outside);
        }
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(sources);
    }

    #[cfg(unix)]
    #[test]
    fn library_open_rejects_symlinked_database_file() {
        use std::os::unix::fs::symlink;
        let root = temp_root("db-symlink");
        let outside = temp_root("db-symlink-outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("database"), b"not sqlite").unwrap();
        symlink(outside.join("database"), root.join("library.sqlite3")).unwrap();
        assert!(LibraryDb::open(&root).is_err());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[cfg(unix)]
    #[test]
    fn library_open_rejects_sqlite_connection_opened_during_catalog_path_swap() {
        let root = temp_root("db-open-race-library");
        let decoy_root = temp_root("db-open-race-decoy");
        let original = LibraryDb::open(&root).unwrap();
        let original_model = original.create_model("Original", None).unwrap();
        drop(original);
        let decoy = LibraryDb::open(&decoy_root).unwrap();
        decoy.create_model("Decoy", None).unwrap();
        drop(decoy);

        let catalog = root.join("library.sqlite3");
        let replacement = root.join("library.sqlite3.replacement");
        let displaced = root.join("library.sqlite3.displaced");
        fs::copy(decoy_root.join("library.sqlite3"), &replacement).unwrap();
        let result = LibraryDb::open_with_hooks(
            &root,
            || {
                fs::rename(&catalog, &displaced).unwrap();
                fs::rename(&replacement, &catalog).unwrap();
            },
            || {
                fs::rename(&catalog, &replacement).unwrap();
                fs::rename(&displaced, &catalog).unwrap();
            },
        );

        assert!(result.is_err());
        let reopened = LibraryDb::open(&root).unwrap();
        assert!(reopened.get_model(&original_model.id).unwrap().is_some());
        assert!(reopened
            .list_models(None, None)
            .unwrap()
            .iter()
            .all(|model| model.display_name != "Decoy"));
        drop(reopened);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(decoy_root);
    }

    #[cfg(unix)]
    #[test]
    fn catalog_commit_aborts_when_the_visible_database_is_replaced() {
        let root = temp_root("db-commit-race-library");
        let decoy_root = temp_root("db-commit-race-decoy");
        let db = LibraryDb::open(&root).unwrap();
        db.create_model("Original", None).unwrap();
        let decoy = LibraryDb::open(&decoy_root).unwrap();
        decoy.create_model("Decoy", None).unwrap();
        drop(decoy);

        let catalog = root.join("library.sqlite3");
        let replacement = root.join("library.sqlite3.replacement");
        let displaced = root.join("library.sqlite3.displaced");
        fs::copy(decoy_root.join("library.sqlite3"), &replacement).unwrap();
        db.catalog_file
            .as_ref()
            .unwrap()
            .install_before_commit_hook(move || {
                fs::rename(&catalog, &displaced).unwrap();
                fs::rename(&replacement, &catalog).unwrap();
            });

        assert!(db.create_model("Must not commit", None).is_err());
        let visible = LibraryDb::open(&root).unwrap();
        let visible_models = visible.list_models(None, None).unwrap();
        assert!(visible_models
            .iter()
            .any(|model| model.display_name == "Decoy"));
        assert!(visible_models
            .iter()
            .all(|model| model.display_name != "Must not commit"));
        drop(visible);
        drop(db);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(decoy_root);
    }

    #[cfg(unix)]
    #[test]
    fn catalog_commit_fails_if_the_visible_database_changes_after_identity_check() {
        let root = temp_root("db-commit-after-check-library");
        let decoy_root = temp_root("db-commit-after-check-decoy");
        let db = LibraryDb::open(&root).unwrap();
        db.create_model("Original", None).unwrap();
        let decoy = LibraryDb::open(&decoy_root).unwrap();
        decoy.create_model("Decoy", None).unwrap();
        drop(decoy);

        let catalog = root.join("library.sqlite3");
        let replacement = root.join("library.sqlite3.replacement");
        let displaced = root.join("library.sqlite3.displaced");
        let decoy_database = decoy_root.join("library.sqlite3");
        fs::copy(&decoy_database, &replacement).unwrap();
        let replacement_connection = Connection::open(&replacement).unwrap();
        let replacement_model: String = replacement_connection
            .query_row(
                "SELECT display_name FROM models WHERE deleted_at IS NULL LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(replacement_model, "Decoy");
        drop(replacement_connection);
        let hook_catalog = catalog.clone();
        let hook_replacement = replacement.clone();
        let hook_displaced = displaced.clone();
        db.catalog_file
            .as_ref()
            .unwrap()
            .install_after_commit_identity_check_hook(move || {
                fs::rename(&hook_catalog, &hook_displaced).unwrap();
                fs::rename(&hook_replacement, &hook_catalog).unwrap();
            });

        let result = db.create_model("Must not commit", None);
        assert!(
            result.is_err(),
            "a catalog replacement after the commit identity check must fail closed"
        );
        assert!(
            displaced.exists(),
            "the after-check replacement hook did not run"
        );
        let raw_visible = Connection::open(&catalog).unwrap();
        let raw_visible_model: String = raw_visible
            .query_row(
                "SELECT display_name FROM models WHERE deleted_at IS NULL LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let journal_path = PathBuf::from(format!("{}-journal", catalog.display()));
        assert_eq!(
            raw_visible_model,
            "Decoy",
            "the visible replacement bytes changed; rollback_journal_exists={}",
            journal_path.exists()
        );
        drop(raw_visible);
        let visible = LibraryDb::open(&root).unwrap();
        let visible_models = visible.list_models(None, None).unwrap();
        assert!(
            visible_models
                .iter()
                .any(|model| model.display_name == "Decoy"),
            "visible replacement catalog lost its decoy record: {visible_models:?}"
        );
        assert!(visible_models
            .iter()
            .all(|model| model.display_name != "Must not commit"));
        drop(visible);
        drop(db);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(decoy_root);
    }

    #[test]
    fn indexed_scan_is_in_place_and_reconciles_change_rename_missing_and_duplicates() {
        let library_root = temp_root("indexed-library");
        let source_root = temp_root("indexed-source");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(source_root.join("unique.stl"), b"unique-v1").unwrap();
        fs::write(source_root.join("duplicate-a.stl"), b"duplicate").unwrap();
        fs::write(source_root.join("duplicate-b.stl"), b"duplicate").unwrap();

        let db = LibraryDb::open(&library_root).unwrap();
        let root = db.add_index_root(&source_root, 15, 1_000_000).unwrap();
        let initial = vec![
            ScannedModel {
                relative_path: "unique.stl".into(),
                content_hash: hash(1),
                bytes: 9,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("Unique metadata".into()),
                archive_member: None,
                outer_fingerprint: None,
            },
            ScannedModel {
                relative_path: "duplicate-a.stl".into(),
                content_hash: hash(2),
                bytes: 9,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: None,
                archive_member: None,
                outer_fingerprint: None,
            },
            ScannedModel {
                relative_path: "duplicate-b.stl".into(),
                content_hash: hash(2),
                bytes: 9,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: None,
                archive_member: None,
                outer_fingerprint: None,
            },
        ];
        assert_eq!(db.record_scan_entries(&root.id, &initial).unwrap().added, 3);
        db.complete_scan(
            &root.id,
            &initial
                .iter()
                .map(|entry| entry.relative_path.clone())
                .collect::<Vec<_>>(),
            "scan-1",
        )
        .unwrap();
        let first_models = db.list_models(None, None).unwrap();
        let unique = first_models
            .iter()
            .find(|model| model.display_name == "Unique metadata")
            .unwrap()
            .clone();
        let duplicate_ids = first_models
            .iter()
            .filter(|model| model.content_hash.as_deref() == Some(hash(2).as_str()))
            .map(|model| model.id.clone())
            .collect::<HashSet<_>>();
        assert_eq!(duplicate_ids.len(), 2);
        assert!(library_root
            .join("originals")
            .read_dir()
            .unwrap()
            .next()
            .is_none());
        assert_eq!(
            fs::read(source_root.join("unique.stl")).unwrap(),
            b"unique-v1"
        );

        fs::rename(
            source_root.join("unique.stl"),
            source_root.join("renamed.stl"),
        )
        .unwrap();
        let renamed = ScannedModel {
            relative_path: "renamed.stl".into(),
            content_hash: hash(1),
            bytes: 9,
            format: "stl-binary".into(),
            modified_at: 2,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        let rename_batch = db
            .record_scan_entries(&root.id, std::slice::from_ref(&renamed))
            .unwrap();
        let rename_result = db
            .complete_scan_with_new_models(
                &root.id,
                &[
                    renamed.relative_path.clone(),
                    "duplicate-a.stl".into(),
                    "duplicate-b.stl".into(),
                ],
                &rename_batch.added_model_ids,
                "scan-2",
            )
            .unwrap();
        assert_eq!(rename_result.renamed, 1);
        let after_rename = db.get_model(&unique.id).unwrap().unwrap();
        assert_eq!(after_rename.display_name, "Unique metadata");
        assert!(after_rename.metadata_version > unique.metadata_version);
        assert!(matches!(
            db.remove_models_recoverably(&[VersionedTarget {
                model_id: unique.id.clone(),
                expected_version: unique.metadata_version,
            }]),
            Err(LibraryError::MetadataConflict { .. })
        ));
        assert!(after_rename
            .source_path
            .as_deref()
            .unwrap()
            .ends_with("renamed.stl"));
        assert_eq!(
            db.resolve_model_source(&unique.id).unwrap(),
            source_root.join("renamed.stl").canonicalize().unwrap()
        );

        let thumbnail_relative = PathBuf::from("thumbnails/unique.png");
        let thumbnail_bytes = png_fixture();
        let thumbnail_hash = hex_digest(&thumbnail_bytes);
        fs::write(library_root.join(&thumbnail_relative), &thumbnail_bytes).unwrap();
        assert!(matches!(
            db.set_thumbnail_for_source(&unique.id, &hash(8), &thumbnail_hash, &thumbnail_relative),
            Err(LibraryError::SourceChanged)
        ));
        db.set_thumbnail_for_source(&unique.id, &hash(1), &thumbnail_hash, &thumbnail_relative)
            .unwrap();
        let (thumbnail, png_hash) = db
            .thumbnail_for_model_with_hash(&unique.id)
            .unwrap()
            .unwrap();
        assert_eq!(
            thumbnail,
            library_root
                .join(&thumbnail_relative)
                .canonicalize()
                .unwrap()
        );
        assert_eq!(png_hash, thumbnail_hash);
        fs::write(source_root.join("renamed.stl"), b"unique-v2").unwrap();
        let changed = ScannedModel {
            relative_path: "renamed.stl".into(),
            content_hash: hash(3),
            bytes: 9,
            format: "stl-binary".into(),
            modified_at: 3,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        db.record_scan_entries(&root.id, std::slice::from_ref(&changed))
            .unwrap();
        let after_content_change = db.get_model(&unique.id).unwrap().unwrap();
        assert_eq!(
            after_content_change.content_hash.as_deref(),
            Some(hash(3).as_str())
        );
        assert!(after_content_change.metadata_version > after_rename.metadata_version);
        assert!(matches!(
            db.remove_models_recoverably(&[VersionedTarget {
                model_id: unique.id.clone(),
                expected_version: after_rename.metadata_version,
            }]),
            Err(LibraryError::MetadataConflict { .. })
        ));
        assert!(db.thumbnail_for_model(&unique.id).unwrap().is_none());
        assert_eq!(
            db.purge_stale_thumbnails().unwrap(),
            vec![library_root
                .canonicalize()
                .unwrap()
                .join(&thumbnail_relative)]
        );

        db.record_scan_entries(&root.id, std::slice::from_ref(&changed))
            .unwrap();
        let unchanged = db.get_model(&unique.id).unwrap().unwrap();
        assert!(!unchanged.source_missing);
        assert_eq!(
            unchanged.metadata_version,
            after_content_change.metadata_version
        );
        let timestamp_only = ScannedModel {
            modified_at: changed.modified_at + 1,
            ..changed.clone()
        };
        db.record_scan_entries(&root.id, std::slice::from_ref(&timestamp_only))
            .unwrap();
        assert_eq!(
            db.get_model(&unique.id).unwrap().unwrap().metadata_version,
            after_content_change.metadata_version
        );
        let completed = db
            .complete_scan(&root.id, &["renamed.stl".into()], "scan-3")
            .unwrap();
        assert_eq!(completed.missing, 2);
        assert_eq!(
            db.list_models(None, None)
                .unwrap()
                .iter()
                .filter(|model| model.source_missing)
                .count(),
            2
        );
        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(source_root);
    }

    #[test]
    fn schema_v3_migration_backs_up_and_adds_filament_catalog() {
        let root = temp_root("schema-v3-filaments");
        fs::create_dir_all(&root).unwrap();
        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(
            "DROP TABLE capture_downloads; DROP TABLE capture_items;
             DROP INDEX thumbnails_active_model_idx; DROP INDEX thumbnails_model_idx;
             ALTER TABLE thumbnails DROP COLUMN is_active;
             ALTER TABLE thumbnails DROP COLUMN ordinal;
             CREATE INDEX thumbnails_model_idx ON thumbnails(model_id,is_custom);
             ALTER TABLE models DROP COLUMN catalog_kind;
             DROP TABLE model_filaments; DROP TABLE filaments; DROP TABLE filament_sources;
             ALTER TABLE models DROP COLUMN favorite; ALTER TABLE models DROP COLUMN rating;
             PRAGMA user_version=3;",
        )
        .unwrap();
        downgrade_printer_schema_to_v6(&conn);
        conn.pragma_update(None, "user_version", 3).unwrap();
        drop(conn);
        let db = LibraryDb::open(&root).unwrap();
        assert_eq!(db.list_filaments(true).unwrap(), Vec::<Filament>::new());
        let version: i64 = db
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        let backup = root.join("library.sqlite3.schema-v3.backup");
        assert!(backup.is_file());
        let backup_conn = Connection::open(backup).unwrap();
        let backup_version: i64 = backup_conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(backup_version, 3);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn schema_v4_migration_backs_up_and_seeds_gallery_selection() {
        let root = temp_root("schema-v4-gallery");
        fs::create_dir_all(&root).unwrap();
        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(
            "DROP TABLE capture_downloads; DROP TABLE capture_items;
             DROP INDEX thumbnails_active_model_idx; DROP INDEX thumbnails_model_idx;
             ALTER TABLE thumbnails DROP COLUMN is_active;
             ALTER TABLE thumbnails DROP COLUMN ordinal;
             CREATE INDEX thumbnails_model_idx ON thumbnails(model_id,is_custom);
             ALTER TABLE models DROP COLUMN catalog_kind;
             ALTER TABLE models DROP COLUMN favorite; ALTER TABLE models DROP COLUMN rating;
             INSERT INTO models(id,display_name,description,folder_id,metadata_version,created_at,updated_at)
               VALUES('gallery-model','Gallery','', 'root',1,'1','1');
             INSERT INTO thumbnails(id,model_id,relative_path,content_hash,is_custom,source_content_hash)
               VALUES('generated','gallery-model','thumbnails/generated.png','aa',0,NULL),
                     ('custom','gallery-model','thumbnails/custom.png','bb',1,NULL);
             PRAGMA user_version=4;",
        )
        .unwrap();
        downgrade_printer_schema_to_v6(&conn);
        conn.pragma_update(None, "user_version", 4).unwrap();
        drop(conn);
        let db = LibraryDb::open(&root).unwrap();
        let gallery = db.list_thumbnail_gallery("gallery-model").unwrap();
        assert_eq!(gallery.len(), 2);
        assert_eq!(gallery.iter().filter(|item| item.is_active).count(), 1);
        assert!(
            gallery
                .iter()
                .find(|item| item.id == "custom")
                .unwrap()
                .is_active
        );
        assert_eq!(
            db.get_model("gallery-model").unwrap().unwrap().catalog_kind,
            "file"
        );
        let backup = root.join("library.sqlite3.schema-v4.backup");
        assert!(backup.is_file());
        let version: i64 = db
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        let version: i64 = Connection::open(backup)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn schema_v5_migration_backs_up_and_adds_rating_and_favorite_defaults() {
        let root = temp_root("schema-v5-rating");
        fs::create_dir_all(&root).unwrap();
        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(
            "ALTER TABLE models DROP COLUMN favorite;
             ALTER TABLE models DROP COLUMN rating;
             INSERT INTO models(id,display_name,description,folder_id,metadata_version,created_at,updated_at)
             VALUES('legacy-model','Legacy','', 'root',1,'1','1');
             PRAGMA user_version=5;",
        )
        .unwrap();
        downgrade_printer_schema_to_v6(&conn);
        conn.pragma_update(None, "user_version", 5).unwrap();
        drop(conn);

        let db = LibraryDb::open(&root).unwrap();
        let model = db.get_model("legacy-model").unwrap().unwrap();
        assert_eq!(model.metadata.rating, 0);
        assert!(!model.metadata.favorite);
        let backup = root.join("library.sqlite3.schema-v5.backup");
        assert!(backup.is_file());
        let backup_version: i64 = Connection::open(backup)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(backup_version, 5);
        let version: i64 = db
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        drop(db);

        let db = LibraryDb::open(&root).unwrap();
        let model = db.get_model("legacy-model").unwrap().unwrap();
        let updated = db
            .update_model_metadata(
                &model.id,
                model.metadata_version,
                &MetadataPatch {
                    rating: Some(5),
                    favorite: Some(true),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        assert_eq!(updated.metadata.rating, 5);
        assert!(updated.metadata.favorite);
        let (filtered, _) = db
            .list_models_page(
                &ModelPageQuery {
                    minimum_rating: Some(5),
                    favorite: Some(true),
                    ..ModelPageQuery::default()
                },
                None,
                None,
                10,
            )
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, model.id);
        assert!(db
            .update_model_metadata(
                &updated.id,
                updated.metadata_version,
                &MetadataPatch {
                    rating: Some(6),
                    ..MetadataPatch::default()
                },
            )
            .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn gallery_and_capture_receipts_are_versioned_bounded_and_idempotent() {
        use crate::models::{CaptureDownloadInput, CapturePageInput};

        let root = temp_root("gallery-capture");
        let db = LibraryDb::open(&root).unwrap();
        let model = db.create_model("Gallery", None).unwrap();
        let png = png_fixture();
        let png_hash = hex_digest(&png);
        let first_path = PathBuf::from("thumbnails/first.png");
        let second_path = PathBuf::from("thumbnails/second.png");
        fs::write(root.join(&first_path), &png).unwrap();
        fs::write(root.join(&second_path), &png).unwrap();
        let (model, first) = db
            .add_custom_thumbnail(&model.id, model.metadata_version, &png_hash, &first_path)
            .unwrap();
        let (model, second) = db
            .add_custom_thumbnail(&model.id, model.metadata_version, &png_hash, &second_path)
            .unwrap();
        assert!(second.is_active);
        let gallery = db.list_thumbnail_gallery(&model.id).unwrap();
        assert_eq!(gallery.len(), 2);
        assert!(
            !gallery
                .iter()
                .find(|item| item.id == first.id)
                .unwrap()
                .is_active
        );
        let model = db
            .set_active_thumbnail(&model.id, model.metadata_version, &first.id)
            .unwrap();
        assert!(db
            .delete_thumbnail_gallery_item(&model.id, model.metadata_version, &first.id)
            .is_err());
        let (model, deleted_path) = db
            .delete_thumbnail_gallery_item(&model.id, model.metadata_version, &second.id)
            .unwrap();
        assert_eq!(
            deleted_path,
            root.canonicalize().unwrap().join(&second_path)
        );
        assert_eq!(db.list_thumbnail_gallery(&model.id).unwrap().len(), 1);

        let receipt = CaptureReceiptInput {
            item_id: "item-1".into(),
            payload_hash: hash(7),
            created_at: "2026-10-11T00:00:00Z".into(),
            page: CapturePageInput {
                url: "https://example.test/model".into(),
                title: "Captured / Bookmark".into(),
                designer: Some("Designer".into()),
                model_license: Some("CC0".into()),
                notes: Some("Captured while offline".into()),
                parent_external_id: None,
            },
            downloads: vec![
                CaptureDownloadInput {
                    capture_id: "download-1".into(),
                    file_name: "part.stl".into(),
                    sha256: Some(hash(8)),
                    bytes: Some(84),
                },
                CaptureDownloadInput {
                    capture_id: "download-2".into(),
                    file_name: "other.3mf".into(),
                    sha256: None,
                    bytes: None,
                },
            ],
        };
        let created = db.create_or_get_capture_receipt(&receipt).unwrap();
        let bookmark_id = created.page_model_id.clone().unwrap();
        let bookmark = db.get_model(&bookmark_id).unwrap().unwrap();
        assert_eq!(bookmark.catalog_kind, "bookmark");
        assert!(bookmark.content_hash.is_none());
        assert_eq!(created.status, "partial");
        assert_eq!(
            db.create_or_get_capture_receipt(&receipt)
                .unwrap()
                .page_model_id,
            Some(bookmark_id.clone())
        );
        let mut conflicting = receipt.clone();
        conflicting.payload_hash = hash(9);
        assert!(db.create_or_get_capture_receipt(&conflicting).is_err());
        let mut second_receipt = receipt.clone();
        second_receipt.item_id = "item-2".into();
        second_receipt.payload_hash = hash(10);
        second_receipt.created_at = "2026-10-11T00:00:01Z".into();
        for (index, download) in second_receipt.downloads.iter_mut().enumerate() {
            download.capture_id = format!("item-2-download-{index}");
        }
        db.create_or_get_capture_receipt(&second_receipt).unwrap();
        let (first_page, more) = db.list_capture_summaries_page(None, None, 1).unwrap();
        assert!(more);
        assert_eq!(first_page[0].item_id, "item-2");
        assert_eq!(first_page[0].download_count, 2);
        let (second_page, more) = db
            .list_capture_summaries_page(
                Some(&first_page[0].created_at),
                Some(&first_page[0].item_id),
                1,
            )
            .unwrap();
        assert!(!more);
        assert_eq!(second_page[0].item_id, "item-1");
        let imported = db.create_model("Imported part", None).unwrap();
        let receipt = db
            .update_capture_download_result("item-1", "download-1", Some(&imported.id), None)
            .unwrap();
        assert_eq!(receipt.status, "partial");
        let receipt = db
            .update_capture_download_result("item-1", "download-2", None, Some("needs_upload"))
            .unwrap();
        assert_eq!(receipt.status, "failed");
        let other = db.create_model("Other part", None).unwrap();
        let receipt = db
            .update_capture_download_result("item-1", "download-2", Some(&other.id), None)
            .unwrap();
        assert_eq!(receipt.status, "imported");
        db.remove_models_recoverably(&[VersionedTarget {
            model_id: imported.id.clone(),
            expected_version: imported.metadata_version,
        }])
        .unwrap();
        let receipt = db.get_capture_receipt("item-1").unwrap().unwrap();
        assert_eq!(receipt.downloads[0].status, "imported_removed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manual_filaments_assign_atomically_archive_references_and_preserve_unknown_history() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Filament model", None).unwrap();
        let red = db
            .create_filament(&FilamentInput {
                brand: "Example".into(),
                material: "PLA".into(),
                color: "#FF0000".into(),
                name: "Red".into(),
                notes: "local".into(),
            })
            .unwrap();
        let assigned = db
            .set_model_filaments(
                &model.id,
                model.metadata_version,
                std::slice::from_ref(&red.id),
            )
            .unwrap();
        assert_eq!(assigned.filament_ids, std::slice::from_ref(&red.id));
        assert!(matches!(
            db.set_model_filaments(&model.id, model.metadata_version, &[]),
            Err(LibraryError::MetadataConflict { .. })
        ));
        let attempt = db
            .create_print_attempt(
                &model.id,
                &PrintAttemptInput {
                    printed_at: "2026-10-10T12:00:00Z".into(),
                    quantity: 1,
                    notes: String::new(),
                    filament_ids: vec!["legacy-unknown".into()],
                    outcome: PrintOutcome::Printed,
                    parts: None,
                },
            )
            .unwrap();
        assert_eq!(attempt.filament_ids, ["legacy-unknown"]);
        let archived = db.delete_filament(&red.id, red.version).unwrap().unwrap();
        assert!(archived.archived);
        assert_eq!(
            db.get_print_attempt(&attempt.id)
                .unwrap()
                .unwrap()
                .filament_ids,
            ["legacy-unknown"]
        );
        let extra = (0..5)
            .map(|number| {
                db.create_filament(&FilamentInput {
                    name: format!("F{number}"),
                    ..FilamentInput::default()
                })
                .unwrap()
            })
            .collect::<Vec<_>>();
        let current = db.get_model(&model.id).unwrap().unwrap();
        assert!(db
            .set_model_filaments(
                &model.id,
                current.metadata_version,
                &extra.iter().map(|item| item.id.clone()).collect::<Vec<_>>()
            )
            .is_ok());
        let current = db.get_model(&model.id).unwrap().unwrap();
        let mut six = current.filament_ids.clone();
        six.push(red.id);
        assert!(db
            .set_model_filaments(&model.id, current.metadata_version, &six)
            .is_err());
        let unreferenced = db
            .create_filament(&FilamentInput {
                name: "Disposable".into(),
                ..FilamentInput::default()
            })
            .unwrap();
        assert!(db
            .delete_filament(&unreferenced.id, unreferenced.version)
            .unwrap()
            .is_none());
    }

    #[test]
    fn heterogeneous_filament_assignments_prevalidate_every_target_atomically() {
        let db = LibraryDb::open_in_memory().unwrap();
        let a = db.create_model("A", None).unwrap();
        let b = db.create_model("B", None).unwrap();
        let make = |name: &str| {
            db.create_filament(&FilamentInput {
                name: name.into(),
                ..FilamentInput::default()
            })
            .unwrap()
        };
        let first = make("First");
        let second = make("Second");
        let shared = make("Shared");
        let a = db
            .set_model_filaments(&a.id, a.metadata_version, std::slice::from_ref(&first.id))
            .unwrap();
        let b = db
            .set_model_filaments(&b.id, b.metadata_version, std::slice::from_ref(&second.id))
            .unwrap();
        let changed_b = db
            .update_model_metadata(
                &b.id,
                b.metadata_version,
                &MetadataPatch {
                    notes: Some("changed".into()),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        assert!(matches!(
            db.bulk_assign_model_filaments(&[
                ModelFilamentAssignment {
                    target: VersionedTarget {
                        model_id: a.id.clone(),
                        expected_version: a.metadata_version,
                    },
                    filament_ids: vec![first.id.clone(), shared.id.clone()],
                },
                ModelFilamentAssignment {
                    target: VersionedTarget {
                        model_id: b.id.clone(),
                        expected_version: b.metadata_version,
                    },
                    filament_ids: vec![second.id.clone(), shared.id.clone()],
                },
            ]),
            Err(LibraryError::MetadataConflict { .. })
        ));
        assert_eq!(
            db.get_model(&a.id).unwrap().unwrap().filament_ids,
            std::slice::from_ref(&first.id)
        );
        assert!(db
            .bulk_assign_model_filaments(&[
                ModelFilamentAssignment {
                    target: VersionedTarget {
                        model_id: a.id.clone(),
                        expected_version: a.metadata_version,
                    },
                    filament_ids: vec![first.id.clone(), shared.id.clone()],
                },
                ModelFilamentAssignment {
                    target: VersionedTarget {
                        model_id: b.id.clone(),
                        expected_version: changed_b.metadata_version,
                    },
                    filament_ids: vec!["unknown".into()],
                },
            ])
            .is_err());
        assert_eq!(
            db.get_model(&a.id).unwrap().unwrap().filament_ids,
            std::slice::from_ref(&first.id)
        );
        let updated = db
            .bulk_assign_model_filaments(&[
                ModelFilamentAssignment {
                    target: VersionedTarget {
                        model_id: a.id.clone(),
                        expected_version: a.metadata_version,
                    },
                    filament_ids: vec![first.id.clone(), shared.id.clone()],
                },
                ModelFilamentAssignment {
                    target: VersionedTarget {
                        model_id: b.id,
                        expected_version: changed_b.metadata_version,
                    },
                    filament_ids: vec![second.id.clone(), shared.id.clone()],
                },
            ])
            .unwrap();
        assert_eq!(updated[0].filament_ids, [first.id, shared.id.clone()]);
        assert_eq!(updated[1].filament_ids, [second.id, shared.id]);
    }

    #[test]
    fn spoolman_complete_snapshots_keep_ids_and_references_across_missing_and_reappearance() {
        let db = LibraryDb::open_in_memory().unwrap();
        let source = db
            .upsert_spoolman_source("http://spoolman.local/api/v1/")
            .unwrap();
        let filament = SpoolmanFilamentRecord {
            external_id: "1".into(),
            brand: "Vendor".into(),
            material: "PLA".into(),
            color: "FF0000".into(),
            name: "Red PLA".into(),
            notes: String::new(),
        };
        let spool = SpoolmanSpoolRecord {
            external_id: "10".into(),
            filament_external_id: "1".into(),
            brand: filament.brand.clone(),
            material: filament.material.clone(),
            color: filament.color.clone(),
            name: filament.name.clone(),
            notes: "Shelf A".into(),
            archived: true,
        };
        let first = db
            .sync_spoolman_snapshot(
                &source.id,
                std::slice::from_ref(&filament),
                std::slice::from_ref(&spool),
                "2026-10-10T12:00:00Z",
            )
            .unwrap();
        assert_eq!(first.added, 2);
        let imported = db.list_filaments(true).unwrap();
        let type_id = imported
            .iter()
            .find(|item| item.resource_kind == FilamentResourceKind::Filament)
            .unwrap()
            .id
            .clone();
        let spool_row = imported
            .iter()
            .find(|item| item.resource_kind == FilamentResourceKind::Spool)
            .unwrap()
            .clone();
        assert_eq!(
            spool_row.parent_filament_id.as_deref(),
            Some(type_id.as_str())
        );
        assert!(spool_row.archived);
        let repeated = db
            .sync_spoolman_snapshot(
                &source.id,
                std::slice::from_ref(&filament),
                std::slice::from_ref(&spool),
                "2026-10-10T12:01:00Z",
            )
            .unwrap();
        assert_eq!(
            (repeated.added, repeated.updated, repeated.missing),
            (0, 0, 0)
        );
        let model = db.create_model("Synced", None).unwrap();
        db.set_model_filaments(
            &model.id,
            model.metadata_version,
            std::slice::from_ref(&spool_row.id),
        )
        .unwrap();
        let malformed = SpoolmanSpoolRecord {
            filament_external_id: "missing".into(),
            ..spool.clone()
        };
        assert!(db
            .sync_spoolman_snapshot(
                &source.id,
                std::slice::from_ref(&filament),
                &[malformed],
                "2026-10-10T12:02:00Z"
            )
            .is_err());
        assert!(
            !db.get_filament(&spool_row.id)
                .unwrap()
                .unwrap()
                .source_missing
        );
        let missing = db
            .sync_spoolman_snapshot(&source.id, &[], &[], "2026-10-10T12:03:00Z")
            .unwrap();
        assert_eq!(missing.missing, 2);
        assert!(
            db.get_filament(&spool_row.id)
                .unwrap()
                .unwrap()
                .source_missing
        );
        assert_eq!(
            db.get_model(&model.id).unwrap().unwrap().filament_ids,
            std::slice::from_ref(&spool_row.id)
        );
        db.sync_spoolman_snapshot(
            &source.id,
            std::slice::from_ref(&filament),
            std::slice::from_ref(&spool),
            "2026-10-10T12:04:00Z",
        )
        .unwrap();
        assert!(
            !db.get_filament(&spool_row.id)
                .unwrap()
                .unwrap()
                .source_missing
        );
        assert_eq!(
            db.get_filament(&spool_row.id).unwrap().unwrap().id,
            spool_row.id
        );
        let other = db
            .upsert_spoolman_source("https://other.example/api/v1")
            .unwrap();
        db.sync_spoolman_snapshot(
            &other.id,
            std::slice::from_ref(&filament),
            std::slice::from_ref(&spool),
            "2026-10-10T12:05:00Z",
        )
        .unwrap();
        assert_eq!(db.list_filaments(true).unwrap().len(), 4);
    }

    #[test]
    fn index_roots_reject_overlap_and_removed_roots_reject_stale_scan_writes() {
        let library_root = temp_root("root-validation-library");
        let source_root = temp_root("root-validation-source");
        fs::create_dir_all(source_root.join("nested")).unwrap();
        fs::write(source_root.join("part.stl"), b"part").unwrap();
        let db = LibraryDb::open(&library_root).unwrap();
        assert!(db.add_index_root(&source_root, 0, 1024).is_err());
        assert!(db.add_index_root(&source_root, 1_441, 1024).is_err());
        assert!(db
            .add_index_root(&source_root, 10, MAX_INDEXED_FILE_BYTES + 1)
            .is_err());
        let root = db.add_index_root(&source_root, 10, 1024).unwrap();
        assert!(db
            .add_index_root(source_root.join("nested"), 10, 1024)
            .is_err());
        db.remove_index_root(&root.id).unwrap();
        let entry = ScannedModel {
            relative_path: "part.stl".into(),
            content_hash: hash(4),
            bytes: 4,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        assert!(matches!(
            db.record_scan_entries(&root.id, &[entry]),
            Err(LibraryError::IndexRootNotFound(_))
        ));
        assert!(matches!(
            db.complete_scan(&root.id, &[], "cancelled"),
            Err(LibraryError::IndexRootNotFound(_))
        ));
        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(source_root);
    }

    #[test]
    fn persisted_index_root_values_are_checked_before_scanner_use() {
        let library_root = temp_root("corrupt-root-settings-library");
        let source_root = temp_root("corrupt-root-settings-source");
        fs::create_dir_all(&source_root).unwrap();
        let db = LibraryDb::open(&library_root).unwrap();
        let root = db.add_index_root(&source_root, 60, 1024).unwrap();
        db.conn
            .execute_batch("PRAGMA ignore_check_constraints = ON;")
            .unwrap();

        db.conn
            .execute(
                "UPDATE index_roots SET max_file_bytes = ?1 WHERE id = ?2",
                params![MAX_INDEXED_FILE_BYTES as i64 + 1, root.id],
            )
            .unwrap();
        assert!(db.list_index_roots().is_err());

        db.conn
            .execute(
                "UPDATE index_roots SET max_file_bytes = 1024, scan_interval_minutes = 4294967356
                 WHERE id = ?1",
                params![root.id],
            )
            .unwrap();
        assert!(db.list_index_roots().is_err());

        db.conn
            .execute(
                "UPDATE index_roots SET scan_interval_minutes = 60, enabled = 2 WHERE id = ?1",
                params![root.id],
            )
            .unwrap();
        assert!(db.list_index_roots().is_err());
        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(source_root);
    }

    #[test]
    fn offline_index_root_settings_can_be_updated_without_changing_catalog_identity() {
        let library_root = temp_root("offline-settings-library");
        let source_root = temp_root("offline-settings-source");
        let offline_location = temp_root("offline-settings-unmounted");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(source_root.join("part.stl"), b"part").unwrap();
        let db = LibraryDb::open(&library_root).unwrap();
        let root = db.add_index_root(&source_root, 10, 1024).unwrap();
        let entry = ScannedModel {
            relative_path: "part.stl".into(),
            content_hash: hash(4),
            bytes: 4,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: Some("Offline part".into()),
            archive_member: None,
            outer_fingerprint: None,
        };
        let added = db.record_scan_entries(&root.id, &[entry]).unwrap();
        let model_id = added.added_model_ids[0].clone();
        db.complete_scan_with_new_models(
            &root.id,
            &["part.stl".into()],
            &added.added_model_ids,
            "scan-1",
        )
        .unwrap();
        fs::rename(&source_root, &offline_location).unwrap();

        let updated = db
            .update_index_root(&IndexRoot {
                enabled: false,
                scan_interval_minutes: 120,
                max_file_bytes: 2048,
                ..root
            })
            .unwrap();
        assert!(!updated.enabled);
        assert_eq!(updated.scan_interval_minutes, 120);
        assert_eq!(updated.max_file_bytes, 2048);
        let model = db.get_model(&model_id).unwrap().unwrap();
        assert_eq!(model.id, model_id);
        assert!(!model.source_missing);
        assert_eq!(db.list_models(None, None).unwrap().len(), 1);
        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(offline_location);
    }

    #[test]
    fn restored_root_mapping_rewrites_links_and_disables_unmapped_roots() {
        let temp = temp_root("restore-root-map");
        let old = temp.join("old");
        let new = temp.join("new");
        let other = temp.join("other");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(old.join("part.stl"), vec![0_u8; 84]).unwrap();
        let db = LibraryDb::open(temp.join("library")).unwrap();
        let root = db.add_index_root(&old, 60, 1024).unwrap();
        let unmapped = db.add_index_root(&other, 60, 1024).unwrap();
        db.record_scan_entries(
            &root.id,
            &[ScannedModel {
                relative_path: "part.stl".into(),
                content_hash: hash(1),
                bytes: 84,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: None,
                archive_member: None,
                outer_fingerprint: None,
            }],
        )
        .unwrap();
        db.remap_restored_index_roots(&[(
            root.id.clone(),
            "operator-root".into(),
            new.canonicalize().unwrap(),
            true,
        )])
        .unwrap();
        let roots = db.list_index_roots().unwrap();
        let mapped = roots
            .iter()
            .find(|root| root.id == "operator-root")
            .unwrap();
        assert_eq!(mapped.path, new.canonicalize().unwrap());
        assert!(mapped.enabled);
        assert!(
            !roots
                .iter()
                .find(|root| root.id == unmapped.id)
                .unwrap()
                .enabled
        );
        let model = db.list_models(None, None).unwrap().remove(0);
        assert_eq!(model.source_root_id.as_deref(), Some("operator-root"));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn duplicate_survivor_is_not_consumed_as_a_rename_and_keeps_its_identity() {
        let library_root = temp_root("duplicate-survivor-library");
        let source_root = temp_root("duplicate-survivor-source");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(source_root.join("a.stl"), b"same").unwrap();
        fs::write(source_root.join("b.stl"), b"same").unwrap();
        let db = LibraryDb::open(&library_root).unwrap();
        let root = db.add_index_root(&source_root, 10, 1024).unwrap();
        let entries = [
            ScannedModel {
                relative_path: "a.stl".into(),
                content_hash: hash(6),
                bytes: 4,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("Metadata A".into()),
                archive_member: None,
                outer_fingerprint: None,
            },
            ScannedModel {
                relative_path: "b.stl".into(),
                content_hash: hash(6),
                bytes: 4,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("Metadata B".into()),
                archive_member: None,
                outer_fingerprint: None,
            },
        ];
        let added = db.record_scan_entries(&root.id, &entries).unwrap();
        db.complete_scan_with_new_models(
            &root.id,
            &["a.stl".into(), "b.stl".into()],
            &added.added_model_ids,
            "scan-1",
        )
        .unwrap();
        let survivor_before = db
            .list_models(None, Some("Metadata B"))
            .unwrap()
            .pop()
            .unwrap();
        fs::remove_file(source_root.join("a.stl")).unwrap();

        let completed = db
            .complete_scan_with_new_models(&root.id, &["b.stl".into()], &[], "scan-2")
            .unwrap();
        assert_eq!(completed.renamed, 0);
        let survivor_after = db.get_model(&survivor_before.id).unwrap().unwrap();
        assert_eq!(survivor_after.display_name, "Metadata B");
        assert!(!survivor_after.source_missing);
        assert_eq!(
            db.list_models(None, None)
                .unwrap()
                .iter()
                .filter(|model| model.source_missing)
                .count(),
            1
        );
        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(source_root);
    }

    #[test]
    fn cancelled_scan_additions_are_not_later_treated_as_new_rename_targets() {
        let library_root = temp_root("cancelled-rename-library");
        let source_root = temp_root("cancelled-rename-source");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(source_root.join("old.stl"), b"same").unwrap();
        let db = LibraryDb::open(&library_root).unwrap();
        let root = db.add_index_root(&source_root, 10, 1024).unwrap();
        let old = ScannedModel {
            relative_path: "old.stl".into(),
            content_hash: hash(7),
            bytes: 4,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: Some("Old metadata".into()),
            archive_member: None,
            outer_fingerprint: None,
        };
        let first = db
            .record_scan_entries(&root.id, std::slice::from_ref(&old))
            .unwrap();
        db.complete_scan_with_new_models(
            &root.id,
            &["old.stl".into()],
            &first.added_model_ids,
            "scan-1",
        )
        .unwrap();
        fs::rename(source_root.join("old.stl"), source_root.join("new.stl")).unwrap();
        let new = ScannedModel {
            relative_path: "new.stl".into(),
            content_hash: hash(7),
            bytes: 4,
            format: "stl-binary".into(),
            modified_at: 2,
            display_name: Some("Cancelled identity".into()),
            archive_member: None,
            outer_fingerprint: None,
        };
        let cancelled = db
            .record_scan_entries(&root.id, std::slice::from_ref(&new))
            .unwrap();
        assert_eq!(cancelled.added_model_ids.len(), 1);

        let resumed = db
            .record_scan_entries(&root.id, std::slice::from_ref(&new))
            .unwrap();
        assert!(resumed.added_model_ids.is_empty());
        let completed = db
            .complete_scan_with_new_models(
                &root.id,
                &["new.stl".into()],
                &resumed.added_model_ids,
                "scan-2",
            )
            .unwrap();
        assert_eq!(completed.renamed, 0);
        assert_eq!(db.list_models(None, None).unwrap().len(), 2);
        assert!(db
            .get_model(&cancelled.added_model_ids[0])
            .unwrap()
            .is_some());
        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(source_root);
    }

    #[cfg(unix)]
    #[test]
    fn indexed_sources_reject_parent_escape_and_symlink_escape() {
        use std::os::unix::fs::symlink;

        let library_root = temp_root("escape-library");
        let source_root = temp_root("escape-source");
        let outside_root = temp_root("escape-outside");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&outside_root).unwrap();
        fs::write(outside_root.join("outside.stl"), b"outside").unwrap();
        symlink(
            outside_root.join("outside.stl"),
            source_root.join("link.stl"),
        )
        .unwrap();
        let db = LibraryDb::open(&library_root).unwrap();
        let root = db.add_index_root(&source_root, 10, 1024).unwrap();
        let escaped = ScannedModel {
            relative_path: "../outside.stl".into(),
            content_hash: hash(5),
            bytes: 7,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        assert!(db.record_scan_entries(&root.id, &[escaped]).is_err());
        let linked = ScannedModel {
            relative_path: "link.stl".into(),
            content_hash: hash(5),
            bytes: 7,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        assert!(db.record_scan_entries(&root.id, &[linked]).is_err());
        fs::remove_file(source_root.join("link.stl")).unwrap();
        let moved_root = temp_root("escape-source-moved");
        fs::rename(&source_root, &moved_root).unwrap();
        symlink(&outside_root, &source_root).unwrap();
        let replaced_root = ScannedModel {
            relative_path: "outside.stl".into(),
            content_hash: hash(5),
            bytes: 7,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        assert!(db.record_scan_entries(&root.id, &[replaced_root]).is_err());
        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_file(source_root);
        let _ = fs::remove_dir_all(moved_root);
        let _ = fs::remove_dir_all(outside_root);
    }

    #[test]
    fn purge_thumbnails_removes_generated_records_but_preserves_custom_records() {
        let root = temp_root("purge-thumbnails");
        let source_dir = temp_root("purge-thumbnails-source");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("part.stl");
        fs::write(&source, stl_fixture()).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        let generated_bytes = png_fixture();
        let generated_hash = hex_digest(&generated_bytes);
        fs::write(root.join("thumbnails/generated.png"), &generated_bytes).unwrap();
        fs::write(root.join("thumbnails/custom.png"), b"custom").unwrap();
        db.set_thumbnail(
            &model.id,
            &generated_hash,
            Path::new("thumbnails/generated.png"),
        )
        .unwrap();
        db.conn.execute("INSERT INTO thumbnails (id, model_id, relative_path, content_hash, is_custom) VALUES ('custom', ?1, 'thumbnails/custom.png', 'custom-hash', 1)", params![model.id]).unwrap();
        let purged = db.purge_thumbnails().unwrap();
        assert_eq!(
            purged,
            vec![root
                .canonicalize()
                .unwrap()
                .join("thumbnails/generated.png")]
        );
        assert_eq!(
            db.thumbnail_for_model(&model.id).unwrap(),
            Some(root.join("thumbnails/custom.png").canonicalize().unwrap())
        );
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_dir);
    }

    #[test]
    fn purge_removes_generated_row_but_preserves_file_shared_with_custom_row() {
        let root = temp_root("purge-shared-custom");
        let source_dir = temp_root("purge-shared-custom-source");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("part.stl");
        fs::write(&source, stl_fixture()).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        let thumbnail_bytes = png_fixture();
        let thumbnail_hash = hex_digest(&thumbnail_bytes);
        let relative = "thumbnails/shared.png";
        let absolute = db.root().join(relative);
        fs::write(&absolute, &thumbnail_bytes).unwrap();
        db.set_thumbnail(&model.id, &thumbnail_hash, Path::new(relative))
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO thumbnails
                 (id, model_id, relative_path, content_hash, is_custom)
                 VALUES ('shared-custom', ?1, ?2, ?3, 1)",
                params![model.id, relative, thumbnail_hash],
            )
            .unwrap();

        let preferred = db
            .thumbnail_cache_entry_for_model(&model.id)
            .unwrap()
            .unwrap();
        assert!(preferred.is_custom);
        assert_eq!(preferred.png_hash, thumbnail_hash);

        let physical_deletes = db.purge_thumbnails().unwrap();
        for path in physical_deletes {
            fs::remove_file(path).unwrap();
        }
        assert!(absolute.is_file());
        let generated: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE is_custom = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let custom: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE id = 'shared-custom'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generated, 0);
        assert_eq!(custom, 1);
        assert!(db.thumbnail_for_model(&model.id).unwrap().is_some());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_dir);
    }

    #[test]
    fn purge_refuses_wrong_namespace_before_deleting_rows_or_originals() {
        let root = temp_root("purge-wrong-namespace");
        let source_dir = temp_root("purge-wrong-namespace-source");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("part.stl");
        let source_bytes = stl_fixture();
        fs::write(&source, &source_bytes).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        let original_relative = model.storage_path.clone().unwrap();
        let original_path = db.root().join(&original_relative);
        let before = fs::read(&original_path).unwrap();
        db.conn
            .execute(
                "INSERT INTO thumbnails
                 (id, model_id, relative_path, content_hash, is_custom, source_content_hash)
                 VALUES ('malicious-original', ?1, ?2, ?3, 0, ?4)",
                params![
                    model.id,
                    original_relative,
                    hex_digest(&source_bytes),
                    model.content_hash
                ],
            )
            .unwrap();

        assert!(matches!(
            db.purge_thumbnails(),
            Err(LibraryError::InvalidInput(_))
        ));
        assert_eq!(fs::read(&original_path).unwrap(), before);
        let retained: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE id = 'malicious-original'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, 1);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_dir);
    }

    #[cfg(unix)]
    #[test]
    fn purge_refuses_symlinked_thumbnail_ancestor_and_retains_outside_file_and_row() {
        use std::os::unix::fs::symlink;

        let root = temp_root("purge-symlink-ancestor");
        let source_dir = temp_root("purge-symlink-ancestor-source");
        let outside = temp_root("purge-symlink-ancestor-outside");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let source = source_dir.join("part.stl");
        fs::write(&source, stl_fixture()).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        let outside_bytes = png_fixture();
        let outside_path = outside.join("outside.png");
        fs::write(&outside_path, &outside_bytes).unwrap();
        symlink(&outside, db.root().join("thumbnails/link")).unwrap();
        db.conn
            .execute(
                "INSERT INTO thumbnails
                 (id, model_id, relative_path, content_hash, is_custom, source_content_hash)
                 VALUES ('malicious-symlink', ?1, 'thumbnails/link/outside.png', ?2, 0, ?3)",
                params![model.id, hex_digest(&outside_bytes), model.content_hash],
            )
            .unwrap();

        assert!(matches!(
            db.purge_thumbnails(),
            Err(LibraryError::InvalidInput(_))
        ));
        assert_eq!(fs::read(&outside_path).unwrap(), outside_bytes);
        let retained: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE id = 'malicious-symlink'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, 1);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_dir);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn nonempty_folder_cannot_be_deleted() {
        let db = LibraryDb::open_in_memory().unwrap();
        let folder = db.create_folder("Nested", None).unwrap();
        db.create_model("Part", Some(&folder.id)).unwrap();
        assert!(matches!(
            db.delete_folder(&folder.id),
            Err(LibraryError::FolderNotEmpty)
        ));
    }

    #[test]
    fn moving_a_folder_rejects_descendant_cycles() {
        let db = LibraryDb::open_in_memory().unwrap();
        let parent = db.create_folder("Parent", None).unwrap();
        let child = db.create_folder("Child", Some(&parent.id)).unwrap();
        assert!(matches!(
            db.move_folder(&parent.id, &child.id),
            Err(LibraryError::FolderCycle)
        ));
        assert_eq!(
            db.move_folder(&child.id, ROOT_FOLDER_ID)
                .unwrap()
                .parent_id
                .as_deref(),
            Some(ROOT_FOLDER_ID)
        );
    }

    fn printer_input(name: &str) -> PrinterInput {
        PrinterInput {
            nickname: name.into(),
            make: Some("Example Works".into()),
            model: Some("P-1".into()),
            printer_type: Some("FDM".into()),
            firmware: Some("1.0".into()),
            host: None,
            web_url: None,
            bed_mm: Some([220.0, 220.0, 250.0]),
            notes: "Workshop printer".into(),
        }
    }

    fn printer_reminder_input(due_at: &str, interval_days: Option<u32>) -> PrinterReminderInput {
        PrinterReminderInput {
            title: "Clean nozzle".into(),
            notes: "Check for buildup".into(),
            due_at: due_at.into(),
            interval_days,
        }
    }

    fn print_attempt_input() -> PrintAttemptInput {
        PrintAttemptInput {
            printed_at: "2026-10-14T12:00:00Z".into(),
            quantity: 1,
            notes: "Test print".into(),
            filament_ids: Vec::new(),
            outcome: PrintOutcome::Printed,
            parts: None,
        }
    }

    fn printer_data_import_printer(
        external_ref: &str,
        input: PrinterInput,
    ) -> PrinterDataImportPrinter {
        PrinterDataImportPrinter {
            external_ref: external_ref.into(),
            map_to_local_printer_id: None,
            created_at: None,
            updated_at: None,
            input,
        }
    }

    #[test]
    fn printer_data_import_generates_local_ids_and_forces_new_printers_unapproved() {
        let db = LibraryDb::open_in_memory().unwrap();
        let mut input = printer_input("Imported P-1");
        input.host = Some("192.0.2.21".into());
        let mut batch = PrinterDataImport::default();
        batch
            .printers
            .push(printer_data_import_printer("source-printer-1", input));

        let result = db.import_printer_data(&batch).unwrap();
        assert_eq!(result.imported_printer_count, 1);
        assert_eq!(result.mapped_printer_count, 0);
        let mapped = &result.printer_ref_map[0];
        assert_eq!(mapped.external_ref, "source-printer-1");
        assert_ne!(mapped.local_printer_id, mapped.external_ref);
        let printer = db.get_printer(&mapped.local_printer_id).unwrap().unwrap();
        assert_eq!(printer.source_kind, PrinterSourceKind::Imported);
        assert!(!printer.endpoint_approved);
        assert_eq!(printer.host.as_deref(), Some("192.0.2.21"));
        assert_eq!(printer.connection_status, PrinterConnectionStatus::Unknown);
        assert_eq!(printer.job_status, PrinterJobStatus::Unknown);
    }

    #[test]
    fn printer_data_import_preserves_new_printer_timestamps_and_resets_trust_state() {
        let db = LibraryDb::open_in_memory().unwrap();
        let batch: PrinterDataImport = serde_json::from_value(serde_json::json!({
            "printers": [{
                "externalRef": "source-printer",
                "createdAt": "1700000000",
                "updatedAt": "1700000001",
                "sourceKind": "native",
                "endpointApproved": true,
                "connectionStatus": "connected",
                "jobStatus": "printing",
                "input": printer_input("Imported with history")
            }]
        }))
        .unwrap();

        let imported = db.import_printer_data(&batch).unwrap();
        let printer = db
            .get_printer(&imported.printer_ref_map[0].local_printer_id)
            .unwrap()
            .unwrap();
        assert_eq!(printer.created_at, "1700000000");
        assert_eq!(printer.updated_at, "1700000001");
        assert_eq!(printer.source_kind, PrinterSourceKind::Imported);
        assert!(!printer.endpoint_approved);
        assert_eq!(printer.connection_status, PrinterConnectionStatus::Unknown);
        assert_eq!(printer.job_status, PrinterJobStatus::Unknown);
    }

    #[test]
    fn printer_data_import_defaults_missing_printer_timestamps_to_import_time() {
        let db = LibraryDb::open_in_memory().unwrap();
        let batch: PrinterDataImport = serde_json::from_value(serde_json::json!({
            "printers": [{
                "externalRef": "source-printer",
                "input": printer_input("Imported without history")
            }]
        }))
        .unwrap();

        let imported = db.import_printer_data(&batch).unwrap();
        let printer = db
            .get_printer(&imported.printer_ref_map[0].local_printer_id)
            .unwrap()
            .unwrap();
        assert_eq!(printer.created_at, printer.updated_at);
        assert!(printer
            .created_at
            .chars()
            .any(|character| character.is_ascii_digit()));
    }

    #[test]
    fn printer_data_import_rejects_unbounded_or_invalid_printer_timestamps() {
        for (field, value) in [
            ("createdAt", " ".into()),
            ("updatedAt", "x".repeat(129)),
            ("createdAt", "bad\u{0007}timestamp".into()),
        ] {
            let db = LibraryDb::open_in_memory().unwrap();
            let mut printer = serde_json::json!({
                "externalRef": "source-printer",
                "input": printer_input("Invalid timestamp")
            });
            printer[field] = serde_json::Value::String(value);
            let batch: PrinterDataImport = serde_json::from_value(serde_json::json!({
                "printers": [printer]
            }))
            .unwrap();

            assert!(
                db.import_printer_data(&batch).is_err(),
                "{field} should be rejected"
            );
            assert!(db.list_printers().unwrap().is_empty());
        }
    }

    #[test]
    fn printer_json_profile_import_and_export_support_the_5000_record_limit() {
        let db = LibraryDb::open_in_memory().unwrap();
        assert_eq!(MAX_PRINTER_DATA_IMPORT_RECORDS, 65_000);
        let mut batch = PrinterDataImport::default();
        for index in 0..5_000 {
            batch.printers.push(printer_data_import_printer(
                &format!("profile-{index}"),
                printer_input(&format!("Printer {index}")),
            ));
        }
        for index in 0..50_000 {
            batch
                .maintenance_logs
                .push(PrinterDataImportMaintenanceLog {
                    external_ref: format!("full-cap-log-{index}"),
                    printer_external_ref: Some("profile-0".into()),
                    printer_nickname: Some("Printer 0".into()),
                    input: PrinterMaintenanceInput {
                        title: "Service printer".into(),
                        performed_at: "2026-10-14T12:00:00Z".into(),
                        notes: String::new(),
                    },
                    created_at: None,
                });
        }
        for index in 0..10_000 {
            batch.reminders.push(PrinterDataImportReminder {
                external_ref: format!("full-cap-reminder-{index}"),
                printer_external_ref: Some("profile-0".into()),
                printer_nickname: Some("Printer 0".into()),
                input: printer_reminder_input("2026-11-01T00:00:00Z", None),
                status: PrinterReminderStatus::Active,
                last_completed_at: None,
                created_at: "1700000000".into(),
                updated_at: "1700000000".into(),
            });
        }

        let imported = db.import_printer_data(&batch).unwrap();
        assert_eq!(imported.imported_printer_count, 5_000);
        assert_eq!(imported.imported_maintenance_log_count, 50_000);
        assert_eq!(imported.imported_reminder_count, 10_000);
        let snapshot = db.export_printer_data_snapshot().unwrap();
        assert_eq!(snapshot.printers.len(), 5_000);
        assert_eq!(snapshot.maintenance_logs.len(), 50_000);
        assert_eq!(snapshot.reminders.len(), 10_000);

        let mut oversized_batch = PrinterDataImport::default();
        for index in 0..5_001 {
            oversized_batch.printers.push(printer_data_import_printer(
                &format!("over-profile-{index}"),
                printer_input(&format!("Over profile {index}")),
            ));
        }
        assert!(db.import_printer_data(&oversized_batch).is_err());
        assert_eq!(db.list_printers().unwrap().len(), 5_000);

        db.conn
            .execute(
                "INSERT INTO printers(id,nickname,created_at,updated_at)
                 VALUES('over-cap-printer','Over cap','1700000000','1700000000')",
                [],
            )
            .unwrap();
        let error = db.export_printer_data_snapshot().unwrap_err();
        assert!(matches!(
            error,
            LibraryError::InvalidInput(message)
                if message.contains("printers")
                    && message.contains("supported maximum of 5000")
        ));
    }

    #[test]
    fn model_json_export_snapshot_accepts_5000_and_rejects_5001_before_listing() {
        let db = LibraryDb::open_in_memory().unwrap();
        let tx = db.conn.unchecked_transaction().unwrap();
        tx.execute(
            "WITH RECURSIVE sequence(id) AS (
                 SELECT 1 UNION ALL SELECT id+1 FROM sequence WHERE id<?1
             )
             INSERT INTO models(id,display_name,folder_id,created_at,updated_at)
             SELECT 'export-model-'||id,'Export model '||id,'root','1700000000','1700000000'
             FROM sequence",
            params![MAX_LIBRARY_JSON_EXPORT_MODELS as i64],
        )
        .unwrap();
        tx.commit().unwrap();

        assert_eq!(db.list_models_for_json_export().unwrap().len(), 5_000);

        db.conn
            .execute(
                "INSERT INTO models(id,display_name,folder_id,created_at,updated_at)
                 VALUES('export-model-over-cap','Export model over cap','root','1700000000','1700000000')",
                [],
            )
            .unwrap();
        let error = db.list_models_for_json_export().unwrap_err();
        assert!(matches!(
            error,
            LibraryError::InvalidInput(message)
                if message.contains("models")
                    && message.contains("supported maximum of 5000")
        ));
    }

    #[test]
    fn printer_data_export_snapshot_and_ui_list_read_maintenance_beyond_ten_thousand() {
        let db = LibraryDb::open_in_memory().unwrap();
        let mut batch = PrinterDataImport::default();
        batch.printers.push(printer_data_import_printer(
            "export-printer",
            printer_input("Export P-1"),
        ));
        for index in 0..10_001 {
            batch
                .maintenance_logs
                .push(PrinterDataImportMaintenanceLog {
                    external_ref: format!("maintenance-{index}"),
                    printer_external_ref: Some("export-printer".into()),
                    printer_nickname: Some("Export P-1".into()),
                    input: PrinterMaintenanceInput {
                        title: "Service printer".into(),
                        performed_at: "2026-10-14T12:00:00Z".into(),
                        notes: String::new(),
                    },
                    created_at: None,
                });
            if index < 10_000 {
                batch.reminders.push(PrinterDataImportReminder {
                    external_ref: format!("reminder-{index}"),
                    printer_external_ref: Some("export-printer".into()),
                    printer_nickname: Some("Export P-1".into()),
                    input: printer_reminder_input("2026-11-01T00:00:00Z", None),
                    status: PrinterReminderStatus::Active,
                    last_completed_at: None,
                    created_at: "1700000000".into(),
                    updated_at: "1700000000".into(),
                });
            }
        }

        let imported = db.import_printer_data(&batch).unwrap();
        assert_eq!(imported.imported_maintenance_log_count, 10_001);
        assert_eq!(imported.imported_reminder_count, 10_000);
        assert_eq!(
            db.list_printer_maintenance_logs(None).unwrap().len(),
            10_001
        );
        assert_eq!(db.list_printer_reminders(None, true).unwrap().len(), 10_000);

        let snapshot = db.export_printer_data_snapshot().unwrap();

        assert_eq!(snapshot.printers.len(), 1);
        assert_eq!(snapshot.maintenance_logs.len(), 10_001);
        assert_eq!(snapshot.reminders.len(), 10_000);
        assert!(snapshot
            .maintenance_logs
            .windows(2)
            .all(|pair| pair[0].id < pair[1].id));
        assert!(snapshot
            .reminders
            .windows(2)
            .all(|pair| pair[0].id < pair[1].id));
    }

    #[test]
    fn native_and_imported_printer_web_urls_persist_only_the_origin() {
        let db = LibraryDb::open_in_memory().unwrap();
        let mut native_input = printer_input("Native URL P-1");
        native_input.web_url = Some(
            "https://printer.local:8443/ui/secret-token-value?token=query-secret#fragment-secret"
                .into(),
        );

        let native = db.create_printer(&native_input, false).unwrap();

        assert_eq!(
            native.web_url.as_deref(),
            Some("https://printer.local:8443")
        );
        let mut update = native_input.clone();
        update.nickname = "Native URL P-1 Updated".into();
        let updated = db
            .update_printer(&native.id, native.metadata_version, &update, None)
            .unwrap();
        assert_eq!(
            updated.web_url.as_deref(),
            Some("https://printer.local:8443")
        );

        let mut imported_input = printer_input("Imported URL P-1");
        imported_input.web_url = Some(
            "http://printer.local/maintenance/api-token-value?token=query-secret#fragment-secret"
                .into(),
        );
        let mut batch = PrinterDataImport::default();
        batch.printers.push(printer_data_import_printer(
            "source-web-printer",
            imported_input,
        ));
        let imported = db.import_printer_data(&batch).unwrap();
        let persisted = db
            .get_printer(&imported.printer_ref_map[0].local_printer_id)
            .unwrap()
            .unwrap();

        assert_eq!(persisted.web_url.as_deref(), Some("http://printer.local"));
        assert!(!persisted.web_url.as_deref().unwrap().contains("token"));
    }

    #[test]
    fn printer_history_writes_stop_at_global_limits_without_partial_changes() {
        let db = LibraryDb::open_in_memory().unwrap();
        let mut batch = PrinterDataImport::default();
        batch.printers.push(printer_data_import_printer(
            "capacity-printer",
            printer_input("Capacity P-1"),
        ));
        for index in 0..49_998 {
            batch
                .maintenance_logs
                .push(PrinterDataImportMaintenanceLog {
                    external_ref: format!("capacity-log-{index}"),
                    printer_external_ref: Some("capacity-printer".into()),
                    printer_nickname: Some("Capacity P-1".into()),
                    input: PrinterMaintenanceInput {
                        title: "Initial maintenance".into(),
                        performed_at: "2026-10-14T12:00:00Z".into(),
                        notes: String::new(),
                    },
                    created_at: None,
                });
        }
        for index in 0..9_998 {
            batch.reminders.push(PrinterDataImportReminder {
                external_ref: format!("capacity-reminder-{index}"),
                printer_external_ref: Some("capacity-printer".into()),
                printer_nickname: Some("Capacity P-1".into()),
                input: printer_reminder_input("2026-11-01T00:00:00Z", None),
                status: PrinterReminderStatus::Active,
                last_completed_at: None,
                created_at: "1700000000".into(),
                updated_at: "1700000000".into(),
            });
        }
        let imported = db.import_printer_data(&batch).unwrap();
        let printer_id = imported.printer_ref_map[0].local_printer_id.clone();

        db.create_printer_maintenance_log(
            &printer_id,
            &PrinterMaintenanceInput {
                title: "Last normal log slot".into(),
                performed_at: "2026-10-14T12:00:00Z".into(),
                notes: String::new(),
            },
        )
        .unwrap();
        let first_reminder = db
            .create_printer_reminder(
                &printer_id,
                &printer_reminder_input("2026-11-02T00:00:00Z", None),
            )
            .unwrap();
        let second_reminder = db
            .create_printer_reminder(
                &printer_id,
                &printer_reminder_input("2026-11-03T00:00:00Z", None),
            )
            .unwrap();
        let reminder_error = db
            .create_printer_reminder(
                &printer_id,
                &printer_reminder_input("2026-11-04T00:00:00Z", None),
            )
            .err()
            .unwrap();
        assert!(matches!(
            reminder_error,
            LibraryError::InvalidInput(message) if message.contains("reminder limit of 10000")
        ));

        let completion_input = PrinterReminderCompletionInput {
            completed_at: "2026-10-15T12:00:00Z".into(),
            notes: "Final log slot".into(),
        };
        db.complete_printer_reminder(
            &first_reminder.id,
            first_reminder.metadata_version,
            &completion_input,
        )
        .unwrap();
        let log_error = db
            .create_printer_maintenance_log(
                &printer_id,
                &PrinterMaintenanceInput {
                    title: "Over-cap normal log".into(),
                    performed_at: "2026-10-15T12:00:00Z".into(),
                    notes: String::new(),
                },
            )
            .err()
            .unwrap();
        assert!(matches!(
            log_error,
            LibraryError::InvalidInput(message) if message.contains("maintenance log limit of 50000")
        ));
        let completion_error = db
            .complete_printer_reminder(
                &second_reminder.id,
                second_reminder.metadata_version,
                &completion_input,
            )
            .err()
            .unwrap();
        assert!(matches!(
            completion_error,
            LibraryError::InvalidInput(message) if message.contains("maintenance log limit of 50000")
        ));

        let unchanged_reminder = db
            .get_printer_reminder(&second_reminder.id)
            .unwrap()
            .unwrap();
        assert_eq!(
            unchanged_reminder.metadata_version,
            second_reminder.metadata_version
        );
        assert_eq!(unchanged_reminder.status, PrinterReminderStatus::Active);
        assert_eq!(
            db.list_printer_maintenance_logs(None).unwrap().len(),
            50_000
        );
        assert_eq!(db.list_printer_reminders(None, true).unwrap().len(), 10_000);

        let mut over_cap_log_import = PrinterDataImport::default();
        over_cap_log_import.printers.push(PrinterDataImportPrinter {
            external_ref: "mapped-capacity-printer".into(),
            map_to_local_printer_id: Some(printer_id.clone()),
            created_at: None,
            updated_at: None,
            input: printer_input("Ignored mapped name"),
        });
        over_cap_log_import
            .maintenance_logs
            .push(PrinterDataImportMaintenanceLog {
                external_ref: "over-cap-log".into(),
                printer_external_ref: Some("mapped-capacity-printer".into()),
                printer_nickname: Some("Capacity P-1".into()),
                input: PrinterMaintenanceInput {
                    title: "Over-cap imported log".into(),
                    performed_at: "2026-10-15T12:00:00Z".into(),
                    notes: String::new(),
                },
                created_at: None,
            });
        let log_import_error = db.import_printer_data(&over_cap_log_import).err().unwrap();
        assert!(matches!(
            log_import_error,
            LibraryError::InvalidInput(message) if message.contains("maintenance log limit of 50000")
        ));

        let mut over_cap_reminder_import = PrinterDataImport::default();
        over_cap_reminder_import
            .printers
            .push(PrinterDataImportPrinter {
                external_ref: "mapped-capacity-printer".into(),
                map_to_local_printer_id: Some(printer_id),
                created_at: None,
                updated_at: None,
                input: printer_input("Ignored mapped name"),
            });
        over_cap_reminder_import
            .reminders
            .push(PrinterDataImportReminder {
                external_ref: "over-cap-reminder".into(),
                printer_external_ref: Some("mapped-capacity-printer".into()),
                printer_nickname: Some("Capacity P-1".into()),
                input: printer_reminder_input("2026-11-05T00:00:00Z", None),
                status: PrinterReminderStatus::Active,
                last_completed_at: None,
                created_at: "1700000000".into(),
                updated_at: "1700000000".into(),
            });
        let reminder_import_error = db
            .import_printer_data(&over_cap_reminder_import)
            .err()
            .unwrap();
        assert!(matches!(
            reminder_import_error,
            LibraryError::InvalidInput(message) if message.contains("reminder limit of 10000")
        ));
        assert_eq!(
            db.list_printer_maintenance_logs(None).unwrap().len(),
            50_000
        );
        assert_eq!(db.list_printer_reminders(None, true).unwrap().len(), 10_000);
    }

    #[test]
    fn printer_data_export_snapshot_rejects_history_over_its_cap() {
        let db = LibraryDb::open_in_memory().unwrap();
        let printer = db
            .create_printer(&printer_input("Export P-1"), false)
            .unwrap();
        let tx = db.conn.unchecked_transaction().unwrap();
        for index in 0..=MAX_PRINTER_JSON_EXPORT_REMINDERS {
            tx.execute(
                "INSERT INTO printer_reminders
                 (id,metadata_version,printer_id,printer_nickname,title,notes,due_at,
                  interval_days,status,last_completed_at,created_at,updated_at)
                 VALUES(?1,1,?2,?3,'Check printer','',?4,NULL,'active',NULL,?5,?5)",
                params![
                    format!("over-cap-reminder-{index}"),
                    printer.id.as_str(),
                    printer.nickname.as_str(),
                    "2026-11-01T00:00:00Z",
                    "1700000000",
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();

        let list_error = db.list_printer_reminders(None, true).unwrap_err();
        assert!(matches!(
            list_error,
            LibraryError::InvalidInput(message)
                if message.contains("supported maximum of 10000")
        ));

        let error = db.export_printer_data_snapshot().unwrap_err();

        assert!(matches!(
            error,
            LibraryError::InvalidInput(message)
                if message.contains("maintenance reminders")
                    && message.contains("supported maximum of 10000")
        ));
    }

    #[test]
    fn printer_data_import_rejects_invalid_and_duplicate_profile_refs_but_preserves_orphan_history()
    {
        let db = LibraryDb::open_in_memory().unwrap();
        let mut invalid_ref = PrinterDataImport::default();
        invalid_ref.printers.push(printer_data_import_printer(
            "bad/ref",
            printer_input("Invalid ref"),
        ));
        assert!(db.import_printer_data(&invalid_ref).is_err());
        assert!(db.list_printers().unwrap().is_empty());

        let mut duplicate_ref = PrinterDataImport::default();
        duplicate_ref.printers.extend([
            printer_data_import_printer("same-ref", printer_input("First")),
            printer_data_import_printer("same-ref", printer_input("Second")),
        ]);
        assert!(db.import_printer_data(&duplicate_ref).is_err());
        assert!(db.list_printers().unwrap().is_empty());

        let mut invalid_history_refs = PrinterDataImport::default();
        invalid_history_refs
            .maintenance_logs
            .push(PrinterDataImportMaintenanceLog {
                external_ref: "invalid-log-ref".into(),
                printer_external_ref: Some("bad/ref".into()),
                printer_nickname: Some("Historical Name".into()),
                input: PrinterMaintenanceInput {
                    title: "Clean nozzle".into(),
                    performed_at: "2026-10-01T12:00:00Z".into(),
                    notes: String::new(),
                },
                created_at: None,
            });
        invalid_history_refs
            .reminders
            .push(PrinterDataImportReminder {
                external_ref: "invalid-reminder-ref".into(),
                printer_external_ref: Some("bad/ref".into()),
                printer_nickname: Some("Historical Name".into()),
                input: printer_reminder_input("2026-11-01T12:00:00Z", None),
                status: PrinterReminderStatus::Active,
                last_completed_at: None,
                created_at: "2026-10-01T12:00:00Z".into(),
                updated_at: "2026-10-01T12:00:00Z".into(),
            });
        assert!(db.import_printer_data(&invalid_history_refs).is_err());
        assert!(db.list_printer_maintenance_logs(None).unwrap().is_empty());
        assert!(db.list_printer_reminders(None, true).unwrap().is_empty());

        let mut orphan_history = PrinterDataImport::default();
        orphan_history
            .maintenance_logs
            .push(PrinterDataImportMaintenanceLog {
                external_ref: "log-1".into(),
                printer_external_ref: Some("missing-printer".into()),
                printer_nickname: Some("Historical Name".into()),
                input: PrinterMaintenanceInput {
                    title: "Clean nozzle".into(),
                    performed_at: "2026-10-01T12:00:00Z".into(),
                    notes: String::new(),
                },
                created_at: None,
            });
        orphan_history.reminders.push(PrinterDataImportReminder {
            external_ref: "reminder-1".into(),
            printer_external_ref: Some("missing-printer".into()),
            printer_nickname: None,
            input: printer_reminder_input("2026-11-01T12:00:00Z", None),
            status: PrinterReminderStatus::Active,
            last_completed_at: None,
            created_at: "2026-10-01T12:00:00Z".into(),
            updated_at: "2026-10-01T12:00:00Z".into(),
        });

        let imported = db.import_printer_data(&orphan_history).unwrap();
        assert_eq!(imported.imported_printer_count, 0);
        assert_eq!(imported.imported_maintenance_log_count, 1);
        assert_eq!(imported.imported_reminder_count, 1);
        assert!(db.list_printers().unwrap().is_empty());
        let logs = db.list_printer_maintenance_logs(None).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].printer_id, None);
        assert_eq!(logs[0].printer_nickname, "Historical Name");
        let reminders = db.list_printer_reminders(None, true).unwrap();
        assert_eq!(reminders.len(), 1);
        assert_eq!(reminders[0].printer_id, None);
        assert_eq!(reminders[0].printer_nickname, "");
    }

    #[test]
    fn printer_data_import_accepts_null_history_profile_references() {
        let db = LibraryDb::open_in_memory().unwrap();
        let batch: PrinterDataImport = serde_json::from_value(serde_json::json!({
            "maintenanceLogs": [{
                "externalRef": "null-log",
                "printerExternalRef": null,
                "input": {
                    "title": "Clean nozzle",
                    "performedAt": "2026-10-01T12:00:00Z",
                    "notes": ""
                },
                "createdAt": "2026-10-01T12:00:00Z"
            }],
            "reminders": [{
                "externalRef": "null-reminder",
                "printerExternalRef": null,
                "printerNickname": "",
                "input": {
                    "title": "Inspect belts",
                    "notes": "",
                    "dueAt": "2026-11-01T12:00:00Z",
                    "intervalDays": null
                },
                "status": "active",
                "lastCompletedAt": null,
                "createdAt": "2026-10-01T12:00:00Z",
                "updatedAt": "2026-10-01T12:00:00Z"
            }]
        }))
        .unwrap();

        let imported = db.import_printer_data(&batch).unwrap();
        assert_eq!(imported.imported_printer_count, 0);
        let logs = db.list_printer_maintenance_logs(None).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].printer_id, None);
        assert_eq!(logs[0].printer_nickname, "");
        let reminders = db.list_printer_reminders(None, true).unwrap();
        assert_eq!(reminders.len(), 1);
        assert_eq!(reminders[0].printer_id, None);
        assert_eq!(reminders[0].printer_nickname, "");
        assert!(db.list_printers().unwrap().is_empty());
    }

    #[test]
    fn schema_v10_history_migration_preserves_rows_indexes_foreign_keys_and_backup() {
        let root = temp_root("schema-v10-history-names");
        let db = LibraryDb::open(&root).unwrap();
        let printer = db
            .create_printer(&printer_input("Migration history printer"), false)
            .unwrap();
        let log = db
            .create_printer_maintenance_log(
                &printer.id,
                &PrinterMaintenanceInput {
                    title: "Replace nozzle".into(),
                    performed_at: "2026-10-01T12:00:00Z".into(),
                    notes: "Preserve this log".into(),
                },
            )
            .unwrap();
        let reminder = db
            .create_printer_reminder(
                &printer.id,
                &PrinterReminderInput {
                    title: "Inspect belts".into(),
                    notes: "Preserve this reminder".into(),
                    due_at: "2026-11-01T12:00:00Z".into(),
                    interval_days: Some(30),
                },
            )
            .unwrap();
        drop(db);

        let connection = Connection::open(root.join("library.sqlite3")).unwrap();
        connection.pragma_update(None, "user_version", 10).unwrap();
        drop(connection);

        let migrated = LibraryDb::open(&root).unwrap();
        assert_eq!(
            migrated.get_printer_maintenance_log(&log.id).unwrap(),
            Some(log)
        );
        assert_eq!(
            migrated.get_printer_reminder(&reminder.id).unwrap(),
            Some(reminder)
        );
        assert_eq!(
            migrated
                .conn
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            11
        );
        let mut empty_orphans = PrinterDataImport::default();
        empty_orphans
            .maintenance_logs
            .push(PrinterDataImportMaintenanceLog {
                external_ref: "empty-unlinked-log".into(),
                printer_external_ref: None,
                printer_nickname: None,
                input: PrinterMaintenanceInput {
                    title: "Preserve empty history name".into(),
                    performed_at: "2026-10-02T12:00:00Z".into(),
                    notes: String::new(),
                },
                created_at: None,
            });
        empty_orphans.reminders.push(PrinterDataImportReminder {
            external_ref: "empty-unlinked-reminder".into(),
            printer_external_ref: None,
            printer_nickname: None,
            input: PrinterReminderInput {
                title: "Preserve empty reminder name".into(),
                notes: String::new(),
                due_at: "2026-11-02T12:00:00Z".into(),
                interval_days: None,
            },
            status: PrinterReminderStatus::Active,
            last_completed_at: None,
            created_at: "2026-10-02T12:00:00Z".into(),
            updated_at: "2026-10-02T12:00:00Z".into(),
        });
        let imported = migrated.import_printer_data(&empty_orphans).unwrap();
        assert_eq!(imported.imported_printer_count, 0);
        assert_eq!(migrated.list_printers().unwrap().len(), 1);
        let logs = migrated.list_printer_maintenance_logs(None).unwrap();
        let empty_log = logs
            .iter()
            .find(|record| record.title == "Preserve empty history name")
            .unwrap();
        assert_eq!(empty_log.printer_id, None);
        assert_eq!(empty_log.printer_nickname, "");
        let reminders = migrated.list_printer_reminders(None, true).unwrap();
        let empty_reminder = reminders
            .iter()
            .find(|record| record.title == "Preserve empty reminder name")
            .unwrap();
        assert_eq!(empty_reminder.printer_id, None);
        assert_eq!(empty_reminder.printer_nickname, "");
        drop(migrated);

        let connection = Connection::open(root.join("library.sqlite3")).unwrap();
        for index in [
            "printer_maintenance_printer_idx",
            "printer_reminders_due_idx",
            "printer_reminders_printer_idx",
        ] {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1)",
                    [index],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "migration dropped index {index}");
        }
        for table in ["printer_maintenance_logs", "printer_reminders"] {
            let mut statement = connection
                .prepare(&format!("PRAGMA foreign_key_list({table})"))
                .unwrap();
            let foreign_keys = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(6)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(foreign_keys.contains(&(
                "printers".into(),
                "printer_id".into(),
                "SET NULL".into()
            )));
        }
        drop(connection);

        let backup = Connection::open(root.join("library.sqlite3.schema-v10.backup")).unwrap();
        assert_eq!(
            backup
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            10
        );
    }

    #[test]
    fn printer_data_import_keeps_mapped_printers_unchanged_and_preserves_log_name() {
        let db = LibraryDb::open_in_memory().unwrap();
        let mut existing_input = printer_input("Local P-1");
        existing_input.host = Some("192.0.2.10".into());
        let existing = db.create_printer(&existing_input, true).unwrap();
        let mut imported_input = printer_input("Source Name Must Not Replace Local Name");
        imported_input.host = Some("192.0.2.12".into());
        let mut batch = PrinterDataImport::default();
        batch.printers.push(PrinterDataImportPrinter {
            external_ref: "source-printer".into(),
            map_to_local_printer_id: Some(existing.id.clone()),
            created_at: Some("1700000000".into()),
            updated_at: Some("1700000001".into()),
            input: imported_input,
        });
        batch
            .maintenance_logs
            .push(PrinterDataImportMaintenanceLog {
                external_ref: "source-log".into(),
                printer_external_ref: Some("source-printer".into()),
                printer_nickname: Some("Retired Historical Nickname".into()),
                input: PrinterMaintenanceInput {
                    title: "Replace nozzle".into(),
                    performed_at: "2026-10-02T12:00:00Z".into(),
                    notes: "Source history".into(),
                },
                created_at: Some("1700000000".into()),
            });

        let result = db.import_printer_data(&batch).unwrap();
        assert_eq!(result.imported_printer_count, 0);
        assert_eq!(result.mapped_printer_count, 1);
        assert_eq!(
            db.get_printer(&existing.id).unwrap().unwrap(),
            existing,
            "mapping an imported printer must not modify the local profile or approval"
        );
        let logs = db
            .list_printer_maintenance_logs(Some(&existing.id))
            .unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].printer_id.as_deref(), Some(existing.id.as_str()));
        assert_ne!(logs[0].id, "source-log");
        assert_eq!(logs[0].printer_nickname, "Retired Historical Nickname");
        assert_eq!(logs[0].created_at, "1700000000");
    }

    #[test]
    fn printer_data_import_round_trips_paused_and_completed_reminders() {
        let db = LibraryDb::open_in_memory().unwrap();
        let mut batch = PrinterDataImport::default();
        batch.printers.push(printer_data_import_printer(
            "source-printer",
            printer_input("Imported Reminder P-1"),
        ));
        batch.reminders.extend([
            PrinterDataImportReminder {
                external_ref: "paused-reminder".into(),
                printer_external_ref: Some("source-printer".into()),
                printer_nickname: Some("Historical paused printer name".into()),
                input: printer_reminder_input("2026-11-01T00:00:00Z", Some(14)),
                status: PrinterReminderStatus::Paused,
                last_completed_at: None,
                created_at: "1700000000".into(),
                updated_at: "1700000001".into(),
            },
            PrinterDataImportReminder {
                external_ref: "completed-reminder".into(),
                printer_external_ref: Some("source-printer".into()),
                printer_nickname: None,
                input: printer_reminder_input("2026-11-02T00:00:00Z", None),
                status: PrinterReminderStatus::Completed,
                last_completed_at: Some("2026-10-10T12:30:00Z".into()),
                created_at: "1700000002".into(),
                updated_at: "1700000003".into(),
            },
        ]);

        let result = db.import_printer_data(&batch).unwrap();
        assert_eq!(result.imported_reminder_count, 2);
        let printer_id = &result.printer_ref_map[0].local_printer_id;
        let reminders = db.list_printer_reminders(Some(printer_id), true).unwrap();
        let paused = reminders
            .iter()
            .find(|reminder| reminder.due_at.starts_with("2026-11-01"))
            .unwrap();
        assert_eq!(paused.status, PrinterReminderStatus::Paused);
        assert_eq!(paused.printer_nickname, "Historical paused printer name");
        assert_eq!(paused.last_completed_at, None);
        assert_eq!(paused.created_at, "1700000000");
        assert_eq!(paused.updated_at, "1700000001");

        let completed = reminders
            .iter()
            .find(|reminder| reminder.due_at.starts_with("2026-11-02"))
            .unwrap();
        assert_ne!(paused.id, "paused-reminder");
        assert_ne!(completed.id, "completed-reminder");
        assert_eq!(completed.status, PrinterReminderStatus::Completed);
        assert_eq!(completed.printer_id.as_deref(), Some(printer_id.as_str()));
        assert_eq!(completed.printer_nickname, "Imported Reminder P-1");
        assert_eq!(
            completed.last_completed_at.as_deref(),
            Some("2026-10-10T12:30:00.000Z")
        );
        assert_eq!(completed.created_at, "1700000002");
        assert_eq!(completed.updated_at, "1700000003");
    }

    #[test]
    fn printer_data_import_rolls_back_when_a_later_row_fails() {
        let db = LibraryDb::open_in_memory().unwrap();
        db.conn
            .execute_batch(
                "CREATE TRIGGER reject_imported_reminder
                 BEFORE INSERT ON printer_reminders
                 WHEN NEW.title='Reject this import row'
                 BEGIN SELECT RAISE(ABORT, 'test import row rejected'); END;",
            )
            .unwrap();
        let mut batch = PrinterDataImport::default();
        batch.printers.push(printer_data_import_printer(
            "source-printer",
            printer_input("Atomic import P-1"),
        ));
        batch
            .maintenance_logs
            .push(PrinterDataImportMaintenanceLog {
                external_ref: "source-log".into(),
                printer_external_ref: Some("source-printer".into()),
                printer_nickname: Some("Atomic import P-1".into()),
                input: PrinterMaintenanceInput {
                    title: "Valid earlier row".into(),
                    performed_at: "2026-10-02T12:00:00Z".into(),
                    notes: String::new(),
                },
                created_at: None,
            });
        batch.reminders.push(PrinterDataImportReminder {
            external_ref: "source-reminder".into(),
            printer_external_ref: Some("source-printer".into()),
            printer_nickname: None,
            input: PrinterReminderInput {
                title: "Reject this import row".into(),
                notes: String::new(),
                due_at: "2026-11-02T00:00:00Z".into(),
                interval_days: None,
            },
            status: PrinterReminderStatus::Active,
            last_completed_at: None,
            created_at: "1700000000".into(),
            updated_at: "1700000001".into(),
        });

        assert!(db.import_printer_data(&batch).is_err());
        assert!(db.list_printers().unwrap().is_empty());
        assert!(db.list_printer_maintenance_logs(None).unwrap().is_empty());
        assert!(db.list_printer_reminders(None, true).unwrap().is_empty());
    }

    #[test]
    fn schema_v6_migration_adds_printer_catalog_without_changing_existing_history() {
        let root = temp_root("schema-v6-printers");
        let db = LibraryDb::open(&root).unwrap();
        let model = db.create_model("Legacy print", None).unwrap();
        let attempt = db
            .create_print_attempt(&model.id, &print_attempt_input())
            .unwrap();
        downgrade_printer_schema_to_v6(&db.conn);
        drop(db);

        let migrated = LibraryDb::open(&root).unwrap();
        assert_eq!(
            migrated
                .conn
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            CURRENT_SCHEMA_VERSION
        );
        let loaded = migrated.get_print_attempt(&attempt.id).unwrap().unwrap();
        assert_eq!(loaded.printer_id, None);
        assert_eq!(loaded.printer_name, None);
        assert_eq!(loaded.printed_at, "2026-10-14T12:00:00.000Z");
        assert!(migrated
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='printers')",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap());
        let backup = root.join("library.sqlite3.schema-v6.backup");
        assert!(backup.is_file());
        assert_eq!(
            Connection::open(backup)
                .unwrap()
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            6
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn schema_v7_migration_adds_maintenance_versions_and_preserves_rows() {
        let root = temp_root("schema-v7-maintenance-version");
        let db = LibraryDb::open(&root).unwrap();
        let printer = db
            .create_printer(&printer_input("Versioned maintenance P-1"), false)
            .unwrap();
        let log = db
            .create_printer_maintenance_log(
                &printer.id,
                &PrinterMaintenanceInput {
                    title: "Clean nozzle".into(),
                    performed_at: "2026-10-14T12:00:00Z".into(),
                    notes: "Before migration".into(),
                },
            )
            .unwrap();
        downgrade_printer_maintenance_logs_to_v7(&db.conn);
        drop(db);

        let migrated = LibraryDb::open(&root).unwrap();
        assert_eq!(
            migrated
                .conn
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            CURRENT_SCHEMA_VERSION
        );
        let recovered = migrated
            .get_printer_maintenance_log(&log.id)
            .unwrap()
            .unwrap();
        assert_eq!(recovered.metadata_version, 1);
        assert_eq!(recovered.title, "Clean nozzle");
        assert_eq!(recovered.notes, "Before migration");
        let backup = root.join("library.sqlite3.schema-v7.backup");
        assert!(backup.is_file());
        let backup_db = Connection::open(backup).unwrap();
        assert_eq!(
            backup_db
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            7
        );
        assert_eq!(
            backup_db
                .query_row(
                    "SELECT COUNT(*) FROM printer_maintenance_logs WHERE id=?1",
                    params![log.id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        drop(migrated);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn printer_crud_checks_versions_and_bounded_fields() {
        let db = LibraryDb::open_in_memory().unwrap();
        let printer = db
            .create_printer(&printer_input("Workshop P-1"), false)
            .unwrap();
        assert_eq!(printer.metadata_version, 1);
        assert_eq!(printer.connection_status, PrinterConnectionStatus::Unknown);
        assert_eq!(printer.job_status, PrinterJobStatus::Unknown);
        assert_eq!(printer.source_kind, PrinterSourceKind::Manual);

        let updated = db
            .update_printer(
                &printer.id,
                printer.metadata_version,
                &printer_input("Workshop P-1 v2"),
                None,
            )
            .unwrap();
        assert_eq!(updated.metadata_version, 2);
        assert_eq!(updated.nickname, "Workshop P-1 v2");
        assert!(db
            .update_printer(
                &printer.id,
                printer.metadata_version,
                &printer_input("stale write"),
                None,
            )
            .is_err());

        let mut invalid = printer_input("bad dimensions");
        invalid.bed_mm = Some([220.0, f32::NAN, 250.0]);
        assert!(db.create_printer(&invalid, false).is_err());
        let invalid = printer_input("\u{0000}bad");
        assert!(db.create_printer(&invalid, false).is_err());
        let mut invalid = printer_input("bad notes");
        invalid.notes = "x".repeat(20_001);
        assert!(db.create_printer(&invalid, false).is_err());
        let mut credentials = printer_input("credentialed endpoint");
        credentials.web_url = Some("http://user:password@printer.local/".into());
        assert!(db.create_printer(&credentials, false).is_err());
        let mut unapproved = printer_input("unapproved endpoint");
        unapproved.host = Some("192.0.2.1".into());
        let unapproved = db.create_printer(&unapproved, false).unwrap();
        assert!(!unapproved.endpoint_approved);

        let mut approved_input = printer_input("approved endpoint");
        approved_input.host = Some("192.0.2.10".into());
        let approved = db.create_printer(&approved_input, true).unwrap();
        let mut changed_endpoint = approved_input.clone();
        changed_endpoint.host = Some("192.0.2.11".into());
        let changed = db
            .update_printer(
                &approved.id,
                approved.metadata_version,
                &changed_endpoint,
                None,
            )
            .unwrap();
        assert!(!changed.endpoint_approved);
        assert!(db
            .record_printer_status(
                &changed.id,
                PrinterConnectionStatus::Offline,
                PrinterJobStatus::Unknown,
                "2026-10-14T12:02:00Z",
                None,
            )
            .is_err());
        let reapproved = db
            .update_printer(
                &changed.id,
                changed.metadata_version,
                &changed_endpoint,
                Some(true),
            )
            .unwrap();
        assert!(reapproved.endpoint_approved);

        assert!(db.list_printers().unwrap().contains(&updated));
        assert_eq!(db.list_printers().unwrap().len(), 3);
        assert!(db
            .delete_printer(&printer.id, printer.metadata_version)
            .is_err());
        db.delete_printer(&unapproved.id, unapproved.metadata_version)
            .unwrap();
        db.delete_printer(&reapproved.id, reapproved.metadata_version)
            .unwrap();
        db.delete_printer(&updated.id, updated.metadata_version)
            .unwrap();
        assert!(db.get_printer(&printer.id).unwrap().is_none());
    }

    #[test]
    fn part_stock_crud_uses_versions_and_matches_printventory_fields() {
        let db = LibraryDb::open_in_memory().unwrap();
        let input = PartStockInput {
            name: "M3x8 screw".into(),
            category: Some("Screws".into()),
            quantity: 20,
            unit: "pcs".into(),
            notes: "Black oxide".into(),
            low_stock: 4,
        };

        let created = db.create_part_stock(&input).unwrap();
        assert_eq!(created.metadata_version, 1);
        assert_eq!(created.name, "M3x8 screw");
        assert_eq!(created.category.as_deref(), Some("Screws"));
        assert_eq!(created.quantity, 20);
        assert_eq!(created.unit, "pcs");
        assert_eq!(created.notes, "Black oxide");
        assert_eq!(created.low_stock, 4);

        let updated_input = PartStockInput {
            quantity: 16,
            low_stock: 6,
            ..input.clone()
        };
        let updated = db
            .update_part_stock(&created.id, created.metadata_version, &updated_input)
            .unwrap();
        assert_eq!(updated.metadata_version, 2);
        assert_eq!(updated.quantity, 16);
        assert_eq!(updated.low_stock, 6);
        assert!(matches!(
            db.update_part_stock(&created.id, created.metadata_version, &input),
            Err(LibraryError::PartStockVersionConflict {
                part_id,
                expected: 1,
                actual: 2,
            }) if part_id == created.id
        ));
        assert_eq!(db.list_part_stock().unwrap(), vec![updated.clone()]);

        let invalid = PartStockInput {
            quantity: 1_000_001,
            ..updated_input
        };
        assert!(db
            .update_part_stock(&updated.id, updated.metadata_version, &invalid)
            .is_err());
        assert!(matches!(
            db.delete_part_stock(&updated.id, created.metadata_version),
            Err(LibraryError::PartStockVersionConflict {
                part_id,
                expected: 1,
                actual: 2,
            }) if part_id == updated.id
        ));
        db.delete_part_stock(&updated.id, updated.metadata_version)
            .unwrap();
        assert!(db.get_part_stock(&updated.id).unwrap().is_none());
    }

    #[test]
    fn print_attempt_parts_consume_per_copy_for_failed_events_and_restore_on_delete() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Failed print with hardware", None).unwrap();
        let part = db
            .create_part_stock(&PartStockInput {
                name: "M3 nut".into(),
                category: Some("Nuts".into()),
                quantity: 20,
                unit: "pcs".into(),
                notes: String::new(),
                low_stock: 3,
            })
            .unwrap();
        let mut input = print_attempt_input();
        input.outcome = PrintOutcome::Failed;
        input.quantity = 2;
        input.parts = Some(vec![PartUsageInput {
            id: part.id.clone(),
            quantity: 4,
            snapshot: None,
        }]);

        let event = db.create_print_attempt(&model.id, &input).unwrap();
        assert_eq!(db.get_part_stock(&part.id).unwrap().unwrap().quantity, 12);
        assert_eq!(event.parts.len(), 1);
        assert_eq!(event.parts[0].quantity_per_copy, 4);
        assert_eq!(event.parts[0].quantity, 8);
        assert_eq!(event.parts[0].name, "M3 nut");

        db.delete_print_attempt(&event.id).unwrap();
        assert_eq!(db.get_part_stock(&part.id).unwrap().unwrap().quantity, 20);
        assert!(db.get_print_attempt(&event.id).unwrap().is_none());
    }

    #[test]
    fn deleting_imported_history_does_not_refund_a_current_stock_snapshot() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Imported stock snapshot", None).unwrap();
        let part = db
            .create_part_stock(&PartStockInput {
                name: "M4 nut".into(),
                category: Some("Hardware".into()),
                quantity: 10,
                unit: "pcs".into(),
                notes: String::new(),
                low_stock: 2,
            })
            .unwrap();
        let mut input = print_attempt_input();
        input.quantity = 2;
        input.parts = Some(vec![PartUsageInput {
            id: part.id.clone(),
            quantity: 3,
            snapshot: None,
        }]);

        let imported = db
            .import_print_attempt_with_printer(&model.id, &input, None, None, None)
            .unwrap();
        assert_eq!(db.get_part_stock(&part.id).unwrap().unwrap().quantity, 10);
        assert_eq!(imported.parts[0].quantity, 6);

        let mut edited = print_attempt_input();
        edited.notes = "reviewed historical note".into();
        let updated = db.update_print_attempt(&imported.id, &edited).unwrap();
        assert_eq!(updated.parts[0].quantity, 3);
        assert_eq!(db.get_part_stock(&part.id).unwrap().unwrap().quantity, 10);

        db.delete_print_attempt(&imported.id).unwrap();
        assert_eq!(db.get_part_stock(&part.id).unwrap().unwrap().quantity, 10);
    }

    #[test]
    fn imported_orphan_part_snapshot_never_links_or_refunds_replacement_stock() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Orphaned imported part", None).unwrap();
        let replacement = db
            .create_part_stock(&PartStockInput {
                name: "M5 bolt".into(),
                category: Some("Hardware".into()),
                quantity: 12,
                unit: "pcs".into(),
                notes: String::new(),
                low_stock: 2,
            })
            .unwrap();
        let orphan_id = format!("{PRINTVENTORY_ORPHAN_PART_ID_PREFIX}abcdef0123456789");
        let mut input = print_attempt_input();
        input.parts = Some(vec![PartUsageInput {
            id: orphan_id.clone(),
            quantity: 2,
            snapshot: Some(PartUsageSnapshot {
                name: "M5 bolt".into(),
                category: Some("Hardware".into()),
                unit: "pcs".into(),
            }),
        }]);

        assert!(db.create_print_attempt(&model.id, &input).is_err());
        let imported = db
            .import_print_attempt_with_printer(&model.id, &input, None, None, None)
            .unwrap();
        assert_eq!(imported.parts.len(), 1);
        assert_eq!(imported.parts[0].id.as_deref(), Some(orphan_id.as_str()));
        assert_eq!(imported.parts[0].name, "M5 bolt");
        assert_eq!(imported.parts[0].quantity, 2);
        assert_eq!(
            db.get_part_stock(&replacement.id)
                .unwrap()
                .unwrap()
                .quantity,
            12
        );

        db.delete_print_attempt(&imported.id).unwrap();
        assert_eq!(
            db.get_part_stock(&replacement.id)
                .unwrap()
                .unwrap()
                .quantity,
            12
        );
    }

    #[test]
    fn cancelled_print_attempt_is_queued_not_counted_but_still_consumes_parts() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Cancelled print", None).unwrap();
        let part = db
            .create_part_stock(&PartStockInput {
                name: "Bearing".into(),
                category: Some("Bearings".into()),
                quantity: 5,
                unit: "pcs".into(),
                notes: String::new(),
                low_stock: 0,
            })
            .unwrap();
        let mut input = print_attempt_input();
        input.outcome = PrintOutcome::Cancelled;
        input.quantity = 2;
        input.parts = Some(vec![PartUsageInput {
            id: part.id.clone(),
            quantity: 1,
            snapshot: None,
        }]);

        let event = db.create_print_attempt(&model.id, &input).unwrap();
        let updated_model = db.get_model(&model.id).unwrap().unwrap();
        assert_eq!(event.outcome, PrintOutcome::Cancelled);
        assert_eq!(updated_model.metadata.print_status, PrintStatus::Queued);
        assert_eq!(updated_model.metadata.print_count, 0);
        assert_eq!(db.get_part_stock(&part.id).unwrap().unwrap().quantity, 3);
    }

    #[test]
    fn print_attempt_edit_applies_only_part_stock_delta_and_rolls_back_short_stock() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Edited hardware event", None).unwrap();
        let screw = db
            .create_part_stock(&PartStockInput {
                name: "M3 screw".into(),
                category: Some("Screws".into()),
                quantity: 20,
                unit: "pcs".into(),
                notes: String::new(),
                low_stock: 0,
            })
            .unwrap();
        let bearing = db
            .create_part_stock(&PartStockInput {
                name: "608 bearing".into(),
                category: Some("Bearings".into()),
                quantity: 10,
                unit: "pcs".into(),
                notes: String::new(),
                low_stock: 0,
            })
            .unwrap();
        let mut input = print_attempt_input();
        input.quantity = 3;
        input.parts = Some(vec![PartUsageInput {
            id: screw.id.clone(),
            quantity: 2,
            snapshot: None,
        }]);
        let event = db.create_print_attempt(&model.id, &input).unwrap();
        assert_eq!(db.get_part_stock(&screw.id).unwrap().unwrap().quantity, 14);

        let mut note_only = print_attempt_input();
        note_only.notes = "edit notes, keep inventory links".into();
        let preserved = db.update_print_attempt(&event.id, &note_only).unwrap();
        assert_eq!(db.get_part_stock(&screw.id).unwrap().unwrap().quantity, 18);
        assert_eq!(preserved.parts.len(), 1);
        assert_eq!(preserved.parts[0].quantity_per_copy, 2);
        assert_eq!(preserved.parts[0].quantity, 2);

        let mut edited = input.clone();
        edited.quantity = 1;
        edited.parts = Some(vec![
            PartUsageInput {
                id: screw.id.clone(),
                quantity: 1,
                snapshot: None,
            },
            PartUsageInput {
                id: bearing.id.clone(),
                quantity: 3,
                snapshot: None,
            },
        ]);
        let updated = db.update_print_attempt(&event.id, &edited).unwrap();
        assert_eq!(db.get_part_stock(&screw.id).unwrap().unwrap().quantity, 19);
        assert_eq!(db.get_part_stock(&bearing.id).unwrap().unwrap().quantity, 7);
        assert_eq!(updated.parts.len(), 2);

        let mut insufficient = edited.clone();
        insufficient.parts.as_mut().unwrap()[1].quantity = 20;
        assert!(db.update_print_attempt(&event.id, &insufficient).is_err());
        assert_eq!(db.get_part_stock(&screw.id).unwrap().unwrap().quantity, 19);
        assert_eq!(db.get_part_stock(&bearing.id).unwrap().unwrap().quantity, 7);
        assert_eq!(db.get_print_attempt(&event.id).unwrap().unwrap(), updated);

        let mut cleared = edited.clone();
        cleared.parts = Some(Vec::new());
        let cleared = db.update_print_attempt(&event.id, &cleared).unwrap();
        assert!(cleared.parts.is_empty());
        assert_eq!(db.get_part_stock(&screw.id).unwrap().unwrap().quantity, 20);
        assert_eq!(
            db.get_part_stock(&bearing.id).unwrap().unwrap().quantity,
            10
        );
        db.delete_print_attempt(&event.id).unwrap();
        assert_eq!(db.get_part_stock(&screw.id).unwrap().unwrap().quantity, 20);
        assert_eq!(
            db.get_part_stock(&bearing.id).unwrap().unwrap().quantity,
            10
        );
    }

    #[test]
    fn deleting_part_preserves_event_snapshot_without_restoring_replacement_stock() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Deleted hardware", None).unwrap();
        let original = db
            .create_part_stock(&PartStockInput {
                name: "Heat-set insert".into(),
                category: Some("Inserts".into()),
                quantity: 10,
                unit: "pcs".into(),
                notes: String::new(),
                low_stock: 0,
            })
            .unwrap();
        let mut input = print_attempt_input();
        input.outcome = PrintOutcome::Failed;
        input.parts = Some(vec![PartUsageInput {
            id: original.id.clone(),
            quantity: 2,
            snapshot: None,
        }]);
        let event = db.create_print_attempt(&model.id, &input).unwrap();
        let after_consume = db.get_part_stock(&original.id).unwrap().unwrap();
        db.delete_part_stock(&original.id, after_consume.metadata_version)
            .unwrap();
        let replacement = db
            .create_part_stock(&PartStockInput {
                name: "Heat-set insert".into(),
                category: Some("Inserts".into()),
                quantity: 5,
                unit: "pcs".into(),
                notes: String::new(),
                low_stock: 0,
            })
            .unwrap();
        let historical = db.get_print_attempt(&event.id).unwrap().unwrap();
        assert_eq!(historical.parts[0].name, "Heat-set insert");
        assert_eq!(
            historical.parts[0].id.as_deref(),
            Some(original.id.as_str())
        );
        db.delete_print_attempt(&event.id).unwrap();
        assert_eq!(
            db.get_part_stock(&replacement.id)
                .unwrap()
                .unwrap()
                .quantity,
            5
        );
    }

    #[test]
    fn schema_v8_migration_preserves_catalog_history_and_creates_backup() {
        let root = temp_root("schema-v8-parts-stock");
        let db = LibraryDb::open(&root).unwrap();
        let model = db.create_model("Parts migration", None).unwrap();
        let printer = db
            .create_printer(&printer_input("Migration printer"), false)
            .unwrap();
        let event = db
            .create_print_attempt_with_printer(
                &model.id,
                &PrintAttemptInputWithPrinter {
                    input: print_attempt_input(),
                    printer_id: Some(Some(printer.id.clone())),
                },
            )
            .unwrap();
        let model_after_event = db.get_model(&model.id).unwrap().unwrap();
        let printer_after_event = db.get_printer(&printer.id).unwrap().unwrap();
        drop(db);

        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute_batch(
            "DROP TABLE print_attempt_parts;
             DROP TABLE parts_stock;
             ALTER TABLE print_attempts DROP COLUMN stock_adjusted;
             PRAGMA user_version=8;",
        )
        .unwrap();
        drop(conn);

        let migrated = LibraryDb::open(&root).unwrap();
        assert_eq!(
            migrated.get_model(&model.id).unwrap().unwrap(),
            model_after_event
        );
        assert_eq!(
            migrated.get_printer(&printer.id).unwrap().unwrap(),
            printer_after_event
        );
        let mut expected_event = event.clone();
        expected_event.stock_adjusted = None;
        assert_eq!(
            migrated.get_print_attempt(&event.id).unwrap().unwrap(),
            expected_event
        );
        assert!(migrated.list_part_stock().unwrap().is_empty());
        assert_eq!(
            migrated
                .conn
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            CURRENT_SCHEMA_VERSION
        );
        drop(migrated);

        let backup = Connection::open(root.join("library.sqlite3.schema-v8.backup")).unwrap();
        assert_eq!(
            backup
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            8
        );
        assert_eq!(
            backup
                .query_row("SELECT COUNT(*) FROM print_attempts", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn schema_v9_migration_marks_every_event_unknown_without_guessing_stock_history() {
        let root = temp_root("schema-v9-stock-adjustment-marker");
        let db = LibraryDb::open(&root).unwrap();
        let model = db.create_model("Schema v9 stock marker", None).unwrap();
        let part = db
            .create_part_stock(&PartStockInput {
                name: "M2 spacer".into(),
                category: Some("Hardware".into()),
                quantity: 10,
                unit: "pcs".into(),
                notes: String::new(),
                low_stock: 2,
            })
            .unwrap();
        let mut input = print_attempt_input();
        input.parts = Some(vec![PartUsageInput {
            id: part.id.clone(),
            quantity: 2,
            snapshot: None,
        }]);
        let event = db.create_print_attempt(&model.id, &input).unwrap();
        assert_eq!(db.get_part_stock(&part.id).unwrap().unwrap().quantity, 8);
        let historical = db
            .import_print_attempt(&model.id, &print_attempt_input(), "2026-10-16T12:00:00Z")
            .unwrap();
        drop(db);

        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute_batch(
            "ALTER TABLE print_attempts DROP COLUMN stock_adjusted;
             PRAGMA user_version=9;",
        )
        .unwrap();
        drop(conn);

        let migrated = LibraryDb::open(&root).unwrap();
        let mut expected_event = event.clone();
        expected_event.stock_adjusted = None;
        assert_eq!(
            migrated.get_print_attempt(&event.id).unwrap().unwrap(),
            expected_event
        );
        assert_eq!(
            migrated
                .conn
                .query_row(
                    "SELECT stock_adjusted FROM print_attempts WHERE id=?1",
                    params![event.id],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .unwrap(),
            None
        );
        assert_eq!(
            migrated
                .get_print_attempt(&historical.id)
                .unwrap()
                .unwrap()
                .stock_adjusted,
            None
        );
        let mut historical_edit = print_attempt_input();
        historical_edit.parts = Some(vec![PartUsageInput {
            id: part.id.clone(),
            quantity: 1,
            snapshot: None,
        }]);
        let updated_historical = migrated
            .update_print_attempt(&historical.id, &historical_edit)
            .unwrap();
        assert_eq!(updated_historical.stock_adjusted, None);
        assert_eq!(
            migrated.get_part_stock(&part.id).unwrap().unwrap().quantity,
            8
        );
        migrated.delete_print_attempt(&historical.id).unwrap();
        migrated.delete_print_attempt(&event.id).unwrap();
        assert_eq!(
            migrated.get_part_stock(&part.id).unwrap().unwrap().quantity,
            8
        );
        assert!(root.join("library.sqlite3.schema-v9.backup").is_file());
        drop(migrated);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovered_printer_batch_is_atomic_and_keeps_per_item_approval() {
        let db = LibraryDb::open_in_memory().unwrap();
        let mut approved_input = printer_input("Discovered approved");
        approved_input.host = Some("192.168.50.20".into());
        let unapproved_input = printer_input("Discovered unapproved");

        let created = db
            .create_discovered_printers(&[(approved_input, true), (unapproved_input, false)])
            .unwrap();

        assert_eq!(created.len(), 2);
        assert_eq!(created[0].source_kind, PrinterSourceKind::Discovered);
        assert!(created[0].endpoint_approved);
        assert!(!created[1].endpoint_approved);
        assert_eq!(db.list_printers().unwrap().len(), 2);

        let mut invalid_input = printer_input("Invalid discovered");
        invalid_input.bed_mm = Some([220.0, 0.0, 250.0]);
        assert!(db
            .create_discovered_printers(&[
                (printer_input("Would roll back"), false),
                (invalid_input, false),
            ])
            .is_err());
        assert_eq!(db.list_printers().unwrap().len(), 2);
    }

    #[test]
    fn discovered_printer_batch_has_a_small_explicit_limit() {
        let db = LibraryDb::open_in_memory().unwrap();
        let items = (0..=MAX_DISCOVERED_PRINTER_BATCH)
            .map(|index| (printer_input(&format!("Discovered {index}")), false))
            .collect::<Vec<_>>();

        assert!(db.create_discovered_printers(&items).is_err());
        assert!(db.list_printers().unwrap().is_empty());
    }

    #[test]
    fn legacy_settings_printer_import_is_idempotent_by_id_and_preserves_edits() {
        let db = LibraryDb::open_in_memory().unwrap();
        let legacy = vec![
            LegacyPrinter {
                id: "legacy-a".into(),
                name: "Old printer".into(),
                bed_mm: [256.0, 256.0, 300.0],
            },
            LegacyPrinter {
                id: "legacy-b".into(),
                name: "Second printer".into(),
                bed_mm: [200.0, 200.0, 200.0],
            },
        ];
        let first = db.import_legacy_printers(&legacy).unwrap();
        assert_eq!(first.inserted, 2);
        assert_eq!(first.already_present, 0);
        let edited = db
            .update_printer("legacy-a", 1, &printer_input("Renamed by user"), None)
            .unwrap();

        let second = db.import_legacy_printers(&legacy).unwrap();
        assert_eq!(second.inserted, 0);
        assert_eq!(second.already_present, 2);
        assert_eq!(db.get_printer("legacy-a").unwrap().unwrap(), edited);

        let mut invalid = legacy.clone();
        invalid[1].id = "legacy-a".into();
        assert!(db.import_legacy_printers(&invalid).is_err());
        assert_eq!(db.list_printers().unwrap().len(), 2);
    }

    #[test]
    fn legacy_settings_printer_import_accepts_5000_and_rejects_5001_atomically() {
        let db = LibraryDb::open_in_memory().unwrap();
        let legacy = (0..MAX_PRINTER_IMPORT_COUNT)
            .map(|index| LegacyPrinter {
                id: format!("legacy-boundary-{index}"),
                name: format!("Legacy boundary printer {index}"),
                bed_mm: [220.0, 220.0, 250.0],
            })
            .collect::<Vec<_>>();

        let imported = db.import_legacy_printers(&legacy).unwrap();
        assert_eq!(imported.inserted, MAX_PRINTER_IMPORT_COUNT);
        assert_eq!(imported.already_present, 0);

        let retried = db.import_legacy_printers(&legacy).unwrap();
        assert_eq!(retried.inserted, 0);
        assert_eq!(retried.already_present, MAX_PRINTER_IMPORT_COUNT);
        assert_eq!(db.list_printers().unwrap().len(), MAX_PRINTER_IMPORT_COUNT);

        let mut over_cap = legacy;
        over_cap.push(LegacyPrinter {
            id: "legacy-boundary-over-cap".into(),
            name: "Over-cap printer".into(),
            bed_mm: [220.0, 220.0, 250.0],
        });
        let error = db.import_legacy_printers(&over_cap).unwrap_err();
        assert!(matches!(
            error,
            LibraryError::InvalidInput(message)
                if message.contains("legacy printer import exceeds")
        ));
        assert_eq!(db.list_printers().unwrap().len(), MAX_PRINTER_IMPORT_COUNT);
    }

    #[test]
    fn printer_status_separates_connectivity_from_job_state_and_bounds_codes() {
        let db = LibraryDb::open_in_memory().unwrap();
        let mut status_input = printer_input("Status P-1");
        status_input.host = Some("192.0.2.20".into());
        let printer = db.create_printer(&status_input, true).unwrap();
        assert!(printer.endpoint_approved);
        let checked = db
            .record_printer_status(
                &printer.id,
                PrinterConnectionStatus::Online,
                PrinterJobStatus::Printing,
                "2026-10-14T12:00:00Z",
                None,
            )
            .unwrap();
        assert_eq!(checked.connection_status, PrinterConnectionStatus::Online);
        assert_eq!(checked.job_status, PrinterJobStatus::Printing);
        assert_eq!(
            checked.last_checked_at.as_deref(),
            Some("2026-10-14T12:00:00.000Z")
        );

        assert!(db
            .record_printer_status(
                &printer.id,
                PrinterConnectionStatus::Offline,
                PrinterJobStatus::Unknown,
                "2026-10-14T12:01:00Z",
                Some(&"x".repeat(65)),
            )
            .is_err());
        let unchanged = db.get_printer(&printer.id).unwrap().unwrap();
        assert_eq!(unchanged.connection_status, PrinterConnectionStatus::Online);
        assert_eq!(unchanged.job_status, PrinterJobStatus::Printing);

        let mut changed_endpoint = status_input;
        changed_endpoint.host = Some("192.0.2.21".into());
        let changed = db
            .update_printer(
                &printer.id,
                unchanged.metadata_version,
                &changed_endpoint,
                None,
            )
            .unwrap();
        assert!(!changed.endpoint_approved);
        assert_eq!(changed.connection_status, PrinterConnectionStatus::Unknown);
        assert_eq!(changed.job_status, PrinterJobStatus::Unknown);
        assert_eq!(changed.last_checked_at, None);
        assert!(db
            .record_printer_status(
                &changed.id,
                PrinterConnectionStatus::Online,
                PrinterJobStatus::Idle,
                "2026-10-14T12:03:00Z",
                None,
            )
            .is_err());
    }

    #[test]
    fn printer_status_commit_rejects_an_endpoint_changed_during_the_check() {
        let db = LibraryDb::open_in_memory().unwrap();
        let mut input = printer_input("Concurrent status P-1");
        input.host = Some("192.0.2.30".into());
        let before_check = db.create_printer(&input, true).unwrap();

        let mut replacement = input;
        replacement.host = Some("192.0.2.31".into());
        let changed = db
            .update_printer(
                &before_check.id,
                before_check.metadata_version,
                &replacement,
                None,
            )
            .unwrap();
        assert!(!changed.endpoint_approved);

        assert!(db
            .record_printer_status_if_current(
                &before_check,
                PrinterConnectionStatus::Online,
                PrinterJobStatus::Idle,
                "2026-10-14T12:02:00Z",
                None,
            )
            .is_err());
        let persisted = db.get_printer(&before_check.id).unwrap().unwrap();
        assert_eq!(persisted.host.as_deref(), Some("192.0.2.31"));
        assert_eq!(
            persisted.connection_status,
            PrinterConnectionStatus::Unknown
        );
        assert_eq!(persisted.job_status, PrinterJobStatus::Unknown);
        assert_eq!(persisted.last_checked_at, None);
    }

    #[test]
    fn reminder_completion_and_reschedule_are_atomic_and_write_a_maintenance_log() {
        let db = LibraryDb::open_in_memory().unwrap();
        let printer = db
            .create_printer(&printer_input("Reminder P-1"), false)
            .unwrap();
        let reminder = db
            .create_printer_reminder(
                &printer.id,
                &printer_reminder_input("2026-10-14T00:00:00Z", Some(7)),
            )
            .unwrap();

        db.conn
            .execute_batch(
                "CREATE TRIGGER fail_phase8_maintenance_log
                 BEFORE INSERT ON printer_maintenance_logs
                 BEGIN SELECT RAISE(ABORT,'synthetic log failure'); END;",
            )
            .unwrap();
        let completion_input = PrinterReminderCompletionInput {
            completed_at: "2026-10-14T12:00:00Z".into(),
            notes: "Nozzle cleaned".into(),
        };
        assert!(db
            .complete_printer_reminder(&reminder.id, reminder.metadata_version, &completion_input)
            .is_err());
        let rolled_back = db.get_printer_reminder(&reminder.id).unwrap().unwrap();
        assert_eq!(rolled_back.metadata_version, reminder.metadata_version);
        assert_eq!(rolled_back.due_at, reminder.due_at);
        assert!(db
            .list_printer_maintenance_logs(Some(&printer.id))
            .unwrap()
            .is_empty());
        db.conn
            .execute_batch("DROP TRIGGER fail_phase8_maintenance_log")
            .unwrap();

        let completion = db
            .complete_printer_reminder(&reminder.id, reminder.metadata_version, &completion_input)
            .unwrap();
        assert_eq!(completion.reminder.due_at, "2026-10-21T12:00:00.000Z");
        assert_eq!(completion.reminder.metadata_version, 2);
        assert_eq!(
            completion.maintenance_log.printer_nickname,
            printer.nickname
        );
        assert_eq!(completion.maintenance_log.title, reminder.title);
        assert_eq!(
            db.list_printer_maintenance_logs(Some(&printer.id))
                .unwrap()
                .len(),
            1
        );
        assert!(db
            .complete_printer_reminder(
                &reminder.id,
                reminder.metadata_version,
                &PrinterReminderCompletionInput {
                    completed_at: "not-a-date".into(),
                    notes: "Must roll back".into(),
                },
            )
            .is_err());
        assert_eq!(
            db.list_printer_maintenance_logs(Some(&printer.id))
                .unwrap()
                .len(),
            1
        );

        let rescheduled = db
            .reschedule_printer_reminder(
                &reminder.id,
                completion.reminder.metadata_version,
                "2026-11-01T00:00:00Z",
            )
            .unwrap();
        assert_eq!(rescheduled.due_at, "2026-11-01T00:00:00.000Z");
        assert_eq!(rescheduled.metadata_version, 3);
        assert!(db
            .reschedule_printer_reminder(
                &reminder.id,
                completion.reminder.metadata_version,
                "invalid"
            )
            .is_err());
        assert_eq!(
            db.list_printer_maintenance_logs(Some(&printer.id))
                .unwrap()
                .len(),
            1
        );

        let edited = db
            .update_printer_reminder(
                &reminder.id,
                rescheduled.metadata_version,
                &PrinterReminderInput {
                    title: "Nozzle inspection".into(),
                    notes: "Updated note".into(),
                    due_at: "2026-11-02T00:00:00Z".into(),
                    interval_days: Some(14),
                },
            )
            .unwrap();
        assert_eq!(edited.metadata_version, 4);
        assert_eq!(edited.title, "Nozzle inspection");
        let paused = db
            .set_printer_reminder_paused(&reminder.id, edited.metadata_version, true)
            .unwrap();
        assert_eq!(paused.status, PrinterReminderStatus::Paused);
        let resumed = db
            .set_printer_reminder_paused(&reminder.id, paused.metadata_version, false)
            .unwrap();
        assert_eq!(resumed.status, PrinterReminderStatus::Active);
        db.delete_printer_reminder(&reminder.id, resumed.metadata_version)
            .unwrap();
        assert!(db.get_printer_reminder(&reminder.id).unwrap().is_none());
    }

    #[test]
    fn printer_maintenance_log_crud_preserves_historical_printer_snapshot() {
        let db = LibraryDb::open_in_memory().unwrap();
        let printer = db
            .create_printer(&printer_input("Maintenance P-1"), false)
            .unwrap();
        let log = db
            .create_printer_maintenance_log(
                &printer.id,
                &PrinterMaintenanceInput {
                    title: "Replace nozzle".into(),
                    performed_at: "2026-10-14T12:00:00Z".into(),
                    notes: "Old nozzle removed".into(),
                },
            )
            .unwrap();
        assert_eq!(log.metadata_version, 1);
        let updated = db
            .update_printer_maintenance_log(
                &log.id,
                log.metadata_version,
                &PrinterMaintenanceInput {
                    title: "Replace nozzle and clean".into(),
                    performed_at: "2026-10-15T12:00:00Z".into(),
                    notes: "Completed".into(),
                },
            )
            .unwrap();
        assert_eq!(updated.metadata_version, 2);
        assert!(matches!(
            db.update_printer_maintenance_log(
                &log.id,
                log.metadata_version,
                &PrinterMaintenanceInput {
                    title: "Stale edit".into(),
                    performed_at: "2026-10-16T12:00:00Z".into(),
                    notes: String::new(),
                },
            ),
            Err(LibraryError::InvalidInput(message))
                if message.contains("changed since it was read")
        ));
        assert!(matches!(
            db.delete_printer_maintenance_log(&updated.id, log.metadata_version),
            Err(LibraryError::InvalidInput(message))
                if message.contains("changed since it was read")
        ));
        db.delete_printer(&printer.id, printer.metadata_version)
            .unwrap();
        assert!(matches!(
            db.update_printer_maintenance_log(
                &updated.id,
                updated.metadata_version,
                &PrinterMaintenanceInput {
                    title: "Stale after printer deletion".into(),
                    performed_at: "2026-10-16T12:00:00Z".into(),
                    notes: String::new(),
                },
            ),
            Err(LibraryError::InvalidInput(message))
                if message.contains("changed since it was read")
        ));
        assert!(matches!(
            db.delete_printer_maintenance_log(&updated.id, updated.metadata_version),
            Err(LibraryError::InvalidInput(message))
                if message.contains("changed since it was read")
        ));
        let historical = db
            .get_printer_maintenance_log(&updated.id)
            .unwrap()
            .unwrap();
        assert_eq!(historical.printer_id, None);
        assert_eq!(historical.metadata_version, updated.metadata_version + 1);
        assert_eq!(historical.printer_nickname, "Maintenance P-1");
        assert_eq!(historical.title, "Replace nozzle and clean");
        db.delete_printer_maintenance_log(&historical.id, historical.metadata_version)
            .unwrap();
        assert!(db
            .get_printer_maintenance_log(&historical.id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn printer_reminder_interval_enforces_inclusive_backend_boundaries() {
        let db = LibraryDb::open_in_memory().unwrap();
        let printer = db
            .create_printer(&printer_input("Reminder boundaries P-1"), false)
            .unwrap();

        for interval_days in [1, 36_500] {
            let reminder = db
                .create_printer_reminder(
                    &printer.id,
                    &printer_reminder_input("2026-11-01T00:00:00Z", Some(interval_days)),
                )
                .unwrap();
            assert_eq!(reminder.interval_days, Some(interval_days));
        }
        for interval_days in [0, 36_501] {
            assert!(matches!(
                db.create_printer_reminder(
                    &printer.id,
                    &printer_reminder_input("2026-11-01T00:00:00Z", Some(interval_days)),
                ),
                Err(LibraryError::InvalidInput(message))
                    if message.contains("between 1 and 36500 days")
            ));
        }
        assert_eq!(db.list_printer_reminders(None, true).unwrap().len(), 2);
    }

    #[test]
    fn imported_print_attempt_preserves_linked_historical_printer_name() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Imported linked print", None).unwrap();
        let printer = db
            .create_printer(&printer_input("Legacy P-1"), false)
            .unwrap();
        let renamed = db
            .update_printer(
                &printer.id,
                printer.metadata_version,
                &printer_input("Current P-1"),
                None,
            )
            .unwrap();

        let imported = db
            .import_print_attempt_with_printer(
                &model.id,
                &print_attempt_input(),
                Some("1700000000"),
                Some(&renamed.id),
                Some("Legacy P-1"),
            )
            .unwrap();
        assert_eq!(imported.printer_id.as_deref(), Some(renamed.id.as_str()));
        assert_eq!(imported.printer_name.as_deref(), Some("Legacy P-1"));
        assert_eq!(imported.created_at, "1700000000");
    }

    #[test]
    fn imported_print_attempt_allows_an_unlinked_printer_name_snapshot() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Imported unlinked print", None).unwrap();
        let imported = db
            .import_print_attempt_with_printer(
                &model.id,
                &print_attempt_input(),
                None,
                None,
                Some("Unmapped legacy printer"),
            )
            .unwrap();
        assert_eq!(imported.printer_id, None);
        assert_eq!(
            imported.printer_name.as_deref(),
            Some("Unmapped legacy printer")
        );
        assert!(!imported.created_at.is_empty());
    }

    #[test]
    fn imported_print_attempt_rejects_an_unmapped_local_printer_id() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Imported invalid link", None).unwrap();
        assert!(db
            .import_print_attempt_with_printer(
                &model.id,
                &print_attempt_input(),
                None,
                Some("external-printer-ref"),
                Some("Source nickname"),
            )
            .is_err());
        assert!(db.list_print_attempts(&model.id).unwrap().is_empty());
    }

    #[test]
    fn print_attempt_printer_patch_preserves_clears_and_snapshots_attribution() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Printer attribution", None).unwrap();
        let printer = db
            .create_printer(&printer_input("Snapshot P-1"), false)
            .unwrap();
        let create: PrintAttemptInputWithPrinter = serde_json::from_value(serde_json::json!({
            "printed_at": "2026-10-14T12:00:00Z",
            "quantity": 1,
            "notes": "Initial",
            "filament_ids": [],
            "outcome": "printed",
            "printer_id": printer.id
        }))
        .unwrap();
        let attempt = db
            .create_print_attempt_with_printer(&model.id, &create)
            .unwrap();
        assert_eq!(attempt.printer_id.as_deref(), Some(printer.id.as_str()));
        assert_eq!(attempt.printer_name.as_deref(), Some("Snapshot P-1"));

        let omitted: PrintAttemptInputWithPrinter = serde_json::from_value(serde_json::json!({
            "printed_at": "2026-10-14T12:00:00Z",
            "quantity": 2,
            "notes": "Updated without attribution key",
            "filament_ids": [],
            "outcome": "printed"
        }))
        .unwrap();
        let preserved = db
            .update_print_attempt_with_printer(&attempt.id, &omitted)
            .unwrap();
        assert_eq!(preserved.printer_id.as_deref(), Some(printer.id.as_str()));
        assert_eq!(preserved.printer_name.as_deref(), Some("Snapshot P-1"));

        let cleared: PrintAttemptInputWithPrinter = serde_json::from_value(serde_json::json!({
            "printed_at": "2026-10-14T12:00:00Z",
            "quantity": 2,
            "notes": "Explicit null clears",
            "filament_ids": [],
            "outcome": "printed",
            "printer_id": null
        }))
        .unwrap();
        let cleared = db
            .update_print_attempt_with_printer(&attempt.id, &cleared)
            .unwrap();
        assert_eq!(cleared.printer_id, None);
        assert_eq!(cleared.printer_name, None);

        let linked_again = db
            .update_print_attempt_with_printer(&attempt.id, &create)
            .unwrap();
        db.delete_printer(&printer.id, printer.metadata_version)
            .unwrap();
        let after_delete = db.get_print_attempt(&attempt.id).unwrap().unwrap();
        assert_eq!(after_delete.printer_id, None);
        assert_eq!(after_delete.printer_name.as_deref(), Some("Snapshot P-1"));
        assert_eq!(linked_again.printer_name.as_deref(), Some("Snapshot P-1"));
    }
}
