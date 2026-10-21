use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use library_core::catalog_vfs::{open_pinned_file, same_file_identity, CatalogVfs};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

pub const MAX_BACKUP_FILE_COUNT: usize = 100_000;
pub const MAX_BACKUP_TREE_ENTRY_COUNT: usize = MAX_BACKUP_FILE_COUNT * 2;
pub const MAX_BACKUP_DIRECTORY_DEPTH: usize = 128;
pub const MAX_BACKUP_PATH_BYTES: usize = 16 * 1024 * 1024;
const MAX_FILES: usize = MAX_BACKUP_FILE_COUNT;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const MAX_BACKUP_ARCHIVE_BYTES: u64 = MAX_TOTAL_BYTES + 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupResult {
    pub path: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupArchiveInfo {
    pub files: usize,
    pub bytes: u64,
    pub sha256: String,
}

/// Supplies paths relative to the Library root and opens them for backup.
/// Implementations may pin the source directory and use handle-relative reads.
#[doc(hidden)]
pub trait BackupAssetSource {
    fn asset_paths(&self) -> Result<Vec<PathBuf>, String>;
    fn open_asset(&self, relative_path: &Path) -> Result<File, String>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    files: Vec<ManifestFile>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestFile {
    path: String,
    bytes: u64,
    sha256: String,
}

pub fn create_backup(library_root: &Path, destination: &Path) -> Result<BackupResult, String> {
    let root = canonical_real_dir(library_root)?;
    if !destination.is_absolute()
        || !matches!(fs::symlink_metadata(destination),Err(error) if error.kind()==std::io::ErrorKind::NotFound)
    {
        return Err("Backup destination must be a new absolute path".into());
    }
    let parent = destination
        .parent()
        .ok_or("Backup destination has no parent")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if parent.starts_with(&root) {
        return Err("Backup destination must be outside the Library".into());
    }
    let named = tempfile::NamedTempFile::new_in(&parent).map_err(|e| e.to_string())?;
    let mut archive = named.reopen().map_err(|e| e.to_string())?;
    let info = write_backup_archive(&root, &mut archive)?;
    archive.sync_all().map_err(|e| e.to_string())?;
    drop(archive);
    named
        .persist_noclobber(destination)
        .map_err(|e| e.error.to_string())?;
    Ok(BackupResult {
        path: destination.to_path_buf(),
        files: info.files,
        bytes: info.bytes,
        sha256: info.sha256,
    })
}

/// Writes a complete backup ZIP to an empty, seekable reader/writer.
/// The path-based convenience source still opens the database and assets by path.
pub fn write_backup_archive<W: Read + Write + Seek>(
    library_root: &Path,
    output: &mut W,
) -> Result<BackupArchiveInfo, String> {
    write_backup_archive_with_limit(library_root, output, MAX_BACKUP_ARCHIVE_BYTES)
}

/// Writes a backup while enforcing the supplied final-archive size limit.
#[doc(hidden)]
pub fn write_backup_archive_with_limit<W: Read + Write + Seek>(
    library_root: &Path,
    output: &mut W,
    archive_limit: u64,
) -> Result<BackupArchiveInfo, String> {
    let root = canonical_real_dir(library_root)?;
    let source = PathBackupAssetSource { root: root.clone() };
    write_backup_archive_from_source_with_limit(&root, &source, output, archive_limit)
}

/// Writes a backup using the supplied asset source. SQLite snapshotting still
/// opens the catalog database by `library_root`; asset enumeration and reads
/// use the caller-provided source.
#[doc(hidden)]
pub fn write_backup_archive_from_source<W: Read + Write + Seek, S: BackupAssetSource + ?Sized>(
    library_root: &Path,
    asset_source: &S,
    output: &mut W,
) -> Result<BackupArchiveInfo, String> {
    write_backup_archive_from_source_with_limit(
        library_root,
        asset_source,
        output,
        MAX_BACKUP_ARCHIVE_BYTES,
    )
}

#[doc(hidden)]
pub fn write_backup_archive_from_source_with_limit<
    W: Read + Write + Seek,
    S: BackupAssetSource + ?Sized,
>(
    library_root: &Path,
    asset_source: &S,
    output: &mut W,
    archive_limit: u64,
) -> Result<BackupArchiveInfo, String> {
    if output.seek(SeekFrom::End(0)).map_err(|e| e.to_string())? != 0 {
        return Err("Backup output must be empty".into());
    }
    output.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let root = canonical_real_dir(library_root)?;
    let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let snapshot = temp.path().join("library.sqlite3");
    let db = root.join("library.sqlite3");
    let catalog_file = open_catalog_file(&db)?;
    let catalog_vfs = CatalogVfs::new(&db, &catalog_file).map_err(|error| error.to_string())?;
    let conn = Connection::open_with_flags_and_vfs(
        catalog_vfs.sqlite_path(),
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        catalog_vfs.vfs_name(),
    )
    .map_err(|error| format!("Could not open the Library catalog for backup: {error:?}"))?;
    catalog_vfs
        .check_connection(&conn)
        .map_err(|error| error.to_string())?;
    let snapshot_sqlite_path = snapshot
        .to_str()
        .ok_or("Backup snapshot path is not valid UTF-8")?;
    catalog_vfs
        .allow_output_path(&snapshot)
        .map_err(|error| error.to_string())?;
    conn.execute("VACUUM INTO ?1", [snapshot_sqlite_path])
        .map_err(|error| format!("Could not create the backup snapshot: {error:?}"))?;
    catalog_vfs
        .check_connection(&conn)
        .map_err(|error| error.to_string())?;
    let mut sources = vec![(
        "library.sqlite3".to_string(),
        BackupFileSource::Snapshot(snapshot),
    )];
    let mut seen = BTreeSet::from(["library.sqlite3".to_string()]);
    let mut total_path_bytes = 0usize;
    let mut assets = asset_source
        .asset_paths()?
        .into_iter()
        .map(|relative| {
            let archive_path = backup_asset_archive_path(&relative)?;
            let path_bytes = archive_path
                .len()
                .checked_add(relative.as_os_str().len())
                .ok_or("Backup path size overflow")?;
            total_path_bytes = total_path_bytes
                .checked_add(path_bytes)
                .ok_or("Backup path size overflow")?;
            if total_path_bytes > MAX_BACKUP_PATH_BYTES {
                return Err("Backup paths exceed total size limit".into());
            }
            if !seen.insert(archive_path.clone()) {
                return Err(format!("Backup asset path is duplicated: {archive_path}"));
            }
            Ok((archive_path, relative))
        })
        .collect::<Result<Vec<_>, String>>()?;
    assets.sort_by(|left, right| {
        backup_namespace_order(&left.0)
            .cmp(&backup_namespace_order(&right.0))
            .then_with(|| left.0.cmp(&right.0))
    });
    for (archive_path, _) in &assets {
        for (separator, _) in archive_path.match_indices('/') {
            if seen.contains(&archive_path[..separator]) {
                return Err(format!(
                    "Backup asset path collides with a file ancestor: {archive_path}"
                ));
            }
        }
    }
    for (archive_path, relative) in assets {
        sources.push((archive_path, BackupFileSource::Asset(relative)));
    }
    if sources.len() > MAX_FILES {
        return Err("Backup contains too many files".into());
    }
    catalog_vfs
        .check_connection(&conn)
        .map_err(|error| error.to_string())?;
    let mut total = 0u64;
    let mut listed = Vec::new();
    for (path, source) in &sources {
        let mut file = open_backup_file(asset_source, source)?;
        let (bytes, hash) = hash_open_file(&mut file, MAX_FILE_BYTES)?;
        total = total.checked_add(bytes).ok_or("Backup size overflow")?;
        if total > MAX_TOTAL_BYTES {
            return Err("Backup exceeds total size limit".into());
        }
        listed.push(ManifestFile {
            path: path.clone(),
            bytes,
            sha256: hash,
        });
    }
    catalog_vfs
        .check_connection(&conn)
        .map_err(|error| error.to_string())?;
    let manifest = Manifest {
        version: 1,
        files: listed,
    };
    let mut zip = zip::ZipWriter::new(output);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o600);
    zip.start_file("backup-manifest.json", options)
        .map_err(|e| e.to_string())?;
    zip.write_all(&serde_json::to_vec(&manifest).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    for ((path, source), expected) in sources.iter().zip(&manifest.files) {
        zip.start_file(path, options).map_err(|e| e.to_string())?;
        let mut input = open_backup_file(asset_source, source)?;
        let mut digest = Sha256::new();
        let mut copied = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            copied = copied
                .checked_add(n as u64)
                .ok_or("Backup source size overflow")?;
            if copied > expected.bytes {
                return Err(format!("Backup source grew: {path}"));
            }
            digest.update(&buffer[..n]);
            zip.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        }
        if copied != expected.bytes || format!("{:x}", digest.finalize()) != expected.sha256 {
            return Err(format!("Backup source changed: {path}"));
        }
    }
    catalog_vfs
        .check_connection(&conn)
        .map_err(|error| error.to_string())?;
    let completed = zip.finish().map_err(|e| e.to_string())?;
    completed.flush().map_err(|e| e.to_string())?;
    let backup_hash = hash_seekable(completed, archive_limit)?;
    catalog_vfs
        .check_connection(&conn)
        .map_err(|error| error.to_string())?;
    Ok(BackupArchiveInfo {
        files: manifest.files.len(),
        bytes: total,
        sha256: backup_hash,
    })
}

enum BackupFileSource {
    Snapshot(PathBuf),
    Asset(PathBuf),
}

fn open_backup_file<S: BackupAssetSource + ?Sized>(
    asset_source: &S,
    source: &BackupFileSource,
) -> Result<File, String> {
    match source {
        BackupFileSource::Snapshot(path) => {
            open_pinned_file(path).map_err(|error| error.to_string())
        }
        BackupFileSource::Asset(relative) => asset_source.open_asset(relative),
    }
}

fn backup_namespace_order(path: &str) -> u8 {
    match path.split('/').next().unwrap_or("") {
        "originals" => 0,
        "manuals" => 1,
        "thumbnails" => 2,
        "trash" => 3,
        _ => u8::MAX,
    }
}

fn backup_asset_archive_path(relative: &Path) -> Result<String, String> {
    let components = relative.components().collect::<Vec<_>>();
    if components.len() < 2
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("Backup asset path must name a file under an owned namespace".into());
    }
    let archive_path = components
        .iter()
        .map(|component| match component {
            Component::Normal(value) => value.to_string_lossy(),
            _ => unreachable!("components were validated above"),
        })
        .collect::<Vec<_>>()
        .join("/");
    validate_relative(&archive_path)?;
    if archive_path == "library.sqlite3" {
        return Err("Backup asset path collides with the database".into());
    }
    Ok(archive_path)
}

