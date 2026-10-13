use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};
use tauri::State;

use crate::extension_api::{self, AppError, AppState, OriginalSource, SlicerApp, SourceFormat};
use crate::library_commands::LibraryState;

static HANDOFF_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static HANDOFF_STORAGE_LOCK: Mutex<()> = Mutex::new(());

const MAX_BATCH_MODELS: usize = 100;
const MAX_BATCH_BYTES: u64 = 256 * 1024 * 1024;
const MAX_HANDOFF_STORAGE_BYTES: u64 = 1024 * 1024 * 1024;
const HANDOFF_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
const HANDOFF_ROOT_MARKER: &str = ".gogglelab-handoff-root";
const HANDOFF_DIRECTORY_MARKER: &str = ".gogglelab-handoff-directory";
const HANDOFF_ROOT_ID: &str = "gogglelab-handoff-root-v1";

/// Opens the original bytes for the current viewer generation in a supported,
/// freshly detected slicer. `with_original_source` keeps the generation and
/// current-model locks held through launch admission, so a newer model cannot
/// replace the source between validation and the external side effect.
#[tauri::command]
pub fn open_in_slicer(
    viewer: State<'_, AppState>,
    library: State<'_, LibraryState>,
    generation: u64,
    slicer_id: String,
) -> Result<(), AppError> {
    let _io = library.begin_io()?;
    extension_api::with_original_source_digest(&viewer, generation, |source, digest| {
        checked_owned_source_launch(
            source,
            digest,
            library.root(),
            &slicer_id,
            extension_api::installed_slicers,
            launch,
        )
        .map(|_| ())
    })
}

fn checked_owned_source_launch(
    source: &OriginalSource,
    expected_digest: &str,
    library_root: &Path,
    slicer_id: &str,
    detect: impl FnOnce() -> Vec<SlicerApp>,
    launch: impl FnOnce(&SlicerApp, &Path) -> Result<(), AppError>,
) -> Result<PathBuf, AppError> {
    let directory = copy_loaded_source(source, expected_digest, library_root)?;
    let path = directory.join(handoff_filename(source));
    let installed = detect();
    let result = checked_source_launch(
        &OriginalSource {
            path: path.clone(),
            format: source.format,
        },
        slicer_id,
        &installed,
        launch,
    );
    if result.is_err() {
        let _ = fs::remove_dir_all(&directory);
    }
    result.map(|_| path)
}

fn copy_loaded_source(
    source: &OriginalSource,
    expected_digest: &str,
    library_root: &Path,
) -> Result<PathBuf, AppError> {
    if expected_digest.len() != 64 || !expected_digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(AppError::Library {
            message: "Loaded source digest is invalid".into(),
        });
    }
    let metadata =
        fs::symlink_metadata(&source.path).map_err(|error| crate::error::map_io_error(&error))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > stl_core::limits::MAX_FILE_BYTES
    {
        return Err(AppError::Library {
            message: "Loaded source is not a safe regular model file".into(),
        });
    }
    let mut input =
        fs::File::open(&source.path).map_err(|error| crate::error::map_io_error(&error))?;
    let mut bytes = Vec::new();
    (&mut input)
        .take(stl_core::limits::MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| crate::error::map_io_error(&error))?;
    if bytes.len() as u64 != metadata.len()
        || format!("{:x}", Sha256::digest(&bytes)) != expected_digest
    {
        return Err(AppError::Library {
            message: "Loaded source changed after it was opened".into(),
        });
    }
    stage_handoff_bytes(
        &bytes,
        &handoff_filename(source),
        expected_digest,
        library_root,
    )
    .map(|(directory, _)| directory)
}

fn stage_handoff_bytes(
    bytes: &[u8],
    file_name: &str,
    expected_digest: &str,
    library_root: &Path,
) -> Result<(PathBuf, PathBuf), AppError> {
    if bytes.len() as u64 > stl_core::limits::MAX_FILE_BYTES
        || expected_digest.len() != 64
        || !expected_digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        || format!("{:x}", Sha256::digest(bytes)) != expected_digest
    {
        return Err(AppError::Library {
            message: "Handoff source failed size or digest verification".into(),
        });
    }
    let root = stable_handoff_root(library_root)?;
    let _storage = HANDOFF_STORAGE_LOCK.lock().map_err(|_| AppError::Library {
        message: "Handoff storage is unavailable".into(),
    })?;
    create_owned_handoff_root(&root)?;
    prepare_handoff_storage(&root, bytes.len() as u64)?;
    for _ in 0..1024 {
        let directory = root.join(format!(
            "single-{}-{}-{}",
            std::process::id(),
            HANDOFF_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            &expected_digest[..12]
        ));
        match create_private_directory(&directory) {
            Ok(()) => {
                if let Err(error) = mark_owned_handoff_directory(&directory) {
                    let _ = fs::remove_dir_all(&directory);
                    return Err(error);
                }
                let path = directory.join(file_name);
                let result = (|| {
                    let mut output = open_private_file(&path)
                        .map_err(|error| crate::error::map_io_error(&error))?;
                    set_private_file_permissions(&path)?;
                    output
                        .write_all(bytes)
                        .map_err(|error| crate::error::map_io_error(&error))?;
                    output
                        .sync_all()
                        .map_err(|error| crate::error::map_io_error(&error))?;
                    Ok::<_, AppError>(())
                })();
                if let Err(error) = result {
                    let _ = fs::remove_dir_all(&directory);
                    return Err(error);
                }
                return Ok((directory, path));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(crate::error::map_io_error(&error)),
        }
    }
    Err(AppError::Library {
        message: "Could not allocate stable handoff storage".into(),
    })
}

pub(crate) struct VerifiedHandoffRequest<'a> {
    pub bytes: &'a [u8],
    pub file_name: &'a str,
    pub expected_digest: &'a str,
    pub format: SourceFormat,
    pub library_root: &'a Path,
    pub slicer_id: &'a str,
}

