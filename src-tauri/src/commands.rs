use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Instant;

use tauri::{Emitter, Manager};

use crate::error::{map_io_error, AppError};
use crate::geometry_buffer;
use crate::state::{AppState, LoadedModel};
use crate::types::{
    EmbeddedFilamentSummary, IntegrityReportJson, MaterialSummary, ModelSummary, PartSummary,
    PlateSummary,
};

/// The commit-iff-latest logic (spec §2, R-02), kept free of any Tauri
/// runtime type so it can be unit-tested directly against a plain `AppState`
/// — no mock app, no async runtime, no IPC. `open_model` (below) is a thin
/// wrapper that adds the background integrity dispatch, which does need an
pub fn load_and_commit(
    state: &AppState,
    path: &str,
    generation: u64,
    source_identity: &str,
) -> Result<(ModelSummary, Arc<stl_core::ParsedModel>), AppError> {
    let start = Instant::now();
    state.register_generation(generation);

    let metadata = std::fs::metadata(path).map_err(|e| map_io_error(&e))?;
    let size = metadata.len();
    if size == 0 {
        return Err(AppError::EmptyFile);
    }
    if size > stl_core::limits::MAX_FILE_BYTES {
        return Err(AppError::FileTooLarge {
            size,
            limit: stl_core::limits::MAX_FILE_BYTES,
        });
    }

    let bytes = std::fs::read(path).map_err(|e| map_io_error(&e))?;
    let source_digest = format!("{:x}", Sha256::digest(&bytes));
    let parsed = stl_core::parse_model_with_plates(&bytes)?;

    // Parsing and measurement are done off-lock. The final commit below takes
    // the request/commit boundary and checks freshness immediately before the
    // active-model write, so a newer request can never be overwritten by a
    // slower one that started earlier.
    let geometry = stl_core::measure(&parsed.model);
    let plates = plate_summaries(&parsed);
    let parse_ms = start.elapsed().as_millis() as u32;
    let file_name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let summary = ModelSummary {
        generation,
        file_name,
        file_size_bytes: size,
        format: parsed.model.format.into(),
        triangle_count: parsed.model.triangle_count,
        geometry: geometry.map(Into::into),
        parse_ms,
        plates,
        materials: parsed
            .materials
            .iter()
            .cloned()
            .map(|material| MaterialSummary {
                id: material.id,
                name: material.name,
                color: material.color,
                source: material.source.into(),
            })
            .collect(),
        embedded_filaments: parsed
            .embedded_filaments
            .iter()
            .cloned()
            .map(|filament| EmbeddedFilamentSummary {
                slot: filament.slot,
                name: filament.name,
                material: filament.material,
                color: filament.color,
            })
            .collect(),
        part_warning: parsed.part_warning.clone(),
        plate_metadata: parsed.plate_metadata,
        plate_warning: parsed.plate_warning.clone(),
        source_identity: source_identity.to_string(),
        source_digest,
    };

    let parsed = Arc::new(parsed);
    commit_loaded(state, generation, path, &summary, parsed.clone())?;

    Ok((summary, parsed))
}

fn plate_summaries(parsed: &stl_core::ParsedModel) -> Vec<PlateSummary> {
    parsed
        .plates
        .iter()
        .map(|plate| PlateSummary {
            id: plate.id,
            name: plate.name.clone(),
            triangle_count: plate.triangle_count,
            geometry: parsed
                .plate_mesh(plate.id)
                .and_then(|mesh| stl_core::measure(&mesh))
                .map(Into::into),
            parts: parsed
                .plate_parts(plate.id)
                .unwrap_or_default()
                .into_iter()
                .map(|part| PartSummary {
                    id: part.id,
                    name: part.name,
                    triangle_start: part.triangle_start,
                    triangle_count: part.triangle_count,
                    material_id: part.material_id,
                    filament_slot: part.filament_slot,
                })
                .collect(),
        })
        .collect()
}

/// Publish a completed load while holding the same boundary used to register
/// new requests. This closes the gap where a newer request could register
/// between a freshness check and the active-model write.
fn commit_loaded(
    state: &AppState,
    generation: u64,
    path: &str,
    _summary: &ModelSummary,
    parsed: Arc<stl_core::ParsedModel>,
) -> Result<(), AppError> {
    let _request_guard = state.request_commit.lock().unwrap();
    if !state.is_latest_locked(generation) {
        return Err(AppError::Superseded);
    }

    // A poisoned lock means a prior panic already crashed a thread holding
    // it — an internal invariant violation, not bad input, so unwinding
    // here is correct rather than something to route around.
    let mut guard = state.current.lock().unwrap();
    *guard = Some(LoadedModel {
        generation,
        path: path.to_string(),
        parsed,
    });
    Ok(())
}