fn hash_open_file(file: &mut File, limit: u64) -> Result<(u64, String), String> {
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("Backup file is unsafe or oversized".into());
    }
    let mut digest = Sha256::new();
    let bytes = std::io::copy(&mut (&mut *file).take(limit.saturating_add(1)), &mut digest)
        .map_err(|error| error.to_string())?;
    if bytes > limit || bytes != metadata.len() {
        return Err("Backup file changed or grew beyond size limit".into());
    }
    Ok((bytes, format!("{:x}", digest.finalize())))
}

fn hash_seekable<R: Read + Seek>(reader: &mut R, limit: u64) -> Result<String, String> {
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let bytes = std::io::copy(&mut reader.take(limit.saturating_add(1)), &mut digest)
        .map_err(|error| error.to_string())?;
    if bytes > limit {
        return Err("Backup archive exceeds size limit".into());
    }
    reader
        .seek(SeekFrom::End(0))
        .map_err(|error| error.to_string())?;
    Ok(format!("{:x}", digest.finalize()))
}

#[doc(hidden)]
pub trait RestoreTarget {
    fn create_new_file(&mut self, relative_path: &Path) -> std::io::Result<File>;
    fn staging_path(&self) -> std::io::Result<PathBuf>;
    fn sync_file_parent(&self, relative_path: &Path) -> std::io::Result<()>;
    fn sync_root(&self) -> std::io::Result<()>;
}

/// Restores a validated archive through a caller-owned target rooted at an
/// already-open staging directory.
#[doc(hidden)]
pub fn restore_backup_to_target<T: RestoreTarget>(
    source: &Path,
    target: &mut T,
) -> Result<(), String> {
    restore_backup_with_target(source, target)
}

/// Path convenience API retained for existing callers. Applications that
/// restore into an adversarially writable directory should use
/// [`restore_backup_from_reader`] with a rooted [`RestoreTarget`].
pub fn restore_backup_to_staging(source: &Path, staging: &Path) -> Result<(), String> {
    if !source.is_absolute() || !staging.is_absolute() || staging.exists() {
        return Err("Restore paths are invalid".into());
    }
    fs::create_dir(staging).map_err(|e| e.to_string())?;
    let mut target = PathRestoreTarget {
        staging: staging.to_path_buf(),
    };
    let result = restore_backup_with_target(source, &mut target);
    match result {
        Ok(()) => Ok(()),
        Err(error) => Err(failed_restore_staging_error(staging, error)),
    }
}

fn failed_restore_staging_error(staging: &Path, error: String) -> String {
    format!(
        "{error}; failed restore staging preserved at {}",
        staging.display()
    )
}

