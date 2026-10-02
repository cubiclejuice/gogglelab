use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use zip::result::ZipError;
use zip::{CompressionMethod, ZipArchive};

use crate::error::AppError;

const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;
const MAX_PATH_DEPTH: usize = 32;
const MAX_PATH_BYTES: usize = 4096;
const COPY_BUFFER_BYTES: usize = 64 * 1024;

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Serialize)]
pub struct ExtractResult {
    pub destination: String,
    pub files: u32,
}

#[derive(Clone, Copy)]
struct ExtractionLimits {
    archive_bytes: u64,
    declared_extracted_bytes: u64,
    extracted_bytes: u64,
    entries: usize,
    path_depth: usize,
    path_bytes: usize,
}

const RUNTIME_LIMITS: ExtractionLimits = ExtractionLimits {
    archive_bytes: MAX_ARCHIVE_BYTES,
    declared_extracted_bytes: MAX_EXTRACTED_BYTES,
    extracted_bytes: MAX_EXTRACTED_BYTES,
    entries: MAX_ENTRIES,
    path_depth: MAX_PATH_DEPTH,
    path_bytes: MAX_PATH_BYTES,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Directory,
    File,
}

struct EntryPlan {
    index: usize,
    path: PathBuf,
    kind: EntryKind,
}

struct StagingGuard {
    path: PathBuf,
    active: bool,
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if self.active {
            // This path was created by this extraction attempt with a unique,
            // private name. Never broaden cleanup to its parent or destination.
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn archive_error(message: impl Into<String>) -> AppError {
    AppError::Archive {
        message: message.into(),
    }
}

/// Extract a ZIP selected from the file tree. All blocking filesystem work is
/// kept off Tauri's async executor.
#[tauri::command]
pub async fn extract_zip(root: String, path: String) -> Result<ExtractResult, AppError> {
    tauri::async_runtime::spawn_blocking(move || extract_zip_core(&root, &path))
        .await
        .map_err(|_| archive_error("ZIP extraction stopped unexpectedly. Please try again."))?
}

pub fn extract_zip_core(root: &str, path: &str) -> Result<ExtractResult, AppError> {
    extract_zip_with_limits(root, path, RUNTIME_LIMITS)
}

fn extract_zip_with_limits(
    root: &str,
    path: &str,
    limits: ExtractionLimits,
) -> Result<ExtractResult, AppError> {
    let root = canonical_directory(root)?;
    let source_input = Path::new(path);
    let source_link_meta = fs::symlink_metadata(source_input).map_err(map_path_error)?;
    if source_link_meta.file_type().is_symlink() {
        return Err(archive_error(
            "ZIP shortcuts and symbolic links cannot be extracted.",
        ));
    }
    let source = source_input.canonicalize().map_err(map_path_error)?;
    if !source.starts_with(&root) {
        return Err(AppError::OutsideRoot);
    }
    if !source_link_meta.is_file()
        || !source
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        return Err(archive_error(
            "Choose a regular .zip file inside the selected folder.",
        ));
    }

    let file = File::open(&source).map_err(map_path_error)?;
    let archive_size = file.metadata().map_err(|_| AppError::NotReadable)?.len();
    if archive_size > limits.archive_bytes {
        return Err(archive_error(format!(
            "This ZIP is larger than the {} GiB compressed-size limit.",
            limits.archive_bytes / (1024 * 1024 * 1024)
        )));
    }
    let mut archive = ZipArchive::new(file).map_err(map_zip_open_error)?;
    let plans = preflight(&mut archive, limits)?;

    let parent = source
        .parent()
        .ok_or_else(|| archive_error("The ZIP has no writable parent folder."))?;
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| archive_error("The ZIP filename has no usable name."))?;

    let staging_path = create_staging(parent)?;
    let mut staging = StagingGuard {
        path: staging_path.clone(),
        active: true,
    };
    let files = extract_plans(&mut archive, &plans, &staging_path, limits)?;
    let destination = publish_staging(parent, stem, &staging_path)?;
    staging.active = false;