fn canonical_source(path: &str) -> Result<(String, String), AppError> {
    let canonical = std::fs::canonicalize(path).map_err(|e| map_io_error(&e))?;
    let canonical_path = canonical.to_str().ok_or(AppError::NotReadable)?;
    Ok((canonical_path.to_string(), format!("path:{canonical_path}")))
}

/// Phase 1 (spec §5): parse + cheap measures only. `integrity()` is
/// dispatched to a background thread after this returns (R-013) rather than
/// computed inline, so it never sits on the path to first frame.
#[tauri::command]
pub async fn open_model(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    generation: u64,
) -> Result<ModelSummary, AppError> {
    state.register_generation(generation);
    let (canonical_path, source_identity) = canonical_source(&path)?;
    load_and_dispatch_integrity(app, &state, &canonical_path, generation, &source_identity)
}

pub(crate) fn load_and_dispatch_integrity(
    app: tauri::AppHandle,
    state: &AppState,
    path: &str,
    generation: u64,
    source_identity: &str,
) -> Result<ModelSummary, AppError> {
    let (summary, parsed) = load_and_commit(state, path, generation, source_identity)?;

    let app_for_thread = app;
    std::thread::spawn(move || {
        let state = app_for_thread.state::<AppState>();
        for plate in &parsed.plates {
            // Stop abandoned work before allocating another plate's edge map.
            if !state.is_latest_locked(generation) {
                return;
            }
            let payload = integrity_for_plate(&parsed, generation, plate.id);
            let request_guard = state.request_commit.lock().unwrap();
            if !state.is_latest_locked(generation) {
                return;
            }
            if let Some(payload) = payload {
                let _ = app_for_thread.emit("integrity", payload);
            }
            drop(request_guard);
        }
    });

    Ok(summary)
}

fn integrity_for_plate(
    parsed: &stl_core::ParsedModel,
    generation: u64,
    plate_id: u32,
) -> Option<IntegrityReportJson> {
    let mesh = parsed.plate_mesh(plate_id)?;
    Some(match stl_core::integrity(&mesh) {
        stl_core::IntegrityResult::Computed(integrity) => IntegrityReportJson::Computed {
            generation,
            plate_id,
            integrity: integrity.into(),
        },
        stl_core::IntegrityResult::SkippedTooLarge {
            triangle_count,
            limit,
        } => IntegrityReportJson::SkippedTooLarge {
            generation,
            plate_id,
            triangle_count,
            limit,
        },
    })
}

/// Core logic for `model_geometry`, free of any Tauri runtime type for the
/// same reason as `load_and_commit`: it needs nothing from `tauri::State`
/// beyond a `&AppState`, so the command wrapper below is a one-line adapter.
pub fn geometry_for_generation(state: &AppState, generation: u64) -> Result<Vec<u8>, AppError> {
    geometry_for_plate(state, generation, None)
}

pub fn geometry_for_plate(
    state: &AppState,
    generation: u64,
    plate_id: Option<u32>,
) -> Result<Vec<u8>, AppError> {
    let _request_guard = state.request_commit.lock().unwrap();
    let guard = state.current.lock().unwrap();
    let model = guard.as_ref().ok_or(AppError::NoModelLoaded)?;
    if model.generation != generation || !state.is_latest_locked(generation) {
        return Err(AppError::Superseded);
    }
    let parsed = model.parsed.clone();
    drop(guard);
    drop(_request_guard);
    let buffer = match plate_id {
        None => geometry_buffer::build(&parsed.model),
        Some(id) => {
            let plate = parsed
                .plates
                .iter()
                .find(|p| p.id == id)
                .ok_or(AppError::UnknownPlate)?;
            let start = plate.triangle_start as usize * 9;
            let end = start + plate.triangle_count as usize * 9;
            let positions = parsed
                .model
                .positions
                .get(start..end)
                .ok_or(AppError::UnknownPlate)?;
            geometry_buffer::build_positions(plate.triangle_count, positions)
        }
    };
    finish_geometry(state, generation, buffer)
}

