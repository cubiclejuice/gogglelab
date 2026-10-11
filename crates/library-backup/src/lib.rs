use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

const MAX_FILES: usize = 100_000;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BackupResult {
    pub path: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub sha256: String,
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
    let temp = tempfile::tempdir_in(&parent).map_err(|e| e.to_string())?;
    let snapshot = temp.path().join("library.sqlite3");
    let db = root.join("library.sqlite3");
    let db_meta = fs::symlink_metadata(&db).map_err(|e| e.to_string())?;
    if db_meta.file_type().is_symlink() || !db_meta.is_file() {
        return Err("Library database is not a regular owned file".into());
    }
    let conn = Connection::open(&db).map_err(|e| e.to_string())?;
    conn.execute("VACUUM INTO ?1", [snapshot.to_string_lossy().as_ref()])
        .map_err(|e| e.to_string())?;
    drop(conn);
    let mut sources = vec![("library.sqlite3".to_string(), snapshot)];
    for namespace in ["originals", "manuals", "thumbnails", "trash"] {
        collect_files(&root.join(namespace), namespace, &mut sources)?;
    }
    if sources.len() > MAX_FILES {
        return Err("Backup contains too many files".into());
    }
    let mut total = 0u64;
    let mut listed = Vec::new();
    for (path, file) in &sources {
        let (bytes, hash) = hash_file(file, MAX_FILE_BYTES)?;
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
    let manifest = Manifest {
        version: 1,
        files: listed,
    };
    let named = tempfile::NamedTempFile::new_in(&parent).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(named.reopen().map_err(|e| e.to_string())?);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o600);
    zip.start_file("backup-manifest.json", options)
        .map_err(|e| e.to_string())?;
    zip.write_all(&serde_json::to_vec(&manifest).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    for ((path, file), expected) in sources.iter().zip(&manifest.files) {
        zip.start_file(path, options).map_err(|e| e.to_string())?;
        let mut input = File::open(file).map_err(|e| e.to_string())?;
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
    zip.finish()
        .map_err(|e| e.to_string())?
        .sync_all()
        .map_err(|e| e.to_string())?;
    let backup_hash = hash_file(named.path(), MAX_TOTAL_BYTES + 16 * 1024 * 1024)?.1;
    named
        .persist_noclobber(destination)
        .map_err(|e| e.error.to_string())?;
    Ok(BackupResult {
        path: destination.to_path_buf(),
        files: manifest.files.len(),
        bytes: total,
        sha256: backup_hash,
    })
}

pub fn restore_backup_to_staging(source: &Path, staging: &Path) -> Result<(), String> {
    let source_meta = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    if !source.is_absolute()
        || source_meta.file_type().is_symlink()
        || !source_meta.is_file()
        || staging.exists()
    {
        return Err("Restore paths are invalid".into());
    }
    let file = File::open(source).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
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
    fs::create_dir(staging).map_err(|e| e.to_string())?;
    let result = (|| {
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
            let output = staging.join(&item.path);
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)
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
        }
        validate_staged_database_schema(&staging.join("library.sqlite3"))?;
        library_recovery::inspect_library(staging).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(staging);
    }
    result
}

fn validate_staged_database_schema(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Restored database is not a regular file".into());
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
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
    Ok(())
}

fn collect_files(
    root: &Path,
    prefix: &str,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    match fs::symlink_metadata(root) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        _ => return Err(format!("Unsafe backup namespace: {prefix}")),
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if meta.file_type().is_symlink() {
                return Err("Backup refuses symlinks".into());
            }
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                let rel = path.strip_prefix(root).map_err(|e| e.to_string())?;
                let name = Path::new(prefix)
                    .join(rel)
                    .to_string_lossy()
                    .replace('\\', "/");
                validate_relative(&name)?;
                if out.len() >= MAX_FILES {
                    return Err("Backup contains too many files".into());
                }
                out.push((name, path));
            } else {
                return Err("Backup refuses special files".into());
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(())
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
fn hash_file(path: &Path, limit: u64) -> Result<(u64, String), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > limit {
        return Err("Backup file is unsafe or oversized".into());
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let bytes = std::io::copy(&mut (&mut file).take(limit.saturating_add(1)), &mut digest)
        .map_err(|e| e.to_string())?;
    if bytes > limit {
        return Err("Backup file grew beyond size limit".into());
    }
    Ok((bytes, format!("{:x}", digest.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use library_core::{CapturePageInput, CaptureReceiptInput, FilamentInput, LibraryDb};
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
        let backup = temp.path().join("backup.zip");
        let result = create_backup(&root, &backup).unwrap();
        assert!(result.files >= 2);
        let staged = temp.path().join("staged");
        restore_backup_to_staging(&backup, &staged).unwrap();
        let restored = LibraryDb::open(&staged).unwrap();
        assert_eq!(restored.get_model(&model.id).unwrap().unwrap().id, model.id);
        assert_eq!(
            restored.get_model(&model.id).unwrap().unwrap().filament_ids,
            std::slice::from_ref(&filament.id)
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
        assert!(create_backup(&root, &backup).is_err());
    }
    #[test]
    fn restore_rejects_manifest_mismatch_and_cleans_staging() {
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
        assert!(restore_backup_to_staging(&zip_path, &staged).is_err());
        assert!(!staged.exists());
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
        assert!(!staged.exists());
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