    Ok(ExtractResult {
        destination: destination.to_string_lossy().into_owned(),
        files,
    })
}

fn canonical_directory(path: &str) -> Result<PathBuf, AppError> {
    let canonical = Path::new(path).canonicalize().map_err(map_path_error)?;
    if !canonical.is_dir() {
        return Err(AppError::NotReadable);
    }
    Ok(canonical)
}

fn map_path_error(error: io::Error) -> AppError {
    match error.kind() {
        io::ErrorKind::NotFound => AppError::NotFound,
        _ => AppError::NotReadable,
    }
}

fn map_zip_open_error(error: ZipError) -> AppError {
    match error {
        ZipError::CompressionMethodNotSupported(_) => {
            archive_error("This ZIP uses a compression method that GoggleLab does not support.")
        }
        ZipError::UnsupportedArchive(reason) if reason == ZipError::PASSWORD_REQUIRED => {
            archive_error("Password-protected ZIP entries are not supported.")
        }
        _ => archive_error("This ZIP is corrupt or unreadable."),
    }
}

fn preflight<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
    limits: ExtractionLimits,
) -> Result<Vec<EntryPlan>, AppError> {
    if archive.len() > limits.entries {
        return Err(archive_error(format!(
            "This ZIP contains more than {} entries.",
            limits.entries
        )));
    }

    let mut plans = Vec::with_capacity(archive.len());
    let mut exact_paths = HashSet::with_capacity(archive.len());
    let mut file_paths = HashSet::with_capacity(archive.len());
    let mut paths_with_children = HashSet::with_capacity(archive.len());
    let mut declared_total = 0u64;

    for index in 0..archive.len() {
        let file = archive
            // Metadata-only access keeps encrypted and unsupported entries
            // inspectable, so validation can return the specific remedy.
            .by_index_raw(index)
            .map_err(|_| archive_error("This ZIP has an unreadable directory entry."))?;
        let path = validate_entry(&file, limits)?;
        let kind = if file.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        let components = normalized_components(&path);
        let key = components.join("/").to_lowercase();
        declared_total = declared_total
            .checked_add(file.size())
            .ok_or_else(|| archive_error("The ZIP's declared extracted size is too large."))?;
        if declared_total > limits.declared_extracted_bytes {
            return Err(archive_error(format!(
                "This ZIP declares more than {} GiB of extracted data.",
                limits.declared_extracted_bytes / (1024 * 1024 * 1024)
            )));
        }

        if !exact_paths.insert(key.clone()) {
            return Err(archive_error(format!(
                "The ZIP contains duplicate paths that collide: {}.",
                path.display()
            )));
        }
        let mut prefix = String::new();
        for component in components.iter().take(components.len().saturating_sub(1)) {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(&component.to_lowercase());
            if file_paths.contains(&prefix) {
                return Err(archive_error(format!(
                    "The ZIP uses the same path as both a file and a folder: {}.",
                    path.display()
                )));
            }
            paths_with_children.insert(prefix.clone());
        }
        if kind == EntryKind::File {
            if paths_with_children.contains(&key) {
                return Err(archive_error(format!(
                    "The ZIP uses the same path as both a file and a folder: {}.",
                    path.display()
                )));
            }
            file_paths.insert(key);
        }
        plans.push(EntryPlan { index, path, kind });
    }
    Ok(plans)
}