fn finish_geometry(
    state: &AppState,
    generation: u64,
    buffer: Vec<u8>,
) -> Result<Vec<u8>, AppError> {
    // Encoding happens off-lock. Reject a newer request that arrived meanwhile.
    let _request_guard = state.request_commit.lock().unwrap();
    if !state.is_latest_locked(generation) {
        return Err(AppError::Superseded);
    }
    Ok(buffer)
}

/// Geometry for the model committed at `generation` (spec §2). Rejects with
/// `Superseded` unless the single-slot cache still holds exactly that
/// generation — the same backend-enforced rule as `open_model`'s commit.
#[tauri::command]
pub fn model_geometry(
    state: tauri::State<'_, AppState>,
    generation: u64,
    plate_id: Option<u32>,
) -> Result<tauri::ipc::Response, AppError> {
    let buffer = match plate_id {
        Some(id) => geometry_for_plate(&state, generation, Some(id)),
        None => geometry_for_generation(&state, generation),
    };
    buffer.map(tauri::ipc::Response::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    fn write_temp_stl(triangles: u32) -> tempfile_path::TempStlFile {
        tempfile_path::TempStlFile::new(triangles)
    }

    /// Minimal self-contained temp-file helper — avoids adding a `tempfile`
    /// dev-dependency for a handful of tests that just need a real path on
    /// disk (std::fs::metadata/read operate on paths, not byte slices).
    mod tempfile_path {
        use std::io::Write;
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_ID: AtomicU64 = AtomicU64::new(0);

        pub struct TempStlFile {
            pub path: std::path::PathBuf,
        }

        impl TempStlFile {
            pub fn new(triangle_count: u32) -> Self {
                Self::new_with_offset(triangle_count, 0.0)
            }

            pub fn new_with_offset(triangle_count: u32, offset: f32) -> Self {
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let mut bytes = Vec::new();
                bytes.extend_from_slice(&[0u8; 80]);
                bytes.extend_from_slice(&triangle_count.to_le_bytes());
                for i in 0..triangle_count {
                    bytes.extend_from_slice(&[0.0f32; 3].map(f32::to_le_bytes).concat());
                    let base = offset + i as f32;
                    for v in 0..3 {
                        for c in 0..3 {
                            bytes.extend_from_slice(&(base + v as f32 + c as f32).to_le_bytes());
                        }
                    }
                    bytes.extend_from_slice(&[0u8, 0u8]);
                }
                let path = std::env::temp_dir().join(format!(
                    "gogglelab-test-{}-{}-{id}.stl",
                    std::process::id(),
                    triangle_count
                ));
                let mut f = std::fs::File::create(&path).unwrap();
                f.write_all(&bytes).unwrap();
                Self { path }
            }
        }

        impl Drop for TempStlFile {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    #[test]
    fn a_slow_load_never_overwrites_a_newer_one() {
        // This is the exact race R-02 exists to prevent: load A starts, is
        // slow enough that load B starts and finishes first, and then A
        // finally finishes. Without the backend commit-iff-latest rule, A's
        // stale result would land in the cache after B's fresh one.
        let state = AppState::default();
        let file_a = write_temp_stl(3);
        let file_b = write_temp_stl(7);

        // Simulate "B was requested after A" by bumping latest_requested to
        // B's generation before A's load_and_commit call ever checks it —
        // load_and_commit's own fetch_max(generation) would otherwise treat
        // whichever call runs first as the latest, so the test drives the
        // ledger directly to model B genuinely arriving first in wall-clock
        // time, exactly as the two-terminal race would.
        state.latest_requested.store(1, Ordering::SeqCst);

        let b_result = load_and_commit(&state, file_b.path.to_str().unwrap(), 2, "path:test-b");
        assert!(b_result.is_ok(), "B (the newer load) must commit");
        assert_eq!(b_result.unwrap().0.triangle_count, 7);

        // A finally finishes, but generation 1 is no longer latest_requested.
        let a_result = load_and_commit(&state, file_a.path.to_str().unwrap(), 1, "path:test-a");
        assert!(
            matches!(a_result, Err(AppError::Superseded)),
            "A (the stale load) must be rejected as Superseded, got {a_result:?}"
        );

        // The cache must still hold B, unclobbered by A's late arrival.
        let guard = state.current.lock().unwrap();
        let cached = guard.as_ref().expect("cache must hold B");
        assert_eq!(cached.generation, 2);
        assert_eq!(cached.parsed.model.triangle_count, 7);
    }

    #[test]
    fn model_geometry_rejects_a_stale_generation_request() {
        let state = AppState::default();
        let file = write_temp_stl(2);
        let (summary, _) =
            load_and_commit(&state, file.path.to_str().unwrap(), 5, "path:test").unwrap();
        assert_eq!(summary.generation, 5);

        let stale = geometry_for_generation(&state, 4);
        assert!(matches!(stale, Err(AppError::Superseded)));

        let current = geometry_for_generation(&state, 5);
        assert!(current.is_ok());
    }
    #[test]
    fn source_identity_and_digest_are_derived_from_loaded_bytes() {
        let file = write_temp_stl(2);
        let identity = format!("path:{}", file.path.to_string_lossy());
        let state = AppState::default();
        let (summary, _) =
            load_and_commit(&state, file.path.to_str().unwrap(), 6, &identity).unwrap();

        assert_eq!(summary.source_identity, identity);
        assert_eq!(
            summary.source_digest,
            "bba25b28ebe1bf65e6242f3742ded47e1352d81c6b5aedda5784ebf0b233e3fd"
        );
    }

    #[test]
    fn canonical_path_identity_uses_utf8_absolute_path() {
        let file = write_temp_stl(1);
        let relative_name = format!("gogglelab-relative-{}.stl", std::process::id());
        let relative = std::path::PathBuf::from(&relative_name);
        std::fs::copy(&file.path, &relative).unwrap();
        let relative_input = relative_name;
        let absolute_input = std::fs::canonicalize(&relative)
            .unwrap()
            .to_string_lossy()
            .to_string();
        let (canonical_relative, relative_identity) = canonical_source(&relative_input).unwrap();
        let (canonical_absolute, absolute_identity) = canonical_source(&absolute_input).unwrap();
        let _ = std::fs::remove_file(&relative);
        assert_eq!(canonical_relative, canonical_absolute);
        assert_eq!(relative_identity, absolute_identity);
        assert_eq!(relative_identity, format!("path:{canonical_absolute}"));
    }
    #[test]
    fn different_loaded_content_has_different_source_digest() {
        let first = tempfile_path::TempStlFile::new_with_offset(1, 0.0);
        let second = tempfile_path::TempStlFile::new_with_offset(1, 10.0);
        let state = AppState::default();
        let (first_summary, _) =
            load_and_commit(&state, first.path.to_str().unwrap(), 7, "path:first").unwrap();
        let (second_summary, _) =
            load_and_commit(&state, second.path.to_str().unwrap(), 8, "library:model").unwrap();
        assert_ne!(first_summary.source_digest, second_summary.source_digest);
        assert_eq!(second_summary.source_identity, "library:model");
    }

    #[test]
    fn plate_summaries_include_plate_local_part_ranges() {
        let mut parsed = two_plate_model();
        parsed.parts = vec![
            stl_core::ParsedPart {
                id: "first-part".into(),
                name: "First part".into(),
                triangle_start: 0,
                triangle_count: 1,
                material_id: Some("mat".into()),
                filament_slot: Some(2),
            },
            stl_core::ParsedPart {
                id: "second-part".into(),
                name: "Second part".into(),
                triangle_start: 1,
                triangle_count: 1,
                material_id: None,
                filament_slot: None,
            },
        ];

        let summaries = plate_summaries(&parsed);
        assert_eq!(summaries[0].parts[0].triangle_start, 0);
        assert_eq!(summaries[1].parts[0].triangle_start, 0);
        assert_eq!(summaries[1].parts[0].triangle_count, 1);
    }

    #[test]
    fn a_newer_request_registered_during_measurement_wins_final_commit() {
        let state = AppState::default();
        let file_a = write_temp_stl(3);
        let file_b = write_temp_stl(7);

        state.register_generation(1);
        let parsed_a = Arc::new(
            stl_core::parse_model_with_plates(&std::fs::read(&file_a.path).unwrap()).unwrap(),
        );
        let parsed_b = Arc::new(
            stl_core::parse_model_with_plates(&std::fs::read(&file_b.path).unwrap()).unwrap(),
        );
        let summary_a = ModelSummary {
            generation: 1,
            file_name: "a.stl".into(),
            file_size_bytes: 0,
            format: parsed_a.model.format.into(),
            triangle_count: parsed_a.model.triangle_count,
            geometry: None,
            parse_ms: 0,
            plates: plate_summaries(&parsed_a),
            materials: Vec::new(),
            embedded_filaments: Vec::new(),
            part_warning: None,
            plate_metadata: false,
            plate_warning: None,
            source_identity: "path:a".into(),
            source_digest: String::new(),
        };
        let summary_b = ModelSummary {
            generation: 2,
            file_name: "b.stl".into(),
            file_size_bytes: 0,
            format: parsed_b.model.format.into(),
            triangle_count: parsed_b.model.triangle_count,
            geometry: None,
            parse_ms: 0,
            plates: plate_summaries(&parsed_b),
            materials: Vec::new(),
            embedded_filaments: Vec::new(),
            part_warning: None,
            plate_metadata: false,
            plate_warning: None,
            source_identity: "path:b".into(),
            source_digest: String::new(),
        };
        // A is paused after parsing/measuring. B registers and commits first;
        // A's final commit must be rejected rather than clobbering B.
        state.register_generation(2);
        commit_loaded(&state, 2, "b.stl", &summary_b, parsed_b).unwrap();
        let stale = commit_loaded(&state, 1, "a.stl", &summary_a, parsed_a);
        assert!(matches!(stale, Err(AppError::Superseded)));
        assert_eq!(
            state.current.lock().unwrap().as_ref().unwrap().generation,
            2
        );
    }

    fn two_plate_model() -> stl_core::ParsedModel {
        stl_core::ParsedModel {
            model: stl_core::ParsedStl {
                format: stl_core::StlFormat::ThreeMf,
                triangle_count: 2,
                positions: vec![
                    0., 0., 0., 10., 0., 0., 0., 10., 0., 300., 0., 0., 310., 0., 0., 300., 10., 0.,
                ],
                stored_normals: vec![0., 0., 1., 0., 0., 1.],
            },
            plates: vec![
                stl_core::Plate {
                    id: 0,
                    name: "First".into(),
                    triangle_start: 0,
                    triangle_count: 1,
                },
                stl_core::Plate {
                    id: 1,
                    name: "Second".into(),
                    triangle_start: 1,
                    triangle_count: 1,
                },
            ],
            parts: Vec::new(),
            materials: Vec::new(),
            embedded_filaments: Vec::new(),
            part_warning: None,
            plate_metadata: true,
            plate_warning: None,
        }
    }

    #[test]
    fn plate_geometry_and_summary_exclude_other_plates() {
        let parsed = two_plate_model();
        let summaries = plate_summaries(&parsed);
        assert_eq!(summaries.len(), 2);
        assert_eq!(
            summaries[1].geometry.as_ref().unwrap().dimensions,
            [10., 10., 0.]
        );
        let state = AppState::default();
        state.register_generation(8);
        *state.current.lock().unwrap() = Some(LoadedModel {
            generation: 8,
            path: "original.3mf".into(),
            parsed: Arc::new(parsed),
        });
        let plate = geometry_for_plate(&state, 8, Some(1)).unwrap();
        assert_eq!(plate.len(), 16 + 36);
        assert_eq!(u32::from_le_bytes(plate[8..12].try_into().unwrap()), 1);
        assert_eq!(f32::from_le_bytes(plate[16..20].try_into().unwrap()), 300.);
        assert_eq!(geometry_for_generation(&state, 8).unwrap().len(), 16 + 72);
        assert!(matches!(
            geometry_for_plate(&state, 8, Some(99)),
            Err(AppError::UnknownPlate)
        ));
        assert!(matches!(
            geometry_for_plate(&state, 7, Some(0)),
            Err(AppError::Superseded)
        ));
        state.register_generation(9);
        assert!(matches!(
            finish_geometry(&state, 8, plate),
            Err(AppError::Superseded)
        ));
        assert!(matches!(
            geometry_for_plate(&state, 8, Some(0)),
            Err(AppError::Superseded)
        ));
        assert_eq!(
            state.current.lock().unwrap().as_ref().unwrap().path,
            "original.3mf"
        );
    }

    #[test]
    fn integrity_reports_identify_the_plate_and_only_check_its_mesh() {
        let parsed = two_plate_model();
        let Some(IntegrityReportJson::Computed {
            generation,
            plate_id,
            integrity,
        }) = integrity_for_plate(&parsed, 7, 1)
        else {
            panic!("expected computed report")
        };
        assert_eq!((generation, plate_id), (7, 1));
        assert_eq!(integrity.boundary_edges, 3);
        assert_eq!(integrity.distinct_vertices, 3);
        assert!(integrity_for_plate(&parsed, 7, 99).is_none());
    }
}