pub(crate) struct StreamHandoffRequest<'a> {
    pub content_id: &'a str,
    pub file_name: &'a str,
    pub expected_digest: &'a str,
    pub expected_bytes: u64,
    pub format: SourceFormat,
    pub library_root: &'a Path,
    pub slicer_id: &'a str,
}

pub(crate) fn checked_bytes_launch(
    request: VerifiedHandoffRequest<'_>,
    detect: impl FnOnce() -> Vec<SlicerApp>,
    launch: impl FnOnce(&SlicerApp, &Path) -> Result<(), AppError>,
) -> Result<PathBuf, AppError> {
    let VerifiedHandoffRequest {
        bytes,
        file_name,
        expected_digest,
        format,
        library_root,
        slicer_id,
    } = request;
    let installed = detect();
    if supported_slicer(slicer_id, &installed).is_none() {
        return Err(AppError::UnknownSlicer);
    }
    let name = handoff_filename(&OriginalSource {
        path: PathBuf::from(file_name),
        format,
    });
    let (directory, path) = stage_handoff_bytes(bytes, &name, expected_digest, library_root)?;
    let result = checked_source_launch(
        &OriginalSource {
            path: path.clone(),
            format,
        },
        slicer_id,
        &installed,
        launch,
    );
    if result.is_err() {
        let _ = fs::remove_dir_all(&directory);
    }
    result.map(|_| path)
}