fn validate_entry<R: Read>(
    file: &zip::read::ZipFile<'_, R>,
    limits: ExtractionLimits,
) -> Result<PathBuf, AppError> {
    let raw = file.name_raw();
    if raw.is_empty() || raw.contains(&0) {
        return Err(archive_error(
            "The ZIP contains an empty or NUL-containing path.",
        ));
    }
    if raw.len() > limits.path_bytes {
        return Err(archive_error(format!(
            "The ZIP contains a path longer than {} bytes.",
            limits.path_bytes
        )));
    }
    if raw.contains(&b'\\') {
        return Err(archive_error("The ZIP contains an unsafe backslash path."));
    }
    if raw.starts_with(b"/") || (raw.len() >= 2 && raw[0].is_ascii_alphabetic() && raw[1] == b':') {
        return Err(archive_error("The ZIP contains an absolute path."));
    }
    if file.encrypted() {
        return Err(archive_error(
            "Password-protected ZIP entries are not supported.",
        ));
    }
    if !matches!(
        file.compression(),
        CompressionMethod::Stored | CompressionMethod::Deflated
    ) {
        return Err(archive_error(
            "This ZIP uses a compression method that GoggleLab does not support.",
        ));
    }
    if file.is_symlink() {
        return Err(archive_error(
            "ZIP entries that are symbolic links are not allowed.",
        ));
    }
    if !file.is_dir() && !file.is_file() {
        return Err(archive_error(
            "The ZIP contains an unsupported special file.",
        ));
    }

    let path = file
        .enclosed_name()
        .ok_or_else(|| archive_error("The ZIP contains a path outside its destination."))?;
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(archive_error("The ZIP contains a traversal path."));
            }
        }
    }
    if depth == 0 || depth > limits.path_depth {
        return Err(archive_error(format!(
            "The ZIP contains a path deeper than {} folders.",
            limits.path_depth
        )));
    }

    if let Some(mode) = file.unix_mode() {
        let file_type = mode & 0o170000;
        let expected = if file.is_dir() { 0o040000 } else { 0o100000 };
        if file_type != 0 && file_type != expected {
            return Err(archive_error(
                "The ZIP contains an unsupported special file.",
            ));
        }
    }
    Ok(path)
}

fn normalized_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn create_staging(parent: &Path) -> Result<PathBuf, AppError> {
    for _ in 0..1024 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".gogglelab-extract-{}-{sequence}",
            std::process::id()
        ));
        #[cfg(unix)]
        let result = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700).create(&path)
        };
        #[cfg(not(unix))]
        let result = fs::create_dir(&path);
        match result {
            Ok(()) => {
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                return Err(archive_error(
                    "Could not create a private extraction folder.",
                ))
            }
        }
    }
    Err(archive_error(
        "Could not allocate a private extraction folder.",
    ))
}

fn extract_plans<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
    plans: &[EntryPlan],
    staging: &Path,
    limits: ExtractionLimits,
) -> Result<u32, AppError> {
    let mut total = 0u64;
    let mut files = 0u32;
    let mut buffer = vec![0u8; COPY_BUFFER_BYTES];
    let mut created_directories = HashSet::new();

    for plan in plans {
        let output = staging.join(&plan.path);
        if plan.kind == EntryKind::Directory {
            ensure_directories(staging, &plan.path, &mut created_directories)?;
            continue;
        }
        if let Some(relative_parent) = plan.path.parent() {
            ensure_directories(staging, relative_parent, &mut created_directories)?;
        }
        let mut input = archive
            .by_index(plan.index)
            .map_err(|_| archive_error("A ZIP entry became unreadable during extraction."))?;
        let mut output_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|_| {
                archive_error("The ZIP contains paths that collide on this filesystem.")
            })?;

        loop {
            let read = input.read(&mut buffer).map_err(|_| {
                archive_error("A ZIP entry is corrupt or could not be decompressed.")
            })?;
            if read == 0 {
                break;
            }
            total = total
                .checked_add(read as u64)
                .ok_or_else(|| archive_error("The ZIP expands beyond the extraction limit."))?;
            if total > limits.extracted_bytes {
                return Err(archive_error(format!(
                    "This ZIP expands beyond the {} GiB extraction limit.",
                    limits.extracted_bytes / (1024 * 1024 * 1024)
                )));
            }
            output_file
                .write_all(&buffer[..read])
                .map_err(|_| archive_error("Could not write an extracted file."))?;
        }
        files = files
            .checked_add(1)
            .ok_or_else(|| archive_error("The ZIP contains too many files."))?;
    }
    Ok(files)
}

