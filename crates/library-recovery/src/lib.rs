//! Read-only recovery of the pre-existing Library schema. This crate never
//! creates a Library directory or opens the mutable LibraryDb.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Component, Path, PathBuf};

use library_core::catalog_vfs::{open_pinned_file, same_file_identity, CatalogVfs};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

pub const MANIFEST_VERSION: u32 = 1;
pub const LIBRARY_SCHEMA_VERSION: u32 = 11;
pub const MAX_COLORWAY_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_MODELS: usize = 10_000;
const MAX_ROWS: usize = 50_000;
const MAX_PRINTERS: usize = 5_000;
const MAX_PRINTER_HISTORY_ROWS: usize = 10_000;
const MAX_PART_STOCK_RECORDS: usize = 10_000;
const MAX_PART_USAGE_PER_ATTEMPT: usize = 256;
const MAX_PRINTER_LABEL_CHARS: usize = 128;
const MAX_PRINTER_NOTES_BYTES: usize = 20_000;
const MAX_METADATA_BYTES: i64 = 16 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub enum RecoveryError {
    Io(std::io::Error),
    Database(rusqlite::Error),
    Zip(zip::result::ZipError),
    Json(serde_json::Error),
    Invalid(String),
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "Recovery I/O failed: {error}"),
            Self::Database(error) => write!(formatter, "Recovery database read failed: {error}"),
            Self::Zip(error) => write!(formatter, "Recovery ZIP export failed: {error}"),
            Self::Json(error) => write!(formatter, "Recovery manifest failed: {error}"),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for RecoveryError {}
impl From<std::io::Error> for RecoveryError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<rusqlite::Error> for RecoveryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}
impl From<zip::result::ZipError> for RecoveryError {
    fn from(error: zip::result::ZipError) -> Self {
        Self::Zip(error)
    }
}
impl From<serde_json::Error> for RecoveryError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

fn invalid(message: impl Into<String>) -> RecoveryError {
    RecoveryError::Invalid(message.into())
}

fn safe_relative(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !value.contains('\\')
        && !value.contains('\0')
        && !path.is_absolute()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}
fn valid_rfc3339(value: &str) -> bool {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).is_ok()
}

fn valid_database_timestamp(value: &str) -> bool {
    valid_rfc3339(value) || value.parse::<u64>().is_ok()
}

fn invalid_external_id(value: &str) -> bool {
    value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
}

fn invalid_capture_id(value: &str) -> bool {
    value.is_empty()
        || value.len() > 128
        || value.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        })
}

fn invalid_catalog_record_id(value: &str) -> bool {
    value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || value.chars().any(char::is_control)
        || value.contains('/')
        || value.contains('\\')
}

fn invalid_printer_label(value: &str) -> bool {
    value.is_empty()
        || value.trim() != value
        || value.chars().count() > MAX_PRINTER_LABEL_CHARS
        || value.chars().any(char::is_control)
}

fn invalid_printer_snapshot_label(value: &str) -> bool {
    !value.is_empty() && invalid_printer_label(value)
}

fn invalid_optional_printer_label(value: Option<&str>) -> bool {
    value.is_some_and(invalid_printer_label)
}

fn invalid_printer_notes(value: &str) -> bool {
    value.len() > MAX_PRINTER_NOTES_BYTES
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
}

fn valid_printer_port(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u16>().is_ok()
}

fn valid_host_authority(authority: &str) -> bool {
    if authority.is_empty()
        || authority.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '@' | '/' | '\\' | '?' | '#')
        })
    {
        return false;
    }

    if let Some(rest) = authority.strip_prefix('[') {
        let Some(close) = rest.find(']') else {
            return false;
        };
        if rest[..close].parse::<Ipv6Addr>().is_err() {
            return false;
        }
        let suffix = &rest[close + 1..];
        return suffix.is_empty() || suffix.strip_prefix(':').is_some_and(valid_printer_port);
    }

    if authority.matches(':').count() > 1 {
        return false;
    }
    let host = if let Some((host, port)) = authority.rsplit_once(':') {
        if host.is_empty() || !valid_printer_port(port) {
            return false;
        }
        host
    } else {
        authority
    };
    if host.parse::<Ipv4Addr>().is_ok() {
        return true;
    }

    let host = host.strip_suffix('.').unwrap_or(host);
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.chars().count() <= 63
                && label.chars().next().is_some_and(char::is_alphanumeric)
                && label.chars().last().is_some_and(char::is_alphanumeric)
                && label
                    .chars()
                    .all(|character| character.is_alphanumeric() || character == '-')
        })
}

fn invalid_printer_host(value: &str) -> bool {
    value.len() > 255 || value.trim() != value || !valid_host_authority(value)
}

fn invalid_printer_web_url(value: &str) -> bool {
    if value.len() > 2048
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || value.contains(['?', '#', '\\'])
    {
        return true;
    }
    let Some((scheme, remainder)) = value.split_once("://") else {
        return true;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return true;
    }
    let authority = remainder.split('/').next().unwrap_or_default();
    !valid_host_authority(authority)
}