#[cfg(test)]
pub(crate) fn checked_bytes_batch_launch(
    requests: &[VerifiedHandoffRequest<'_>],
    detect: impl FnOnce() -> Vec<SlicerApp>,
    launch: impl FnOnce(&SlicerApp, &[PathBuf]) -> Result<(), AppError>,
) -> Result<Vec<PathBuf>, AppError> {
    for request in requests {
        if request.bytes.len() as u64 > stl_core::limits::MAX_FILE_BYTES
            || request.expected_digest.len() != 64
            || !request
                .expected_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || format!("{:x}", Sha256::digest(request.bytes)) != request.expected_digest
        {
            return Err(AppError::Library {
                message: "Slicer handoff source failed size or digest verification".into(),
            });
        }
    }
    let content_ids = (0..requests.len())
        .map(|index| index.to_string())
        .collect::<Vec<_>>();
    let streams = requests
        .iter()
        .zip(&content_ids)
        .map(|(request, content_id)| StreamHandoffRequest {
            content_id,
            file_name: request.file_name,
            expected_digest: request.expected_digest,
            expected_bytes: request.bytes.len() as u64,
            format: request.format,
            library_root: request.library_root,
            slicer_id: request.slicer_id,
        })
        .collect::<Vec<_>>();
    checked_stream_batch_launch(
        &streams,
        |content_id, sink| {
            let index = content_id.parse::<usize>().map_err(|_| AppError::Library {
                message: "Slicer handoff content ID is invalid".into(),
            })?;
            let request = requests.get(index).ok_or_else(|| AppError::Library {
                message: "Slicer handoff content ID is invalid".into(),
            })?;
            sink.write_all(request.bytes)
                .map_err(|error| crate::error::map_io_error(&error))
        },
        detect,
        launch,
    )
}

pub(crate) fn checked_stream_batch_launch(
    requests: &[StreamHandoffRequest<'_>],
    mut stream: impl FnMut(&str, &mut dyn Write) -> Result<(), AppError>,
    detect: impl FnOnce() -> Vec<SlicerApp>,
    launch: impl FnOnce(&SlicerApp, &[PathBuf]) -> Result<(), AppError>,
) -> Result<Vec<PathBuf>, AppError> {
    if requests.is_empty() || requests.len() > MAX_BATCH_MODELS {
        return Err(AppError::Library {
            message: "Slicer handoff target count is invalid".into(),
        });
    }
    let slicer_id = requests[0].slicer_id;
    let mut total_bytes = 0_u64;
    for request in requests {
        if request.slicer_id != slicer_id || request.library_root != requests[0].library_root {
            return Err(AppError::Library {
                message: "Slicer handoff targets must use the same slicer and library".into(),
            });
        }
        total_bytes = total_bytes
            .checked_add(request.expected_bytes)
            .ok_or_else(|| AppError::Library {
                message: "Slicer handoff byte total is out of range".into(),
            })?;
        if total_bytes > MAX_BATCH_BYTES {
            return Err(AppError::Library {
                message: "Slicer handoff batch exceeds its byte limit".into(),
            });
        }
        if request.expected_bytes == 0
            || request.expected_bytes > stl_core::limits::MAX_FILE_BYTES
            || request.expected_digest.len() != 64
            || !request
                .expected_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(AppError::Library {
                message: "Slicer handoff source failed size or digest verification".into(),
            });
        }
    }
    let installed = detect();
    if supported_slicer(slicer_id, &installed).is_none() {
        return Err(AppError::UnknownSlicer);
    }
    let root = stable_handoff_root(requests[0].library_root)?;
    let _storage = HANDOFF_STORAGE_LOCK.lock().map_err(|_| AppError::Library {
        message: "Handoff storage is unavailable".into(),
    })?;
    create_owned_handoff_root(&root)?;
    prepare_handoff_storage(&root, total_bytes)?;
    let mut directory = None;
    for _ in 0..1024 {
        let candidate = root.join(format!(
            "batch-{}-{}",
            std::process::id(),
            HANDOFF_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        match create_private_directory(&candidate) {
            Ok(()) => {
                if let Err(error) = mark_owned_handoff_directory(&candidate) {
                    let _ = fs::remove_dir_all(&candidate);
                    return Err(error);
                }
                directory = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(crate::error::map_io_error(&error)),
        }
    }
    let directory = directory.ok_or_else(|| AppError::Library {
        message: "Could not allocate stable handoff storage".into(),
    })?;
    let mut paths = Vec::with_capacity(requests.len());
    for (index, request) in requests.iter().enumerate() {
        let source = OriginalSource {
            path: PathBuf::from(request.file_name),
            format: request.format,
        };
        let path = directory.join(format!("{:03}-{}", index + 1, handoff_filename(&source)));
        let result = (|| {
            let file =
                open_private_file(&path).map_err(|error| crate::error::map_io_error(&error))?;
            set_private_file_permissions(&path)?;
            let mut writer = DigestFileWriter::new(file, request.expected_bytes);
            stream(request.content_id, &mut writer)?;
            writer.finish(request.expected_digest)?;
            Ok::<_, AppError>(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_dir_all(&directory);
            return Err(error);
        }
        paths.push(path);
    }
    drop(_storage);
    let result = checked_paths_launch(&paths, slicer_id, &installed, launch);
    if result.is_err() {
        let _ = fs::remove_dir_all(&directory);
    }
    result.map(|()| paths)
}

struct DigestFileWriter {
    file: fs::File,
    digest: Sha256,
    written: u64,
    expected: u64,
}

impl DigestFileWriter {
    fn new(file: fs::File, expected: u64) -> Self {
        Self {
            file,
            digest: Sha256::new(),
            written: 0,
            expected,
        }
    }

    fn finish(self, expected_digest: &str) -> Result<(), AppError> {
        self.file
            .sync_all()
            .map_err(|error| crate::error::map_io_error(&error))?;
        let actual_digest = format!("{:x}", self.digest.finalize());
        if self.written != self.expected || actual_digest != expected_digest {
            return Err(AppError::Library {
                message: "Slicer handoff source failed size or digest verification".into(),
            });
        }
        Ok(())
    }
}

impl Write for DigestFileWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let requested = u64::try_from(bytes.len())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?;
        let next = self
            .written
            .checked_add(requested)
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidData))?;
        if next > self.expected {
            return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
        }
        let written = self.file.write(bytes)?;
        self.digest.update(&bytes[..written]);
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn handoff_filename(source: &OriginalSource) -> String {
    let raw = source
        .path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("model");
    let mut basename = raw
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, ' ' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    basename = basename.trim_matches([' ', '.']).to_owned();
    if basename.is_empty() {
        basename = "model".into();
    }
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if reserved
        .iter()
        .any(|candidate| basename.eq_ignore_ascii_case(candidate))
    {
        basename.insert_str(0, "model-");
    }
    let extension = match source.format {
        SourceFormat::Stl => "stl",
        SourceFormat::ThreeMf => "3mf",
    };
    format!("{basename}.{extension}")
}

pub(crate) fn stable_handoff_root(library_root: &Path) -> Result<PathBuf, AppError> {
    #[cfg(windows)]
    {
        let profile = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| AppError::Library {
                message:
                    "Windows local application data is unavailable for private handoff storage"
                        .into(),
            })?;
        let user_profile = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .ok_or_else(|| AppError::Library {
                message: "Windows user profile is unavailable for private handoff storage".into(),
            })?;
        if !profile.starts_with(&user_profile) {
            return Err(AppError::Library {
                message: "Windows local application data is outside the current user profile"
                    .into(),
            });
        }
        let library_name = library_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("library");
        // This assumes LOCALAPPDATA is the OS-managed current-user profile
        // location; shared or redirected-outside-profile values fail closed.
        // Staged bytes therefore avoid inheriting ACLs from a selected model
        // volume with broader access.
        return Ok(profile.join(format!("GoggleLab-{library_name}-handoff-v1")));
    }
    #[cfg(not(windows))]
    {
        let parent = library_root.parent().ok_or_else(|| AppError::Library {
            message: "Library has no stable parent for handoff assets".into(),
        })?;
        let library_name = library_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("library");
        Ok(parent.join(format!("{library_name}-gogglelab-handoff-v1")))
    }
}

fn create_owned_handoff_root(root: &Path) -> Result<(), AppError> {
    match fs::symlink_metadata(root) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || !is_private(&metadata) =>
        {
            return Err(AppError::Library {
                message: "Handoff namespace is not a real directory".into(),
            });
        }
        Ok(_) => verify_owner_marker(root.join(HANDOFF_ROOT_MARKER), HANDOFF_ROOT_ID)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_directory(root).map_err(|error| crate::error::map_io_error(&error))?;
            set_private_directory_permissions(root)?;
            let marker = root.join(HANDOFF_ROOT_MARKER);
            if let Err(error) = write_owner_marker(marker.clone(), HANDOFF_ROOT_ID) {
                let _ = fs::remove_file(marker);
                let _ = fs::remove_dir(root);
                return Err(error);
            }
        }
        Err(error) => return Err(crate::error::map_io_error(&error)),
    }
    Ok(())
}

fn mark_owned_handoff_directory(directory: &Path) -> Result<(), AppError> {
    set_private_directory_permissions(directory)?;
    let name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::Library {
            message: "Handoff directory name is invalid".into(),
        })?;
    write_owner_marker(
        directory.join(HANDOFF_DIRECTORY_MARKER),
        &format!("gogglelab-handoff-directory-v1\n{name}"),
    )
}

