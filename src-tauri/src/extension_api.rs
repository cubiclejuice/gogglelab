//! Supported seams for separately composed application extensions.
//!
//! This module keeps the Community host independent of Pro while allowing
//! the official host to reuse model parsing, settings, and slicer support.

use std::path::{Path, PathBuf};

use tauri::AppHandle;

pub use crate::error::AppError;
pub use crate::slicers::SlicerApp;
pub use crate::state::AppState;
pub use crate::types::{ModelSummary, SlicerPaletteLayout};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Stl,
    ThreeMf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginalSource {
    pub path: PathBuf,
    pub format: SourceFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlicerConfig {
    pub path: PathBuf,
    pub palette_layout: SlicerPaletteLayout,
}

pub fn load_model_from_path(
    app: AppHandle,
    state: &AppState,
    path: &Path,
    generation: u64,
    source_identity: &str,
) -> Result<ModelSummary, AppError> {
    state.register_generation(generation);
    let canonical =
        std::fs::canonicalize(path).map_err(|error| crate::error::map_io_error(&error))?;
    let canonical = canonical.to_str().ok_or(AppError::NotReadable)?;
    crate::commands::load_and_dispatch_integrity(app, state, canonical, generation, source_identity)
}

/// Admit an operation on the source already loaded for the current viewer
/// generation. The request and current-model locks stay held until `operation`
/// returns so a newer request cannot supersede the source between validation
/// and an external side effect. Callers cannot provide a path; the format
/// comes from the parsed model. Keep the operation short and do not call back
/// into APIs that need this `AppState`.
pub fn with_original_source<T>(
    state: &AppState,
    generation: u64,
    operation: impl FnOnce(&OriginalSource) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let _request_guard = state.request_commit.lock().unwrap();
    let guard = state.current.lock().unwrap();
    let model = guard.as_ref().ok_or(AppError::NoModelLoaded)?;
    if model.generation != generation || !state.is_latest_locked(generation) {
        return Err(AppError::Superseded);
    }
    let format = match model.parsed.model.format {
        stl_core::StlFormat::Binary | stl_core::StlFormat::Ascii => SourceFormat::Stl,
        stl_core::StlFormat::ThreeMf => SourceFormat::ThreeMf,
    };
    let source = OriginalSource {
        path: PathBuf::from(&model.path),
        format,
    };
    operation(&source)
}

/// Read-only installed-slicer discovery for Pro extensions and printer lookup.
pub fn installed_slicers() -> Vec<SlicerApp> {
    crate::slicers::detect_slicers()
}

/// Metadata for a supported slicer's own config file. The ID is resolved
/// through the core allowlist before a path can be constructed.
pub fn slicer_config(slicer_id: &str) -> Option<SlicerConfig> {
    let definition = crate::slicers::slicer_definition(slicer_id)?;
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(SlicerConfig {
        path: home
            .join("Library/Application Support")
            .join(definition.support_dir)
            .join(definition.config_name),
        palette_layout: definition.palette_layout,
    })
}

#[cfg(test)]
mod tests {
    use super::{with_original_source, AppError, AppState, SourceFormat};
    use crate::state::LoadedModel;
    use std::sync::Arc;

    #[test]
    fn original_source_is_limited_to_the_current_generation_and_parsed_format() {
        let state = AppState::default();
        let mut bytes = vec![0_u8; 134];
        bytes[80..84].copy_from_slice(&1_u32.to_le_bytes());
        let parsed = Arc::new(stl_core::parse_model_with_plates(&bytes).unwrap());
        let path = "/tmp/model 模型 with spaces.stl";
        state.register_generation(7);
        *state.current.lock().unwrap() = Some(LoadedModel {
            generation: 7,
            path: path.into(),
            parsed,
        });

        let source = with_original_source(&state, 7, |source| Ok(source.clone())).unwrap();
        assert_eq!(source.path.to_str(), Some(path));
        assert_eq!(source.format, SourceFormat::Stl);

        state.register_generation(8);
        let mut called = false;
        assert!(matches!(
            with_original_source(&state, 7, |_| {
                called = true;
                Ok(())
            }),
            Err(AppError::Superseded)
        ));
        assert!(!called);
    }

    #[test]
    fn original_source_operation_holds_generation_and_model_admission_locks() {
        let state = AppState::default();
        let mut bytes = vec![0_u8; 134];
        bytes[80..84].copy_from_slice(&1_u32.to_le_bytes());
        let parsed = Arc::new(stl_core::parse_model_with_plates(&bytes).unwrap());
        state.register_generation(7);
        *state.current.lock().unwrap() = Some(LoadedModel {
            generation: 7,
            path: "/tmp/model.stl".into(),
            parsed,
        });

        with_original_source(&state, 7, |_| {
            assert!(state.request_commit.try_lock().is_err());
            assert!(state.current.try_lock().is_err());
            Ok(())
        })
        .unwrap();

        state.register_generation(8);
        assert!(matches!(
            with_original_source(&state, 7, |_| Ok(())),
            Err(AppError::Superseded)
        ));
    }

    #[test]
    fn newer_generation_waits_until_source_operation_admission_finishes() {
        use std::sync::{mpsc, Arc, TryLockError};
        use std::thread;

        let state = AppState::default();
        let mut bytes = vec![0_u8; 134];
        bytes[80..84].copy_from_slice(&1_u32.to_le_bytes());
        let parsed = Arc::new(stl_core::parse_model_with_plates(&bytes).unwrap());
        state.register_generation(7);
        *state.current.lock().unwrap() = Some(LoadedModel {
            generation: 7,
            path: "/tmp/model.stl".into(),
            parsed,
        });
        let state = Arc::new(state);

        let operation_state = Arc::clone(&state);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let operation = thread::spawn(move || {
            with_original_source(&operation_state, 7, |source| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(source.path.clone())
            })
        });
        entered_rx.recv().unwrap();

        let registration_state = Arc::clone(&state);
        let (blocked_tx, blocked_rx) = mpsc::channel();
        let (registered_tx, registered_rx) = mpsc::channel();
        let registration = thread::spawn(move || {
            match registration_state.request_commit.try_lock() {
                Err(TryLockError::WouldBlock) => {}
                Ok(guard) => {
                    drop(guard);
                    panic!("generation registration lock was not held by admission");
                }
                Err(TryLockError::Poisoned(error)) => panic!("lock poisoned: {error}"),
            }
            blocked_tx.send(()).unwrap();
            registration_state.register_generation(8);
            registered_tx.send(()).unwrap();
        });
        blocked_rx.recv().unwrap();

        release_tx.send(()).unwrap();
        assert_eq!(
            operation.join().unwrap().unwrap(),
            std::path::PathBuf::from("/tmp/model.stl")
        );
        registered_rx.recv().unwrap();
        registration.join().unwrap();
        assert!(matches!(
            with_original_source(&state, 7, |_| Ok(())),
            Err(AppError::Superseded)
        ));
    }

    #[test]
    fn original_source_rejects_missing_models_before_running_the_operation() {
        let state = AppState::default();
        let mut called = false;
        assert!(matches!(
            with_original_source(&state, 1, |_| {
                called = true;
                Ok(())
            }),
            Err(AppError::NoModelLoaded)
        ));
        assert!(!called);
    }
}
