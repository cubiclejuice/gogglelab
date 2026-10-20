use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use crate::ProcessingError;
use sha2::{Digest, Sha256};
use stl_core::package_bounds::{collect_verified_model_source, PackageProcessingContext};
use stl_core::zip_bounds::{BoundedZipArchive, BoundedZipMember, ZipResourceObserver};
use stl_core::ResourceStop;
use zip::CompressionMethod;

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
    archive_inventory_with_cancel_typed(path, max_member_bytes, cancelled)
        .map_err(|error| error.to_string())
}

pub fn scan_archive_with_cancel(
    path: &Path,
    max_member_bytes: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<ArchiveScan, String> {
    scan_archive_with_cancel_typed(path, max_member_bytes, cancelled)
        .map_err(|error| error.to_string())
}

pub fn extract_member_to_cache(
    library_root: &Path,
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
) -> Result<PathBuf, String> {
    extract_member_to_cache_typed(
        library_root,
        outer_path,
        member_path,
        expected_outer_fingerprint,
        expected_content_hash,
        expected_bytes,
    )
    .map_err(|error| error.to_string())
}

pub fn extract_member_to_temporary(
    library_root: &Path,
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
) -> Result<TemporaryArchiveMember, String> {
    extract_member_to_temporary_typed(
        library_root,
        outer_path,
        member_path,
        expected_outer_fingerprint,
        expected_content_hash,
        expected_bytes,
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn archive_inventory_with_cancel_typed(
    path: &Path,
    max_member_bytes: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<ArchiveInventory, ProcessingError> {
    let observer = ZipResourceObserver::new();
    let result = (|| {
        let before = validated_archive_metadata(path)?;
        let file = File::open(path).map_err(ProcessingError::from_io)?;
        let mut archive = observer
            .open_bounded_zip(file)
            .map_err(ProcessingError::from)?;
        let (plans, central_fingerprint) = preflight(&mut archive, max_member_bytes, cancelled)?;
        let archive_hash = hash_file_with_cancel(path, MAX_ARCHIVE_BYTES, cancelled)?;
        let after = validated_archive_metadata(path)?;
        if file_fingerprint(&before) != file_fingerprint(&after) {
            return Err(ProcessingError::ResourceStopped(
                ResourceStop::EnvelopeChanged,
            ));
        }
        Ok(ArchiveInventory {
            outer_fingerprint: outer_fingerprint(&after, &central_fingerprint, &archive_hash),
            member_paths: plans.into_iter().map(|plan| plan.path).collect(),
        })
    })();
    observer.finish(result)
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

pub(crate) fn scan_archive_with_cancel_typed(
    path: &Path,
    max_member_bytes: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<ArchiveScan, ProcessingError> {
    let observer = ZipResourceObserver::new();
    let result = (|| {
        let before = validated_archive_metadata(path)?;
        let file = File::open(path).map_err(ProcessingError::from_io)?;
        let mut archive = observer
            .open_bounded_zip(file)
            .map_err(ProcessingError::from)?;
        let (plans, central_fingerprint) = preflight(&mut archive, max_member_bytes, cancelled)?;
        let archive_hash = hash_file_with_cancel(path, MAX_ARCHIVE_BYTES, cancelled)?;
        let mut outer_context = PackageProcessingContext::new(observer.clone());
        let mut members = Vec::with_capacity(plans.len());
        for plan in plans {
            if cancelled() {
                return Err("Archive scan was cancelled".into());
            }
            let mut entry = archive
                .by_index(plan.index)
                .map_err(ProcessingError::from)?;
            let bytes = read_zip_entry(&mut entry, max_member_bytes, &mut outer_context)?;
            let mut model_context = PackageProcessingContext::new(observer.clone());
            let parsed = stl_core::parse_model_in_context(&bytes, &mut model_context)
                .map_err(ProcessingError::from)?;
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
            return Err(ProcessingError::ResourceStopped(
                ResourceStop::EnvelopeChanged,
            ));
        }
        let outer_fingerprint = outer_fingerprint(&after, &central_fingerprint, &archive_hash);
        Ok(ArchiveScan {
            outer_fingerprint,
            members,
        })
    })();
    observer.finish(result)
}

#[doc(hidden)]
pub fn extract_member_to_cache_typed(
    library_root: &Path,
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
) -> Result<PathBuf, ProcessingError> {
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

#[doc(hidden)]
pub fn extract_member_to_cache_in_context_typed(
    library_root: &Path,
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
    model_context: &mut PackageProcessingContext,
) -> Result<PathBuf, ProcessingError> {
    extract_member_to_cache_with_publish_hook_in_context(
        library_root,
        outer_path,
        member_path,
        expected_outer_fingerprint,
        expected_content_hash,
        expected_bytes,
        || {},
        model_context,
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
) -> Result<PathBuf, ProcessingError>
where
    F: FnOnce(),
{
    let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
    extract_member_to_cache_with_publish_hook_in_context(
        library_root,
        outer_path,
        member_path,
        expected_outer_fingerprint,
        expected_content_hash,
        expected_bytes,
        after_staging,
        &mut context,
    )
}

fn extract_member_to_cache_with_publish_hook_in_context<F>(
    library_root: &Path,
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
    after_staging: F,
    model_context: &mut PackageProcessingContext,
) -> Result<PathBuf, ProcessingError>
where
    F: FnOnce(),
{
    let observer = model_context.observer();
    observer.check().map_err(ProcessingError::from)?;
    let result = (|| {
        let bytes = read_verified_archive_member_in_context(
            outer_path,
            member_path,
            expected_outer_fingerprint,
            expected_content_hash,
            expected_bytes,
            model_context,
        )?;

        let canonical_library = library_root
            .canonicalize()
            .map_err(ProcessingError::from_io)?;
        if canonical_library != library_root {
            return Err("Library root identity changed".into());
        }
        observer.check().map_err(ProcessingError::from)?;
        let cache_root = library_root.join("indexed-cache");
        create_real_directory(&cache_root)?;
        let hash_dir = cache_root.join(expected_content_hash);
        create_real_directory(&hash_dir)?;
        let filename = sanitized_filename(member_path);
        let destination = hash_dir.join(filename);
        let directory = Directory::open(&hash_dir).map_err(ProcessingError::from_io)?;
        let destination_name = destination
            .file_name()
            .ok_or_else(|| "Indexed cache filename is invalid".to_string())?;
        match directory.open_file(destination_name) {
            Ok(file) => {
                validate_cached_member_file(
                    file,
                    expected_content_hash,
                    expected_bytes,
                    &observer,
                )?;
                return Ok(destination);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(ProcessingError::from_io(error)),
        }
        let staging = hash_dir.join(format!(
            ".member-{}-{}.part",
            std::process::id(),
            CACHE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let staging_name = staging
            .file_name()
            .ok_or_else(|| "Indexed cache staging filename is invalid".to_string())?;
        observer.check().map_err(ProcessingError::from)?;
        let mut file = directory
            .create_new_file(staging_name)
            .map_err(ProcessingError::from_io)?;
        let write_result = (|| {
            use std::io::Write;
            file.write_all(&bytes)?;
            file.sync_all()
        })();
        drop(file);
        if let Err(error) = write_result {
            let _ = directory.hold_entry(staging_name);
            return Err(ProcessingError::from_io(error));
        }
        after_staging();
        if let Err(error) = observer.check() {
            let _ = directory.hold_entry(staging_name);
            return Err(ProcessingError::from(error));
        }
        if let Err(error) = directory.rename_child_exclusive(staging_name, destination_name) {
            let _ = directory.hold_entry(staging_name);
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                let file = directory
                    .open_file(destination_name)
                    .map_err(ProcessingError::from_io)?;
                validate_cached_member_file(
                    file,
                    expected_content_hash,
                    expected_bytes,
                    &observer,
                )?;
                return Ok(destination);
            }
            return Err(ProcessingError::from_io(error));
        }
        Ok(destination)
    })();
    observer.finish(result)
}

#[doc(hidden)]
pub fn extract_member_to_temporary_typed(
    library_root: &Path,
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
) -> Result<TemporaryArchiveMember, ProcessingError> {
    let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
    extract_member_to_temporary_in_context_typed(
        library_root,
        outer_path,
        member_path,
        expected_outer_fingerprint,
        expected_content_hash,
        expected_bytes,
        &mut context,
    )
}

#[doc(hidden)]
pub fn extract_member_to_temporary_in_context_typed(
    library_root: &Path,
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
    model_context: &mut PackageProcessingContext,
) -> Result<TemporaryArchiveMember, ProcessingError> {
    let observer = model_context.observer();
    observer.check().map_err(ProcessingError::from)?;
    let result = (|| {
        let bytes = read_verified_archive_member_in_context(
            outer_path,
            member_path,
            expected_outer_fingerprint,
            expected_content_hash,
            expected_bytes,
            model_context,
        )?;
        let canonical_library = library_root
            .canonicalize()
            .map_err(ProcessingError::from_io)?;
        if canonical_library != library_root {
            return Err("Library root identity changed".into());
        }
        observer.check().map_err(ProcessingError::from)?;
        let staging = library_root.join("staging");
        create_real_directory(&staging)?;
        let directory = Directory::open(&staging).map_err(ProcessingError::from_io)?;
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
            observer.check().map_err(ProcessingError::from)?;
            match directory.create_new_file(name) {
                Ok(mut file) => {
                    use std::io::Write;
                    if let Err(error) = file.write_all(&bytes) {
                        drop(file);
                        let _ = directory.hold_entry(name);
                        return Err(ProcessingError::from_io(error));
                    }
                    file.sync_all().map_err(ProcessingError::from_io)?;
                    return Ok(TemporaryArchiveMember { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(ProcessingError::from_io(error)),
            }
        }
        Err("Could not allocate a temporary archive member path".into())
    })();
    observer.finish(result)
}

#[cfg(test)]
fn read_verified_archive_member(
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
) -> Result<Vec<u8>, ProcessingError> {
    read_verified_archive_member_with_reader(
        outer_path,
        member_path,
        expected_outer_fingerprint,
        expected_content_hash,
        expected_bytes,
        |path| File::open(path),
    )
}

#[cfg(test)]
fn read_verified_archive_member_with_reader<R: Read + Seek>(
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
    open: impl FnOnce(&Path) -> std::io::Result<R>,
) -> Result<Vec<u8>, ProcessingError> {
    let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
    read_verified_archive_member_with_reader_in_context(
        outer_path,
        member_path,
        expected_outer_fingerprint,
        expected_content_hash,
        expected_bytes,
        open,
        &mut context,
    )
}

fn read_verified_archive_member_in_context(
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
    model_context: &mut PackageProcessingContext,
) -> Result<Vec<u8>, ProcessingError> {
    read_verified_archive_member_with_reader_in_context(
        outer_path,
        member_path,
        expected_outer_fingerprint,
        expected_content_hash,
        expected_bytes,
        |path| File::open(path),
        model_context,
    )
}

fn read_verified_archive_member_with_reader_in_context<R: Read + Seek>(
    outer_path: &Path,
    member_path: &str,
    expected_outer_fingerprint: &str,
    expected_content_hash: &str,
    expected_bytes: u64,
    open: impl FnOnce(&Path) -> std::io::Result<R>,
    model_context: &mut PackageProcessingContext,
) -> Result<Vec<u8>, ProcessingError> {
    let observer = model_context.observer();
    observer.check().map_err(ProcessingError::from)?;
    let result = (|| {
        if expected_bytes > stl_core::limits::MAX_FILE_BYTES {
            return Err(ProcessingError::from_io(
                observer.stop_io(ResourceStop::InputLimitExceeded),
            ));
        }
        let before = validated_archive_metadata(outer_path)?;
        let file = open(outer_path).map_err(ProcessingError::from_io)?;
        let mut archive = observer
            .open_bounded_zip(file)
            .map_err(ProcessingError::from)?;
        let (_, central_fingerprint) =
            preflight(&mut archive, stl_core::limits::MAX_FILE_BYTES, &|| false)?;
        let archive_hash = hash_file(outer_path, MAX_ARCHIVE_BYTES)?;
        let after = validated_archive_metadata(outer_path)?;
        if file_fingerprint(&before) != file_fingerprint(&after)
            || outer_fingerprint(&after, &central_fingerprint, &archive_hash)
                != expected_outer_fingerprint
        {
            return Err(ProcessingError::from_io(
                observer.stop_io(ResourceStop::EnvelopeChanged),
            ));
        }
        let mut entry = archive.by_name(member_path).map_err(|error| {
            ProcessingError::from(error).with_ordinary_message("Archive member is missing")
        })?;
        validate_entry(&entry, stl_core::limits::MAX_FILE_BYTES)?;
        if entry.size() != expected_bytes {
            return Err(ProcessingError::from_io(
                observer.stop_io(ResourceStop::EnvelopeChanged),
            ));
        }
        let mut outer_context = PackageProcessingContext::new(observer.clone());
        let bytes = collect_verified_model_source(
            outer_context.inflated_reader(&mut entry),
            expected_bytes,
            &observer,
        )
        .map_err(ProcessingError::from)?;
        if hex_digest(&bytes) != expected_content_hash {
            return Err(ProcessingError::from_io(
                observer.stop_io(ResourceStop::EnvelopeChanged),
            ));
        }
        stl_core::parse_model_in_context(&bytes, model_context).map_err(ProcessingError::from)?;
        Ok(bytes)
    })();
    observer.finish(result)
}

#[derive(Debug)]
struct EntryPlan {
    index: usize,
    path: String,
}

fn preflight<R: Read + Seek>(
    archive: &mut BoundedZipArchive<R>,
    max_member_bytes: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<(Vec<EntryPlan>, String), ProcessingError> {
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
        let entry = archive.by_index_raw(index).map_err(ProcessingError::from)?;
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
                return Err(
                    format!("Archive model member exceeds {max_member_bytes} bytes").into(),
                );
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

fn validate_entry<R: Read + Seek>(
    entry: &BoundedZipMember<'_, R>,
    max_member_bytes: u64,
) -> Result<String, ProcessingError> {
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

fn read_zip_entry<R: Read + Seek>(
    entry: &mut BoundedZipMember<'_, R>,
    limit: u64,
    context: &mut PackageProcessingContext,
) -> Result<Vec<u8>, ProcessingError> {
    context
        .collect_model_member(entry, limit)
        .map_err(ProcessingError::from)
}

fn validated_archive_metadata(path: &Path) -> Result<fs::Metadata, ProcessingError> {
    let metadata = fs::symlink_metadata(path).map_err(ProcessingError::from_io)?;
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

fn hash_file(path: &Path, limit: u64) -> Result<String, ProcessingError> {
    hash_file_with_cancel(path, limit, &|| false)
}

fn hash_file_with_cancel(
    path: &Path,
    limit: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<String, ProcessingError> {
    let mut file = File::open(path).map_err(ProcessingError::from_io)?;
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
            .map_err(ProcessingError::from_io)?;
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

fn create_real_directory(path: &Path) -> Result<(), ProcessingError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err("Indexed cache path is not a real directory".into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(ProcessingError::from_io)
        }
        Err(error) => Err(ProcessingError::from_io(error)),
    }
}

fn validate_cached_member_file(
    file: File,
    expected_hash: &str,
    expected_bytes: u64,
    observer: &ZipResourceObserver,
) -> Result<(), ProcessingError> {
    observer.check().map_err(ProcessingError::from)?;
    if file.metadata().map_err(ProcessingError::from_io)?.len() != expected_bytes {
        return Err("Indexed cache entry has an unexpected size".into());
    }
    let bytes = collect_verified_model_source(file, expected_bytes, observer)
        .map_err(ProcessingError::from)?;
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
        assert!(matches!(
            scan_archive_with_cancel_typed(&archive, 1024 * 1024, &|| false),
            Err(ProcessingError::ResourceStopped(
                ResourceStop::InputLimitExceeded
            ))
        ));
        assert_eq!(
            scan_archive(&archive, 1024 * 1024).unwrap_err(),
            ResourceStop::InputLimitExceeded.to_string()
        );
        let library = root.join("library");
        fs::create_dir(&library).unwrap();
        for temporary in [false, true] {
            let error = if temporary {
                extract_member_to_temporary_typed(
                    &library,
                    &archive,
                    "model.stl",
                    "unused",
                    "unused",
                    0,
                )
                .err()
                .unwrap()
            } else {
                extract_member_to_cache_typed(
                    &library,
                    &archive,
                    "model.stl",
                    "unused",
                    "unused",
                    0,
                )
                .unwrap_err()
            };
            assert!(matches!(
                error,
                ProcessingError::ResourceStopped(ResourceStop::InputLimitExceeded)
            ));
            assert_eq!(
                fs::read_dir(&library).unwrap().count(),
                0,
                "no staging/cache creation before verified-read finish"
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    struct StoppedArchiveReader {
        file: File,
        allocation: bool,
        stop: ResourceStop,
    }

    impl Read for StoppedArchiveReader {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(if self.allocation {
                stl_core::zip_bounds::ZipAllocationFailed.into_io_error()
            } else {
                self.stop.into_io_error()
            })
        }
    }

    impl Seek for StoppedArchiveReader {
        fn seek(&mut self, from: std::io::SeekFrom) -> std::io::Result<u64> {
            self.file.seek(from)
        }
    }

    #[test]
    fn typed_archive_verified_reads_preserve_terminal_input_failures_and_source_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("models.zip");
        let model = empty_stl();
        write_zip(&archive, &[("model.stl", &model)]);
        let scan = scan_archive(&archive, 1024).unwrap();
        let selected = &scan.members[0];
        let original = fs::read(&archive).unwrap();
        for (allocation, stop) in [
            (false, ResourceStop::InputLimitExceeded),
            (false, ResourceStop::ReadWorkExceeded),
            (false, ResourceStop::EnvelopeChanged),
            (true, ResourceStop::InputLimitExceeded),
        ] {
            let error = read_verified_archive_member_with_reader(
                &archive,
                "model.stl",
                &scan.outer_fingerprint,
                &selected.content_hash,
                selected.bytes,
                |path| {
                    Ok(StoppedArchiveReader {
                        file: File::open(path)?,
                        allocation,
                        stop,
                    })
                },
            )
            .unwrap_err();
            match error {
                ProcessingError::AllocationFailed => assert!(allocation),
                ProcessingError::ResourceStopped(actual) => {
                    assert!(!allocation);
                    assert_eq!(actual, stop);
                }
                ordinary => panic!("lost terminal archive failure: {ordinary:?}"),
            }
        }
        let error = read_verified_archive_member(
            &archive,
            "model.stl",
            "stale fingerprint",
            &selected.content_hash,
            selected.bytes,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ProcessingError::ResourceStopped(ResourceStop::EnvelopeChanged)
        ));
        assert_eq!(fs::read(&archive).unwrap(), original);
        assert_eq!(
            read_verified_archive_member(
                &archive,
                "model.stl",
                &scan.outer_fingerprint,
                &selected.content_hash,
                selected.bytes
            )
            .unwrap(),
            model
        );
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
    #[test]
    fn model_member_actual_lower_policy_is_terminal_but_declared_scan_failure_stays_ordinary() {
        let root = temp_root("actual-policy");
        let archive_path = root.join("models.zip");
        let payload = empty_stl();
        write_zip(&archive_path, &[("part.stl", &payload)]);
        let error =
            scan_archive_with_cancel_typed(&archive_path, payload.len() as u64 - 1, &|| false)
                .unwrap_err();
        assert!(
            !error.is_terminal(),
            "declared preflight failure remains ordinary and no-read"
        );
        let observer = ZipResourceObserver::new();
        let mut archive = observer
            .open_bounded_zip(File::open(&archive_path).unwrap())
            .unwrap();
        let mut entry = archive.by_name("part.stl").unwrap();
        let mut context = PackageProcessingContext::new(observer.clone());
        assert!(matches!(
            read_zip_entry(&mut entry, payload.len() as u64 - 1, &mut context),
            Err(ProcessingError::ResourceStopped(
                ResourceStop::InputLimitExceeded
            ))
        ));
        assert!(observer.check().is_err());
    }

    #[test]
    fn verified_model_member_exact_envelope_is_not_a_policy_limit_and_outer_counter_spends_once() {
        let root = temp_root("exact-envelope");
        let archive_path = root.join("models.zip");
        let payload = empty_stl();
        write_zip(&archive_path, &[("part.stl", &payload)]);
        for expected in [
            payload.len() as u64,
            payload.len() as u64 - 1,
            payload.len() as u64 + 1,
        ] {
            let observer = ZipResourceObserver::new();
            let mut archive = observer
                .open_bounded_zip(File::open(&archive_path).unwrap())
                .unwrap();
            let entry = archive.by_name("part.stl").unwrap();
            let mut outer = PackageProcessingContext::new(observer.clone());
            let result =
                collect_verified_model_source(outer.inflated_reader(entry), expected, &observer);
            if expected == payload.len() as u64 {
                assert_eq!(result.unwrap(), payload);
                assert_eq!(outer.usage().inflation_bytes, payload.len() as u64);
                assert_eq!(outer.usage().retained_input_bytes, 0);
            } else {
                assert_eq!(
                    result.unwrap_err(),
                    stl_core::StlError::ResourceStopped(ResourceStop::EnvelopeChanged)
                );
            }
        }
    }
    #[test]
    fn verified_member_policy_excess_latches_before_source_metadata_or_open() {
        let observer = ZipResourceObserver::new();
        let mut context = PackageProcessingContext::new(observer.clone());
        let opened = std::cell::Cell::new(false);
        let error = read_verified_archive_member_with_reader_in_context(
            Path::new("/nonexistent-synthetic-policy-fixture.zip"),
            "part.3mf",
            "unused",
            "unused",
            stl_core::limits::MAX_FILE_BYTES + 1,
            |_| {
                opened.set(true);
                Ok(std::io::Cursor::new(Vec::<u8>::new()))
            },
            &mut context,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ProcessingError::ResourceStopped(ResourceStop::InputLimitExceeded)
        ));
        assert!(!opened.get());
        assert!(matches!(
            observer.check(),
            Err(stl_core::zip_bounds::ZipOpenError::ResourceStopped(
                ResourceStop::InputLimitExceeded
            ))
        ));
        assert_eq!(context.usage().inflation_bytes, 0);
    }
}