fn invalid_sha256(value: &str) -> bool {
    value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryManifest {
    pub manifest_version: u32,
    pub library_schema_version: u32,
    pub library_id: String,
    pub library_name: String,
    pub library_created_at: String,
    pub folders: Vec<RecoveryFolder>,
    pub tags: Vec<RecoveryTag>,
    pub model_tags: Vec<RecoveryModelTag>,
    pub models: Vec<RecoveryModel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub index_roots: Vec<RecoveryIndexRoot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_settings: Option<RecoveryCatalogSettings>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bundles: Vec<RecoveryBundle>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filament_sources: Vec<RecoveryFilamentSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filaments: Vec<RecoveryFilament>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub captures: Vec<RecoveryCaptureItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub print_batch_receipts: Vec<RecoveryPrintBatchReceipt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts_stock: Vec<RecoveryPartStock>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub printers: Vec<RecoveryPrinter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub printer_maintenance_logs: Vec<RecoveryPrinterMaintenanceLog>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub printer_reminders: Vec<RecoveryPrinterReminder>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryFolder {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryTag {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryModelTag {
    pub model_id: String,
    pub tag_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryModel {
    pub id: String,
    pub source_identity: String,
    pub display_name: String,
    pub description: String,
    pub folder_id: String,
    pub current_revision_id: Option<String>,
    pub metadata_version: i64,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub revisions: Vec<RecoveryRevision>,
    pub manual: Option<RecoveryAsset>,
    pub thumbnails: Vec<RecoveryThumbnail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed_source: Option<RecoveryIndexedSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<RecoveryOrganization>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub print_attempts: Vec<RecoveryPrintAttempt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filament_ids: Vec<String>,
    #[serde(default = "default_catalog_kind", skip_serializing_if = "is_file_kind")]
    pub catalog_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryOrganization {
    pub designer: Option<String>,
    pub source_url: Option<String>,
    pub notes: Option<String>,
    pub model_license: Option<String>,
    pub print_status: String,
    pub parent_model_id: Option<String>,
    pub last_printed_at: Option<String>,
    pub print_count: u64,
    pub bundle_id: Option<String>,
    #[serde(default)]
    pub rating: u8,
    #[serde(default)]
    pub favorite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPrintAttempt {
    pub id: String,
    pub model_id: String,
    pub printed_at: String,
    pub quantity: u32,
    pub notes: String,
    pub outcome: String,
    #[serde(default)]
    pub stock_adjusted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub printer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub printer_name: Option<String>,
    pub filament_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<RecoveryPrintAttemptPart>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPrintAttemptPart {
    /// Stable historical identity. The stock row may have been deleted.
    pub part_id: String,
    pub part_name: String,
    pub category: Option<String>,
    pub unit: String,
    pub quantity_per_copy: u32,
    /// Total stock consumed by this event across all copies.
    pub quantity: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPartStock {
    pub id: String,
    pub metadata_version: u64,
    pub name: String,
    pub category: Option<String>,
    pub quantity: u64,
    pub unit: String,
    pub notes: String,
    pub low_stock: u32,
    pub created_at: String,
    pub updated_at: String,
}

/// Portable printer metadata with endpoint approval deliberately revoked.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPrinter {
    pub id: String,
    pub metadata_version: u64,
    pub nickname: String,
    pub make: Option<String>,
    pub model: Option<String>,
    pub printer_type: Option<String>,
    pub firmware: Option<String>,
    pub host: Option<String>,
    pub web_url: Option<String>,
    /// Portable recovery data never retains authority to contact an endpoint.
    pub endpoint_approved: bool,
    pub bed_mm: Option<[f32; 3]>,
    pub notes: String,
    pub source_kind: String,
    pub connection_status: String,
    pub job_status: String,
    pub last_checked_at: Option<String>,
    pub connection_error_code: Option<String>,
    pub print_count: u64,
    pub last_printed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPrinterMaintenanceLog {
    pub id: String,
    #[serde(default = "default_recovery_metadata_version")]
    pub metadata_version: u64,
    pub printer_id: Option<String>,
    pub printer_nickname: String,
    pub title: String,
    pub performed_at: String,
    pub notes: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPrinterReminder {
    pub id: String,
    pub metadata_version: u64,
    pub printer_id: Option<String>,
    pub printer_nickname: String,
    pub title: String,
    pub notes: String,
    pub due_at: String,
    pub interval_days: Option<u32>,
    pub status: String,
    pub last_completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPrintBatchReceipt {
    pub idempotency_key: String,
    pub payload_hash: String,
    pub attempt_ids: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryBundle {
    pub id: String,
    pub source_root_id: Option<String>,
    pub source_path: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryFilamentSource {
    pub id: String,
    pub kind: String,
    pub base_url: String,
    pub last_synced_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryFilament {
    pub id: String,
    pub version: u64,
    pub brand: String,
    pub material: String,
    pub color: String,
    pub name: String,
    pub notes: String,
    pub origin: String,
    pub resource_kind: String,
    pub source_id: Option<String>,
    pub external_id: Option<String>,
    pub parent_filament_id: Option<String>,
    pub archived: bool,
    pub source_missing: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryCaptureItem {
    pub item_id: String,
    pub payload_hash: String,
    pub status: String,
    pub page_model_id: Option<String>,
    pub created_at: String,
    pub received_at: String,
    pub updated_at: String,
    pub error_code: Option<String>,
    pub downloads: Vec<RecoveryCaptureDownload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryCaptureDownload {
    pub capture_id: String,
    pub ordinal: u32,
    pub file_name: String,
    pub expected_hash: Option<String>,
    pub expected_bytes: Option<u64>,
    pub status: String,
    pub model_id: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryCatalogSettings {
    pub thumbnail_size: u32,
    pub thumbnail_background: String,
    pub thumbnail_concurrency: u32,
    pub scan_batch_size: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryIndexRoot {
    pub id: String,
    pub path: String,
    pub enabled: bool,
    pub last_scanned_at: Option<String>,
    pub scan_interval_minutes: u32,
    pub max_file_bytes: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub scan_zip: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryIndexedSource {
    pub root_id: String,
    pub relative_path: String,
    pub missing: bool,
    pub modified_at: Option<u64>,
    pub content_hash: String,
    pub original_name: String,
    pub format: String,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_member: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outer_fingerprint: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_recovery_metadata_version() -> u64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryRevision {
    pub id: String,
    pub model_id: String,
    pub blob_hash: String,
    pub original_name: String,
    pub format: String,
    pub created_at: String,
    pub original: RecoveryAsset,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryThumbnail {
    pub id: String,
    pub model_id: String,
    pub is_custom: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub ordinal: u16,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_active: bool,
    pub asset: RecoveryAsset,
}

fn default_catalog_kind() -> String {
    "file".into()
}

fn is_file_kind(value: &String) -> bool {
    value == "file"
}

fn is_zero_u16(value: &u16) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryAsset {
    pub original_relative_path: String,
    pub exported_path: Option<String>,
    pub bytes: u64,
    pub sha256: String,
}

/// Reads the existing catalog from a consistent SQLite snapshot. Managed paths
/// are validated, but content hashes are checked during export.
pub fn inspect_library(root: &Path) -> Result<RecoveryManifest, RecoveryError> {
    read_library(root, None)
}

/// Inspects a disposable restore staging Library without leaving a persistent
/// catalog alias. Normal Library reads use the identity-pinned VFS path.
pub fn inspect_staged_library(root: &Path) -> Result<RecoveryManifest, RecoveryError> {
    read_library_with_mode(root, None, false)
}

fn read_library(
    root: &Path,
    selected_ids: Option<&[String]>,
) -> Result<RecoveryManifest, RecoveryError> {
    read_library_with_mode(root, selected_ids, true)
}

fn read_library_with_mode(
    root: &Path,
    selected_ids: Option<&[String]>,
    use_catalog_vfs: bool,
) -> Result<RecoveryManifest, RecoveryError> {
    let (manifest, ()) = with_library_read(root, selected_ids, use_catalog_vfs, |_, _, _| Ok(()))?;
    Ok(manifest)
}

fn with_library_read<T>(
    root: &Path,
    selected_ids: Option<&[String]>,
    use_catalog_vfs: bool,
    operation: impl FnOnce(
        &Path,
        &mut RecoveryManifest,
        &RecoveryCatalogReader,
    ) -> Result<T, RecoveryError>,
) -> Result<(RecoveryManifest, T), RecoveryError> {
    let root = existing_library_root(root)?;
    let database = root.join("library.sqlite3");
    let catalog = RecoveryCatalogReader::open(&database, use_catalog_vfs)?;
    catalog.check_identity()?;
    catalog
        .connection()
        .execute_batch("BEGIN DEFERRED TRANSACTION")?;
    let mut manifest = read_catalog(catalog.connection())?;
    if let Some(ids) = selected_ids {
        select_models(&mut manifest, ids)?;
    }
    catalog.check_identity()?;
    populate_asset_bytes(&root, &mut manifest)?;
    validate_catalog_assets(&root, &manifest)?;
    if serde_json::to_vec(&manifest)?.len() > MAX_MANIFEST_BYTES {
        return Err(invalid("Recovery manifest exceeds its size limit"));
    }
    catalog.check_identity()?;
    catalog.connection().execute_batch("COMMIT")?;
    catalog.check_identity()?;
    let result = operation(&root, &mut manifest, &catalog)?;
    catalog.check_identity()?;
    Ok((manifest, result))
}

/// Exports selected model IDs, or every model when the selection is empty.
/// Existing destinations are never replaced.
pub fn export_library_zip(
    root: &Path,
    model_ids: &[String],
    destination: &Path,
) -> Result<RecoveryManifest, RecoveryError> {
    let root = existing_library_root(root)?;
    if !destination.is_absolute() {
        return Err(invalid("Recovery destination must be absolute"));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("Recovery destination has no parent"))?;
    if parent.canonicalize()?.starts_with(&root) {
        return Err(invalid(
            "Recovery destination must be outside the Library root",
        ));
    }
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let (manifest, ()) =
        with_library_read(&root, Some(model_ids), true, |root, manifest, catalog| {
            write_archive(temporary.reopen()?, root, manifest)?;
            catalog.check_identity()?;
            Ok(())
        })?;
    temporary
        .persist_noclobber(destination)
        .map_err(|error| RecoveryError::Io(error.error))?;
    Ok(manifest)
}

struct RecoveryCatalogReader {
    // Drop the connection before unregistering its VFS.
    connection: Connection,
    pinned_file: File,
    vfs: Option<CatalogVfs>,
    path: PathBuf,
}

impl RecoveryCatalogReader {
    fn open(path: &Path, use_catalog_vfs: bool) -> Result<Self, RecoveryError> {
        let pinned_file = open_catalog_file(path)?;
        let (connection, vfs) = if use_catalog_vfs {
            let vfs = CatalogVfs::new(path, &pinned_file)?;
            let connection = Connection::open_with_flags_and_vfs(
                vfs.sqlite_path(),
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_NOFOLLOW,
                vfs.vfs_name(),
            )?;
            (connection, Some(vfs))
        } else {
            // SQLite's NOFOLLOW may reject macOS's noncanonical /var spelling
            // because /var itself is a symlink. Canonicalize only the already
            // admitted parent, keep the leaf uns followed, and prove it still
            // names the pinned staging database before SQLite opens it.
            let sqlite_path = canonical_parent_path(path)?;
            check_visible_file_identity(&sqlite_path, &pinned_file, "Staged database")?;
            let connection = Connection::open_with_flags(
                &sqlite_path,
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )?;
            (connection, None)
        };
        let catalog = Self {
            connection,
            pinned_file,
            vfs,
            path: path.to_path_buf(),
        };
        catalog.check_identity()?;
        Ok(catalog)
    }

    fn connection(&self) -> &Connection {
        &self.connection
    }

    fn check_identity(&self) -> Result<(), RecoveryError> {
        if let Some(vfs) = &self.vfs {
            vfs.check_connection(&self.connection)?;
            Ok(())
        } else {
            check_open_catalog_identity(&self.path, &self.pinned_file, &self.connection)
        }
    }
}

fn open_catalog_file(path: &Path) -> Result<File, RecoveryError> {
    open_pinned_regular_file(path, "Library database")
}

fn open_pinned_regular_file(path: &Path, label: &str) -> Result<File, RecoveryError> {
    let before =
        fs::symlink_metadata(path).map_err(|_| invalid(format!("{label} was not found")))?;
    if !before.is_file() || before.file_type().is_symlink() || is_reparse_point(&before) {
        return Err(invalid(format!("{label} must be an ordinary file")));
    }
    let file = open_pinned_file(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() || is_reparse_point(&opened) {
        return Err(invalid(format!(
            "{label} changed while it was being opened"
        )));
    }
    let visible = open_pinned_file(path)?;
    if !same_file_identity(&file, &visible)? {
        return Err(invalid(format!(
            "{label} changed while it was being opened"
        )));
    }
    Ok(file)
}

fn check_open_catalog_identity(
    path: &Path,
    pinned_file: &File,
    connection: &Connection,
) -> Result<(), RecoveryError> {
    check_visible_file_identity(path, pinned_file, "Library database")?;
    let mut moved = 1;
    let result = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            connection.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_HAS_MOVED,
            (&mut moved as *mut i32).cast(),
        )
    };
    if result != rusqlite::ffi::SQLITE_OK || moved != 0 {
        return Err(invalid(
            "SQLite recovery connection changed database identity",
        ));
    }
    Ok(())
}

fn canonical_parent_path(path: &Path) -> Result<PathBuf, RecoveryError> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("SQLite path has no parent"))?
        .canonicalize()?;
    let filename = path
        .file_name()
        .ok_or_else(|| invalid("SQLite path has no filename"))?;
    Ok(parent.join(filename))
}

fn check_visible_file_identity(
    path: &Path,
    pinned_file: &File,
    label: &str,
) -> Result<(), RecoveryError> {
    let visible_metadata = fs::symlink_metadata(path)?;
    if !visible_metadata.is_file()
        || visible_metadata.file_type().is_symlink()
        || is_reparse_point(&visible_metadata)
    {
        return Err(invalid(format!("{label} identity changed during recovery")));
    }
    let visible_file = open_pinned_file(path)?;
    if !same_file_identity(pinned_file, &visible_file)? {
        return Err(invalid(format!("{label} identity changed during recovery")));
    }
    Ok(())
}

#[cfg(unix)]
fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(any(unix, windows)))]
fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

/// Writes the exact UTF-8 bytes supplied by the original webview's
/// localStorage value. Malformed JSON is intentionally preserved.
pub fn export_raw_colorway_json(raw: &str, destination: &Path) -> Result<u64, RecoveryError> {
    if raw.len() > MAX_COLORWAY_BYTES {
        return Err(invalid(
            "Saved colorway data exceeds the recovery export limit",
        ));
    }
    if !destination.is_absolute() {
        return Err(invalid("Recovery destination must be absolute"));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("Recovery destination has no parent"))?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut file = temporary.reopen()?;
    file.write_all(raw.as_bytes())?;
    file.sync_all()?;
    drop(file);
    temporary
        .persist_noclobber(destination)
        .map_err(|error| RecoveryError::Io(error.error))?;
    Ok(raw.len() as u64)
}

fn existing_library_root(root: &Path) -> Result<PathBuf, RecoveryError> {
    let root = root
        .canonicalize()
        .map_err(|_| invalid("No existing Library directory was found"))?;
    if !root.is_dir() {
        return Err(invalid("Library path is not a directory"));
    }
    Ok(root)
}

fn select_models(manifest: &mut RecoveryManifest, ids: &[String]) -> Result<(), RecoveryError> {
    if ids.is_empty() {
        return Ok(());
    }
    let requested: BTreeSet<_> = ids.iter().collect();
    if requested.len() != ids.len() {
        return Err(invalid("Duplicate model ID in recovery selection"));
    }
    let found: BTreeSet<_> = manifest
        .models
        .iter()
        .map(|model| &model.id)
        .filter(|id| requested.contains(id))
        .collect();
    if found != requested {
        return Err(invalid("Recovery selection contains an unknown model ID"));
    }
    let by_id: BTreeMap<_, _> = manifest
        .models
        .iter()
        .map(|model| {
            (
                model.id.clone(),
                model
                    .organization
                    .as_ref()
                    .and_then(|org| org.parent_model_id.clone()),
            )
        })
        .collect();
    let mut selected: BTreeSet<String> = ids.iter().cloned().collect();
    loop {
        let mut changed = false;
        for capture in &manifest.captures {
            let capture_models = capture
                .page_model_id
                .iter()
                .chain(
                    capture
                        .downloads
                        .iter()
                        .filter_map(|download| download.model_id.as_ref()),
                )
                .cloned()
                .collect::<Vec<_>>();
            if capture_models.iter().any(|id| selected.contains(id)) {
                for id in capture_models {
                    changed |= selected.insert(id);
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut pending = selected.iter().cloned().collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        if let Some(Some(parent)) = by_id.get(&id) {
            if selected.insert(parent.clone()) {
                pending.push(parent.clone());
            }
        }
    }
    manifest.models.retain(|model| selected.contains(&model.id));
    let linked_part_ids: BTreeSet<_> = manifest
        .models
        .iter()
        .flat_map(|model| model.print_attempts.iter())
        .flat_map(|attempt| attempt.parts.iter())
        .map(|part| part.part_id.as_str())
        .collect();
    manifest
        .parts_stock
        .retain(|part| linked_part_ids.contains(part.id.as_str()));
    manifest.captures.retain(|capture| {
        capture
            .page_model_id
            .as_ref()
            .is_some_and(|id| selected.contains(id))
            || capture
                .downloads
                .iter()
                .filter_map(|download| download.model_id.as_ref())
                .any(|id| selected.contains(id))
    });
    let selected_attempt_ids = manifest
        .models
        .iter()
        .flat_map(|model| {
            model
                .print_attempts
                .iter()
                .map(|attempt| attempt.id.as_str())
        })
        .collect::<BTreeSet<_>>();
    manifest.print_batch_receipts.retain(|receipt| {
        !receipt.attempt_ids.is_empty()
            && receipt
                .attempt_ids
                .iter()
                .all(|id| selected_attempt_ids.contains(id.as_str()))
    });
    manifest
        .model_tags
        .retain(|link| selected.contains(&link.model_id));
    let tag_ids: BTreeSet<_> = manifest
        .model_tags
        .iter()
        .map(|link| &link.tag_id)
        .collect();
    manifest.tags.retain(|tag| tag_ids.contains(&tag.id));
    let bundle_ids: BTreeSet<_> = manifest
        .models
        .iter()
        .filter_map(|model| model.organization.as_ref()?.bundle_id.as_ref())
        .collect();
    manifest
        .bundles
        .retain(|bundle| bundle_ids.contains(&bundle.id));
    let known_filaments: BTreeSet<_> = manifest
        .filaments
        .iter()
        .map(|filament| filament.id.clone())
        .collect();
    let parent_filaments: BTreeMap<_, _> = manifest
        .filaments
        .iter()
        .map(|filament| (filament.id.clone(), filament.parent_filament_id.clone()))
        .collect();
    let mut filament_ids = BTreeSet::new();
    for model in &manifest.models {
        filament_ids.extend(model.filament_ids.iter().cloned());
        filament_ids.extend(
            model
                .print_attempts
                .iter()
                .flat_map(|attempt| attempt.filament_ids.iter())
                .filter(|id| known_filaments.contains(*id))
                .cloned(),
        );
    }
    let mut pending = filament_ids.iter().cloned().collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        if let Some(Some(parent)) = parent_filaments.get(&id) {
            if filament_ids.insert(parent.clone()) {
                pending.push(parent.clone());
            }
        }
    }
    manifest
        .filaments
        .retain(|filament| filament_ids.contains(&filament.id));
    let source_ids: BTreeSet<_> = manifest
        .filaments
        .iter()
        .filter_map(|filament| filament.source_id.as_ref())
        .collect();
    manifest
        .filament_sources
        .retain(|source| source_ids.contains(&source.id));
    let parents: BTreeMap<String, Option<String>> = manifest
        .folders
        .iter()
        .map(|folder| (folder.id.clone(), folder.parent_id.clone()))
        .collect();
    let mut folder_ids = BTreeSet::new();
    for model in &manifest.models {
        let mut current = Some(model.folder_id.clone());
        while let Some(id) = current {
            if !folder_ids.insert(id.clone()) {
                break;
            }
            current = parents.get(&id).and_then(Clone::clone);
        }
    }
    manifest
        .folders
        .retain(|folder| folder_ids.contains(&folder.id));
    Ok(())
}

fn write_archive(
    file: File,
    root: &Path,
    manifest: &mut RecoveryManifest,
) -> Result<(), RecoveryError> {
    let mut archive = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    let mut entry_number = 0usize;
    let mut total_bytes = 0u64;
    for model in &mut manifest.models {
        for revision in &mut model.revisions {
            write_asset(
                &mut archive,
                options,
                root,
                "originals",
                &mut revision.original,
                &mut entry_number,
                &mut total_bytes,
            )?;
        }
        if let Some(manual) = &mut model.manual {
            write_asset(
                &mut archive,
                options,
                root,
                "manuals",
                manual,
                &mut entry_number,
                &mut total_bytes,
            )?;
        }
        for thumbnail in &mut model.thumbnails {
            write_asset(
                &mut archive,
                options,
                root,
                "thumbnails",
                &mut thumbnail.asset,
                &mut entry_number,
                &mut total_bytes,
            )?;
        }
    }
    let bytes = serde_json::to_vec_pretty(manifest)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(invalid("Recovery manifest exceeds its size limit"));
    }
    archive.start_file("manifest.json", options)?;
    archive.write_all(&bytes)?;
    archive.finish()?.sync_all()?;
    Ok(())
}

fn write_asset(
    archive: &mut zip::ZipWriter<File>,
    options: SimpleFileOptions,
    root: &Path,
    kind: &str,
    asset: &mut RecoveryAsset,
    entry_number: &mut usize,
    total_bytes: &mut u64,
) -> Result<(), RecoveryError> {
    if *entry_number >= MAX_ROWS {
        return Err(invalid("Recovery export has too many entries"));
    }
    let path = managed_file(root, &asset.original_relative_path, kind)?;
    let mut source = open_pinned_file(&path)?;
    let source_bytes = source.metadata()?.len();
    if source_bytes > MAX_ENTRY_BYTES || source_bytes != asset.bytes {
        return Err(invalid(
            "A managed asset changed or exceeds the recovery size limit",
        ));
    }
    *total_bytes = total_bytes
        .checked_add(source_bytes)
        .ok_or_else(|| invalid("Recovery export size overflow"))?;
    if *total_bytes > MAX_TOTAL_BYTES {
        return Err(invalid("Recovery export exceeds its total size limit"));
    }
    let exported_path = format!("files/{:06}.bin", *entry_number);
    archive.start_file(&exported_path, options)?;
    let mut hasher = Sha256::new();
    let mut copied = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        copied += count as u64;
        if copied > source_bytes {
            return Err(invalid("A managed asset changed during recovery"));
        }
        hasher.update(&buffer[..count]);
        archive.write_all(&buffer[..count])?;
    }
    if copied != source_bytes || format!("{:x}", hasher.finalize()) != asset.sha256 {
        return Err(invalid("A managed asset failed its content hash check"));
    }
    asset.exported_path = Some(exported_path);
    *entry_number += 1;
    Ok(())
}

fn validate_catalog_assets(root: &Path, manifest: &RecoveryManifest) -> Result<(), RecoveryError> {
    for model in &manifest.models {
        for revision in &model.revisions {
            validate_asset(root, "originals", &revision.original)?;
        }
        if let Some(manual) = &model.manual {
            validate_asset(root, "manuals", manual)?;
        }
        for thumbnail in &model.thumbnails {
            validate_asset(root, "thumbnails", &thumbnail.asset)?;
        }
    }
    Ok(())
}

fn validate_asset(root: &Path, kind: &str, asset: &RecoveryAsset) -> Result<(), RecoveryError> {
    let path = managed_file(root, &asset.original_relative_path, kind)?;
    let actual_bytes = path.metadata()?.len();
    if actual_bytes > MAX_ENTRY_BYTES || actual_bytes != asset.bytes {
        return Err(invalid("A managed asset is missing, changed, or too large"));
    }
    if asset.sha256.len() != 64 || !asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("A managed asset has an invalid content hash"));
    }
    Ok(())
}

fn managed_file(root: &Path, relative: &str, kind: &str) -> Result<PathBuf, RecoveryError> {
    if relative.contains('\\') || relative.contains('\0') {
        return Err(invalid("The catalog contains an unsafe managed path"));
    }
    let parts: Vec<_> = relative.split('/').collect();
    if parts.len() < 2
        || parts[0] != kind
        || parts
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err(invalid("The catalog contains an unsafe managed path"));
    }
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid("The catalog contains an unsafe managed path"));
    }
    let mut current = root.to_path_buf();
    for part in parts {
        current.push(part);
        if current.symlink_metadata()?.file_type().is_symlink() {
            return Err(invalid("The catalog references a symlinked managed asset"));
        }
    }
    let canonical = current.canonicalize()?;
    if !canonical.starts_with(root) || !canonical.is_file() {
        return Err(invalid(
            "A managed asset escapes the Library root or is not a file",
        ));
    }
    Ok(canonical)
}

fn read_catalog(conn: &Connection) -> Result<RecoveryManifest, RecoveryError> {
    validate_schema(conn)?;
    let schema_version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let (library_id, library_name, library_created_at) = conn
        .query_row(
            "SELECT id, name, created_at FROM libraries WHERE id = 'local'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| invalid("The Library identity is missing"))?;

    let folders = collect_rows(
        conn,
        "SELECT id, name, parent_id FROM folders ORDER BY id",
        |row| {
            Ok(RecoveryFolder {
                id: row.get(0)?,
                name: row.get(1)?,
                parent_id: row.get(2)?,
            })
        },
    )?;
    let tags = collect_rows(
        conn,
        if schema_version >= 3 {
            "SELECT id,name,category FROM tags ORDER BY id"
        } else {
            "SELECT id,name,NULL FROM tags ORDER BY id"
        },
        |row| {
            Ok(RecoveryTag {
                id: row.get(0)?,
                name: row.get(1)?,
                category: row.get(2)?,
            })
        },
    )?;
    if tags.iter().any(|tag| {
        tag.name.is_empty()
            || tag.name.len() > 256
            || tag
                .category
                .as_deref()
                .is_some_and(|value| value.len() > 120)
    }) {
        return Err(invalid("Tag metadata is invalid"));
    }
    if schema_version >= 3
        && tags.iter().any(|tag| {
            tag.name.trim() != tag.name
                || tag.name.contains('/')
                || tag.name.contains('\\')
                || tag.name.chars().any(char::is_control)
                || tag
                    .category
                    .as_deref()
                    .is_some_and(|value| value.trim() != value)
        })
    {
        return Err(invalid("Schema-3 tag metadata is invalid"));
    }
    let model_tags = collect_rows(
        conn,
        "SELECT model_id, tag_id FROM model_tags ORDER BY model_id, tag_id",
        |row| {
            Ok(RecoveryModelTag {
                model_id: row.get(0)?,
                tag_id: row.get(1)?,
            })
        },
    )?;
    let mut models: Vec<RecoveryModel> = collect_rows(
        conn,
        if schema_version >= 5 {
            "SELECT id, display_name, description, folder_id, current_revision_id, metadata_version,
                    deleted_at, created_at, updated_at,catalog_kind FROM models ORDER BY id"
        } else {
            "SELECT id, display_name, description, folder_id, current_revision_id, metadata_version,
                    deleted_at, created_at, updated_at,'file' FROM models ORDER BY id"
        },
        |row| {
            let id: String = row.get(0)?;
            Ok(RecoveryModel {
                source_identity: format!("library:{id}"),
                id,
                display_name: row.get(1)?,
                description: row.get(2)?,
                folder_id: row.get(3)?,
                current_revision_id: row.get(4)?,
                metadata_version: row.get(5)?,
                deleted_at: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
                revisions: Vec::new(),
                manual: None,
                thumbnails: Vec::new(),
                indexed_source: None,
                organization: None,
                print_attempts: Vec::new(),
                filament_ids: Vec::new(),
                catalog_kind: row.get(9)?,
            })
        },
    )?;
    let index: BTreeMap<String, usize> = models
        .iter()
        .enumerate()
        .map(|(position, model)| (model.id.clone(), position))
        .collect();

    let bundles = if schema_version >= 3 {
        collect_rows(
            conn,
            "SELECT id,source_root_id,source_path,created_at FROM bundles ORDER BY id",
            |row| {
                Ok(RecoveryBundle {
                    id: row.get(0)?,
                    source_root_id: row.get(1)?,
                    source_path: row.get(2)?,
                    created_at: row.get(3)?,
                })
            },
        )?
    } else {
        Vec::new()
    };
    let bundle_ids: BTreeSet<_> = bundles.iter().map(|bundle| bundle.id.as_str()).collect();
    if bundle_ids.len() != bundles.len() {
        return Err(invalid("Duplicate bundle identity"));
    }
    let (printers, printer_maintenance_logs, printer_reminders) = if schema_version >= 7 {
        read_printer_catalog(conn, schema_version)?
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };
    let printer_ids = printers
        .iter()
        .map(|printer| printer.id.clone())
        .collect::<BTreeSet<_>>();
    let parts_stock = if schema_version >= 9 {
        let rows = collect_rows(
            conn,
            "SELECT id,metadata_version,name,category,quantity,unit,notes,low_stock,created_at,updated_at
             FROM parts_stock ORDER BY id",
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            },
        )?;
        if rows.len() > MAX_PART_STOCK_RECORDS {
            return Err(invalid(
                "Library has too many part-stock records for recovery",
            ));
        }
        let mut stock = Vec::with_capacity(rows.len());
        for (
            id,
            metadata_version,
            name,
            category,
            quantity,
            unit,
            notes,
            low_stock,
            created_at,
            updated_at,
        ) in rows
        {
            if invalid_catalog_record_id(&id)
                || metadata_version <= 0
                || invalid_printer_label(&name)
                || invalid_optional_printer_label(category.as_deref())
                || quantity < 0
                || invalid_printer_label(&unit)
                || invalid_printer_notes(&notes)
                || !(0..=1_000_000).contains(&low_stock)
                || !valid_database_timestamp(&created_at)
                || !valid_database_timestamp(&updated_at)
            {
                return Err(invalid("Part-stock metadata is invalid"));
            }
            stock.push(RecoveryPartStock {
                id,
                metadata_version: metadata_version as u64,
                name,
                category,
                quantity: quantity as u64,
                unit,
                notes,
                low_stock: low_stock as u32,
                created_at,
                updated_at,
            });
        }
        stock
    } else {
        Vec::new()
    };

    let mut print_batch_receipts = Vec::new();
    if schema_version >= 3 {
        let organization_query = if schema_version >= 6 {
            "SELECT id,designer,source_url,notes,model_license,print_status,parent_model_id,last_printed_at,print_count,bundle_id,rating,favorite FROM models ORDER BY id"
        } else {
            "SELECT id,designer,source_url,notes,model_license,print_status,parent_model_id,last_printed_at,print_count,bundle_id,0,0 FROM models ORDER BY id"
        };
        let rows = collect_rows(conn, organization_query, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, i64>(11)?,
            ))
        })?;
        for (
            id,
            designer,
            source_url,
            notes,
            model_license,
            print_status,
            parent_model_id,
            last_printed_at,
            print_count,
            bundle_id,
            rating,
            favorite,
        ) in rows
        {
            if !matches!(
                print_status.as_str(),
                "unprinted" | "want" | "queued" | "printing" | "printed" | "failed"
            ) || print_count < 0
                || source_url.as_deref().is_some_and(|url| {
                    url.len() > 2048 || !(url.starts_with("http://") || url.starts_with("https://"))
                })
                || last_printed_at
                    .as_deref()
                    .is_some_and(|date| !valid_rfc3339(date))
                || bundle_id
                    .as_deref()
                    .is_some_and(|id| !bundle_ids.contains(id))
                || !(0..=5).contains(&rating)
                || !matches!(favorite, 0 | 1)
            {
                return Err(invalid("Model organization metadata is invalid"));
            }
            let position = *index
                .get(&id)
                .ok_or_else(|| invalid("Unknown model organization metadata"))?;
            models[position].organization = Some(RecoveryOrganization {
                designer,
                source_url,
                notes,
                model_license,
                print_status,
                parent_model_id,
                last_printed_at,
                print_count: print_count as u64,
                bundle_id,
                rating: rating as u8,
                favorite: favorite == 1,
            });
        }

        let attempt_query = if schema_version >= 10 {
            "SELECT id,model_id,printed_at,quantity,notes,outcome,printer_id,printer_name,
                    created_at,updated_at,stock_adjusted
             FROM print_attempts ORDER BY model_id,printed_at,id"
        } else if schema_version >= 9 {
            "SELECT id,model_id,printed_at,quantity,notes,outcome,printer_id,printer_name,
                    created_at,updated_at,NULL
             FROM print_attempts ORDER BY model_id,printed_at,id"
        } else if schema_version >= 7 {
            "SELECT id,model_id,printed_at,quantity,notes,outcome,printer_id,printer_name,
                    created_at,updated_at,NULL FROM print_attempts ORDER BY model_id,printed_at,id"
        } else {
            "SELECT id,model_id,printed_at,quantity,notes,outcome,NULL,NULL,created_at,updated_at,NULL
             FROM print_attempts ORDER BY model_id,printed_at,id"
        };
        let attempts = collect_rows(conn, attempt_query, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, Option<i64>>(10)?,
            ))
        })?;
        let part_usage_rows = if schema_version >= 9 {
            collect_rows(
                conn,
                "SELECT attempt_id,part_id,part_name,category,unit,quantity_per_copy,quantity,ordinal
                 FROM print_attempt_parts ORDER BY attempt_id,ordinal",
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )?
        } else {
            Vec::new()
        };
        let filament_rows=collect_rows(conn,"SELECT attempt_id,filament_id,ordinal FROM print_attempt_filaments ORDER BY attempt_id,ordinal",|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?)))?;
        let mut filaments: BTreeMap<String, Vec<(i64, String)>> = BTreeMap::new();
        for (attempt, filament, ordinal) in filament_rows {
            if ordinal < 0 {
                return Err(invalid("Print attempt filament order is invalid"));
            }
            filaments
                .entry(attempt)
                .or_default()
                .push((ordinal, filament));
        }
        let attempt_ids: BTreeSet<String> = attempts.iter().map(|row| row.0.clone()).collect();
        if attempt_ids.len() != attempts.len()
            || filaments.keys().any(|id| !attempt_ids.contains(id))
        {
            return Err(invalid("A filament references an unknown print attempt"));
        }
        let attempt_quantities: BTreeMap<_, _> =
            attempts.iter().map(|row| (row.0.as_str(), row.3)).collect();
        let mut attempt_parts: BTreeMap<String, Vec<(i64, RecoveryPrintAttemptPart)>> =
            BTreeMap::new();
        let mut unique_part_ids: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (
            attempt_id,
            part_id,
            part_name,
            category,
            unit,
            quantity_per_copy,
            quantity,
            ordinal,
        ) in part_usage_rows
        {
            let Some(attempt_quantity) = attempt_quantities.get(attempt_id.as_str()).copied()
            else {
                return Err(invalid("Part usage references an unknown print attempt"));
            };
            let expected_quantity = quantity_per_copy
                .checked_mul(attempt_quantity)
                .ok_or_else(|| invalid("Part usage quantity is out of range"))?;
            if invalid_catalog_record_id(&part_id)
                || invalid_printer_label(&part_name)
                || invalid_optional_printer_label(category.as_deref())
                || invalid_printer_label(&unit)
                || !(1..=9_999).contains(&quantity_per_copy)
                || !(1..=100_000_000).contains(&quantity)
                || quantity != expected_quantity
                || ordinal < 0
                || ordinal as usize >= MAX_PART_USAGE_PER_ATTEMPT
                || !unique_part_ids
                    .entry(attempt_id.clone())
                    .or_default()
                    .insert(part_id.clone())
            {
                return Err(invalid("Print attempt part usage is invalid"));
            }
            attempt_parts.entry(attempt_id).or_default().push((
                ordinal,
                RecoveryPrintAttemptPart {
                    part_id,
                    part_name,
                    category,
                    unit,
                    quantity_per_copy: quantity_per_copy as u32,
                    quantity: quantity as u32,
                },
            ));
        }
        for rows in attempt_parts.values_mut() {
            rows.sort_by_key(|(ordinal, _)| *ordinal);
            if rows.len() > MAX_PART_USAGE_PER_ATTEMPT
                || rows
                    .iter()
                    .enumerate()
                    .any(|(expected, (ordinal, _))| *ordinal != expected as i64)
            {
                return Err(invalid("Print attempt part order is invalid"));
            }
        }
        for (
            id,
            model_id,
            printed_at,
            quantity,
            notes,
            outcome,
            printer_id,
            printer_name,
            created_at,
            updated_at,
            stock_adjusted,
        ) in attempts
        {
            if !(1..=10_000).contains(&quantity)
                || notes.len() > 20_000
                || !matches!(outcome.as_str(), "printed" | "failed" | "cancelled")
                || stock_adjusted.is_some_and(|marker| !matches!(marker, 0 | 1))
                || !valid_rfc3339(&printed_at)
                || (schema_version >= 7
                    && (!valid_database_timestamp(&created_at)
                        || !valid_database_timestamp(&updated_at)))
                || printer_id
                    .as_deref()
                    .is_some_and(|id| invalid_catalog_record_id(id) || !printer_ids.contains(id))
                || printer_name.as_deref().is_some_and(invalid_printer_label)
                || (printer_id.is_some() && printer_name.is_none())
            {
                return Err(invalid(
                    "Print attempt metadata or printer reference is invalid",
                ));
            }
            let position = *index
                .get(&model_id)
                .ok_or_else(|| invalid("Print attempt references unknown model"))?;
            let mut values = filaments.remove(&id).unwrap_or_default();
            values.sort_by_key(|v| v.0);
            let mut unique = BTreeSet::new();
            if values.len() > 5
                || values
                    .iter()
                    .enumerate()
                    .any(|(expected, (ordinal, _))| *ordinal != expected as i64)
                || values.iter().any(|(_, f)| {
                    f.is_empty()
                        || f.len() > 256
                        || f.trim() != f
                        || f.chars().any(char::is_control)
                        || !unique.insert(f)
                })
            {
                return Err(invalid("Print attempt filaments are invalid"));
            }
            let parts = attempt_parts
                .remove(&id)
                .unwrap_or_default()
                .into_iter()
                .map(|(_, part)| part)
                .collect();
            models[position].print_attempts.push(RecoveryPrintAttempt {
                id,
                model_id,
                printed_at,
                quantity: quantity as u32,
                notes,
                outcome,
                stock_adjusted: stock_adjusted.map(|marker| marker == 1),
                printer_id,
                printer_name,
                filament_ids: values.into_iter().map(|(_, f)| f).collect(),
                parts,
                created_at,
                updated_at,
            });
        }
        let has_receipts: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='print_batch_receipts')",
            [],
            |row| row.get(0),
        )?;
        if has_receipts {
            let rows = collect_rows(
                conn,
                "SELECT idempotency_key,payload_hash,attempt_ids_json,created_at
                 FROM print_batch_receipts ORDER BY idempotency_key",
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )?;
            for (idempotency_key, payload_hash, attempt_ids_json, created_at) in rows {
                let ids: Vec<String> = serde_json::from_str(&attempt_ids_json)?;
                if invalid_capture_id(&idempotency_key)
                    || invalid_sha256(&payload_hash)
                    || !valid_rfc3339(&created_at)
                    || ids.is_empty()
                    || ids.len() > 10_000
                    || ids.iter().any(|id| !attempt_ids.contains(id))
                {
                    return Err(invalid("Print batch receipt metadata is invalid"));
                }
                print_batch_receipts.push(RecoveryPrintBatchReceipt {
                    idempotency_key,
                    payload_hash,
                    attempt_ids: ids,
                    created_at,
                });
            }
        }
    }

    let (filament_sources, filaments) = if schema_version >= 4 {
        let sources = collect_rows(
            conn,
            "SELECT id,kind,base_url,last_synced_at,created_at,updated_at FROM filament_sources ORDER BY id",
            |row| {
                Ok(RecoveryFilamentSource {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    base_url: row.get(2)?,
                    last_synced_at: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            },
        )?;
        let source_ids: BTreeSet<_> = sources.iter().map(|source| source.id.as_str()).collect();
        if source_ids.len() != sources.len()
            || sources.iter().any(|source| {
                source.kind != "spoolman"
                    || source.base_url.len() > 2048
                    || !(source.base_url.starts_with("http://")
                        || source.base_url.starts_with("https://"))
                    || source.base_url.contains(['@', '?', '#'])
                    || source
                        .base_url
                        .split_once("://")
                        .is_none_or(|(_, remainder)| {
                            remainder.is_empty() || remainder.starts_with('/')
                        })
                    || source
                        .base_url
                        .chars()
                        .any(|character| character.is_control() || character.is_whitespace())
                    || source
                        .last_synced_at
                        .as_deref()
                        .is_some_and(|value| !valid_rfc3339(value))
            })
        {
            return Err(invalid("Filament source metadata is invalid"));
        }
        let rows = collect_rows(
            conn,
            "SELECT id,version,brand,material,color,name,notes,origin,resource_kind,source_id,
                    external_id,parent_filament_id,archived,source_missing,created_at,updated_at
             FROM filaments ORDER BY id",
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, i64>(13)?,
                    row.get::<_, String>(14)?,
                    row.get::<_, String>(15)?,
                ))
            },
        )?;
        let filament_ids: BTreeSet<_> = rows.iter().map(|row| row.0.clone()).collect();
        if filament_ids.len() != rows.len() {
            return Err(invalid("Duplicate filament identity"));
        }
        let mut remote_keys = BTreeSet::new();
        let mut values = Vec::with_capacity(rows.len());
        for (
            id,
            version,
            brand,
            material,
            color,
            name,
            notes,
            origin,
            resource_kind,
            source_id,
            external_id,
            parent_filament_id,
            archived,
            source_missing,
            created_at,
            updated_at,
        ) in rows
        {
            let common_invalid = version <= 0
                || brand.len() > 200
                || material.len() > 200
                || color.len() > 256
                || name.is_empty()
                || name.len() > 200
                || notes.len() > 20_000
                || [&brand, &material, &color, &name]
                    .into_iter()
                    .any(|value| value.trim() != value || value.chars().any(char::is_control))
                || notes.trim() != notes
                || notes.chars().any(|character| {
                    character == '\0'
                        || (character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
                })
                || !matches!(archived, 0 | 1)
                || !matches!(source_missing, 0 | 1);
            let relationship_invalid = match (origin.as_str(), resource_kind.as_str()) {
                ("manual", "manual") => {
                    source_id.is_some()
                        || external_id.is_some()
                        || parent_filament_id.is_some()
                        || source_missing != 0
                }
                ("spoolman", "filament") => {
                    archived != 0
                        || parent_filament_id.is_some()
                        || source_id
                            .as_deref()
                            .is_none_or(|value| !source_ids.contains(value))
                        || external_id.as_deref().is_none_or(invalid_external_id)
                }
                ("spoolman", "spool") => {
                    source_id
                        .as_deref()
                        .is_none_or(|value| !source_ids.contains(value))
                        || external_id.as_deref().is_none_or(invalid_external_id)
                        || parent_filament_id
                            .as_deref()
                            .is_none_or(|value| !filament_ids.contains(value))
                }
                _ => true,
            };
            if common_invalid || relationship_invalid {
                return Err(invalid("Filament metadata is invalid"));
            }
            if let (Some(source), Some(external)) = (&source_id, &external_id) {
                if !remote_keys.insert((source.clone(), resource_kind.clone(), external.clone())) {
                    return Err(invalid("Duplicate remote filament identity"));
                }
            }
            values.push(RecoveryFilament {
                id,
                version: version as u64,
                brand,
                material,
                color,
                name,
                notes,
                origin,
                resource_kind,
                source_id,
                external_id,
                parent_filament_id,
                archived: archived == 1,
                source_missing: source_missing == 1,
                created_at,
                updated_at,
            });
        }
        let by_id: BTreeMap<_, _> = values.iter().map(|value| (&value.id, value)).collect();
        for value in &values {
            if value.resource_kind == "spool" {
                let parent = by_id
                    .get(value.parent_filament_id.as_ref().unwrap())
                    .ok_or_else(|| invalid("Spool parent filament is missing"))?;
                if parent.resource_kind != "filament" || parent.source_id != value.source_id {
                    return Err(invalid("Spool parent filament source is invalid"));
                }
            }
        }
        let links = collect_rows(
            conn,
            "SELECT model_id,filament_id,ordinal FROM model_filaments ORDER BY model_id,ordinal",
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )?;
        let mut assigned: BTreeMap<String, Vec<(i64, String)>> = BTreeMap::new();
        for (model_id, filament_id, ordinal) in links {
            if !index.contains_key(&model_id) || !filament_ids.contains(&filament_id) {
                return Err(invalid(
                    "Model filament assignment has an unknown reference",
                ));
            }
            assigned
                .entry(model_id)
                .or_default()
                .push((ordinal, filament_id));
        }
        for (model_id, links) in assigned {
            let mut unique = BTreeSet::new();
            if links.len() > 5
                || links.iter().enumerate().any(|(expected, (ordinal, id))| {
                    *ordinal != expected as i64 || !unique.insert(id)
                })
            {
                return Err(invalid("Model filament assignment order is invalid"));
            }
            models[*index.get(&model_id).unwrap()].filament_ids =
                links.into_iter().map(|(_, id)| id).collect();
        }
        (sources, values)
    } else {
        (Vec::new(), Vec::new())
    };

    let captures = if schema_version >= 5 {
        let items = collect_rows(
            conn,
            "SELECT item_id,payload_hash,status,page_model_id,created_at,received_at,updated_at,error_code
             FROM capture_items ORDER BY item_id",
            |row| {
                Ok(RecoveryCaptureItem {
                    item_id: row.get(0)?,
                    payload_hash: row.get(1)?,
                    status: row.get(2)?,
                    page_model_id: row.get(3)?,
                    created_at: row.get(4)?,
                    received_at: row.get(5)?,
                    updated_at: row.get(6)?,
                    error_code: row.get(7)?,
                    downloads: Vec::new(),
                })
            },
        )?;
        let mut by_item = items
            .into_iter()
            .map(|item| (item.item_id.clone(), item))
            .collect::<BTreeMap<_, _>>();
        for item in by_item.values() {
            if invalid_capture_id(&item.item_id)
                || invalid_sha256(&item.payload_hash)
                || !matches!(item.status.as_str(), "imported" | "partial" | "failed")
                || !valid_rfc3339(&item.created_at)
                || item
                    .page_model_id
                    .as_deref()
                    .is_none_or(|id| !index.contains_key(id))
                || item.error_code.as_deref().is_some_and(invalid_capture_id)
            {
                return Err(invalid("Capture item metadata is invalid"));
            }
            let page = &models[*index.get(item.page_model_id.as_ref().unwrap()).unwrap()];
            if page.catalog_kind != "bookmark" {
                return Err(invalid("Capture page does not reference a bookmark"));
            }
        }
        let downloads = collect_rows(
            conn,
            "SELECT capture_id,item_id,ordinal,file_name,expected_hash,expected_bytes,status,model_id,error_code
             FROM capture_downloads ORDER BY item_id,ordinal,capture_id",
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            },
        )?;
        for (
            capture_id,
            item_id,
            ordinal,
            file_name,
            expected_hash,
            expected_bytes,
            status,
            model_id,
            error_code,
        ) in downloads
        {
            let item = by_item
                .get_mut(&item_id)
                .ok_or_else(|| invalid("Capture download references an unknown item"))?;
            if invalid_capture_id(&capture_id)
                || ordinal != item.downloads.len() as i64
                || file_name.is_empty()
                || file_name.len() > 255
                || file_name.contains(['/', '\\', '\0'])
                || expected_hash.as_deref().is_some_and(invalid_sha256)
                || expected_bytes.is_some_and(|bytes| bytes <= 0 || bytes > 100 * 1024 * 1024)
                || !matches!(status.as_str(), "needs_file" | "imported" | "failed")
                || model_id
                    .as_deref()
                    .is_some_and(|id| !index.contains_key(id))
                || (status == "imported") != model_id.is_some()
                || error_code.as_deref().is_some_and(invalid_capture_id)
            {
                return Err(invalid("Capture download metadata is invalid"));
            }
            item.downloads.push(RecoveryCaptureDownload {
                capture_id,
                ordinal: ordinal as u32,
                file_name,
                expected_hash,
                expected_bytes: expected_bytes.map(|bytes| bytes as u64),
                status,
                model_id,
                error_code,
            });
        }
        by_item.into_values().collect()
    } else {
        Vec::new()
    };

    let index_roots = if schema_version >= 2 {
        let invalid_sources: i64 = conn.query_row(
            "SELECT COUNT(*) FROM models WHERE source_kind IS NULL OR source_kind NOT IN ('managed','indexed')",
            [],|row|row.get(0))?;
        let invalid_links: i64 = conn.query_row(
            "SELECT COUNT(*) FROM indexed_model_data d LEFT JOIN models m ON m.id=d.model_id
             WHERE m.id IS NULL OR m.source_kind IS NULL OR m.source_kind!='indexed'",
            [],
            |row| row.get(0),
        )?;
        if invalid_sources != 0 || invalid_links != 0 {
            return Err(invalid(
                "Indexed source kinds or metadata links are invalid",
            ));
        }

        let roots = collect_rows(
            conn,
            if schema_version >= 3 {
                "SELECT id,path,enabled,last_scanned_at,scan_interval_minutes,max_file_bytes,scan_zip FROM index_roots ORDER BY id"
            } else {
                "SELECT id,path,enabled,last_scanned_at,scan_interval_minutes,max_file_bytes,0 FROM index_roots ORDER BY id"
            },
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )?;
        let mut result = Vec::new();
        for (id, path, enabled, last_scanned_at, interval, limit, scan_zip) in roots {
            if !Path::new(&path).is_absolute()
                || !(0..=1).contains(&enabled)
                || !(1..=1440).contains(&interval)
                || limit <= 0
                || limit > 100 * 1024 * 1024
                || !(0..=1).contains(&scan_zip)
            {
                return Err(invalid("An indexed root has invalid configuration"));
            }
            result.push(RecoveryIndexRoot {
                id,
                path,
                enabled: enabled == 1,
                last_scanned_at,
                scan_interval_minutes: interval as u32,
                max_file_bytes: limit as u64,
                scan_zip: scan_zip == 1,
            });
        }
        let root_ids: BTreeSet<_> = result.iter().map(|root| root.id.as_str()).collect();
        let sources = collect_rows(
            conn,
            if schema_version >= 3 {
                "SELECT m.id,m.source_root_id,m.source_path,m.source_missing,m.source_modified_at,d.content_hash,d.original_name,d.format,d.bytes,m.source_archive_member,d.outer_fingerprint FROM models m LEFT JOIN indexed_model_data d ON d.model_id=m.id WHERE m.source_kind='indexed' ORDER BY m.id"
            } else {
                "SELECT m.id,m.source_root_id,m.source_path,m.source_missing,m.source_modified_at,d.content_hash,d.original_name,d.format,d.bytes,NULL,NULL FROM models m
              LEFT JOIN indexed_model_data d ON d.model_id=m.id WHERE m.source_kind='indexed' ORDER BY m.id"
            },
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ))
            },
        )?;
        for (
            model_id,
            root_id,
            relative_path,
            missing,
            modified_at,
            content_hash,
            original_name,
            format,
            bytes,
            archive_member,
            outer_fingerprint,
        ) in sources
        {
            let relative = Path::new(&relative_path);
            if !root_ids.contains(root_id.as_str())
                || relative.is_absolute()
                || relative_path.contains('\\')
                || relative_path.contains('\0')
                || relative_path
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
                || relative
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
                || !(0..=1).contains(&missing)
                || modified_at.is_some_and(|value| value < 0)
                || bytes < 0
                || content_hash.len() != 64
                || !content_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                || archive_member
                    .as_deref()
                    .is_some_and(|member| !safe_relative(member))
                || archive_member.is_some() != outer_fingerprint.is_some()
                || outer_fingerprint.as_deref().is_some_and(str::is_empty)
            {
                return Err(invalid("An indexed model has invalid source metadata"));
            }
            let position = index
                .get(&model_id)
                .ok_or_else(|| invalid("Unknown indexed model"))?;
            models[*position].indexed_source = Some(RecoveryIndexedSource {
                root_id,
                relative_path,
                missing: missing == 1,
                modified_at: modified_at.map(|v| v as u64),
                content_hash,
                original_name,
                format,
                bytes: bytes as u64,
                archive_member,
                outer_fingerprint,
            });
        }
        result
    } else {
        Vec::new()
    };

    let catalog_settings = if schema_version >= 2 {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='catalog_settings')",
            [],|row|row.get(0))?;
        if exists {
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM catalog_settings", [], |row| {
                    row.get(0)
                })?;
            if count != 1 {
                return Err(invalid("Catalog settings singleton is invalid"));
            }
            let (size,background,workers,batch): (i64,String,i64,i64) = conn.query_row(
                "SELECT thumbnail_size,thumbnail_background,thumbnail_concurrency,scan_batch_size FROM catalog_settings WHERE id='local'",
                [],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?;
            let color = background.as_bytes();
            if ![128, 256, 512].contains(&size)
                || !(1..=2).contains(&workers)
                || !(16..=256).contains(&batch)
                || color.len() != 7
                || color[0] != b'#'
                || !color[1..].iter().all(u8::is_ascii_hexdigit)
            {
                return Err(invalid("Catalog settings values are invalid"));
            }
            Some(RecoveryCatalogSettings {
                thumbnail_size: size as u32,
                thumbnail_background: background,
                thumbnail_concurrency: workers as u32,
                scan_batch_size: batch as u32,
            })
        } else {
            None
        }
    } else {
        None
    };

    let revisions = collect_rows(
        conn,
        "SELECT r.id, r.model_id, r.blob_hash, r.original_name, r.format, r.bytes, r.created_at,
                b.relative_path, b.bytes
         FROM revisions r JOIN blobs b ON b.hash = r.blob_hash ORDER BY r.model_id, r.id",
        |row| {
            let revision_bytes: i64 = row.get(5)?;
            let blob_bytes: i64 = row.get(8)?;
            let hash: String = row.get(2)?;
            Ok((
                RecoveryRevision {
                    id: row.get(0)?,
                    model_id: row.get(1)?,
                    blob_hash: hash.clone(),
                    original_name: row.get(3)?,
                    format: row.get(4)?,
                    created_at: row.get(6)?,
                    original: RecoveryAsset {
                        original_relative_path: row.get(7)?,
                        exported_path: None,
                        bytes: u64::try_from(blob_bytes).unwrap_or(u64::MAX),
                        sha256: hash,
                    },
                },
                revision_bytes,
                blob_bytes,
            ))
        },
    )?;
    let revision_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM revisions", [], |row| row.get(0))?;
    if revision_count != revisions.len() as i64 {
        return Err(invalid("A revision references a missing blob"));
    }
    for (revision, revision_bytes, blob_bytes) in revisions {
        if revision_bytes < 0 || revision_bytes != blob_bytes {
            return Err(invalid("A revision has inconsistent byte counts"));
        }
        let model = index
            .get(&revision.model_id)
            .ok_or_else(|| invalid("A revision references an unknown model"))?;
        models[*model].revisions.push(revision);
    }

    let manuals = collect_rows(
        conn,
        "SELECT model_id, relative_path, content_hash FROM manuals ORDER BY model_id",
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    for (model_id, path, hash) in manuals {
        let model = index
            .get(&model_id)
            .ok_or_else(|| invalid("A manual references an unknown model"))?;
        models[*model].manual = Some(RecoveryAsset {
            original_relative_path: path,
            exported_path: None,
            bytes: 0,
            sha256: hash,
        });
    }
    let thumbnails = collect_rows(
        conn,
        if schema_version >= 5 {
            "SELECT id,model_id,relative_path,content_hash,is_custom,source_content_hash,ordinal,is_active FROM thumbnails ORDER BY model_id,ordinal,id"
        } else if schema_version >= 2 {
            "SELECT id,model_id,relative_path,content_hash,is_custom,source_content_hash,0,0 FROM thumbnails ORDER BY model_id,id"
        } else {
            "SELECT id,model_id,relative_path,content_hash,is_custom,NULL,0,0 FROM thumbnails ORDER BY model_id,id"
        },
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
            ))
        },
    )?;
    let mut gallery_positions = BTreeMap::<String, usize>::new();
    let mut gallery_active = BTreeSet::<String>::new();
    for (id, model_id, path, hash, is_custom, source_content_hash, ordinal, is_active) in thumbnails
    {
        let model = index
            .get(&model_id)
            .ok_or_else(|| invalid("A thumbnail references an unknown model"))?;
        if source_content_hash.as_ref().is_some_and(|hash| {
            hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            return Err(invalid("A thumbnail has invalid source provenance"));
        }
        if !matches!(is_custom, 0 | 1)
            || !matches!(is_active, 0 | 1)
            || ordinal < 0
            || (schema_version >= 5
                && ordinal as usize != *gallery_positions.entry(model_id.clone()).or_default())
            || (is_active == 1 && !gallery_active.insert(model_id.clone()))
        {
            return Err(invalid("A thumbnail has invalid gallery state"));
        }
        *gallery_positions.entry(model_id.clone()).or_default() += 1;
        let ordinal = u16::try_from(ordinal)
            .map_err(|_| invalid("Thumbnail ordinal exceeds recovery limit"))?;
        models[*model].thumbnails.push(RecoveryThumbnail {
            id,
            model_id,
            is_custom: is_custom == 1,
            source_content_hash,
            ordinal,
            is_active: is_active == 1,
            asset: RecoveryAsset {
                original_relative_path: path,
                exported_path: None,
                bytes: 0,
                sha256: hash,
            },
        });
    }
    if schema_version >= 5
        && gallery_positions
            .iter()
            .any(|(model_id, count)| *count > 0 && !gallery_active.contains(model_id))
    {
        return Err(invalid("Thumbnail gallery has no active selection"));
    }
    let folder_ids: BTreeSet<_> = folders
        .iter()
        .map(|folder: &RecoveryFolder| &folder.id)
        .collect();
    let tag_ids: BTreeSet<_> = tags.iter().map(|tag: &RecoveryTag| &tag.id).collect();
    for folder in &folders {
        if folder
            .parent_id
            .as_ref()
            .is_some_and(|parent| !folder_ids.contains(parent))
        {
            return Err(invalid("A folder references an unknown parent"));
        }
    }
    for link in &model_tags {
        if !index.contains_key(&link.model_id) || !tag_ids.contains(&link.tag_id) {
            return Err(invalid("A model tag link references a missing record"));
        }
    }
    for model in &models {
        if !folder_ids.contains(&model.folder_id)
            || model.metadata_version <= 0
            || !matches!(model.catalog_kind.as_str(), "file" | "bookmark")
            || (model.catalog_kind == "bookmark"
                && (model.current_revision_id.is_some()
                    || !model.revisions.is_empty()
                    || model.manual.is_some()
                    || model.indexed_source.is_some()))
        {
            return Err(invalid("A model has unsupported metadata"));
        }
        if let Some(current) = &model.current_revision_id {
            if !model
                .revisions
                .iter()
                .any(|revision| &revision.id == current)
            {
                return Err(invalid("A model's current revision is missing"));
            }
        }
    }
    let root_ids: BTreeSet<_> = index_roots.iter().map(|root| root.id.as_str()).collect();
    for bundle in &bundles {
        if bundle.source_root_id.is_some() != bundle.source_path.is_some()
            || bundle
                .source_root_id
                .as_deref()
                .is_some_and(|id| !root_ids.contains(id))
            || bundle
                .source_path
                .as_deref()
                .is_some_and(|path| !safe_relative(path))
        {
            return Err(invalid("Bundle source metadata is invalid"));
        }
    }
    for model in &models {
        if let Some(org) = &model.organization {
            if org
                .parent_model_id
                .as_deref()
                .is_some_and(|id| !index.contains_key(id))
            {
                return Err(invalid("Model parent reference is invalid"));
            }
            let mut current = org.parent_model_id.as_deref();
            let mut visited = BTreeSet::new();
            while let Some(id) = current {
                if id == model.id || !visited.insert(id) {
                    return Err(invalid("Model parent cycle detected"));
                }
                current = models[*index.get(id).unwrap()]
                    .organization
                    .as_ref()
                    .and_then(|value| value.parent_model_id.as_deref());
            }
            let printed = model
                .print_attempts
                .iter()
                .filter(|a| a.outcome == "printed");
            let count: u64 = printed
                .clone()
                .try_fold(0_u64, |total, attempt| {
                    total.checked_add(u64::from(attempt.quantity))
                })
                .ok_or_else(|| invalid("Model print count overflowed"))?;
            let last = printed.map(|a| a.printed_at.as_str()).max();
            if count != org.print_count || last != org.last_printed_at.as_deref() {
                return Err(invalid("Model print summary is inconsistent"));
            }
            if let Some(bundle_id) = org.bundle_id.as_deref() {
                let bundle = bundles
                    .iter()
                    .find(|bundle| bundle.id == bundle_id)
                    .ok_or_else(|| invalid("Model bundle is missing"))?;
                if let Some(bundle_root) = bundle.source_root_id.as_deref() {
                    let source = model.indexed_source.as_ref().ok_or_else(|| {
                        invalid("Managed model cannot reference an automatic source bundle")
                    })?;
                    if bundle_root != source.root_id {
                        return Err(invalid(
                            "Automatic bundle root does not match its model source",
                        ));
                    }
                    let expected = if source.archive_member.is_some() {
                        source.relative_path.clone()
                    } else {
                        let parent = Path::new(&source.relative_path)
                            .parent()
                            .and_then(Path::to_str)
                            .filter(|value| !value.is_empty())
                            .ok_or_else(|| {
                                invalid(
                                    "Root-level model cannot reference an automatic folder bundle",
                                )
                            })?;
                        format!("{}/", parent.trim_end_matches('/'))
                    };
                    if bundle.source_path.as_deref() != Some(expected.as_str()) {
                        return Err(invalid(
                            "Automatic bundle path does not match its model source",
                        ));
                    }
                }
            }
        }
    }
    Ok(RecoveryManifest {
        manifest_version: MANIFEST_VERSION,
        library_schema_version: schema_version,
        library_id,
        library_name,
        library_created_at,
        folders,
        tags,
        model_tags,
        models,
        index_roots,
        catalog_settings,
        bundles,
        filament_sources,
        filaments,
        captures,
        print_batch_receipts,
        parts_stock,
        printers,
        printer_maintenance_logs,
        printer_reminders,
    })
}