struct PathRestoreTarget {
    staging: PathBuf,
}

impl RestoreTarget for PathRestoreTarget {
    fn create_new_file(&mut self, relative_path: &Path) -> std::io::Result<File> {
        let output = self.staging.join(relative_path);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)
    }

    fn staging_path(&self) -> std::io::Result<PathBuf> {
        Ok(self.staging.clone())
    }

    fn sync_file_parent(&self, relative_path: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            let parent = self
                .staging
                .join(relative_path)
                .parent()
                .ok_or_else(|| std::io::Error::other("restore file has no parent"))?
                .to_path_buf();
            File::open(parent)?.sync_all()
        }
        #[cfg(not(unix))]
        {
            let _ = relative_path;
            Ok(())
        }
    }

    fn sync_root(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            File::open(&self.staging)?.sync_all()
        }
        #[cfg(not(unix))]
        {
            Ok(())
        }
    }
}

fn restore_backup_with_target<T: RestoreTarget>(
    source: &Path,
    target: &mut T,
) -> Result<(), String> {
    if !source.is_absolute() {
        return Err("Restore paths are invalid".into());
    }
    let source_meta = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    if source_meta.file_type().is_symlink() || !source_meta.is_file() {
        return Err("Restore paths are invalid".into());
    }
    let file = open_pinned_file(source).map_err(|e| e.to_string())?;
    restore_backup_from_reader(file, target)
}

/// Restores from an already-open archive file into a rooted target.
#[doc(hidden)]
pub fn restore_backup_from_reader<R: Read + Seek, T: RestoreTarget>(
    source: R,
    target: &mut T,
) -> Result<(), String> {
    restore_backup_with_reader(source, target)
}

fn restore_backup_with_reader<R: Read + Seek, T: RestoreTarget>(
    source: R,
    target: &mut T,
) -> Result<(), String> {
    let staging = target.staging_path().map_err(|e| e.to_string())?;
    if !staging.is_absolute() {
        return Err("Restore paths are invalid".into());
    }
    let mut archive = zip::ZipArchive::new(source).map_err(|e| e.to_string())?;
    if archive.len() > MAX_FILES + 1 {
        return Err("Backup contains too many entries".into());
    }
    let manifest: Manifest = {
        let mut entry = archive
            .by_name("backup-manifest.json")
            .map_err(|_| "Backup manifest is missing")?;
        if entry.encrypted() || entry.size() > 16 * 1024 * 1024 {
            return Err("Backup manifest is invalid".into());
        }
        let mut bytes = Vec::new();
        entry
            .by_ref()
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err("Backup manifest expanded beyond limit".into());
        }
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?
    };
    if manifest.version != 1 || manifest.files.len() > MAX_FILES {
        return Err("Backup manifest version or size is invalid".into());
    }
    let expected: BTreeMap<_, _> = manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    if expected.len() != manifest.files.len() {
        return Err("Backup manifest has duplicate paths".into());
    }
    let actual: BTreeSet<_> = (0..archive.len())
        .filter_map(|i| archive.by_index_raw(i).ok().map(|e| e.name().to_owned()))
        .filter(|name| name != "backup-manifest.json")
        .collect();
    if actual.len() != manifest.files.len()
        || actual
            .iter()
            .any(|name| !expected.contains_key(name.as_str()))
    {
        return Err("Backup entries do not match manifest".into());
    }
    if archive.len() != manifest.files.len() + 1 {
        return Err("Backup contains duplicate or extra entries".into());
    }
    let mut total = 0u64;
    for item in &manifest.files {
        validate_relative(&item.path)?;
        if item.bytes > MAX_FILE_BYTES {
            return Err("Backup entry exceeds size limit".into());
        }
        total = total
            .checked_add(item.bytes)
            .ok_or("Backup size overflow")?;
        if total > MAX_TOTAL_BYTES {
            return Err("Backup exceeds total size limit".into());
        }
        let mut entry = archive.by_name(&item.path).map_err(|e| e.to_string())?;
        if entry.encrypted() || entry.is_symlink() || !entry.is_file() {
            return Err("Backup contains unsupported entry".into());
        }
        let mut out = target
            .create_new_file(Path::new(&item.path))
            .map_err(|e| e.to_string())?;
        let mut digest = Sha256::new();
        let mut copied = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let n = entry.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            copied += n as u64;
            if copied > item.bytes {
                return Err("Backup entry expanded beyond manifest size".into());
            }
            digest.update(&buffer[..n]);
            out.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        }
        if copied != item.bytes || format!("{:x}", digest.finalize()) != item.sha256 {
            return Err("Backup entry hash mismatch".into());
        }
        out.sync_all().map_err(|e| e.to_string())?;
        target
            .sync_file_parent(Path::new(&item.path))
            .map_err(|e| e.to_string())?;
    }
    target.sync_root().map_err(|e| e.to_string())?;
    if target.staging_path().map_err(|e| e.to_string())? != staging {
        return Err("Restore staging directory changed during extraction".into());
    }
    validate_staged_database_schema(&staging.join("library.sqlite3"))?;
    library_recovery::inspect_staged_library(&staging).map_err(|e| e.to_string())?;
    if target.staging_path().map_err(|e| e.to_string())? != staging {
        return Err("Restore staging directory changed during validation".into());
    }
    Ok(())
}

fn validate_staged_database_schema(path: &Path) -> Result<(), String> {
    let pinned_file = open_pinned_regular_file(path, "Restored database")?;
    let sqlite_path = canonical_parent_path(path)?;
    check_staged_file_path(&sqlite_path, &pinned_file)?;
    let connection = Connection::open_with_flags(
        &sqlite_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|e| e.to_string())?;
    check_staged_database_identity(path, &pinned_file, &connection)?;
    connection
        .execute_batch("PRAGMA trusted_schema = OFF; PRAGMA query_only = ON;")
        .map_err(|e| e.to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT type, name, COALESCE(sql, '') FROM sqlite_schema \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = statement.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let kind: String = row.get(0).map_err(|e| e.to_string())?;
        let name: String = row.get(1).map_err(|e| e.to_string())?;
        let sql: String = row.get(2).map_err(|e| e.to_string())?;
        let normalized = sql.trim_start().to_ascii_uppercase();
        if matches!(kind.as_str(), "trigger" | "view")
            || (kind == "table" && normalized.starts_with("CREATE VIRTUAL TABLE"))
            || !matches!(kind.as_str(), "table" | "index")
        {
            return Err(format!(
                "Restored database contains unsupported executable schema object: {name}"
            ));
        }
    }
    check_staged_database_identity(path, &pinned_file, &connection)?;
    Ok(())
}