/// Materialize each directory component with `create_dir`, rather than
/// `create_dir_all`, so the filesystem itself gets the final say about Unicode
/// normalization and case collisions. A path already created for the same
/// logical spelling is reusable; an untracked `AlreadyExists` is a collision.
fn ensure_directories(
    staging: &Path,
    relative: &Path,
    created: &mut HashSet<PathBuf>,
) -> Result<(), AppError> {
    let mut current = PathBuf::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(archive_error("The ZIP contains a traversal path."));
        };
        current.push(name);
        if created.contains(&current) {
            let metadata = fs::symlink_metadata(staging.join(&current))
                .map_err(|_| archive_error("An extraction folder changed unexpectedly."))?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(archive_error("An extraction path changed unexpectedly."));
            }
            continue;
        }
        match fs::create_dir(staging.join(&current)) {
            Ok(()) => {
                created.insert(current.clone());
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(archive_error(
                    "The ZIP contains directory names that collide on this filesystem.",
                ));
            }
            Err(_) => return Err(archive_error("Could not create a folder from the ZIP.")),
        }
    }
    Ok(())
}

fn publish_staging(parent: &Path, stem: &str, staging: &Path) -> Result<PathBuf, AppError> {
    for suffix in 1u32.. {
        let name = if suffix == 1 {
            stem.to_string()
        } else {
            format!("{stem} ({suffix})")
        };
        let destination = parent.join(name);
        if sibling_name_exists_case_insensitively(parent, &destination)? {
            continue;
        }
        match rename_without_replacing(staging, &destination) {
            Ok(()) => return Ok(destination),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(archive_error("Could not publish the extracted folder.")),
        }
    }
    unreachable!("u32 suffix iteration is practically bounded by directory capacity")
}