fn read_printer_catalog(
    conn: &Connection,
    schema_version: u32,
) -> Result<
    (
        Vec<RecoveryPrinter>,
        Vec<RecoveryPrinterMaintenanceLog>,
        Vec<RecoveryPrinterReminder>,
    ),
    RecoveryError,
> {
    let rows = collect_rows(
        conn,
        "SELECT p.id,p.metadata_version,p.nickname,p.make,p.model,p.printer_type,p.firmware,
                p.host,p.web_url,p.endpoint_approved,p.bed_x_mm,p.bed_y_mm,p.bed_z_mm,p.notes,
                p.source_kind,p.connection_status,p.job_status,p.last_checked_at,p.connection_error_code,
                p.created_at,p.updated_at,
                COALESCE((SELECT SUM(a.quantity) FROM print_attempts a
                          WHERE a.printer_id=p.id AND a.outcome='printed'),0),
                (SELECT MAX(a.printed_at) FROM print_attempts a
                 WHERE a.printer_id=p.id AND a.outcome='printed')
         FROM printers p ORDER BY lower(p.nickname),p.id",
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, Option<f64>>(10)?,
                row.get::<_, Option<f64>>(11)?,
                row.get::<_, Option<f64>>(12)?,
                row.get::<_, String>(13)?,
                row.get::<_, String>(14)?,
                row.get::<_, String>(15)?,
                row.get::<_, String>(16)?,
                row.get::<_, Option<String>>(17)?,
                row.get::<_, Option<String>>(18)?,
                row.get::<_, String>(19)?,
                row.get::<_, String>(20)?,
                row.get::<_, i64>(21)?,
                row.get::<_, Option<String>>(22)?,
            ))
        },
    )?;
    if rows.len() > MAX_PRINTERS {
        return Err(invalid("Library has too many printer records for recovery"));
    }

    let mut printers = Vec::with_capacity(rows.len());
    let mut printer_ids = BTreeSet::new();
    for (
        id,
        metadata_version,
        nickname,
        make,
        model,
        printer_type,
        firmware,
        host,
        web_url,
        endpoint_approved,
        bed_x,
        bed_y,
        bed_z,
        notes,
        source_kind,
        connection_status,
        job_status,
        last_checked_at,
        connection_error_code,
        created_at,
        updated_at,
        print_count,
        last_printed_at,
    ) in rows
    {
        let bed_mm = match (bed_x, bed_y, bed_z) {
            (None, None, None) => None,
            (Some(x), Some(y), Some(z))
                if [x, y, z]
                    .iter()
                    .all(|value| value.is_finite() && (0.1..=10_000.0).contains(value)) =>
            {
                Some([x as f32, y as f32, z as f32])
            }
            _ => return Err(invalid("Printer bed dimensions are invalid")),
        };
        if invalid_catalog_record_id(&id)
            || !printer_ids.insert(id.clone())
            || metadata_version <= 0
            || invalid_printer_label(&nickname)
            || invalid_optional_printer_label(make.as_deref())
            || invalid_optional_printer_label(model.as_deref())
            || invalid_optional_printer_label(printer_type.as_deref())
            || invalid_optional_printer_label(firmware.as_deref())
            || host.as_deref().is_some_and(invalid_printer_host)
            || web_url.as_deref().is_some_and(invalid_printer_web_url)
            || !matches!(endpoint_approved, 0 | 1)
            || (endpoint_approved == 1 && host.is_none() && web_url.is_none())
            || invalid_printer_notes(&notes)
            || !matches!(source_kind.as_str(), "manual" | "discovered" | "imported")
            || !matches!(
                connection_status.as_str(),
                "unknown" | "online" | "offline" | "auth_required" | "error"
            )
            || !matches!(
                job_status.as_str(),
                "unknown" | "idle" | "printing" | "paused" | "complete"
            )
            || (connection_status != "online" && job_status != "unknown")
            || connection_error_code.as_deref().is_some_and(|code| {
                code.is_empty()
                    || code.len() > 64
                    || !code.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'_' | b'-')
                    })
                    || matches!(connection_status.as_str(), "online" | "unknown")
            })
            || last_checked_at
                .as_deref()
                .is_some_and(|value| !valid_database_timestamp(value))
            || print_count < 0
            || last_printed_at
                .as_deref()
                .is_some_and(|value| !valid_rfc3339(value))
            || !valid_database_timestamp(&created_at)
            || !valid_database_timestamp(&updated_at)
        {
            return Err(invalid("Printer metadata is invalid"));
        }
        printers.push(RecoveryPrinter {
            id,
            metadata_version: u64::try_from(metadata_version)
                .map_err(|_| invalid("Printer metadata version is invalid"))?,
            nickname,
            make,
            model,
            printer_type,
            firmware,
            host,
            web_url,
            // Approval belongs to this host and is never portable.
            endpoint_approved: false,
            bed_mm,
            notes,
            source_kind,
            connection_status,
            job_status,
            last_checked_at,
            connection_error_code,
            print_count: u64::try_from(print_count)
                .map_err(|_| invalid("Printer print count is invalid"))?,
            last_printed_at,
            created_at,
            updated_at,
        });
    }

    let printer_ids = printers
        .iter()
        .map(|printer| printer.id.clone())
        .collect::<BTreeSet<_>>();
    let maintenance_query = if schema_version >= 8 {
        "SELECT id,metadata_version,printer_id,printer_nickname,title,performed_at,notes,created_at
         FROM printer_maintenance_logs ORDER BY performed_at,id"
    } else {
        "SELECT id,1,printer_id,printer_nickname,title,performed_at,notes,created_at
         FROM printer_maintenance_logs ORDER BY performed_at,id"
    };
    let maintenance_rows = collect_rows(conn, maintenance_query, |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
        ))
    })?;
    if maintenance_rows.len() > MAX_PRINTER_HISTORY_ROWS {
        return Err(invalid(
            "Library has too many printer maintenance logs for recovery",
        ));
    }
    let mut maintenance_logs = Vec::with_capacity(maintenance_rows.len());
    for (
        id,
        metadata_version,
        printer_id,
        printer_nickname,
        title,
        performed_at,
        notes,
        created_at,
    ) in maintenance_rows
    {
        let metadata_version = u64::try_from(metadata_version)
            .map_err(|_| invalid("Printer maintenance log version is invalid"))?;
        maintenance_logs.push(RecoveryPrinterMaintenanceLog {
            id,
            metadata_version,
            printer_id,
            printer_nickname,
            title,
            performed_at,
            notes,
            created_at,
        });
    }
    let mut log_ids = BTreeSet::new();
    if maintenance_logs.iter().any(|log| {
        invalid_catalog_record_id(&log.id)
            || log.metadata_version == 0
            || !log_ids.insert(log.id.as_str())
            || log
                .printer_id
                .as_deref()
                .is_some_and(|id| invalid_catalog_record_id(id) || !printer_ids.contains(id))
            || invalid_printer_snapshot_label(&log.printer_nickname)
            || invalid_printer_label(&log.title)
            || invalid_printer_notes(&log.notes)
            || !valid_rfc3339(&log.performed_at)
            || !valid_database_timestamp(&log.created_at)
    }) {
        return Err(invalid(
            "Printer maintenance log metadata or reference is invalid",
        ));
    }

    let reminders = collect_rows(
        conn,
        "SELECT id,metadata_version,printer_id,printer_nickname,title,notes,due_at,
                interval_days,status,last_completed_at,created_at,updated_at
         FROM printer_reminders ORDER BY due_at,id",
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
            ))
        },
    )?;
    if reminders.len() > MAX_PRINTER_HISTORY_ROWS {
        return Err(invalid(
            "Library has too many printer reminders for recovery",
        ));
    }
    let mut reminder_ids = BTreeSet::new();
    let mut printer_reminders = Vec::with_capacity(reminders.len());
    for (
        id,
        metadata_version,
        printer_id,
        printer_nickname,
        title,
        notes,
        due_at,
        interval_days,
        status,
        last_completed_at,
        created_at,
        updated_at,
    ) in reminders
    {
        if invalid_catalog_record_id(&id)
            || !reminder_ids.insert(id.clone())
            || metadata_version <= 0
            || printer_id
                .as_deref()
                .is_some_and(|id| invalid_catalog_record_id(id) || !printer_ids.contains(id))
            || invalid_printer_snapshot_label(&printer_nickname)
            || invalid_printer_label(&title)
            || invalid_printer_notes(&notes)
            || !valid_rfc3339(&due_at)
            || interval_days.is_some_and(|days| !(1..=36_500).contains(&days))
            || !matches!(status.as_str(), "active" | "paused" | "completed")
            || last_completed_at
                .as_deref()
                .is_some_and(|value| !valid_rfc3339(value))
            || !valid_database_timestamp(&created_at)
            || !valid_database_timestamp(&updated_at)
        {
            return Err(invalid("Printer reminder metadata or reference is invalid"));
        }
        printer_reminders.push(RecoveryPrinterReminder {
            id,
            metadata_version: u64::try_from(metadata_version)
                .map_err(|_| invalid("Printer reminder metadata version is invalid"))?,
            printer_id,
            printer_nickname,
            title,
            notes,
            due_at,
            interval_days: interval_days
                .map(u32::try_from)
                .transpose()
                .map_err(|_| invalid("Printer reminder interval is invalid"))?,
            status,
            last_completed_at,
            created_at,
            updated_at,
        });
    }

    Ok((printers, maintenance_logs, printer_reminders))
}