fn check_staged_database_identity(
    path: &Path,
    pinned_file: &File,
    connection: &Connection,
) -> Result<(), String> {
    check_staged_file_path(path, pinned_file)?;
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
        return Err("SQLite validated a different restored database file".into());
    }
    Ok(())
}

fn canonical_parent_path(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or("Restored database has no parent directory")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let filename = path
        .file_name()
        .ok_or("Restored database has no filename")?;
    Ok(parent.join(filename))
}

fn check_staged_file_path(path: &Path, pinned_file: &File) -> Result<(), String> {
    let visible_metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !visible_metadata.is_file()
        || visible_metadata.file_type().is_symlink()
        || is_reparse_point(&visible_metadata)
    {
        return Err("Restored database identity changed during validation".into());
    }
    let visible_file = open_pinned_file(path).map_err(|error| error.to_string())?;
    if !same_file_identity(pinned_file, &visible_file).map_err(|error| error.to_string())? {
        return Err("Restored database identity changed during validation".into());
    }
    Ok(())
}

fn collect_files(
    root: &Path,
    relative_root: &Path,
    output: &mut Vec<PathBuf>,
    budget: &mut BackupPathWalkBudget,
    maximum_path_bytes: usize,
) -> Result<(), String> {
    match fs::symlink_metadata(root) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        _ => {
            return Err(format!(
                "Unsafe backup namespace: {}",
                relative_root.display()
            ));
        }
    }
    let root_path_bytes = root.as_os_str().len();
    budget.stack_path_bytes = budget
        .stack_path_bytes
        .checked_add(root_path_bytes)
        .ok_or("Backup path size overflow")?;
    ensure_backup_path_walk_budget(budget, maximum_path_bytes)?;
    let mut stack = vec![(root.to_path_buf(), relative_root.to_path_buf(), 0usize)];
    while let Some((directory, relative_directory, depth)) = stack.pop() {
        let active_directory_path_bytes = directory.as_os_str().len();
        if depth > MAX_BACKUP_DIRECTORY_DEPTH {
            return Err("Backup asset directory tree is too deep".into());
        }
        for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let relative_path = relative_directory.join(entry.file_name());
            budget.entries = budget
                .entries
                .checked_add(1)
                .ok_or("Backup entry count overflow")?;
            if budget.entries > MAX_BACKUP_TREE_ENTRY_COUNT {
                return Err("Backup contains too many directory entries".into());
            }
            let relative_path_bytes = relative_path
                .as_os_str()
                .len()
                .checked_mul(2)
                .ok_or("Backup path size overflow")?;
            budget.path_bytes = budget
                .path_bytes
                .checked_add(relative_path_bytes)
                .ok_or("Backup path size overflow")?;
            ensure_backup_path_walk_budget(budget, maximum_path_bytes)?;
            let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if meta.file_type().is_symlink() {
                return Err("Backup refuses symlinks".into());
            }
            if meta.is_dir() {
                if depth >= MAX_BACKUP_DIRECTORY_DEPTH {
                    return Err("Backup asset directory tree is too deep".into());
                }
                budget.stack_path_bytes = budget
                    .stack_path_bytes
                    .checked_add(path.as_os_str().len())
                    .ok_or("Backup path size overflow")?;
                ensure_backup_path_walk_budget(budget, maximum_path_bytes)?;
                stack.push((path, relative_path, depth + 1));
            } else if meta.is_file() {
                if output.len() >= MAX_FILES - 1 {
                    return Err("Backup contains too many files".into());
                }
                output.push(relative_path);
            } else {
                return Err("Backup refuses special files".into());
            }
        }
        budget.stack_path_bytes = budget
            .stack_path_bytes
            .checked_sub(active_directory_path_bytes)
            .ok_or("Backup directory path budget underflow")?;
    }
    Ok(())
}

fn ensure_backup_path_walk_budget(
    budget: &BackupPathWalkBudget,
    maximum_path_bytes: usize,
) -> Result<(), String> {
    let total_path_bytes = budget
        .path_bytes
        .checked_add(budget.stack_path_bytes)
        .ok_or("Backup path size overflow")?;
    if total_path_bytes > maximum_path_bytes {
        return Err("Backup paths exceed total size limit".into());
    }
    Ok(())
}

#[derive(Default)]
struct BackupPathWalkBudget {
    entries: usize,
    path_bytes: usize,
    stack_path_bytes: usize,
}

struct PathBackupAssetSource {
    root: PathBuf,
}

impl BackupAssetSource for PathBackupAssetSource {
    fn asset_paths(&self) -> Result<Vec<PathBuf>, String> {
        let mut paths = Vec::new();
        let mut budget = BackupPathWalkBudget::default();
        for namespace in ["originals", "manuals", "thumbnails", "trash"] {
            collect_files(
                &self.root.join(namespace),
                Path::new(namespace),
                &mut paths,
                &mut budget,
                MAX_BACKUP_PATH_BYTES,
            )?;
        }
        Ok(paths)
    }

    fn open_asset(&self, relative_path: &Path) -> Result<File, String> {
        open_path_asset(&self.root, relative_path)
    }
}

fn open_path_asset(root: &Path, relative_path: &Path) -> Result<File, String> {
    if relative_path.as_os_str().is_empty()
        || relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("Unsafe backup asset path".into());
    }
    let path = root.join(relative_path);
    let before = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if before.file_type().is_symlink() || !before.is_file() {
        return Err("Backup asset is not a regular file".into());
    }
    let file = open_pinned_file(&path).map_err(|error| error.to_string())?;
    let opened = file.metadata().map_err(|error| error.to_string())?;
    if !opened.is_file() || is_reparse_point(&opened) || opened.len() != before.len() {
        return Err("Backup asset changed while opening".into());
    }
    let visible = open_pinned_file(&path).map_err(|error| error.to_string())?;
    if !same_file_identity(&file, &visible).map_err(|error| error.to_string())? {
        return Err("Backup asset changed while opening".into());
    }
    Ok(file)
}

fn validate_relative(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || value.contains('\0')
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("Unsafe backup entry path".into());
    }
    let first = path
        .components()
        .next()
        .and_then(|c| match c {
            Component::Normal(v) => v.to_str(),
            _ => None,
        })
        .unwrap_or("");
    if first != "library.sqlite3"
        && !matches!(first, "originals" | "manuals" | "thumbnails" | "trash")
    {
        return Err("Backup entry is outside owned namespaces".into());
    }
    if first == "library.sqlite3" && path.components().count() != 1 {
        return Err("Invalid database entry path".into());
    }
    Ok(())
}
fn canonical_real_dir(path: &Path) -> Result<PathBuf, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err("Library root is not a real directory".into());
    }
    path.canonicalize().map_err(|e| e.to_string())
}

