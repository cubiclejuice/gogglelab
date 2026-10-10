use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use tauri::State;

use crate::extension_api::{self, AppError, AppState, OriginalSource, SlicerApp, SourceFormat};

/// Opens the original bytes for the current viewer generation in a supported,
/// freshly detected slicer. `with_original_source` keeps the generation and
/// current-model locks held through launch admission, so a newer model cannot
/// replace the source between validation and the external side effect.
#[tauri::command]
pub fn open_in_slicer(
    viewer: State<'_, AppState>,
    generation: u64,
    slicer_id: String,
) -> Result<(), AppError> {
    let installed = extension_api::installed_slicers();
    extension_api::with_original_source(&viewer, generation, |source| {
        checked_source_launch(source, &slicer_id, &installed, launch)
    })
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

#[cfg(any(target_os = "macos", test))]
fn launch_arguments(slicer_id: &str, source_path: &Path) -> [OsString; 3] {
    [
        OsString::from("-b"),
        OsString::from(slicer_id),
        source_path.as_os_str().to_owned(),
    ]
}

#[cfg(target_os = "macos")]
fn launch(slicer: &SlicerApp, source_path: &Path) -> Result<(), AppError> {
    let status = Command::new("open")
        .args(launch_arguments(&slicer.bundle_id, source_path))
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
    launch_executable(slicer, source_path)
}

#[cfg(any(not(target_os = "macos"), test))]
fn launch_executable(slicer: &SlicerApp, source_path: &Path) -> Result<(), AppError> {
    // app_path comes only from the current allowlisted detector. Command and
    // args bypass a shell, preserving the model path as one literal argument.
    Command::new(&slicer.app_path)
        .args(executable_arguments(source_path))
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
fn executable_arguments(source_path: &Path) -> [OsString; 1] {
    [source_path.as_os_str().to_owned()]
}

#[cfg(test)]
mod tests {
    use super::{checked_source_launch, launch_arguments, supported_slicer};
    use crate::extension_api::{AppError, OriginalSource, SlicerApp, SourceFormat};
    use std::path::Path;

    fn installed_bambu() -> SlicerApp {
        SlicerApp {
            id: "com.bambulab.bambu-studio".into(),
            bundle_id: "com.bambulab.bambu-studio".into(),
            display_name: "Bambu Studio".into(),
            app_path: "/Applications/Bambu Studio.app".into(),
        }
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
        assert!(super::launch_executable(&slicer, Path::new("source.stl")).is_err());
    }

    #[test]
    fn executable_launch_preserves_source_as_one_literal_argument() {
        let path = Path::new("/tmp/模型 with spaces; $(echo unsafe).3mf");
        let args = super::executable_arguments(path);
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], path.as_os_str());
    }

    #[test]
    fn macos_launch_preserves_bundle_id_and_source_as_literal_arguments() {
        let path = Path::new("/tmp/模型 with spaces; $(echo unsafe).3mf");
        let args = launch_arguments("com.bambulab.bambu-studio", path);

        assert_eq!(args[0], "-b");
        assert_eq!(args[1], "com.bambulab.bambu-studio");
        assert_eq!(args[2], path.as_os_str());
    }
}