fn populate_asset_bytes(root: &Path, manifest: &mut RecoveryManifest) -> Result<(), RecoveryError> {
    for model in &mut manifest.models {
        if let Some(manual) = &mut model.manual {
            manual.bytes = managed_file(root, &manual.original_relative_path, "manuals")?
                .metadata()?
                .len();
        }
        for thumbnail in &mut model.thumbnails {
            thumbnail.asset.bytes =
                managed_file(root, &thumbnail.asset.original_relative_path, "thumbnails")?
                    .metadata()?
                    .len();
        }
    }
    Ok(())
}

fn collect_rows<T>(
    conn: &Connection,
    sql: &str,
    mut convert: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>, RecoveryError> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        if result.len() >= MAX_ROWS {
            return Err(invalid("Library catalog exceeds recovery row limit"));
        }
        result.push(convert(row)?);
    }
    Ok(result)
}

fn validate_schema(conn: &Connection) -> Result<(), RecoveryError> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if !(1..=LIBRARY_SCHEMA_VERSION as i64).contains(&version) {
        return Err(invalid(format!(
            "Unsupported Library schema version {version}; recovery supports versions 1 through {LIBRARY_SCHEMA_VERSION}"
        )));
    }
    const TABLES: &[(&str, &[&str])] = &[
        ("libraries", &["id", "name", "created_at"]),
        ("folders", &["id", "name", "parent_id"]),
        ("blobs", &["hash", "relative_path", "bytes"]),
        (
            "models",
            &[
                "id",
                "display_name",
                "description",
                "folder_id",
                "current_revision_id",
                "metadata_version",
                "deleted_at",
                "created_at",
                "updated_at",
            ],
        ),
        (
            "revisions",
            &[
                "id",
                "model_id",
                "blob_hash",
                "original_name",
                "format",
                "bytes",
                "created_at",
            ],
        ),
        ("tags", &["id", "name"]),
        ("model_tags", &["model_id", "tag_id"]),
        ("manuals", &["model_id", "relative_path", "content_hash"]),
        (
            "thumbnails",
            &[
                "id",
                "model_id",
                "relative_path",
                "content_hash",
                "is_custom",
            ],
        ),
        (
            "jobs",
            &[
                "id",
                "model_id",
                "kind",
                "status",
                "retry_count",
                "created_at",
                "updated_at",
            ],
        ),
    ];
    let mut tables = TABLES.to_vec();
    if version >= 2 {
        tables.extend_from_slice(&[
            (
                "index_roots",
                &[
                    "id",
                    "path",
                    "enabled",
                    "last_scanned_at",
                    "scan_interval_minutes",
                    "max_file_bytes",
                ],
            ),
            (
                "indexed_model_data",
                &[
                    "model_id",
                    "content_hash",
                    "original_name",
                    "format",
                    "bytes",
                ],
            ),
        ]);
        let mut stmt = conn.prepare("PRAGMA table_info(models)")?;
        let columns: BTreeSet<String> = stmt
            .query_map([], |row| row.get(1))?
            .collect::<Result<_, _>>()?;
        for column in [
            "source_kind",
            "source_path",
            "source_root_id",
            "source_missing",
            "source_modified_at",
        ] {
            if !columns.contains(column) {
                return Err(invalid("Indexed model columns are missing"));
            }
        }
        let source_bytes: i64 = conn.query_row(
            "SELECT COALESCE(SUM(length(source_path)+length(source_root_id)),0) FROM models",
            [],
            |row| row.get(0),
        )?;
        if source_bytes > MAX_METADATA_BYTES {
            return Err(invalid("Indexed metadata exceeds recovery limit"));
        }
    }
    if version >= 3 {
        tables.extend_from_slice(&[
            (
                "bundles",
                &["id", "source_root_id", "source_path", "created_at"],
            ),
            (
                "print_attempts",
                &[
                    "id",
                    "model_id",
                    "printed_at",
                    "quantity",
                    "notes",
                    "outcome",
                    "created_at",
                    "updated_at",
                ],
            ),
            (
                "print_attempt_filaments",
                &["attempt_id", "filament_id", "ordinal"],
            ),
        ]);
        for (table, required) in [
            (
                "models",
                &[
                    "source_archive_member",
                    "designer",
                    "source_url",
                    "notes",
                    "model_license",
                    "print_status",
                    "parent_model_id",
                    "last_printed_at",
                    "print_count",
                    "bundle_id",
                ][..],
            ),
            ("tags", &["category"][..]),
            ("index_roots", &["scan_zip"][..]),
            ("indexed_model_data", &["outer_fingerprint"][..]),
        ] {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let columns: BTreeSet<String> = stmt
                .query_map([], |row| row.get(1))?
                .collect::<Result<_, _>>()?;
            if required.iter().any(|column| !columns.contains(*column)) {
                return Err(invalid(format!(
                    "Library table {table} has an unsupported schema-3 layout"
                )));
            }
        }
        let phase3_bytes:i64=conn.query_row("SELECT COALESCE((SELECT SUM(length(COALESCE(source_archive_member,''))+length(COALESCE(designer,''))+length(COALESCE(source_url,''))+length(COALESCE(notes,''))+length(COALESCE(model_license,''))+length(COALESCE(parent_model_id,''))+length(COALESCE(bundle_id,''))) FROM models),0)+COALESCE((SELECT SUM(length(COALESCE(category,''))) FROM tags),0)+COALESCE((SELECT SUM(length(notes)+length(printed_at)) FROM print_attempts),0)+COALESCE((SELECT SUM(length(filament_id)) FROM print_attempt_filaments),0)",[],|row|row.get(0))?;
        if phase3_bytes > MAX_METADATA_BYTES {
            return Err(invalid("Schema-3 metadata exceeds recovery limit"));
        }
    }
    if version >= 4 {
        tables.extend_from_slice(&[
            (
                "filament_sources",
                &[
                    "id",
                    "kind",
                    "base_url",
                    "last_synced_at",
                    "created_at",
                    "updated_at",
                ],
            ),
            (
                "filaments",
                &[
                    "id",
                    "version",
                    "brand",
                    "material",
                    "color",
                    "name",
                    "notes",
                    "origin",
                    "resource_kind",
                    "source_id",
                    "external_id",
                    "parent_filament_id",
                    "archived",
                    "source_missing",
                    "created_at",
                    "updated_at",
                ],
            ),
            ("model_filaments", &["model_id", "filament_id", "ordinal"]),
        ]);
        let phase4_bytes: i64 = conn.query_row(
            "SELECT COALESCE((SELECT SUM(length(id)+length(kind)+length(base_url)+length(COALESCE(last_synced_at,''))) FROM filament_sources),0)
                  +COALESCE((SELECT SUM(length(id)+length(brand)+length(material)+length(color)+length(name)+length(notes)+length(origin)+length(resource_kind)+length(COALESCE(source_id,''))+length(COALESCE(external_id,''))+length(COALESCE(parent_filament_id,''))) FROM filaments),0)
                  +COALESCE((SELECT SUM(length(model_id)+length(filament_id)) FROM model_filaments),0)",
            [],
            |row| row.get(0),
        )?;
        if phase4_bytes > MAX_METADATA_BYTES {
            return Err(invalid("Schema-4 filament metadata exceeds recovery limit"));
        }
    }
    if version >= 5 {
        tables.extend_from_slice(&[
            (
                "capture_items",
                &[
                    "item_id",
                    "payload_hash",
                    "status",
                    "page_model_id",
                    "created_at",
                    "received_at",
                    "updated_at",
                    "error_code",
                ],
            ),
            (
                "capture_downloads",
                &[
                    "capture_id",
                    "item_id",
                    "ordinal",
                    "file_name",
                    "expected_hash",
                    "expected_bytes",
                    "status",
                    "model_id",
                    "error_code",
                ],
            ),
        ]);
        for (table, required) in [
            ("models", &["catalog_kind"][..]),
            ("thumbnails", &["ordinal", "is_active"][..]),
        ] {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let columns: BTreeSet<String> = stmt
                .query_map([], |row| row.get(1))?
                .collect::<Result<_, _>>()?;
            if required.iter().any(|column| !columns.contains(*column)) {
                return Err(invalid(format!(
                    "Library table {table} has an unsupported schema-5 layout"
                )));
            }
        }
        let phase5_bytes: i64 = conn.query_row(
            "SELECT COALESCE((SELECT SUM(length(item_id)+length(payload_hash)+length(status)+length(COALESCE(page_model_id,''))+length(COALESCE(error_code,''))) FROM capture_items),0)
                  +COALESCE((SELECT SUM(length(capture_id)+length(item_id)+length(file_name)+length(COALESCE(expected_hash,''))+length(status)+length(COALESCE(model_id,''))+length(COALESCE(error_code,''))) FROM capture_downloads),0)",
            [],
            |row| row.get(0),
        )?;
        if phase5_bytes > MAX_METADATA_BYTES {
            return Err(invalid("Schema-5 capture metadata exceeds recovery limit"));
        }
    }
    if version >= 7 {
        tables.extend_from_slice(&[
            (
                "printers",
                &[
                    "id",
                    "metadata_version",
                    "nickname",
                    "make",
                    "model",
                    "printer_type",
                    "firmware",
                    "host",
                    "web_url",
                    "endpoint_approved",
                    "bed_x_mm",
                    "bed_y_mm",
                    "bed_z_mm",
                    "notes",
                    "source_kind",
                    "connection_status",
                    "job_status",
                    "last_checked_at",
                    "connection_error_code",
                    "created_at",
                    "updated_at",
                ],
            ),
            (
                "printer_maintenance_logs",
                &[
                    "id",
                    "printer_id",
                    "printer_nickname",
                    "title",
                    "performed_at",
                    "notes",
                    "created_at",
                ],
            ),
            (
                "printer_reminders",
                &[
                    "id",
                    "metadata_version",
                    "printer_id",
                    "printer_nickname",
                    "title",
                    "notes",
                    "due_at",
                    "interval_days",
                    "status",
                    "last_completed_at",
                    "created_at",
                    "updated_at",
                ],
            ),
        ]);
        tables.push(("print_attempts", &["printer_id", "printer_name"]));
    }
    if version >= 8 {
        tables.push(("printer_maintenance_logs", &["metadata_version"]));
    }
    if version >= 9 {
        tables.extend_from_slice(&[
            (
                "parts_stock",
                &[
                    "id",
                    "metadata_version",
                    "name",
                    "category",
                    "quantity",
                    "unit",
                    "notes",
                    "low_stock",
                    "created_at",
                    "updated_at",
                ],
            ),
            (
                "print_attempt_parts",
                &[
                    "attempt_id",
                    "part_id",
                    "part_name",
                    "category",
                    "unit",
                    "quantity_per_copy",
                    "quantity",
                    "ordinal",
                ],
            ),
        ]);
    }
    if version >= 10 {
        tables.push(("print_attempts", &["stock_adjusted"]));
    }
    for (table, required) in &tables {
        let found_type: Option<String> = conn
            .query_row(
                "SELECT type FROM sqlite_master WHERE name = ?1",
                [table],
                |row| row.get(0),
            )
            .optional()?;
        if found_type.as_deref() != Some("table") {
            return Err(invalid(format!("Library table {table} is missing")));
        }
        let mut columns = BTreeSet::new();
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            columns.insert(row.get::<_, String>(1)?);
        }
        if required.iter().any(|name| !columns.contains(*name)) {
            return Err(invalid(format!(
                "Library table {table} has an unsupported layout"
            )));
        }
    }
    let model_count: i64 = conn.query_row("SELECT COUNT(*) FROM models", [], |row| row.get(0))?;
    if model_count < 0 || model_count as usize > MAX_MODELS {
        return Err(invalid("Library has too many models for recovery"));
    }
    if version >= 9 {
        let parts_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM parts_stock", [], |row| row.get(0))?;
        let part_usage_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM print_attempt_parts", [], |row| {
                row.get(0)
            })?;
        if !(0..=MAX_PART_STOCK_RECORDS as i64).contains(&parts_count)
            || !(0..=MAX_ROWS as i64).contains(&part_usage_count)
        {
            return Err(invalid("Part-stock catalog exceeds recovery limits"));
        }
    }
    let mut metadata_bytes = 0i64;
    for (table, columns) in tables.iter().filter(|(table, _)| *table != "jobs") {
        let terms = columns
            .iter()
            .map(|column| format!("COALESCE(length(CAST({column} AS BLOB)), 0)"))
            .collect::<Vec<_>>()
            .join(" + ");
        let sql = format!("SELECT COALESCE(SUM({terms}), 0) FROM {table}");
        let bytes: i64 = conn.query_row(&sql, [], |row| row.get(0))?;
        metadata_bytes = metadata_bytes
            .checked_add(bytes)
            .ok_or_else(|| invalid("Library metadata exceeds recovery limit"))?;
        if metadata_bytes > MAX_METADATA_BYTES {
            return Err(invalid("Library metadata exceeds recovery limit"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod catalog_vfs_tests {
    use super::*;
    use library_core::LibraryDb;

    #[test]
    fn recovery_reads_committed_wal_state_from_the_catalog_vfs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        drop(LibraryDb::open(&root).unwrap());

        let database = root.join("library.sqlite3");
        let pinned = File::open(&database).unwrap();
        let vfs = library_core::catalog_vfs::CatalogVfs::new(&database, &pinned).unwrap();
        let writer = Connection::open_with_flags_and_vfs(
            vfs.sqlite_path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            vfs.vfs_name(),
        )
        .unwrap();
        let mode: String = writer
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        drop(writer);
        drop(vfs);

        let db = LibraryDb::open(&root).unwrap();
        db.create_folder("Committed WAL folder", None).unwrap();

        let manifest = inspect_library(&root).unwrap();
        assert!(manifest
            .folders
            .iter()
            .any(|folder| folder.name == "Committed WAL folder"));
    }
}

#[cfg(test)]
mod phase8_recovery_tests {
    use super::{export_library_zip, inspect_library, MAX_PRINTER_NOTES_BYTES};
    use library_core::{
        LibraryDb, PrintAttemptInput, PrintAttemptInputWithPrinter, PrintOutcome, PrinterInput,
        PrinterMaintenanceInput, PrinterReminderInput,
    };
    use rusqlite::Connection;
    use std::fs::{self, File};
    use std::io::Read;
    use std::path::PathBuf;
    use zip::ZipArchive;

    const SYNTHETIC_STL: &[u8] = b"solid fixture\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid fixture\n";

    struct Phase8Fixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        model_id: String,
        printer_id: String,
        attempt_id: String,
        maintenance_id: String,
        reminder_id: String,
    }

    fn phase8_fixture() -> Phase8Fixture {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let source = temp.path().join("synthetic.stl");
        fs::write(&source, SYNTHETIC_STL).unwrap();

        let db = LibraryDb::open(&root).unwrap();
        let model = db
            .import_file(&source, None, Some("Synthetic model"))
            .unwrap()
            .model;
        let printer = db
            .create_printer(
                &PrinterInput {
                    nickname: "Synthetic printer".into(),
                    make: Some("Fixture Maker".into()),
                    model: Some("Fixture One".into()),
                    printer_type: Some("FDM".into()),
                    firmware: Some("1.0".into()),
                    host: Some("printer.local".into()),
                    web_url: Some("https://printer.local/".into()),
                    bed_mm: Some([256.0, 256.0, 300.0]),
                    notes: "Local fixture only".into(),
                },
                true,
            )
            .unwrap();
        let attempt = db
            .create_print_attempt_with_printer(
                &model.id,
                &PrintAttemptInputWithPrinter {
                    input: PrintAttemptInput {
                        printed_at: "2026-01-02T03:04:05Z".into(),
                        quantity: 2,
                        notes: "Synthetic print history".into(),
                        filament_ids: vec![],
                        outcome: PrintOutcome::Printed,
                        parts: None,
                    },
                    printer_id: Some(Some(printer.id.clone())),
                },
            )
            .unwrap();
        let maintenance = db
            .create_printer_maintenance_log(
                &printer.id,
                &PrinterMaintenanceInput {
                    title: "Clean nozzle".into(),
                    performed_at: "2026-01-03T04:05:06Z".into(),
                    notes: "Synthetic maintenance log".into(),
                },
            )
            .unwrap();
        let maintenance = db
            .update_printer_maintenance_log(
                &maintenance.id,
                maintenance.metadata_version,
                &PrinterMaintenanceInput {
                    title: "Clean nozzle".into(),
                    performed_at: "2026-01-03T04:05:06Z".into(),
                    notes: "Synthetic maintenance log version 2".into(),
                },
            )
            .unwrap();
        assert_eq!(maintenance.metadata_version, 2);
        let reminder = db
            .create_printer_reminder(
                &printer.id,
                &PrinterReminderInput {
                    title: "Check belts".into(),
                    notes: "Synthetic reminder".into(),
                    due_at: "2026-02-03T04:05:06Z".into(),
                    interval_days: Some(90),
                },
            )
            .unwrap();
        drop(db);

        Phase8Fixture {
            _temp: temp,
            root,
            model_id: model.id,
            printer_id: printer.id,
            attempt_id: attempt.id,
            maintenance_id: maintenance.id,
            reminder_id: reminder.id,
        }
    }

    fn archive_manifest(path: &std::path::Path) -> serde_json::Value {
        let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
        let mut bytes = Vec::new();
        archive
            .by_name("manifest.json")
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn schema_v8_recovery_exports_printers_history_reminders_and_attempt_snapshots() {
        let fixture = phase8_fixture();
        let manifest = inspect_library(&fixture.root).unwrap();

        assert_eq!(
            manifest.library_schema_version,
            crate::LIBRARY_SCHEMA_VERSION
        );
        assert_eq!(manifest.printers.len(), 1);
        let printer = &manifest.printers[0];
        assert_eq!(printer.id, fixture.printer_id);
        assert_eq!(printer.nickname, "Synthetic printer");
        assert_eq!(printer.host.as_deref(), Some("printer.local"));
        assert_eq!(printer.web_url.as_deref(), Some("https://printer.local"));
        assert!(!printer.endpoint_approved);
        assert_eq!(printer.bed_mm, Some([256.0, 256.0, 300.0]));
        assert_eq!(manifest.printer_maintenance_logs.len(), 1);
        assert_eq!(
            manifest.printer_maintenance_logs[0].id,
            fixture.maintenance_id
        );
        assert_eq!(manifest.printer_maintenance_logs[0].metadata_version, 2);
        assert_eq!(manifest.printer_reminders.len(), 1);
        assert_eq!(manifest.printer_reminders[0].id, fixture.reminder_id);
        let attempt = manifest.models[0]
            .print_attempts
            .iter()
            .find(|attempt| attempt.id == fixture.attempt_id)
            .unwrap();
        assert_eq!(
            attempt.printer_id.as_deref(),
            Some(fixture.printer_id.as_str())
        );
        assert_eq!(attempt.printer_name.as_deref(), Some("Synthetic printer"));

        let archive_path = fixture.root.parent().unwrap().join("phase8-recovery.zip");
        export_library_zip(
            &fixture.root,
            std::slice::from_ref(&fixture.model_id),
            &archive_path,
        )
        .unwrap();
        let portable = archive_manifest(&archive_path);
        assert_eq!(portable["printers"][0]["endpoint_approved"], false);
        assert_eq!(
            portable["printer_maintenance_logs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            portable["printer_maintenance_logs"][0]["metadata_version"],
            2
        );
        assert_eq!(portable["printer_reminders"].as_array().unwrap().len(), 1);
        assert_eq!(
            portable["models"][0]["print_attempts"][0]["printer_name"],
            "Synthetic printer"
        );
        let printer_json = portable["printers"][0].as_object().unwrap();
        for secret_field in ["credential", "password", "access_code", "api_key", "token"] {
            assert!(!printer_json.contains_key(secret_field));
        }
    }

    #[test]
    fn schema_v7_recovery_defaults_maintenance_log_version_to_one() {
        let fixture = phase8_fixture();
        let conn = Connection::open(fixture.root.join("library.sqlite3")).unwrap();
        conn.execute_batch(
            "ALTER TABLE printer_maintenance_logs DROP COLUMN metadata_version;
             PRAGMA user_version=7;",
        )
        .unwrap();
        drop(conn);

        let manifest = inspect_library(&fixture.root).unwrap();
        assert_eq!(manifest.library_schema_version, 7);
        assert_eq!(manifest.printer_maintenance_logs.len(), 1);
        assert_eq!(manifest.printer_maintenance_logs[0].metadata_version, 1);
    }

    #[test]
    fn schema_v6_exports_empty_printer_defaults_and_no_attempt_snapshots() {
        let fixture = phase8_fixture();
        let database = fixture.root.join("library.sqlite3");
        let conn = Connection::open(&database).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=OFF;
             DROP INDEX IF EXISTS printer_reminders_due_idx;
             DROP INDEX IF EXISTS printer_reminders_printer_idx;
             DROP INDEX IF EXISTS printer_maintenance_printer_idx;
             DROP INDEX IF EXISTS printer_name_idx;
             DROP INDEX IF EXISTS print_attempt_printer_idx;
             DROP TABLE printer_reminders;
             DROP TABLE printer_maintenance_logs;
             ALTER TABLE print_attempts DROP COLUMN printer_name;
             ALTER TABLE print_attempts DROP COLUMN printer_id;
             DROP TABLE printers;
             PRAGMA user_version=6;",
        )
        .unwrap();
        drop(conn);

        let manifest = inspect_library(&fixture.root).unwrap();
        assert_eq!(manifest.library_schema_version, 6);
        assert!(manifest.printers.is_empty());
        assert!(manifest.printer_maintenance_logs.is_empty());
        assert!(manifest.printer_reminders.is_empty());
        let attempt = manifest.models[0]
            .print_attempts
            .iter()
            .find(|attempt| attempt.id == fixture.attempt_id)
            .unwrap();
        assert_eq!(attempt.printer_id, None);
        assert_eq!(attempt.printer_name, None);

        let archive_path = fixture.root.parent().unwrap().join("phase6-recovery.zip");
        export_library_zip(
            &fixture.root,
            std::slice::from_ref(&fixture.model_id),
            &archive_path,
        )
        .unwrap();
        let portable = archive_manifest(&archive_path);
        assert!(portable.get("printers").is_none());
        assert!(portable.get("printer_maintenance_logs").is_none());
        assert!(portable.get("printer_reminders").is_none());
        assert!(portable["models"][0]["print_attempts"][0]
            .get("printer_id")
            .is_none());
    }

    #[test]
    fn schema_v8_recovery_rejects_dangling_printer_references() {
        let fixture = phase8_fixture();
        let conn = Connection::open(fixture.root.join("library.sqlite3")).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        conn.execute(
            "UPDATE printer_reminders SET printer_id='missing-printer' WHERE id=?1",
            rusqlite::params![&fixture.reminder_id],
        )
        .unwrap();
        drop(conn);

        assert!(inspect_library(&fixture.root).is_err());

        let fixture = phase8_fixture();
        let conn = Connection::open(fixture.root.join("library.sqlite3")).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        conn.execute(
            "UPDATE print_attempts SET printer_id='missing-printer' WHERE id=?1",
            rusqlite::params![&fixture.attempt_id],
        )
        .unwrap();
        drop(conn);
        assert!(inspect_library(&fixture.root).is_err());

        let fixture = phase8_fixture();
        let conn = Connection::open(fixture.root.join("library.sqlite3")).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        conn.execute(
            "UPDATE printer_maintenance_logs SET printer_id='missing-printer' WHERE id=?1",
            rusqlite::params![&fixture.maintenance_id],
        )
        .unwrap();
        drop(conn);
        assert!(inspect_library(&fixture.root).is_err());
    }

    #[test]
    fn schema_v8_recovery_rejects_credentials_embedded_in_printer_endpoints() {
        for (update, value) in [
            ("UPDATE printers SET host=?1", "user:secret@printer.local"),
            (
                "UPDATE printers SET web_url=?1",
                "https://user:secret@printer.local/",
            ),
        ] {
            let fixture = phase8_fixture();
            let conn = Connection::open(fixture.root.join("library.sqlite3")).unwrap();
            conn.execute(update, rusqlite::params![value]).unwrap();
            drop(conn);
            assert!(inspect_library(&fixture.root).is_err());
        }
    }

    #[test]
    fn schema_v8_recovery_rejects_oversized_printer_notes() {
        let fixture = phase8_fixture();
        let conn = Connection::open(fixture.root.join("library.sqlite3")).unwrap();
        conn.execute_batch("PRAGMA ignore_check_constraints=ON;")
            .unwrap();
        conn.execute(
            "UPDATE printers SET notes=?1 WHERE id=?2",
            rusqlite::params!["x".repeat(MAX_PRINTER_NOTES_BYTES + 1), &fixture.printer_id,],
        )
        .unwrap();
        drop(conn);

        assert!(inspect_library(&fixture.root).is_err());
    }
}