fn is_owned_handoff_directory(directory: &Path) -> bool {
    let Some(name) = directory.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if !name.starts_with("single-") && !name.starts_with("batch-") {
        return false;
    }
    verify_owner_marker(
        directory.join(HANDOFF_DIRECTORY_MARKER),
        &format!("gogglelab-handoff-directory-v1\n{name}"),
    )
    .is_ok()
}

fn write_owner_marker(path: PathBuf, value: &str) -> Result<(), AppError> {
    let mut marker = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| crate::error::map_io_error(&error))?;
    marker
        .write_all(value.as_bytes())
        .map_err(|error| crate::error::map_io_error(&error))?;
    marker
        .sync_all()
        .map_err(|error| crate::error::map_io_error(&error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|error| crate::error::map_io_error(&error))?;
    }
    Ok(())
}

fn verify_owner_marker(path: PathBuf, expected: &str) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(&path).map_err(|_| AppError::Library {
        message: "Handoff namespace is not owned by GoggleLab".into(),
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > 128
        || !is_private(&metadata)
    {
        return Err(AppError::Library {
            message: "Handoff namespace ownership marker is invalid".into(),
        });
    }
    let value = fs::read_to_string(path).map_err(|_| AppError::Library {
        message: "Handoff namespace ownership marker is unreadable".into(),
    })?;
    if value != expected {
        return Err(AppError::Library {
            message: "Handoff namespace is not owned by GoggleLab".into(),
        });
    }
    Ok(())
}

fn set_private_directory_permissions(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| crate::error::map_io_error(&error))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn set_private_file_permissions(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| crate::error::map_io_error(&error))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn open_private_file(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn is_private(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o077 == 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        // On Windows the namespace is under the user's protected LOCALAPPDATA
        // DACL; other supported targets are Unix-like.
        true
    }
}

fn prepare_handoff_storage(root: &Path, incoming_bytes: u64) -> Result<(), AppError> {
    let now = SystemTime::now();
    let mut retained_bytes = 0_u64;
    let entries = fs::read_dir(root).map_err(|error| crate::error::map_io_error(&error))?;
    for entry in entries {
        let entry = entry.map_err(|error| crate::error::map_io_error(&error))?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("single-")
            && !name.to_string_lossy().starts_with("batch-")
        {
            continue;
        }
        let path = entry.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| crate::error::map_io_error(&error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() || !is_private(&metadata) {
            continue;
        }
        if !is_owned_handoff_directory(&path) {
            continue;
        }
        let expired = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > HANDOFF_RETENTION);
        if expired {
            fs::remove_dir_all(&path).map_err(|error| crate::error::map_io_error(&error))?;
            continue;
        }
        retained_bytes = retained_bytes
            .checked_add(handoff_tree_bytes(&path)?)
            .ok_or_else(|| AppError::Library {
                message: "Handoff storage size is out of range".into(),
            })?;
    }
    if retained_bytes
        .checked_add(incoming_bytes)
        .is_none_or(|total| total > MAX_HANDOFF_STORAGE_BYTES)
    {
        return Err(AppError::Library {
            message: "Handoff storage is full; retained files expire after 24 hours".into(),
        });
    }
    Ok(())
}

fn handoff_tree_bytes(directory: &Path) -> Result<u64, AppError> {
    let mut total = 0_u64;
    for entry in fs::read_dir(directory).map_err(|error| crate::error::map_io_error(&error))? {
        let entry = entry.map_err(|error| crate::error::map_io_error(&error))?;
        let path = entry.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| crate::error::map_io_error(&error))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let bytes = if metadata.is_file() {
            metadata.len()
        } else if metadata.is_dir() {
            handoff_tree_bytes(&path)?
        } else {
            0
        };
        total = total.checked_add(bytes).ok_or_else(|| AppError::Library {
            message: "Handoff storage size is out of range".into(),
        })?;
    }
    Ok(total)
}

fn checked_source_launch(
    source: &OriginalSource,
    slicer_id: &str,
    installed: &[SlicerApp],
    launch: impl FnOnce(&SlicerApp, &Path) -> Result<(), AppError>,
) -> Result<(), AppError> {
    // SourceFormat is produced by the parser for the current model. Keeping
    // this exhaustive match makes a future format an explicit launch-policy
    // decision instead of silently handing arbitrary input to another app.
    match source.format {
        SourceFormat::Stl | SourceFormat::ThreeMf => {}
    }
    let slicer = supported_slicer(slicer_id, installed).ok_or(AppError::UnknownSlicer)?;
    launch(slicer, &source.path)
}

fn supported_slicer<'a>(slicer_id: &str, installed: &'a [SlicerApp]) -> Option<&'a SlicerApp> {
    installed
        .iter()
        .find(|slicer| slicer.id == slicer_id && slicer.bundle_id == slicer_id)
}

pub(crate) fn is_supported_slicer(slicer_id: &str, installed: &[SlicerApp]) -> bool {
    supported_slicer(slicer_id, installed).is_some()
}

pub(crate) fn checked_paths_launch(
    source_paths: &[PathBuf],
    slicer_id: &str,
    installed: &[SlicerApp],
    launch: impl FnOnce(&SlicerApp, &[PathBuf]) -> Result<(), AppError>,
) -> Result<(), AppError> {
    if source_paths.is_empty() {
        return Err(AppError::Library {
            message: "Bundle has no admitted model sources".into(),
        });
    }
    let slicer = supported_slicer(slicer_id, installed).ok_or(AppError::UnknownSlicer)?;
    launch(slicer, source_paths)
}