fn sibling_name_exists_case_insensitively(
    parent: &Path,
    candidate: &Path,
) -> Result<bool, AppError> {
    let wanted = candidate
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .ok_or_else(|| archive_error("Could not choose an extraction folder name."))?;
    let entries = fs::read_dir(parent)
        .map_err(|_| archive_error("Could not inspect the ZIP's parent folder."))?;
    for entry in entries {
        let entry =
            entry.map_err(|_| archive_error("Could not inspect the ZIP's parent folder."))?;
        if entry.file_name().to_string_lossy().to_lowercase() == wanted {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(target_os = "macos")]
fn rename_without_replacing(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int};
    use std::os::unix::ffi::OsStrExt;

    unsafe extern "C" {
        fn renamex_np(from: *const c_char, to: *const c_char, flags: u32) -> c_int;
    }
    const RENAME_EXCL: u32 = 0x00000004;
    let from = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in staging path"))?;
    let to = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in destination path"))?;
    // SAFETY: both pointers refer to live NUL-terminated strings for this call.
    if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn rename_without_replacing(from: &Path, to: &Path) -> io::Result<()> {
    // Windows rename fails when the destination exists. On other Unix targets,
    // the preceding sibling scan avoids normal collisions; protecting against
    // a hostile concurrent ancestor mutation requires descriptor-relative APIs.
    fs::rename(from, to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "gogglelab-archive-test-{}-{sequence}-{name}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, body) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(body).unwrap();
        }
        fs::write(path, writer.finish().unwrap().into_inner()).unwrap();
    }

    fn assert_archive_error(result: Result<ExtractResult, AppError>) {
        assert!(matches!(result, Err(AppError::Archive { .. })));
    }

    fn archive_error_message(result: Result<ExtractResult, AppError>) -> String {
        match result {
            Err(AppError::Archive { message }) => message,
            _ => panic!("expected archive error"),
        }
    }

    fn patch_u16_after_signatures(path: &Path, signatures: &[([u8; 4], usize)], value: u16) {
        let mut bytes = fs::read(path).unwrap();
        for (signature, offset) in signatures {
            let start = bytes
                .windows(4)
                .position(|window| window == signature)
                .expect("ZIP signature");
            bytes[start + offset..start + offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn extracts_nested_files_into_a_fresh_sibling_and_keeps_the_zip() {
        let root = TestDir::new("nested");
        let archive = root.0.join("Models.ZIP");
        write_zip(
            &archive,
            &[("parts/body.stl", b"body"), ("notes/readme.txt", b"notes")],
        );

        let result = extract_zip_core(root.0.to_str().unwrap(), archive.to_str().unwrap()).unwrap();

        assert_eq!(result.files, 2);
        assert_eq!(Path::new(&result.destination), root.0.join("Models"));
        assert_eq!(
            fs::read(root.0.join("Models/parts/body.stl")).unwrap(),
            b"body"
        );
        assert_eq!(
            fs::read(root.0.join("Models/notes/readme.txt")).unwrap(),
            b"notes"
        );
        assert!(archive.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&result.destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700);
        }
    }

    #[test]
    fn existing_destination_is_untouched_and_numbered_destination_is_used() {
        let root = TestDir::new("destination-collision");
        let archive = root.0.join("Models.zip");
        write_zip(&archive, &[("part.stl", b"new")]);
        fs::create_dir(root.0.join("models")).unwrap();
        fs::write(root.0.join("models/keep.txt"), b"keep").unwrap();

        let result = extract_zip_core(root.0.to_str().unwrap(), archive.to_str().unwrap()).unwrap();

        assert_eq!(Path::new(&result.destination), root.0.join("Models (2)"));
        assert_eq!(fs::read(root.0.join("models/keep.txt")).unwrap(), b"keep");
        assert_eq!(
            fs::read(root.0.join("Models (2)/part.stl")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn rejects_traversal_absolute_backslash_and_drive_paths_without_escape() {
        for (name, entry) in [
            ("parent", "../escape.stl"),
            ("absolute", "/tmp/escape.stl"),
            ("backslash", "..\\escape.stl"),
            ("drive", "C:/escape.stl"),
        ] {
            let root = TestDir::new(name);
            let archive = root.0.join("bad.zip");
            write_zip(&archive, &[(entry, b"bad")]);
            assert_archive_error(extract_zip_core(
                root.0.to_str().unwrap(),
                archive.to_str().unwrap(),
            ));
            assert!(!root.0.join("bad").exists());
        }
    }

    #[test]
    fn rejects_symlink_duplicate_and_file_folder_conflicts() {
        let root = TestDir::new("unsafe-types");
        let symlink_zip = root.0.join("links.zip");
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .add_symlink("link", "../escape", SimpleFileOptions::default())
            .unwrap();
        fs::write(&symlink_zip, writer.finish().unwrap().into_inner()).unwrap();
        assert_archive_error(extract_zip_core(
            root.0.to_str().unwrap(),
            symlink_zip.to_str().unwrap(),
        ));

        for (name, entries) in [
            (
                "duplicate",
                vec![("A.stl", b"a".as_slice()), ("a.STL", b"b".as_slice())],
            ),
            (
                "conflict",
                vec![
                    ("parts", b"file".as_slice()),
                    ("parts/a.stl", b"part".as_slice()),
                ],
            ),
        ] {
            let archive = root.0.join(format!("{name}.zip"));
            write_zip(&archive, &entries);
            assert_archive_error(extract_zip_core(
                root.0.to_str().unwrap(),
                archive.to_str().unwrap(),
            ));
            assert!(!root.0.join(name).exists());
        }
    }

    #[test]
    fn corrupt_zip_and_actual_inflated_limit_leave_no_partial_destination() {
        let root = TestDir::new("limits");
        let corrupt = root.0.join("corrupt.zip");
        fs::write(&corrupt, b"not a zip").unwrap();
        assert_archive_error(extract_zip_core(
            root.0.to_str().unwrap(),
            corrupt.to_str().unwrap(),
        ));

        let archive = root.0.join("large.zip");
        write_zip(&archive, &[("large.bin", &[7; 128])]);
        let limits = ExtractionLimits {
            declared_extracted_bytes: u64::MAX,
            extracted_bytes: 64,
            ..RUNTIME_LIMITS
        };
        assert_archive_error(extract_zip_with_limits(
            root.0.to_str().unwrap(),
            archive.to_str().unwrap(),
            limits,
        ));
        assert!(!root.0.join("large").exists());
        assert!(!fs::read_dir(&root.0).unwrap().flatten().any(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with(".gogglelab-extract-")));
    }

    #[test]
    fn encrypted_and_unsupported_entries_have_actionable_diagnostics() {
        let root = TestDir::new("entry-diagnostics");

        let encrypted = root.0.join("encrypted.zip");
        write_zip(&encrypted, &[("part.stl", b"part")]);
        let mut encrypted_bytes = fs::read(&encrypted).unwrap();
        for (signature, offset) in [(*b"PK\x03\x04", 6usize), (*b"PK\x01\x02", 8usize)] {
            let start = encrypted_bytes
                .windows(4)
                .position(|window| window == signature)
                .unwrap();
            let existing = u16::from_le_bytes([
                encrypted_bytes[start + offset],
                encrypted_bytes[start + offset + 1],
            ]);
            encrypted_bytes[start + offset..start + offset + 2]
                .copy_from_slice(&(existing | 1).to_le_bytes());
        }
        fs::write(&encrypted, encrypted_bytes).unwrap();
        let message = archive_error_message(extract_zip_core(
            root.0.to_str().unwrap(),
            encrypted.to_str().unwrap(),
        ));
        assert!(message.contains("Password-protected"), "{message}");

        let unsupported = root.0.join("unsupported.zip");
        write_zip(&unsupported, &[("part.stl", b"part")]);
        patch_u16_after_signatures(
            &unsupported,
            &[(*b"PK\x03\x04", 8), (*b"PK\x01\x02", 10)],
            98,
        );
        let message = archive_error_message(extract_zip_core(
            root.0.to_str().unwrap(),
            unsupported.to_str().unwrap(),
        ));
        assert!(message.contains("compression method"), "{message}");
    }

    #[test]
    fn declared_inflated_total_is_rejected_before_staging() {
        let root = TestDir::new("declared-limit");
        let archive = root.0.join("declared.zip");
        write_zip(&archive, &[("large.bin", &[1; 128])]);
        let limits = ExtractionLimits {
            declared_extracted_bytes: 64,
            extracted_bytes: u64::MAX,
            ..RUNTIME_LIMITS
        };
        let message = archive_error_message(extract_zip_with_limits(
            root.0.to_str().unwrap(),
            archive.to_str().unwrap(),
            limits,
        ));
        assert!(message.contains("declares"), "{message}");
        assert!(!root.0.join("declared").exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn filesystem_normalized_unicode_directory_collisions_are_rejected() {
        let root = TestDir::new("unicode-collision");
        let archive = root.0.join("unicode.zip");
        write_zip(
            &archive,
            &[
                ("caf\u{e9}/one.stl", b"one"),
                ("cafe\u{301}/two.stl", b"two"),
            ],
        );

        assert_archive_error(extract_zip_core(
            root.0.to_str().unwrap(),
            archive.to_str().unwrap(),
        ));
        assert!(!root.0.join("unicode").exists());
    }

    #[cfg(unix)]
    #[test]
    fn source_symlink_and_source_outside_root_are_rejected() {
        let root = TestDir::new("source-root");
        let outside = TestDir::new("source-outside");
        let archive = outside.0.join("models.zip");
        write_zip(&archive, &[("part.stl", b"part")]);
        std::os::unix::fs::symlink(&archive, root.0.join("linked.zip")).unwrap();

        assert_archive_error(extract_zip_core(
            root.0.to_str().unwrap(),
            root.0.join("linked.zip").to_str().unwrap(),
        ));
        assert!(matches!(
            extract_zip_core(root.0.to_str().unwrap(), archive.to_str().unwrap()),
            Err(AppError::OutsideRoot)
        ));
    }
}