fn open_catalog_file(path: &Path) -> Result<File, String> {
    open_pinned_regular_file(path, "Library database")
}

fn open_pinned_regular_file(path: &Path, label: &str) -> Result<File, String> {
    let before = fs::symlink_metadata(path).map_err(|_| format!("{label} was not found"))?;
    if !before.is_file() || before.file_type().is_symlink() || is_reparse_point(&before) {
        return Err(format!("{label} must be a regular file"));
    }
    let file = open_pinned_file(path).map_err(|error| error.to_string())?;
    let opened = file.metadata().map_err(|error| error.to_string())?;
    if !opened.is_file() || is_reparse_point(&opened) {
        return Err(format!("{label} changed while it was being opened"));
    }
    let visible = open_pinned_file(path).map_err(|error| error.to_string())?;
    if !same_file_identity(&file, &visible).map_err(|error| error.to_string())? {
        return Err(format!("{label} changed while it was being opened"));
    }
    Ok(file)
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

#[cfg(test)]
fn hash_file(path: &Path, limit: u64) -> Result<(u64, String), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > limit {
        return Err("Backup file is unsafe or oversized".into());
    }
    let mut file = open_pinned_file(path).map_err(|e| e.to_string())?;
    hash_open_file(&mut file, limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use library_core::{
        CapturePageInput, CaptureReceiptInput, FilamentInput, LibraryDb, MetadataPatch,
        PartStockInput, PartUsageInput, PrintAttemptInput, PrintAttemptInputWithPrinter,
        PrintOutcome, PrinterInput, PrinterMaintenanceInput, PrinterReminderInput,
    };
    #[test]
    fn path_compatibility_walker_budgets_relative_paths_and_retained_root_paths() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("r".repeat(128)).join("library");
        let asset_root = library_root.join("originals");
        fs::create_dir_all(&asset_root).unwrap();
        let names = ["one.stl", "two.stl", "three.stl"];
        for name in names {
            fs::write(asset_root.join(name), b"model").unwrap();
        }

        let relative_paths: Vec<PathBuf> = names
            .iter()
            .map(|name| Path::new("originals").join(name))
            .collect();
        let relative_budget = relative_paths
            .iter()
            .map(|path| path.as_os_str().len() * 2)
            .sum::<usize>();
        let maximum_path_bytes = asset_root.as_os_str().len() + relative_budget;
        assert!(asset_root.as_os_str().len() > 128);

        let mut paths = Vec::new();
        let mut budget = BackupPathWalkBudget::default();
        collect_files(
            &asset_root,
            Path::new("originals"),
            &mut paths,
            &mut budget,
            maximum_path_bytes,
        )
        .unwrap();
        assert_eq!(paths.len(), names.len());

        let mut limited_paths = Vec::new();
        let mut limited_budget = BackupPathWalkBudget::default();
        assert!(collect_files(
            &asset_root,
            Path::new("originals"),
            &mut limited_paths,
            &mut limited_budget,
            asset_root.as_os_str().len() - 1,
        )
        .is_err());
    }

    #[test]
    fn write_backup_archive_reports_a_verifiable_seekable_archive() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        drop(LibraryDb::open(&root).unwrap());

        let archive_path = temp.path().join("streamed-backup.zip");
        let mut archive = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&archive_path)
            .unwrap();
        let result = write_backup_archive(&root, &mut archive).unwrap();
        drop(archive);

        assert_eq!(result.files, 1);
        assert!(result.bytes > 0);
        assert_eq!(
            hash_file(&archive_path, MAX_TOTAL_BYTES + 16 * 1024 * 1024)
                .unwrap()
                .1,
            result.sha256
        );

        let staged = temp.path().join("staged");
        restore_backup_to_staging(&archive_path, &staged).unwrap();
        assert!(LibraryDb::open(&staged).is_ok());
    }

    #[test]
    fn backup_snapshots_committed_wal_state_from_the_catalog_vfs() {
        use library_core::catalog_vfs::CatalogVfs;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        drop(LibraryDb::open(&root).unwrap());

        let database = root.join("library.sqlite3");
        let pinned = File::open(&database).unwrap();
        let vfs = CatalogVfs::new(&database, &pinned).unwrap();
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

        let db = LibraryDb::open(&root).unwrap();
        db.create_folder("Committed WAL folder", None).unwrap();
        let mut archive = tempfile::tempfile().unwrap();
        write_backup_archive(&root, &mut archive).unwrap();

        archive.seek(SeekFrom::Start(0)).unwrap();
        let mut zip = zip::ZipArchive::new(archive).unwrap();
        let mut snapshot = Vec::new();
        zip.by_name("library.sqlite3")
            .unwrap()
            .read_to_end(&mut snapshot)
            .unwrap();
        let snapshot_path = temp.path().join("snapshot.sqlite3");
        fs::write(&snapshot_path, snapshot).unwrap();
        let snapshot = Connection::open(snapshot_path).unwrap();
        let folder_count: i64 = snapshot
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE name = 'Committed WAL folder'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(folder_count, 1);
    }

    #[test]
    fn source_writer_opens_only_the_assets_returned_by_its_source() {
        struct FixtureAssetSource {
            root: PathBuf,
        }
        impl BackupAssetSource for FixtureAssetSource {
            fn asset_paths(&self) -> Result<Vec<PathBuf>, String> {
                Ok(vec![PathBuf::from("originals/part.stl")])
            }

            fn open_asset(&self, relative_path: &Path) -> Result<File, String> {
                File::open(self.root.join(relative_path)).map_err(|error| error.to_string())
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        drop(LibraryDb::open(&root).unwrap());
        let supplied_assets = temp.path().join("supplied-assets");
        fs::create_dir_all(supplied_assets.join("originals")).unwrap();
        fs::write(
            supplied_assets.join("originals/part.stl"),
            b"synthetic asset",
        )
        .unwrap();

        let mut archive = tempfile::tempfile().unwrap();
        let info = write_backup_archive_from_source(
            &root,
            &FixtureAssetSource {
                root: supplied_assets,
            },
            &mut archive,
        )
        .unwrap();
        assert_eq!(info.files, 2);

        archive.seek(SeekFrom::Start(0)).unwrap();
        let mut zip = zip::ZipArchive::new(archive).unwrap();
        let mut asset = String::new();
        zip.by_name("originals/part.stl")
            .unwrap()
            .read_to_string(&mut asset)
            .unwrap();
        assert_eq!(asset, "synthetic asset");
    }

    #[test]
    fn source_writer_rejects_asset_changes_between_hash_and_zip_passes() {
        struct ChangingAssetSource {
            first: PathBuf,
            second: PathBuf,
            opens: std::cell::Cell<usize>,
        }
        impl BackupAssetSource for ChangingAssetSource {
            fn asset_paths(&self) -> Result<Vec<PathBuf>, String> {
                Ok(vec![PathBuf::from("originals/part.stl")])
            }

            fn open_asset(&self, _relative_path: &Path) -> Result<File, String> {
                let opened = self.opens.get();
                self.opens.set(opened + 1);
                let path = if opened == 0 {
                    &self.first
                } else {
                    &self.second
                };
                File::open(path).map_err(|error| error.to_string())
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        drop(LibraryDb::open(&root).unwrap());
        let first = temp.path().join("first.stl");
        let second = temp.path().join("second.stl");
        fs::write(&first, b"first bytes").unwrap();
        fs::write(&second, b"different bytes").unwrap();
        let source = ChangingAssetSource {
            first,
            second,
            opens: std::cell::Cell::new(0),
        };

        let mut archive = tempfile::tempfile().unwrap();
        assert!(write_backup_archive_from_source(&root, &source, &mut archive).is_err());
        assert_eq!(source.opens.get(), 2);
    }

    #[test]
    fn source_writer_rejects_namespace_roots_and_file_ancestor_collisions() {
        struct InvalidAssetSource {
            paths: Vec<PathBuf>,
            open_called: std::cell::Cell<bool>,
        }
        impl BackupAssetSource for InvalidAssetSource {
            fn asset_paths(&self) -> Result<Vec<PathBuf>, String> {
                Ok(self.paths.clone())
            }

            fn open_asset(&self, _relative_path: &Path) -> Result<File, String> {
                self.open_called.set(true);
                Err("invalid asset paths must be rejected before opening".into())
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        drop(LibraryDb::open(&root).unwrap());
        let invalid_path_sets = [
            vec![PathBuf::from("originals")],
            vec![
                PathBuf::from("originals/file"),
                PathBuf::from("originals/file/child"),
            ],
        ];
        for paths in invalid_path_sets {
            let mut archive = tempfile::tempfile().unwrap();
            let source = InvalidAssetSource {
                paths,
                open_called: std::cell::Cell::new(false),
            };
            assert!(write_backup_archive_from_source(&root, &source, &mut archive).is_err());
            assert!(!source.open_called.get());
        }
    }

    #[test]
    fn backup_round_trip_preserves_database_and_assets() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let source = temp.path().join("part.stl");
        let mut bytes = vec![0u8; 84];
        bytes[80..84].copy_from_slice(&0u32.to_le_bytes());
        fs::write(&source, &bytes).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        let filament = db
            .create_filament(&FilamentInput {
                brand: "Local".into(),
                material: "PLA".into(),
                color: "#112233".into(),
                name: "Backup filament".into(),
                notes: String::new(),
            })
            .unwrap();
        db.set_model_filaments(
            &model.id,
            model.metadata_version,
            std::slice::from_ref(&filament.id),
        )
        .unwrap();
        let current_model = db.get_model(&model.id).unwrap().unwrap();
        db.update_model_metadata(
            &model.id,
            current_model.metadata_version,
            &MetadataPatch {
                rating: Some(4),
                favorite: Some(true),
                ..MetadataPatch::default()
            },
        )
        .unwrap();
        let stock = db
            .create_part_stock(&PartStockInput {
                name: "M3 screw".into(),
                category: Some("Fasteners".into()),
                quantity: 20,
                unit: "pcs".into(),
                notes: "Synthetic archive fixture".into(),
                low_stock: 4,
            })
            .unwrap();
        let attempt = db
            .create_print_attempt(
                &model.id,
                &PrintAttemptInput {
                    printed_at: "2026-10-11T00:30:00Z".into(),
                    quantity: 2,
                    notes: "Synthetic failed print with part usage".into(),
                    filament_ids: vec![],
                    outcome: PrintOutcome::Failed,
                    parts: Some(vec![PartUsageInput {
                        id: stock.id.clone(),
                        quantity: 3,
                        snapshot: None,
                    }]),
                },
            )
            .unwrap();
        let imported_attempt = db
            .import_print_attempt(
                &model.id,
                &PrintAttemptInput {
                    printed_at: "2026-10-11T01:00:00Z".into(),
                    quantity: 1,
                    notes: "Synthetic imported historical event".into(),
                    filament_ids: vec![],
                    outcome: PrintOutcome::Printed,
                    parts: None,
                },
                "2026-10-11T01:00:00Z",
            )
            .unwrap();
        let capture = db
            .create_or_get_capture_receipt(&CaptureReceiptInput {
                item_id: "backup-capture".into(),
                payload_hash: "ab".repeat(32),
                created_at: "2026-10-11T00:00:00Z".into(),
                page: CapturePageInput {
                    url: "https://example.test/model".into(),
                    title: "Backup bookmark".into(),
                    designer: None,
                    model_license: None,
                    notes: None,
                    parent_external_id: None,
                },
                downloads: vec![],
            })
            .unwrap();
        drop(db);
        let recovery = library_recovery::inspect_library(&root).unwrap();
        let organization = recovery
            .models
            .iter()
            .find(|item| item.id == model.id)
            .unwrap()
            .organization
            .as_ref()
            .unwrap();
        assert_eq!(organization.rating, 4);
        assert!(organization.favorite);
        let backup = temp.path().join("backup.zip");
        let result = create_backup(&root, &backup).unwrap();
        assert!(result.files >= 2);
        let staged = temp.path().join("staged");
        restore_backup_to_staging(&backup, &staged).unwrap();
        assert!(fs::read_dir(&staged).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".library.sqlite3.gogglelab-")
        }));
        let restored = LibraryDb::open(&staged).unwrap();
        assert_eq!(restored.get_model(&model.id).unwrap().unwrap().id, model.id);
        assert_eq!(
            restored.get_model(&model.id).unwrap().unwrap().filament_ids,
            std::slice::from_ref(&filament.id)
        );
        let restored_stock = restored
            .get_part_stock(&stock.id)
            .unwrap()
            .expect("part stock should be restored from the SQLite snapshot");
        assert_eq!(restored_stock.name, "M3 screw");
        assert_eq!(restored_stock.quantity, 14);
        assert_eq!(restored_stock.metadata_version, 2);
        let restored_attempt = restored
            .list_print_attempts(&model.id)
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.id == attempt.id)
            .expect("print event should be restored from the SQLite snapshot");
        assert_eq!(restored_attempt.outcome, PrintOutcome::Failed);
        assert_eq!(restored_attempt.quantity, 2);
        assert_eq!(restored_attempt.parts.len(), 1);
        assert_eq!(
            restored_attempt.parts[0].id.as_deref(),
            Some(stock.id.as_str())
        );
        assert_eq!(restored_attempt.parts[0].name, "M3 screw");
        assert_eq!(restored_attempt.parts[0].quantity_per_copy, 3);
        assert_eq!(restored_attempt.parts[0].quantity, 6);
        assert_eq!(
            restored
                .get_model(&model.id)
                .unwrap()
                .unwrap()
                .metadata
                .rating,
            4
        );
        assert!(
            restored
                .get_model(&model.id)
                .unwrap()
                .unwrap()
                .metadata
                .favorite
        );
        assert_eq!(
            restored.get_filament(&filament.id).unwrap().unwrap().name,
            filament.name
        );
        assert_eq!(
            restored
                .get_capture_receipt("backup-capture")
                .unwrap()
                .unwrap()
                .page_model_id,
            capture.page_model_id
        );
        assert_eq!(
            fs::read(restored.resolve_model_source(&model.id).unwrap()).unwrap(),
            bytes
        );
        drop(restored);
        let restored_connection = Connection::open(staged.join("library.sqlite3")).unwrap();
        let restored_stock_adjusted: i64 = restored_connection
            .query_row(
                "SELECT stock_adjusted FROM print_attempts WHERE id=?1",
                [&imported_attempt.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(restored_stock_adjusted, 0);
        assert!(create_backup(&root, &backup).is_err());
    }

    #[test]
    fn schema_v8_backup_round_trip_preserves_printer_history_without_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let source = temp.path().join("part.stl");
        let mut bytes = vec![0u8; 84];
        bytes[80..84].copy_from_slice(&0u32.to_le_bytes());
        fs::write(&source, &bytes).unwrap();

        let db = LibraryDb::open(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        let printer = db
            .create_printer(
                &PrinterInput {
                    nickname: "Synthetic printer before rename".into(),
                    make: Some("Fixture maker".into()),
                    model: Some("Fixture 1".into()),
                    printer_type: Some("FDM".into()),
                    firmware: Some("fixture-fw".into()),
                    host: Some("printer.local".into()),
                    web_url: Some("http://printer.local/".into()),
                    bed_mm: Some([220.0, 220.0, 250.0]),
                    notes: "Synthetic backup fixture".into(),
                },
                true,
            )
            .unwrap();
        let pending_printer = db
            .create_printer(
                &PrinterInput {
                    nickname: "Synthetic printer awaiting approval".into(),
                    make: None,
                    model: None,
                    printer_type: None,
                    firmware: None,
                    host: Some("pending.local".into()),
                    web_url: None,
                    bed_mm: None,
                    notes: String::new(),
                },
                false,
            )
            .unwrap();
        let maintenance = db
            .create_printer_maintenance_log(
                &printer.id,
                &PrinterMaintenanceInput {
                    title: "Cleaned nozzle".into(),
                    performed_at: "2026-10-14T09:00:00Z".into(),
                    notes: "Synthetic maintenance fixture".into(),
                },
            )
            .unwrap();
        let maintenance = db
            .update_printer_maintenance_log(
                &maintenance.id,
                maintenance.metadata_version,
                &PrinterMaintenanceInput {
                    title: "Cleaned nozzle revision 2".into(),
                    performed_at: "2026-10-14T09:00:00Z".into(),
                    notes: "Synthetic maintenance fixture revision 2".into(),
                },
            )
            .unwrap();
        assert_eq!(maintenance.metadata_version, 2);
        let reminder = db
            .create_printer_reminder(
                &printer.id,
                &PrinterReminderInput {
                    title: "Inspect belts".into(),
                    notes: "Synthetic reminder fixture".into(),
                    due_at: "2026-11-14T09:00:00Z".into(),
                    interval_days: Some(31),
                },
            )
            .unwrap();
        let attempt = db
            .create_print_attempt_with_printer(
                &model.id,
                &PrintAttemptInputWithPrinter {
                    input: PrintAttemptInput {
                        printed_at: "2026-10-14T10:00:00Z".into(),
                        quantity: 2,
                        notes: "Synthetic attributed print".into(),
                        filament_ids: vec![],
                        outcome: PrintOutcome::Printed,
                        parts: None,
                    },
                    printer_id: Some(Some(printer.id.clone())),
                },
            )
            .unwrap();
        assert_eq!(attempt.printer_id.as_deref(), Some(printer.id.as_str()));
        assert_eq!(
            attempt.printer_name.as_deref(),
            Some("Synthetic printer before rename")
        );
        let renamed_printer = db
            .update_printer(
                &printer.id,
                printer.metadata_version,
                &PrinterInput {
                    nickname: "Synthetic printer after print".into(),
                    make: Some("Fixture maker".into()),
                    model: Some("Fixture 1".into()),
                    printer_type: Some("FDM".into()),
                    firmware: Some("fixture-fw".into()),
                    host: Some("printer.local".into()),
                    web_url: Some("http://printer.local/".into()),
                    bed_mm: Some([220.0, 220.0, 250.0]),
                    notes: "Synthetic backup fixture".into(),
                },
                None,
            )
            .unwrap();
        assert!(renamed_printer.endpoint_approved);

        drop(db);
        let backup = temp.path().join("printer-catalog-backup.zip");
        create_backup(&root, &backup).unwrap();
        let staged = temp.path().join("staged");
        restore_backup_to_staging(&backup, &staged).unwrap();

        let restored = LibraryDb::open(&staged).unwrap();
        let restored_printer = restored.get_printer(&printer.id).unwrap().unwrap();
        assert_eq!(restored_printer.nickname, renamed_printer.nickname);
        assert_eq!(restored_printer.host.as_deref(), Some("printer.local"));
        assert_eq!(
            restored_printer.web_url.as_deref(),
            Some("http://printer.local")
        );
        // A full local backup preserves the user's explicit endpoint approval.
        assert!(restored_printer.endpoint_approved);
        assert!(
            !restored
                .get_printer(&pending_printer.id)
                .unwrap()
                .unwrap()
                .endpoint_approved
        );

        let restored_maintenance = restored
            .list_printer_maintenance_logs(Some(&printer.id))
            .unwrap();
        assert_eq!(restored_maintenance, vec![maintenance]);
        let restored_reminders = restored
            .list_printer_reminders(Some(&printer.id), true)
            .unwrap();
        assert_eq!(restored_reminders, vec![reminder]);
        let restored_attempts = restored.list_print_attempts(&model.id).unwrap();
        assert_eq!(restored_attempts, vec![attempt]);

        drop(restored);
        let connection = Connection::open(staged.join("library.sqlite3")).unwrap();
        let schema_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(schema_version, library_core::CURRENT_SCHEMA_VERSION);
        for table in [
            "printers",
            "printer_maintenance_logs",
            "printer_reminders",
            "print_attempts",
        ] {
            let mut statement = connection
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            let columns = statement
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            for column in columns {
                let normalized = column.to_ascii_lowercase();
                assert!(
                    !["credential", "password", "api_key", "access_code", "token",]
                        .iter()
                        .any(|secret_field| normalized.contains(secret_field))
                );
            }
        }
    }

    #[test]
    fn restore_rejects_manifest_mismatch_and_preserves_staging() {
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("bad.zip");
        let file = File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        let manifest = Manifest {
            version: 1,
            files: vec![ManifestFile {
                path: "library.sqlite3".into(),
                bytes: 1,
                sha256: "00".repeat(32),
            }],
        };
        zip.start_file("backup-manifest.json", options).unwrap();
        zip.write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        zip.start_file("library.sqlite3", options).unwrap();
        zip.write_all(b"x").unwrap();
        zip.finish().unwrap();
        let staged = temp.path().join("staged");
        let error = restore_backup_to_staging(&zip_path, &staged).unwrap_err();
        assert!(error.contains(&staged.display().to_string()));
        assert_eq!(fs::read(staged.join("library.sqlite3")).unwrap(), b"x");
    }

    #[test]
    fn failed_restore_preserves_same_user_replacement_at_staging_path() {
        let temp = tempfile::tempdir().unwrap();
        let staged = temp.path().join("staged");
        fs::write(
            temp.path().join(".restore-staging-race-test"),
            b"test-owned tempdir",
        )
        .unwrap();
        fs::create_dir(&staged).unwrap();
        fs::write(staged.join("partial.bin"), b"partial restore bytes").unwrap();

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::failed_restore_staging_replacement_child_process",
                "--nocapture",
            ])
            .env("GOGGLELAB_BACKUP_STAGING_REPLACEMENT_PATH", &staged)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "replacement child failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let error = failed_restore_staging_error(&staged, "restore rejected".into());

        assert!(error.contains(&staged.display().to_string()));
        assert_eq!(
            fs::read(staged.join("replacement.bin")).unwrap(),
            b"replacement bytes"
        );
    }

    #[test]
    fn failed_restore_staging_replacement_child_process() {
        let Ok(staged) = std::env::var("GOGGLELAB_BACKUP_STAGING_REPLACEMENT_PATH") else {
            return;
        };
        let staged = PathBuf::from(staged);
        let parent = staged.parent().unwrap();
        assert_eq!(staged.file_name().unwrap(), "staged");
        assert_eq!(
            fs::read(parent.join(".restore-staging-race-test")).unwrap(),
            b"test-owned tempdir"
        );
        fs::remove_dir_all(&staged).unwrap();
        fs::create_dir(&staged).unwrap();
        fs::write(staged.join("replacement.bin"), b"replacement bytes").unwrap();
    }
    #[test]
    fn restore_rejects_database_triggers_before_writable_open() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        LibraryDb::open(&root).unwrap();
        let connection = Connection::open(root.join("library.sqlite3")).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER malicious_metadata_rewrite \
                 AFTER UPDATE ON models BEGIN \
                 UPDATE models SET notes = 'rewritten' WHERE id = NEW.id; \
                 END;",
            )
            .unwrap();
        drop(connection);
        let backup = temp.path().join("trigger.zip");
        create_backup(&root, &backup).unwrap();
        let staged = temp.path().join("staged");
        let error = restore_backup_to_staging(&backup, &staged).unwrap_err();
        assert!(error.contains("unsupported executable schema object"));
        assert!(staged.is_dir());
        assert!(staged.join("library.sqlite3").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn staged_schema_validation_rejects_a_symlinked_database() {
        let temp = tempfile::tempdir().unwrap();
        let actual = temp.path().join("actual.sqlite3");
        Connection::open(&actual)
            .unwrap()
            .execute_batch("CREATE TABLE safe_table (id INTEGER PRIMARY KEY);")
            .unwrap();
        let staged = temp.path().join("staged.sqlite3");
        std::os::unix::fs::symlink(&actual, &staged).unwrap();

        let error = validate_staged_database_schema(&staged).unwrap_err();
        assert!(error.contains("Restored database must be a regular file"));
    }

    #[test]
    fn restore_rejects_traversal_oversize_and_symlink_entries() {
        fn write_case(
            root: &Path,
            item: ManifestFile,
            entry_name: &str,
            permissions: u32,
        ) -> PathBuf {
            let path = root.join(format!("case-{}.zip", entry_name.replace(['/', '.'], "_")));
            let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
            let options = SimpleFileOptions::default();
            zip.start_file("backup-manifest.json", options).unwrap();
            zip.write_all(
                &serde_json::to_vec(&Manifest {
                    version: 1,
                    files: vec![item],
                })
                .unwrap(),
            )
            .unwrap();
            zip.start_file(entry_name, options.unix_permissions(permissions))
                .unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
            path
        }
        let temp = tempfile::tempdir().unwrap();
        let hash = format!("{:x}", Sha256::digest(b"x"));
        let traversal = write_case(
            temp.path(),
            ManifestFile {
                path: "../escape".into(),
                bytes: 1,
                sha256: hash.clone(),
            },
            "../escape",
            0o600,
        );
        assert!(restore_backup_to_staging(&traversal, &temp.path().join("traversal")).is_err());
        let oversized = write_case(
            temp.path(),
            ManifestFile {
                path: "library.sqlite3".into(),
                bytes: MAX_FILE_BYTES + 1,
                sha256: hash.clone(),
            },
            "library.sqlite3",
            0o600,
        );
        assert!(restore_backup_to_staging(&oversized, &temp.path().join("oversized")).is_err());
        let symlink = write_case(
            temp.path(),
            ManifestFile {
                path: "originals/link".into(),
                bytes: 1,
                sha256: hash.clone(),
            },
            "originals/link",
            0o120777,
        );
        assert!(restore_backup_to_staging(&symlink, &temp.path().join("symlink")).is_err());
        let plain = write_case(
            temp.path(),
            ManifestFile {
                path: "library.sqlite3".into(),
                bytes: 1,
                sha256: hash,
            },
            "library.sqlite3",
            0o600,
        );
        let mut encrypted = fs::read(&plain).unwrap();
        for (signature, offset) in [(*b"PK\x03\x04", 6usize), (*b"PK\x01\x02", 8usize)] {
            let start = encrypted
                .windows(4)
                .position(|window| window == signature)
                .unwrap();
            let flags =
                u16::from_le_bytes([encrypted[start + offset], encrypted[start + offset + 1]]);
            encrypted[start + offset..start + offset + 2]
                .copy_from_slice(&(flags | 1).to_le_bytes());
        }
        let encrypted_path = temp.path().join("encrypted.zip");
        fs::write(&encrypted_path, encrypted).unwrap();
        assert!(
            restore_backup_to_staging(&encrypted_path, &temp.path().join("encrypted")).is_err()
        );
    }
}
