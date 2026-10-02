//! Read-only recovery of the pre-existing Library schema. This crate never
//! creates a Library directory or opens the mutable LibraryDb.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

pub const MANIFEST_VERSION: u32 = 1;
pub const LIBRARY_SCHEMA_VERSION: u32 = 1;
pub const MAX_COLORWAY_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_MODELS: usize = 10_000;
const MAX_ROWS: usize = 50_000;
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
    pub asset: RecoveryAsset,
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

fn read_library(
    root: &Path,
    selected_ids: Option<&[String]>,
) -> Result<RecoveryManifest, RecoveryError> {
    let root = existing_library_root(root)?;
    let database = root.join("library.sqlite3");
    let database_metadata = database
        .symlink_metadata()
        .map_err(|_| invalid("No existing Library database was found"))?;
    if !database_metadata.is_file() || database_metadata.file_type().is_symlink() {
        return Err(invalid("Library database must be an ordinary file"));
    }
    let conn = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.execute_batch("BEGIN DEFERRED TRANSACTION")?;
    let mut manifest = read_catalog(&conn)?;
    if let Some(ids) = selected_ids {
        select_models(&mut manifest, ids)?;
    }
    populate_asset_bytes(&root, &mut manifest)?;
    validate_catalog_assets(&root, &manifest)?;
    if serde_json::to_vec(&manifest)?.len() > MAX_MANIFEST_BYTES {
        return Err(invalid("Recovery manifest exceeds its size limit"));
    }
    conn.execute_batch("COMMIT")?;
    Ok(manifest)
}

/// Exports selected model IDs, or every model when the selection is empty.
/// Existing destinations are never replaced.
pub fn export_library_zip(
    root: &Path,
    model_ids: &[String],
    destination: &Path,
) -> Result<RecoveryManifest, RecoveryError> {
    let root = existing_library_root(root)?;
    let mut manifest = read_library(&root, Some(model_ids))?;
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
    write_archive(temporary.reopen()?, &root, &mut manifest)?;
    temporary
        .persist_noclobber(destination)
        .map_err(|error| RecoveryError::Io(error.error))?;
    Ok(manifest)
}

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
    let selected: BTreeSet<_> = ids.iter().collect();
    if selected.len() != ids.len() {
        return Err(invalid("Duplicate model ID in recovery selection"));
    }
    let found: BTreeSet<_> = manifest
        .models
        .iter()
        .map(|model| &model.id)
        .filter(|id| selected.contains(id))
        .collect();
    if found != selected {
        return Err(invalid("Recovery selection contains an unknown model ID"));
    }
    manifest.models.retain(|model| selected.contains(&model.id));
    manifest
        .model_tags
        .retain(|link| selected.contains(&link.model_id));
    let tag_ids: BTreeSet<_> = manifest
        .model_tags
        .iter()
        .map(|link| &link.tag_id)
        .collect();
    manifest.tags.retain(|tag| tag_ids.contains(&tag.id));
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
    let mut source = File::open(path)?;
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
    let tags = collect_rows(conn, "SELECT id, name FROM tags ORDER BY id", |row| {
        Ok(RecoveryTag {
            id: row.get(0)?,
            name: row.get(1)?,
        })
    })?;
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
        "SELECT id, display_name, description, folder_id, current_revision_id, metadata_version,
                deleted_at, created_at, updated_at FROM models ORDER BY id",
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
            })
        },
    )?;
    let index: BTreeMap<String, usize> = models
        .iter()
        .enumerate()
        .map(|(position, model)| (model.id.clone(), position))
        .collect();

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
        "SELECT id, model_id, relative_path, content_hash, is_custom
         FROM thumbnails ORDER BY model_id, id",
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        },
    )?;
    for (id, model_id, path, hash, is_custom) in thumbnails {
        let model = index
            .get(&model_id)
            .ok_or_else(|| invalid("A thumbnail references an unknown model"))?;
        if is_custom != 0 && is_custom != 1 {
            return Err(invalid("A thumbnail has invalid custom state"));
        }
        models[*model].thumbnails.push(RecoveryThumbnail {
            id,
            model_id,
            is_custom: is_custom == 1,
            asset: RecoveryAsset {
                original_relative_path: path,
                exported_path: None,
                bytes: 0,
                sha256: hash,
            },
        });
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
        if !folder_ids.contains(&model.folder_id) || model.metadata_version <= 0 {
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
    Ok(RecoveryManifest {
        manifest_version: MANIFEST_VERSION,
        library_schema_version: LIBRARY_SCHEMA_VERSION,
        library_id,
        library_name,
        library_created_at,
        folders,
        tags,
        model_tags,
        models,
    })
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
    if version != LIBRARY_SCHEMA_VERSION as i64 {
        return Err(invalid(format!(
            "Unsupported Library schema version {version}; recovery supports version 1"
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
    for (table, required) in TABLES {
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
    let mut metadata_bytes = 0i64;
    for (table, columns) in TABLES.iter().filter(|(table, _)| *table != "jobs") {
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