#[cfg(any(target_os = "macos", test))]
fn launch_arguments(slicer_id: &str, source_paths: &[PathBuf]) -> Vec<OsString> {
    let mut arguments = vec![
        OsString::from("-n"),
        OsString::from("-b"),
        OsString::from(slicer_id),
    ];
    arguments.extend(source_paths.iter().map(|path| path.as_os_str().to_owned()));
    arguments
}

#[cfg(target_os = "macos")]
fn launch(slicer: &SlicerApp, source_path: &Path) -> Result<(), AppError> {
    launch_paths(slicer, &[source_path.to_path_buf()])
}

#[cfg(target_os = "macos")]
pub(crate) fn launch_paths(slicer: &SlicerApp, source_paths: &[PathBuf]) -> Result<(), AppError> {
    let status = Command::new("open")
        .args(launch_arguments(&slicer.bundle_id, source_paths))
        .status()
        .map_err(|_| AppError::LaunchFailed { status: None })?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::LaunchFailed {
            status: status.code(),
        })
    }
}

#[cfg(not(target_os = "macos"))]
fn launch(slicer: &SlicerApp, source_path: &Path) -> Result<(), AppError> {
    launch_paths(slicer, &[source_path.to_path_buf()])
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn launch_paths(slicer: &SlicerApp, source_paths: &[PathBuf]) -> Result<(), AppError> {
    launch_executable(slicer, source_paths)
}

#[cfg(any(not(target_os = "macos"), test))]
fn launch_executable(slicer: &SlicerApp, source_paths: &[PathBuf]) -> Result<(), AppError> {
    // app_path comes only from the current allowlisted detector. Command and
    // args bypass a shell, preserving the model path as one literal argument.
    Command::new(&slicer.app_path)
        .args(executable_arguments(source_paths))
        .spawn()
        .map(|mut child| {
            // Reap without blocking the viewer for the lifetime of the slicer.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        })
        .map_err(|_| AppError::LaunchFailed { status: None })
}

#[cfg(any(not(target_os = "macos"), test))]
fn executable_arguments(source_paths: &[PathBuf]) -> Vec<OsString> {
    source_paths
        .iter()
        .map(|path| path.as_os_str().to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        checked_bytes_batch_launch, checked_bytes_launch, checked_owned_source_launch,
        checked_paths_launch, checked_source_launch, checked_stream_batch_launch,
        create_owned_handoff_root, launch_arguments, mark_owned_handoff_directory,
        stable_handoff_root, supported_slicer, StreamHandoffRequest, VerifiedHandoffRequest,
        HANDOFF_SEQUENCE, MAX_BATCH_BYTES, MAX_HANDOFF_STORAGE_BYTES,
    };
    use crate::extension_api::{AppError, OriginalSource, SlicerApp, SourceFormat};
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, SystemTime};

    fn installed_bambu() -> SlicerApp {
        SlicerApp {
            id: "com.bambulab.bambu-studio".into(),
            bundle_id: "com.bambulab.bambu-studio".into(),
            display_name: "Bambu Studio".into(),
            app_path: "/Applications/Bambu Studio.app".into(),
        }
    }

    #[test]
    fn catalog_handoff_stages_verified_bytes_under_a_safe_name() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        fs::create_dir(&library_root).unwrap();
        let bytes = b"verified catalog model bytes";
        let digest = format!("{:x}", Sha256::digest(bytes));
        let mut launched_path = None;

        let path = checked_bytes_launch(
            VerifiedHandoffRequest {
                bytes,
                file_name: "Part;$(touch marker).3mf",
                expected_digest: &digest,
                format: SourceFormat::ThreeMf,
                library_root: &library_root,
                slicer_id: "com.bambulab.bambu-studio",
            },
            || vec![installed_bambu()],
            |slicer, path| {
                assert_eq!(slicer.id, "com.bambulab.bambu-studio");
                assert_eq!(fs::read(path).unwrap(), bytes);
                launched_path = Some(path.to_path_buf());
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(launched_path.as_deref(), Some(path.as_path()));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some("3mf")
        );
        assert!(!path.to_string_lossy().contains('$'));
        assert!(!path.to_string_lossy().contains(';'));
        assert!(!path.to_string_lossy().contains('('));
        assert!(!path.to_string_lossy().contains(')'));
    }

    #[test]
    fn catalog_batch_handoff_preflights_and_launches_all_verified_copies_once() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        fs::create_dir(&library_root).unwrap();
        let first_bytes = b"first verified model";
        let second_bytes = b"second verified model";
        let first_digest = format!("{:x}", Sha256::digest(first_bytes));
        let second_digest = format!("{:x}", Sha256::digest(second_bytes));
        let requests = [
            VerifiedHandoffRequest {
                bytes: first_bytes,
                file_name: "First.stl",
                expected_digest: &first_digest,
                format: SourceFormat::Stl,
                library_root: &library_root,
                slicer_id: "com.bambulab.bambu-studio",
            },
            VerifiedHandoffRequest {
                bytes: second_bytes,
                file_name: "Second.3mf",
                expected_digest: &second_digest,
                format: SourceFormat::ThreeMf,
                library_root: &library_root,
                slicer_id: "com.bambulab.bambu-studio",
            },
        ];
        let mut launch_paths = Vec::new();
        let staged = checked_bytes_batch_launch(
            &requests,
            || vec![installed_bambu()],
            |slicer, paths| {
                assert_eq!(slicer.id, "com.bambulab.bambu-studio");
                assert_eq!(paths.len(), 2);
                assert_eq!(fs::read(&paths[0]).unwrap(), first_bytes);
                assert_eq!(fs::read(&paths[1]).unwrap(), second_bytes);
                launch_paths = paths.to_vec();
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(staged, launch_paths);
        assert_eq!(staged.len(), 2);
        assert_ne!(staged[0], staged[1]);
        assert_eq!(staged[0].extension().unwrap(), "stl");
        assert_eq!(staged[1].extension().unwrap(), "3mf");
    }

    #[test]
    fn catalog_stream_batch_handoff_writes_verified_models_to_stable_files() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        fs::create_dir(&library_root).unwrap();
        let first_bytes = b"first streamed model";
        let second_bytes = b"second streamed model";
        let first_digest = format!("{:x}", Sha256::digest(first_bytes));
        let second_digest = format!("{:x}", Sha256::digest(second_bytes));
        let requests = [
            StreamHandoffRequest {
                content_id: "lease-1",
                file_name: "First.stl",
                expected_digest: &first_digest,
                expected_bytes: first_bytes.len() as u64,
                format: SourceFormat::Stl,
                library_root: &library_root,
                slicer_id: "com.bambulab.bambu-studio",
            },
            StreamHandoffRequest {
                content_id: "lease-2",
                file_name: "Second.3mf",
                expected_digest: &second_digest,
                expected_bytes: second_bytes.len() as u64,
                format: SourceFormat::ThreeMf,
                library_root: &library_root,
                slicer_id: "com.bambulab.bambu-studio",
            },
        ];
        let handoff_root = stable_handoff_root(&library_root).unwrap();
        create_owned_handoff_root(&handoff_root).unwrap();
        let expired = handoff_root.join("batch-expired");
        fs::create_dir(&expired).unwrap();
        mark_owned_handoff_directory(&expired).unwrap();
        fs::write(expired.join("old.stl"), b"expired").unwrap();
        fs::File::open(&expired)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(SystemTime::now() - Duration::from_secs(25 * 60 * 60)),
            )
            .unwrap();
        let unowned = handoff_root.join("batch-user-created");
        fs::create_dir(&unowned).unwrap();
        fs::write(unowned.join("keep.txt"), b"user data").unwrap();
        let staged = checked_stream_batch_launch(
            &requests,
            |lease_id, sink| {
                let bytes: &[u8] = match lease_id {
                    "lease-1" => first_bytes,
                    "lease-2" => second_bytes,
                    other => panic!("unexpected lease {other}"),
                };
                sink.write_all(bytes).map_err(|error| AppError::Library {
                    message: error.to_string(),
                })
            },
            || vec![installed_bambu()],
            |slicer, paths| {
                assert_eq!(slicer.id, "com.bambulab.bambu-studio");
                assert_eq!(fs::read(&paths[0]).unwrap(), first_bytes);
                assert_eq!(fs::read(&paths[1]).unwrap(), second_bytes);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(staged.len(), 2);
        assert_ne!(staged[0], staged[1]);
        assert!(!expired.exists());
        assert_eq!(fs::read(unowned.join("keep.txt")).unwrap(), b"user data");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&handoff_root).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(staged[0].parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&staged[0]).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn catalog_handoff_storage_quota_rejects_a_batch_before_streaming() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        fs::create_dir(&library_root).unwrap();
        let handoff_root = stable_handoff_root(&library_root).unwrap();
        create_owned_handoff_root(&handoff_root).unwrap();
        let existing = handoff_root.join("batch-retained");
        fs::create_dir(&existing).unwrap();
        mark_owned_handoff_directory(&existing).unwrap();
        let retained = fs::File::create(existing.join("retained.stl")).unwrap();
        retained.set_len(MAX_HANDOFF_STORAGE_BYTES).unwrap();

        let request = StreamHandoffRequest {
            content_id: "lease",
            file_name: "Model.stl",
            expected_digest: &format!("{:x}", Sha256::digest(b"x")),
            expected_bytes: 1,
            format: SourceFormat::Stl,
            library_root: &library_root,
            slicer_id: "com.bambulab.bambu-studio",
        };
        let mut streamed = false;
        assert!(checked_stream_batch_launch(
            &[request],
            |_, _| {
                streamed = true;
                Ok(())
            },
            || vec![installed_bambu()],
            |_, _| panic!("full handoff storage must not launch"),
        )
        .is_err());
        assert!(!streamed);
    }

    #[test]
    fn catalog_batch_handoff_enforces_aggregate_byte_limit_before_allocating_storage() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        fs::create_dir(&library_root).unwrap();
        let digest = format!("{:x}", Sha256::digest(b"x"));
        let per_model_bytes = MAX_BATCH_BYTES / 3 + 1;
        let requests = [
            StreamHandoffRequest {
                content_id: "lease-1",
                file_name: "One.stl",
                expected_digest: &digest,
                expected_bytes: per_model_bytes,
                format: SourceFormat::Stl,
                library_root: &library_root,
                slicer_id: "com.bambulab.bambu-studio",
            },
            StreamHandoffRequest {
                content_id: "lease-2",
                file_name: "Two.stl",
                expected_digest: &digest,
                expected_bytes: per_model_bytes,
                format: SourceFormat::Stl,
                library_root: &library_root,
                slicer_id: "com.bambulab.bambu-studio",
            },
            StreamHandoffRequest {
                content_id: "lease-3",
                file_name: "Three.stl",
                expected_digest: &digest,
                expected_bytes: per_model_bytes,
                format: SourceFormat::Stl,
                library_root: &library_root,
                slicer_id: "com.bambulab.bambu-studio",
            },
        ];
        let mut streamed = false;
        let mut detected = false;
        assert!(checked_stream_batch_launch(
            &requests,
            |_, _| {
                streamed = true;
                Ok(())
            },
            || {
                detected = true;
                vec![installed_bambu()]
            },
            |_, _| panic!("oversized handoff must not launch"),
        )
        .is_err());
        assert!(!streamed);
        assert!(!detected);
        assert!(!stable_handoff_root(&library_root).unwrap().exists());
    }

    #[test]
    fn handoff_refuses_to_claim_a_preexisting_unowned_namespace() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        fs::create_dir(&library_root).unwrap();
        let handoff_root = stable_handoff_root(&library_root).unwrap();
        fs::create_dir(&handoff_root).unwrap();
        let user_file = handoff_root.join("keep.txt");
        fs::write(&user_file, b"user data").unwrap();
        let bytes = b"verified model";
        let digest = format!("{:x}", Sha256::digest(bytes));

        assert!(checked_bytes_launch(
            VerifiedHandoffRequest {
                bytes,
                file_name: "Model.stl",
                expected_digest: &digest,
                format: SourceFormat::Stl,
                library_root: &library_root,
                slicer_id: "com.bambulab.bambu-studio",
            },
            || vec![installed_bambu()],
            |_, _| panic!("unowned handoff namespace must not launch"),
        )
        .is_err());
        assert_eq!(fs::read(user_file).unwrap(), b"user data");
    }

    #[cfg(windows)]
    #[test]
    fn windows_handoff_storage_uses_the_users_local_app_data_acl() {
        let library_root = PathBuf::from(r"C:\shared\models\library");
        let (Some(local_app_data), Some(user_profile)) = (
            std::env::var_os("LOCALAPPDATA"),
            std::env::var_os("USERPROFILE"),
        ) else {
            assert!(stable_handoff_root(&library_root).is_err());
            return;
        };

        let root = stable_handoff_root(&library_root).unwrap();
        assert!(root.starts_with(PathBuf::from(local_app_data)));
        assert!(root.starts_with(PathBuf::from(user_profile)));
    }

    #[test]
    fn catalog_batch_handoff_rejects_a_bad_member_before_launch_or_staging() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        fs::create_dir(&library_root).unwrap();
        let bytes = b"verified model";
        let digest = format!("{:x}", Sha256::digest(bytes));
        let requests = [
            VerifiedHandoffRequest {
                bytes,
                file_name: "First.stl",
                expected_digest: &digest,
                format: SourceFormat::Stl,
                library_root: &library_root,
                slicer_id: "com.bambulab.bambu-studio",
            },
            VerifiedHandoffRequest {
                bytes,
                file_name: "Second.stl",
                expected_digest: &"0".repeat(64),
                format: SourceFormat::Stl,
                library_root: &library_root,
                slicer_id: "com.bambulab.bambu-studio",
            },
        ];
        assert!(checked_bytes_batch_launch(
            &requests,
            || vec![installed_bambu()],
            |_, _| panic!("a batch with a bad digest must not launch"),
        )
        .is_err());
        assert!(!stable_handoff_root(&library_root).unwrap().exists());
    }

    #[test]
    fn catalog_handoff_rejects_unknown_slicers_and_bad_hashes_before_launch() {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        fs::create_dir(&library_root).unwrap();
        let bytes = b"verified catalog model bytes";
        let digest = format!("{:x}", Sha256::digest(bytes));

        assert!(matches!(
            checked_bytes_launch(
                VerifiedHandoffRequest {
                    bytes,
                    file_name: "model.stl",
                    expected_digest: &digest,
                    format: SourceFormat::Stl,
                    library_root: &library_root,
                    slicer_id: "unknown.slicer",
                },
                || vec![installed_bambu()],
                |_, _| panic!("unknown slicer must not launch"),
            ),
            Err(AppError::UnknownSlicer)
        ));
        assert!(!stable_handoff_root(&library_root).unwrap().exists());

        assert!(checked_bytes_launch(
            VerifiedHandoffRequest {
                bytes,
                file_name: "model.stl",
                expected_digest: &"0".repeat(64),
                format: SourceFormat::Stl,
                library_root: &library_root,
                slicer_id: "com.bambulab.bambu-studio",
            },
            || vec![installed_bambu()],
            |_, _| panic!("digest mismatch must not launch"),
        )
        .is_err());
        assert!(!stable_handoff_root(&library_root).unwrap().exists());
    }

    #[test]
    fn free_handoff_launches_the_current_original_source() {
        let source = OriginalSource {
            path: "/tmp/模型 with spaces.stl".into(),
            format: SourceFormat::Stl,
        };
        let installed = [installed_bambu()];
        let mut launch_count = 0;

        checked_source_launch(
            &source,
            "com.bambulab.bambu-studio",
            &installed,
            |slicer, path| {
                launch_count += 1;
                assert_eq!(slicer.id, "com.bambulab.bambu-studio");
                assert_eq!(path, Path::new("/tmp/模型 with spaces.stl"));
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(launch_count, 1);
    }

    #[test]
    fn handoff_accepts_only_a_freshly_detected_bundle_identifier() {
        let installed = [installed_bambu()];

        assert!(supported_slicer("com.bambulab.bambu-studio", &installed).is_some());
        assert!(supported_slicer("/Applications/Other.app", &installed).is_none());
        assert!(supported_slicer("Bambu Studio", &installed).is_none());

        let mismatched = [SlicerApp {
            id: "com.bambulab.bambu-studio".into(),
            bundle_id: "attacker.controlled.bundle".into(),
            display_name: "Bambu Studio".into(),
            app_path: "/Applications/Bambu Studio.app".into(),
        }];
        assert!(supported_slicer("com.bambulab.bambu-studio", &mismatched).is_none());
    }

    #[test]
    fn unknown_slicer_never_reaches_the_launch_side_effect() {
        let source = OriginalSource {
            path: "/tmp/model.3mf".into(),
            format: SourceFormat::ThreeMf,
        };
        let installed = [installed_bambu()];
        let mut launched = false;

        let result = checked_source_launch(&source, "not.installed", &installed, |_, _| {
            launched = true;
            Ok(())
        });

        assert!(matches!(result, Err(AppError::UnknownSlicer)));
        assert!(!launched);
    }

    #[test]
    fn executable_launch_reports_missing_detected_executable() {
        let missing =
            std::env::temp_dir().join(format!("gogglelab-missing-slicer-{}", std::process::id()));
        let slicer = SlicerApp {
            app_path: missing.to_string_lossy().into_owned(),
            ..installed_bambu()
        };
        assert!(super::launch_executable(&slicer, &[PathBuf::from("source.stl")]).is_err());
    }

    #[test]
    fn executable_launch_preserves_source_as_one_literal_argument() {
        let path = Path::new("/tmp/模型 with spaces; $(echo unsafe).3mf");
        let args = super::executable_arguments(&[path.to_path_buf()]);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], path.as_os_str());
    }

    #[test]
    fn macos_launch_preserves_bundle_id_and_source_as_literal_arguments() {
        let path = Path::new("/tmp/模型 with spaces; $(echo unsafe).3mf");
        let args = launch_arguments("com.bambulab.bambu-studio", &[path.to_path_buf()]);

        assert_eq!(args[0], "-n");
        assert_eq!(args[1], "-b");
        assert_eq!(args[2], "com.bambulab.bambu-studio");
        assert_eq!(args[3], path.as_os_str());
    }

    #[test]
    fn bundle_launch_preserves_every_literal_path_in_order() {
        let paths = vec![
            PathBuf::from("/tmp/第一 part; $(unsafe).stl"),
            PathBuf::from("/tmp/second & final.3mf"),
        ];
        let installed = [installed_bambu()];
        let mut launched = false;
        checked_paths_launch(
            &paths,
            "com.bambulab.bambu-studio",
            &installed,
            |slicer, admitted| {
                launched = true;
                assert_eq!(slicer.id, "com.bambulab.bambu-studio");
                assert_eq!(admitted, paths);
                Ok(())
            },
        )
        .unwrap();
        assert!(launched);
        assert_eq!(super::executable_arguments(&paths), paths);
        let mac = launch_arguments("com.bambulab.bambu-studio", &paths);
        assert_eq!(&mac[..3], ["-n", "-b", "com.bambulab.bambu-studio"]);
        assert_eq!(mac[3], paths[0].as_os_str());
        assert_eq!(mac[4], paths[1].as_os_str());
    }

    #[test]
    fn single_handoff_copy_survives_library_root_rename() {
        let parent = std::env::temp_dir().join(format!(
            "gogglelab-single-handoff-{}-{}",
            std::process::id(),
            HANDOFF_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let library = parent.join("library");
        fs::create_dir_all(&library).unwrap();
        let source_path = library.join("original 模型.stl");
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&0_u32.to_le_bytes());
        fs::write(&source_path, &bytes).unwrap();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let source = OriginalSource {
            path: source_path,
            format: SourceFormat::Stl,
        };
        let slicer = installed_bambu();
        let copied = checked_owned_source_launch(
            &source,
            &digest,
            &library,
            &slicer.id,
            || vec![slicer.clone()],
            |_, path| {
                assert_eq!(fs::read(path).unwrap(), bytes);
                assert_eq!(path.file_name().unwrap(), "original 模型.stl");
                Ok(())
            },
        )
        .unwrap();
        let previous = parent.join("library.previous");
        fs::rename(&library, &previous).unwrap();
        assert_eq!(fs::read(&copied).unwrap(), bytes);
        assert!(copied.starts_with(stable_handoff_root(&library).unwrap()));
        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn single_handoff_filename_uses_parser_format_and_avoids_windows_reserved_names() {
        let source = OriginalSource {
            path: PathBuf::from("/tmp/CON.exe"),
            format: SourceFormat::ThreeMf,
        };
        assert_eq!(super::handoff_filename(&source), "model-CON.3mf");
        let source = OriginalSource {
            path: PathBuf::from("/tmp/part; $(unsafe).stl"),
            format: SourceFormat::Stl,
        };
        assert_eq!(super::handoff_filename(&source), "part_ __unsafe_.stl");
    }

    #[test]
    fn single_handoff_rejects_same_size_source_change_before_launch() {
        let parent = std::env::temp_dir().join(format!(
            "gogglelab-single-handoff-change-{}-{}",
            std::process::id(),
            HANDOFF_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let library = parent.join("library");
        fs::create_dir_all(&library).unwrap();
        let source_path = library.join("model.stl");
        let mut original = vec![0_u8; 84];
        original[80..84].copy_from_slice(&0_u32.to_le_bytes());
        fs::write(&source_path, &original).unwrap();
        let digest = format!("{:x}", Sha256::digest(&original));
        let mut changed = original;
        changed[0] = 1;
        fs::write(&source_path, changed).unwrap();
        let source = OriginalSource {
            path: source_path,
            format: SourceFormat::Stl,
        };
        let mut launched = false;
        assert!(checked_owned_source_launch(
            &source,
            &digest,
            &library,
            "com.bambulab.bambu-studio",
            || vec![installed_bambu()],
            |_, _| {
                launched = true;
                Ok(())
            },
        )
        .is_err());
        assert!(!launched);
        let _ = fs::remove_dir_all(&parent);
    }
}
