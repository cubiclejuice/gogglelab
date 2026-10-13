use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};
use tauri::State;

use crate::extension_api::{self, AppError, AppState, OriginalSource, SlicerApp, SourceFormat};
use crate::library_commands::LibraryState;

static HANDOFF_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
    create_real_directory(&root)?;
    for _ in 0..1024 {
        let directory = root.join(format!(
            "single-{}-{}-{}",
            std::process::id(),
            HANDOFF_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            &expected_digest[..12]
        ));
        match fs::create_dir(&directory) {
            Ok(()) => {
                let path = directory.join(file_name);
                let result = (|| {
                    let mut output = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .map_err(|error| crate::error::map_io_error(&error))?;
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
    let parent = library_root.parent().ok_or_else(|| AppError::Library {
        message: "Library has no stable parent for handoff assets".into(),
    })?;
    let library_name = library_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("library");
    Ok(parent.join(format!("{library_name}-handoff-assets")))
}

fn create_real_directory(path: &Path) -> Result<(), AppError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(AppError::Library {
                message: "Handoff namespace is not a real directory".into(),
            })
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| crate::error::map_io_error(&error))
        }
        Err(error) => Err(crate::error::map_io_error(&error)),
    }
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
        checked_bytes_launch, checked_owned_source_launch, checked_paths_launch,
        checked_source_launch, launch_arguments, stable_handoff_root, supported_slicer,
        VerifiedHandoffRequest, HANDOFF_SEQUENCE,
    };
    use crate::extension_api::{AppError, OriginalSource, SlicerApp, SourceFormat};
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;

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
