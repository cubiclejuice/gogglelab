use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};
use zip::{CompressionMethod, ZipArchive};

use crate::secure_fs::Directory;

const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 10_000;
const MAX_TOTAL_INFLATED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MEMBER_PATH_BYTES: usize = 1024;
const MAX_MEMBER_DEPTH: usize = 32;
static CACHE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct ScannedArchiveMember {
    pub member_path: String,
    pub content_hash: String,
    pub bytes: u64,
    pub format: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ArchiveScan {
    pub outer_fingerprint: String,
    pub members: Vec<ScannedArchiveMember>,
}

#[derive(Debug)]
pub struct ArchiveInventory {
    pub outer_fingerprint: String,
    pub member_paths: Vec<String>,
}

pub fn archive_inventory_with_cancel(
    path: &Path,
    max_member_bytes: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<ArchiveInventory, String> {
    let before = validated_archive_metadata(path)?;
    let file = File::open(path).map_err(|error| error.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
    let (plans, central_fingerprint) = preflight(&mut archive, max_member_bytes, cancelled)?;
    let archive_hash = hash_file_with_cancel(path, MAX_ARCHIVE_BYTES, cancelled)?;
    let after = validated_archive_metadata(path)?;
    if file_fingerprint(&before) != file_fingerprint(&after) {
        return Err("Archive changed while its directory was being read".into());
    }
    Ok(ArchiveInventory {
        outer_fingerprint: outer_fingerprint(&after, &central_fingerprint, &archive_hash),
        member_paths: plans.into_iter().map(|plan| plan.path).collect(),
    })
}

pub struct TemporaryArchiveMember {
    path: PathBuf,
}

impl TemporaryArchiveMember {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryArchiveMember {
    fn drop(&mut self) {
        let Some(parent) = self.path.parent() else {
            return;
        };
        let Some(name) = self.path.file_name() else {
            return;
        };
        if let Ok(directory) = Directory::open(parent) {
            let _ = directory.hold_entry(name);
        }
    }
}

pub fn scan_archive(path: &Path, max_member_bytes: u64) -> Result<ArchiveScan, String> {
    scan_archive_with_cancel(path, max_member_bytes, &|| false)
}

pub fn scan_archive_with_cancel(
    path: &Path,
    max_member_bytes: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<ArchiveScan, String> {
    let before = validated_archive_metadata(path)?;
    let file = File::open(path).map_err(|error| error.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
    let (plans, central_fingerprint) = preflight(&mut archive, max_member_bytes, cancelled)?;
    let archive_hash = hash_file_with_cancel(path, MAX_ARCHIVE_BYTES, cancelled)?;
    let mut members = Vec::with_capacity(plans.len());
    for plan in plans {
        if cancelled() {
            return Err("Archive scan was cancelled".into());
        }
        let mut entry = archive
            .by_index(plan.index)
            .map_err(|error| error.to_string())?;
        let bytes = read_zip_entry(&mut entry, max_member_bytes)?;
        let parsed = stl_core::parse_model(&bytes).map_err(|error| error.to_string())?;
        let format = match parsed.format {
            stl_core::StlFormat::Binary => "stl-binary",
            stl_core::StlFormat::Ascii => "stl-ascii",
            stl_core::StlFormat::ThreeMf => "3mf",
        };
        let display_name = Path::new(&plan.path)
            .file_stem()
            .and_then(|name| name.to_str())
            .map(str::to_owned);
        members.push(ScannedArchiveMember {
            member_path: plan.path,
            content_hash: hex_digest(&bytes),
            bytes: bytes.len() as u64,
            format: format.into(),
            display_name,
        });
    }
    let after = validated_archive_metadata(path)?;
    if file_fingerprint(&before) != file_fingerprint(&after) {
        return Err("Archive changed while it was being indexed".into());
    }
    let outer_fingerprint = outer_fingerprint(&after, &central_fingerprint, &archive_hash);
    Ok(ArchiveScan {
        outer_fingerprint,
        members,
    })
}

pub fn extract_member_to_cache(
    library_root: &Path,
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
) -> Result<PathBuf, String> {
    extract_member_to_cache_with_publish_hook(
        library_root,
        outer_path,
        member_path,
        expected_outer_fingerprint,
        expected_content_hash,
        expected_bytes,
        || {},
    )
}

fn extract_member_to_cache_with_publish_hook<F>(
    library_root: &Path,
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
    after_staging: F,
) -> Result<PathBuf, String>
where
    F: FnOnce(),
{
    let bytes = read_verified_archive_member(
        outer_path,
        member_path,
        expected_outer_fingerprint,
        expected_content_hash,
        expected_bytes,
    )?;

    let canonical_library = library_root
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if canonical_library != library_root {
        return Err("Library root identity changed".into());
    }
    let cache_root = library_root.join("indexed-cache");
    create_real_directory(&cache_root)?;
    let hash_dir = cache_root.join(expected_content_hash);
    create_real_directory(&hash_dir)?;
    let filename = sanitized_filename(member_path);
    let destination = hash_dir.join(filename);
    let directory = Directory::open(&hash_dir).map_err(|error| error.to_string())?;
    let destination_name = destination
        .file_name()
        .ok_or_else(|| "Indexed cache filename is invalid".to_string())?;
    match directory.open_file(destination_name) {
        Ok(file) => {
            validate_cached_member_file(file, expected_content_hash, expected_bytes)?;
            return Ok(destination);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let staging = hash_dir.join(format!(
        ".member-{}-{}.part",
        std::process::id(),
        CACHE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let staging_name = staging
        .file_name()
        .ok_or_else(|| "Indexed cache staging filename is invalid".to_string())?;
    let mut file = directory
        .create_new_file(staging_name)
        .map_err(|error| error.to_string())?;
    let write_result = (|| {
        use std::io::Write;
        file.write_all(&bytes)?;
        file.sync_all()
    })();
    drop(file);
    if let Err(error) = write_result {
        let _ = directory.hold_entry(staging_name);
        return Err(error.to_string());
    }
    after_staging();
    if let Err(error) = directory.rename_child_exclusive(staging_name, destination_name) {
        let _ = directory.hold_entry(staging_name);
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            let file = directory
                .open_file(destination_name)
                .map_err(|open_error| open_error.to_string())?;
            validate_cached_member_file(file, expected_content_hash, expected_bytes)?;
            return Ok(destination);
        }
        return Err(error.to_string());
    }
    Ok(destination)
}

pub fn extract_member_to_temporary(
    library_root: &Path,
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
) -> Result<TemporaryArchiveMember, String> {
    let bytes = read_verified_archive_member(
        outer_path,
        member_path,
        expected_outer_fingerprint,
        expected_content_hash,
        expected_bytes,
    )?;
    let canonical_library = library_root
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if canonical_library != library_root {
        return Err("Library root identity changed".into());
    }
    let staging = library_root.join("staging");
    create_real_directory(&staging)?;
    let directory = Directory::open(&staging).map_err(|error| error.to_string())?;
    let extension = Path::new(member_path)
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| value.eq_ignore_ascii_case("stl") || value.eq_ignore_ascii_case("3mf"))
        .unwrap_or("stl");
    for _ in 0..1024 {
        let path = staging.join(format!(
            "archive-thumbnail-{}-{}.{}",
            std::process::id(),
            CACHE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            extension.to_ascii_lowercase()
        ));
        let name = path
            .file_name()
            .ok_or_else(|| "Temporary archive member filename is invalid".to_string())?;
        match directory.create_new_file(name) {
            Ok(mut file) => {
                use std::io::Write;
                if let Err(error) = file.write_all(&bytes) {
                    drop(file);
                    let _ = directory.hold_entry(name);
                    return Err(error.to_string());
                }
                file.sync_all().map_err(|error| error.to_string())?;
                return Ok(TemporaryArchiveMember { path });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("Could not allocate a temporary archive member path".into())
}

fn read_verified_archive_member(
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
) -> Result<Vec<u8>, String> {
    if expected_bytes > stl_core::limits::MAX_FILE_BYTES {
        return Err("Archive member exceeds the safe model size limit".into());
    }
    let before = validated_archive_metadata(outer_path)?;
    let file = File::open(outer_path).map_err(|error| error.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|error| error.to_string())?;
    let (_, central_fingerprint) =
        preflight(&mut archive, stl_core::limits::MAX_FILE_BYTES, &|| false)?;
    let archive_hash = hash_file(outer_path, MAX_ARCHIVE_BYTES)?;
    let after = validated_archive_metadata(outer_path)?;
    if file_fingerprint(&before) != file_fingerprint(&after)
        || outer_fingerprint(&after, &central_fingerprint, &archive_hash)
            != expected_outer_fingerprint
    {
        return Err("Archive changed since it was indexed".into());
    }
    let mut entry = archive
        .by_name(member_path)
        .map_err(|_| "Archive member is missing".to_string())?;
    validate_entry(&entry, stl_core::limits::MAX_FILE_BYTES)?;
    if entry.size() != expected_bytes {
        return Err("Archive member changed since it was indexed".into());
    }
    let bytes = read_zip_entry(&mut entry, expected_bytes)?;
    if hex_digest(&bytes) != expected_content_hash {
        return Err("Archive member content hash changed".into());
    }
    stl_core::parse_model(&bytes).map_err(|error| error.to_string())?;
    Ok(bytes)
}

#[derive(Debug)]
struct EntryPlan {
    index: usize,
    path: String,
}

fn preflight<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    max_member_bytes: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<(Vec<EntryPlan>, String), String> {
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("Archive contains too many entries".into());
    }
    let mut total = 0_u64;
    let mut paths = HashSet::with_capacity(archive.len());
    let mut plans = Vec::new();
    let mut central_digest = Sha256::new();
    for index in 0..archive.len() {
        if cancelled() {
            return Err("Archive scan was cancelled".into());
        }
        let entry = archive
            .by_index_raw(index)
            .map_err(|error| error.to_string())?;
        let path = validate_entry(&entry, max_member_bytes)?;
        central_digest.update(path.as_bytes());
        central_digest.update(entry.crc32().to_le_bytes());
        central_digest.update(entry.size().to_le_bytes());
        central_digest.update(entry.compressed_size().to_le_bytes());
        let compression_marker = match entry.compression() {
            CompressionMethod::Stored => 0_u8,
            CompressionMethod::Deflated => 8_u8,
            _ => 255_u8,
        };
        central_digest.update([compression_marker]);
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| "Archive inflation total overflowed".to_string())?;
        if total > MAX_TOTAL_INFLATED_BYTES {
            return Err("Archive declares more than 512 MiB of expanded data".into());
        }
        let key = path.to_ascii_lowercase();
        if !paths.insert(key) {
            return Err("Archive contains colliding member paths".into());
        }
        if !entry.is_dir() && is_model_member(&path) {
            if entry.size() > max_member_bytes {
                return Err(format!(
                    "Archive model member exceeds {max_member_bytes} bytes"
                ));
            }
            plans.push(EntryPlan { index, path });
        }
    }
    Ok((
        plans,
        central_digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    ))
}

fn validate_entry<R: Read>(
    entry: &zip::read::ZipFile<'_, R>,
    max_member_bytes: u64,
) -> Result<String, String> {
    let raw = entry.name_raw();
    if raw.is_empty() || raw.contains(&0) || raw.len() > MAX_MEMBER_PATH_BYTES {
        return Err("Archive contains an invalid member path".into());
    }
    if raw.contains(&b'\\')
        || raw.starts_with(b"/")
        || (raw.len() >= 2 && raw[0].is_ascii_alphabetic() && raw[1] == b':')
    {
        return Err("Archive contains an absolute or backslash member path".into());
    }
    if entry.encrypted() {
        return Err("Encrypted archive members are not supported".into());
    }
    if !matches!(
        entry.compression(),
        CompressionMethod::Stored | CompressionMethod::Deflated
    ) {
        return Err("Archive member uses unsupported compression".into());
    }
    if entry.is_symlink() || (!entry.is_dir() && !entry.is_file()) {
        return Err("Archive contains a symlink or special member".into());
    }
    if entry.size() > MAX_TOTAL_INFLATED_BYTES
        || (is_model_member(entry.name()) && entry.size() > max_member_bytes)
    {
        return Err("Archive member exceeds the inflation limit".into());
    }
    let path = entry
        .enclosed_name()
        .ok_or_else(|| "Archive member path traverses outside the archive".to_string())?;
    let mut depth = 0;
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err("Archive member path contains traversal".into());
        }
        depth += 1;
    }
    if depth == 0 || depth > MAX_MEMBER_DEPTH {
        return Err("Archive member path depth is invalid".into());
    }
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "Archive member path is not valid UTF-8".into())
}

fn read_zip_entry<R: Read>(
    entry: &mut zip::read::ZipFile<'_, R>,
    limit: u64,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    entry
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Archive member expanded beyond its size limit".into());
    }
    Ok(bytes)
}

fn validated_archive_metadata(path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Archive source is not a regular file".into());
    }
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err("Archive exceeds the 1 GiB compressed size limit".into());
    }
    Ok(metadata)
}

fn file_fingerprint(metadata: &fs::Metadata) -> (u64, u128) {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |value| value.as_nanos());
    (metadata.len(), modified)
}

fn outer_fingerprint(
    metadata: &fs::Metadata,
    central_fingerprint: &str,
    archive_hash: &str,
) -> String {
    let (bytes, modified_nanos) = file_fingerprint(metadata);
    format!("zipmeta:{bytes}:{modified_nanos}:{central_fingerprint}:{archive_hash}")
}

fn hash_file(path: &Path, limit: u64) -> Result<String, String> {
    hash_file_with_cancel(path, limit, &|| false)
}

fn hash_file_with_cancel(
    path: &Path,
    limit: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancelled() {
            return Err("Archive scan was cancelled".into());
        }
        let remaining = limit.saturating_add(1).saturating_sub(copied);
        if remaining == 0 {
            break;
        }
        let read_limit = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
        let read = file
            .read(&mut buffer[..read_limit])
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        copied += read as u64;
        digest.update(&buffer[..read]);
    }
    if copied > limit {
        return Err("Archive grew beyond the compressed size limit".into());
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn is_model_member(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("stl") || extension.eq_ignore_ascii_case("3mf")
        })
}

fn sanitized_filename(member_path: &str) -> String {
    let raw = Path::new(member_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("model.stl");
    let sanitized = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "model.stl".into()
    } else {
        sanitized
    }
}

fn create_real_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err("Indexed cache path is not a real directory".into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn validate_cached_member_file(
    file: File,
    expected_hash: &str,
    expected_bytes: u64,
) -> Result<(), String> {
    if file.metadata().map_err(|error| error.to_string())?.len() != expected_bytes {
        return Err("Indexed cache entry has an unexpected size".into());
    }
    let mut bytes = Vec::new();
    file.take(expected_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 != expected_bytes || hex_digest(&bytes) != expected_hash {
        return Err("Indexed cache entry hash is invalid".into());
    }
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use zip::write::SimpleFileOptions;

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "gogglelab-archive-index-{label}-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    fn empty_stl() -> Vec<u8> {
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        bytes
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        for (name, bytes) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn scans_models_in_place_without_touching_archive_timestamp() {
        let root = temp_root("scan");
        let archive = root.join("models.zip");
        write_zip(&archive, &[("parts/one.stl", &empty_stl())]);
        let before = fs::metadata(&archive).unwrap().modified().unwrap();
        let scan = scan_archive(&archive, 1024 * 1024).unwrap();
        assert_eq!(scan.members.len(), 1);
        assert_eq!(scan.members[0].member_path, "parts/one.stl");
        assert_eq!(before, fs::metadata(&archive).unwrap().modified().unwrap());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_traversal_and_backslash_members() {
        for name in ["../escape.stl", "folder\\escape.stl", "C:/escape.stl"] {
            let root = temp_root("unsafe");
            let archive = root.join("models.zip");
            write_zip(&archive, &[(name, &empty_stl())]);
            assert!(scan_archive(&archive, 1024 * 1024).is_err());
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn rejects_encrypted_member_metadata_before_inflation() {
        let root = temp_root("encrypted");
        let archive = root.join("models.zip");
        write_zip(&archive, &[("part.stl", &empty_stl())]);
        let mut bytes = fs::read(&archive).unwrap();
        for (signature, offset) in [(*b"PK\x03\x04", 6_usize), (*b"PK\x01\x02", 8_usize)] {
            let start = bytes
                .windows(4)
                .position(|window| window == signature)
                .unwrap();
            let flags = u16::from_le_bytes([bytes[start + offset], bytes[start + offset + 1]]);
            bytes[start + offset..start + offset + 2].copy_from_slice(&(flags | 1).to_le_bytes());
        }
        fs::write(&archive, bytes).unwrap();
        assert!(scan_archive(&archive, 1024 * 1024)
            .unwrap_err()
            .contains("Encrypted"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_archives_over_the_central_entry_limit() {
        let root = temp_root("entry-limit");
        let archive = root.join("models.zip");
        let file = File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        for index in 0..=MAX_ARCHIVE_ENTRIES {
            writer
                .start_file(
                    format!("metadata/{index}.txt"),
                    SimpleFileOptions::default(),
                )
                .unwrap();
        }
        writer.finish().unwrap();
        assert!(scan_archive(&archive, 1024 * 1024)
            .unwrap_err()
            .contains("too many entries"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn archive_inventory_honors_cancellation_before_member_reads() {
        let root = temp_root("cancel");
        let archive = root.join("models.zip");
        write_zip(&archive, &[("part.stl", &empty_stl())]);
        assert!(
            archive_inventory_with_cancel(&archive, 1024 * 1024, &|| true)
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(scan_archive_with_cancel(&archive, 1024 * 1024, &|| true)
            .unwrap_err()
            .contains("cancelled"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_only_selected_member_to_owned_cache_and_reuses_verified_bytes() {
        let library = temp_root("cache-library");
        fs::create_dir(library.join("indexed-cache")).unwrap();
        let source = temp_root("cache-source");
        let archive = source.join("models.zip");
        let model = empty_stl();
        write_zip(
            &archive,
            &[("parts/selected.stl", &model), ("parts/other.stl", &model)],
        );
        let before = fs::metadata(&archive).unwrap().modified().unwrap();
        let scan = scan_archive(&archive, 1024 * 1024).unwrap();
        let selected = &scan.members[0];
        let cached = extract_member_to_cache(
            &library,
            &archive,
            &selected.member_path,
            &scan.outer_fingerprint,
            &selected.content_hash,
            selected.bytes,
        )
        .unwrap();
        assert_eq!(fs::read(&cached).unwrap(), model);
        assert_eq!(
            cached,
            extract_member_to_cache(
                &library,
                &archive,
                &selected.member_path,
                &scan.outer_fingerprint,
                &selected.content_hash,
                selected.bytes,
            )
            .unwrap()
        );
        assert_eq!(before, fs::metadata(&archive).unwrap().modified().unwrap());
        let temporary = extract_member_to_temporary(
            &library,
            &archive,
            &selected.member_path,
            &scan.outer_fingerprint,
            &selected.content_hash,
            selected.bytes,
        )
        .unwrap();
        let temporary_path = temporary.path().to_path_buf();
        assert_eq!(fs::read(&temporary_path).unwrap(), model);
        assert!(cached.exists());
        let replacement = b"concurrent replacement bytes";
        fs::remove_file(&temporary_path).unwrap();
        fs::write(&temporary_path, replacement).unwrap();
        drop(temporary);
        assert!(!temporary_path.exists());
        let held = fs::read_dir(library.join("staging"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gogglelab-held-")
                    && entry.file_name().to_string_lossy().ends_with(".data")
            })
            .find(|entry| fs::read(entry.path()).is_ok_and(|bytes| bytes == replacement));
        assert!(
            held.is_some(),
            "drop must preserve the replacement in a recoverable hold"
        );
        assert!(
            cached.exists(),
            "viewer cache must survive thumbnail cleanup"
        );
        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(source);
    }

    #[test]
    fn cache_publication_preserves_a_destination_created_after_preflight() {
        let library = temp_root("cache-race-library");
        fs::create_dir(library.join("indexed-cache")).unwrap();
        let source = temp_root("cache-race-source");
        let archive = source.join("models.zip");
        let model = empty_stl();
        write_zip(&archive, &[("parts/selected.stl", &model)]);
        let scan = scan_archive(&archive, 1024 * 1024).unwrap();
        let selected = &scan.members[0];
        let destination = library
            .join("indexed-cache")
            .join(&selected.content_hash)
            .join("selected.stl");
        let concurrent = b"concurrent cache replacement";

        let result = extract_member_to_cache_with_publish_hook(
            &library,
            &archive,
            &selected.member_path,
            &scan.outer_fingerprint,
            &selected.content_hash,
            selected.bytes,
            || {
                fs::write(&destination, concurrent).unwrap();
            },
        );

        assert!(result.is_err());
        assert_eq!(fs::read(&destination).unwrap(), concurrent);
        let held = fs::read_dir(destination.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gogglelab-held-")
                    && entry.file_name().to_string_lossy().ends_with(".data")
            })
            .find(|entry| fs::read(entry.path()).is_ok_and(|bytes| bytes == model));
        assert!(
            held.is_some(),
            "verified archive bytes must remain recoverable"
        );
        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(source);
    }
}
