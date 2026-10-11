use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use library_core::{Model, ResolvedModelSource};
#[cfg(test)]
use library_protocol::BundlePreviewMember;
pub use library_protocol::BundlePreviewSummary;
use library_protocol::{BundleEngine, Capability, RequestContext};
use sha2::{Digest, Sha256};
use tauri::State;

use crate::extension_api::AppError;
use crate::library_commands::{library_error, LibraryState};

#[cfg(test)]
const MAX_PREVIEW_MEMBERS: usize = 32;
#[cfg(test)]
const MAX_PREVIEW_TRIANGLES: u32 = 2_000_000;
const MAX_HANDOFF_MEMBERS: usize = 10_000;
const MAX_HANDOFF_BYTES: u64 = 4 * 1024 * 1024 * 1024;
#[cfg(test)]
static TOKEN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Default)]
pub struct BundlePreviewState {
    prepared: Arc<Mutex<Option<PreparedBundle>>>,
    latest_requested: Arc<AtomicU64>,
    request_commit: Arc<Mutex<()>>,
    token_epoch: Arc<AtomicU64>,
}

#[derive(Clone)]
struct BundleSnapshotMember {
    model_id: String,
    content_hash: String,
    bytes: u64,
}

struct PreparedBundle {
    token: String,
    bundle_id: String,
    snapshot: Vec<BundleSnapshotMember>,
    #[cfg(test)]
    geometry: Vec<u8>,
    library_generation: u64,
    token_epoch: u64,
}

fn desktop_bundle_context() -> RequestContext {
    RequestContext {
        principal_id: "desktop".into(),
        capabilities: BTreeSet::from([Capability::MediaRead]),
        request_id: "desktop-bundle".into(),
    }
}

#[tauri::command]
pub async fn library_prepare_bundle_preview(
    library: State<'_, LibraryState>,
    preview: State<'_, BundlePreviewState>,
    bundle_id: String,
    generation: u64,
) -> Result<BundlePreviewSummary, AppError> {
    if library.maintenance_active() {
        return Err(AppError::Library {
            message: "Library maintenance is in progress".into(),
        });
    }
    let library_generation = library.management_generation();
    {
        let _guard = preview.request_commit.lock().unwrap();
        preview
            .latest_requested
            .fetch_max(generation, Ordering::SeqCst);
    }
    let library = library.inner().clone();
    let preview = preview.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _io = library.begin_io()?;
        let context = desktop_bundle_context();
        let summary = library
            .runtime()
            .prepare_bundle_preview(&context, &bundle_id)
            .map_err(AppError::from)?;
        let snapshot = snapshot(&bundle_models(&library, &bundle_id)?)?;
        let _request_guard = preview.request_commit.lock().unwrap();
        if preview.latest_requested.load(Ordering::SeqCst) != generation {
            let _ = library
                .runtime()
                .close_bundle_preview(&context, &summary.token);
            return Err(AppError::Superseded);
        }
        if library.management_generation() != library_generation || library.maintenance_active() {
            return Err(AppError::Library {
                message: "Library changed while preparing bundle preview".into(),
            });
        }
        let token_epoch = preview.token_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        *preview.prepared.lock().unwrap() = Some(PreparedBundle {
            token: summary.token.clone(),
            bundle_id,
            snapshot,
            #[cfg(test)]
            geometry: Vec::new(),
            library_generation,
            token_epoch,
        });
        Ok(summary)
    })
    .await
    .map_err(|error| AppError::Library {
        message: format!("Bundle preview worker failed: {error}"),
    })?
}

#[tauri::command]
pub fn library_bundle_preview_geometry(
    library: State<'_, LibraryState>,
    preview: State<'_, BundlePreviewState>,
    token: String,
) -> Result<tauri::ipc::Response, AppError> {
    preview
        .prepared
        .lock()
        .unwrap()
        .as_ref()
        .filter(|prepared| {
            prepared.token == token
                && prepared.library_generation == library.management_generation()
                && !library.maintenance_active()
        })
        .ok_or_else(|| AppError::Library {
            message: "Bundle preview token is stale".into(),
        })?;
    let mut geometry = Vec::new();
    library
        .runtime()
        .stream_bundle_geometry(&desktop_bundle_context(), &token, &mut geometry)
        .map_err(AppError::from)?;
    Ok(tauri::ipc::Response::new(geometry))
}

#[tauri::command]
pub fn library_close_bundle_preview(
    library: State<'_, LibraryState>,
    preview: State<'_, BundlePreviewState>,
    token: String,
) {
    let mut guard = preview.prepared.lock().unwrap();
    if guard
        .as_ref()
        .is_some_and(|prepared| prepared.token == token)
    {
        *guard = None;
        preview.token_epoch.fetch_add(1, Ordering::SeqCst);
    }
    let _ = library
        .runtime()
        .close_bundle_preview(&desktop_bundle_context(), &token);
}

#[tauri::command]
pub async fn library_send_bundle_to_slicer(
    library: State<'_, LibraryState>,
    preview: State<'_, BundlePreviewState>,
    bundle_id: String,
    slicer_id: String,
    token: String,
) -> Result<(), AppError> {
    let library = library.inner().clone();
    let (snapshot, token_epoch, library_generation) = {
        let guard = preview.prepared.lock().unwrap();
        let prepared = guard
            .as_ref()
            .filter(|prepared| {
                prepared.token == token
                    && prepared.bundle_id == bundle_id
                    && prepared.library_generation == library.management_generation()
                    && !library.maintenance_active()
            })
            .ok_or_else(|| AppError::Library {
                message: "Bundle preview token is stale".into(),
            })?;
        (
            prepared.snapshot.clone(),
            prepared.token_epoch,
            prepared.library_generation,
        )
    };
    let preview = preview.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _io = library.begin_io()?;
        library
            .runtime()
            .describe_bundle_geometry(&desktop_bundle_context(), &token)
            .map_err(AppError::from)?;
        send_bundle_to_slicer(
            &library,
            &preview,
            &bundle_id,
            &slicer_id,
            &token,
            &snapshot,
            token_epoch,
            library_generation,
        )
    })
    .await
    .map_err(|error| AppError::Library {
        message: format!("Bundle handoff worker failed: {error}"),
    })?
}

#[cfg(test)]
fn prepare_bundle_preview(
    library: &LibraryState,
    preview: &BundlePreviewState,
    bundle_id: &str,
    generation: u64,
    library_generation: u64,
) -> Result<BundlePreviewSummary, AppError> {
    let models = bundle_models(library, bundle_id)?;
    if models.is_empty() {
        return Err(AppError::Library {
            message: "Bundle contains no models".into(),
        });
    }
    let snapshot = snapshot(&models)?;
    let previewed = models.len().min(MAX_PREVIEW_MEMBERS);
    let combined = is_combined_bundle(&models);
    let mut meshes = Vec::with_capacity(previewed);
    let mut triangle_count = 0_u32;
    for model in models.iter().take(previewed) {
        if library.management_generation() != library_generation || library.maintenance_active() {
            return Err(AppError::Library {
                message: "Library changed while preparing bundle preview".into(),
            });
        }
        let parsed = load_model(library, model)?;
        triangle_count = triangle_count
            .checked_add(parsed.triangle_count)
            .ok_or_else(|| AppError::Library {
                message: "Bundle preview triangle count overflowed".into(),
            })?;
        if triangle_count > MAX_PREVIEW_TRIANGLES {
            return Err(AppError::Library {
                message: format!(
                    "Bundle preview exceeds the {MAX_PREVIEW_TRIANGLES} triangle limit"
                ),
            });
        }
        meshes.push((model, parsed));
    }
    let offsets = layout_offsets(&meshes, combined)?;
    let mut positions = Vec::new();
    let mut members = Vec::with_capacity(meshes.len());
    for ((model, mesh), offset) in meshes.into_iter().zip(offsets) {
        if mesh.stored_normals.iter().any(|value| !value.is_finite()) {
            return Err(AppError::Library {
                message: "Bundle preview contains a non-finite stored normal".into(),
            });
        }
        for point in mesh.positions.chunks_exact(3) {
            let translated = [
                point[0] + offset[0],
                point[1] + offset[1],
                point[2] + offset[2],
            ];
            if translated.iter().any(|value| !value.is_finite()) {
                return Err(AppError::Library {
                    message: "Bundle preview layout exceeds finite coordinate limits".into(),
                });
            }
            positions.extend(translated);
        }
        members.push(BundlePreviewMember {
            model_id: model.id.clone(),
            display_name: model.display_name.clone(),
            triangle_count: mesh.triangle_count,
            offset,
        });
    }
    let geometry = crate::geometry_buffer::build_positions(triangle_count, &positions);
    let geometry_bytes = geometry.len() as u64;
    let geometry_sha256 = hex_digest(&geometry);
    let token = format!(
        "bundle-preview-{}-{}-{generation}",
        std::process::id(),
        TOKEN_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let omitted_count = models.len() - previewed;
    let notice = preview_notice(models.len(), previewed);
    let _request_guard = preview.request_commit.lock().unwrap();
    if preview.latest_requested.load(Ordering::SeqCst) != generation {
        return Err(AppError::Superseded);
    }
    if library.management_generation() != library_generation || library.maintenance_active() {
        return Err(AppError::Library {
            message: "Library changed while preparing bundle preview".into(),
        });
    }
    let token_epoch = preview.token_epoch.fetch_add(1, Ordering::SeqCst) + 1;
    *preview.prepared.lock().unwrap() = Some(PreparedBundle {
        token: token.clone(),
        bundle_id: bundle_id.to_owned(),
        snapshot,
        geometry,
        library_generation,
        token_epoch,
    });
    Ok(BundlePreviewSummary {
        token,
        bundle_id: bundle_id.to_owned(),
        member_count: models.len(),
        previewed_count: previewed,
        omitted_count,
        triangle_count,
        layout: if combined { "combined" } else { "grid" }.into(),
        members,
        notice,
        geometry_bytes,
        geometry_sha256,
        library_epoch: library.runtime().library_epoch(),
    })
}

#[cfg(test)]
fn preview_notice(member_count: usize, previewed_count: usize) -> Option<String> {
    (member_count > previewed_count).then(|| {
        format!(
            "Preview shows {previewed_count} of {member_count} models. Send to slicer still includes all {member_count} models."
        )
    })
}

fn bundle_models(library: &LibraryState, bundle_id: &str) -> Result<Vec<Model>, AppError> {
    library.with_db(|db| db.list_bundle_models(bundle_id).map_err(library_error))
}

fn snapshot(models: &[Model]) -> Result<Vec<BundleSnapshotMember>, AppError> {
    models
        .iter()
        .map(|model| {
            Ok(BundleSnapshotMember {
                model_id: model.id.clone(),
                content_hash: model
                    .content_hash
                    .clone()
                    .ok_or_else(|| AppError::Library {
                        message: format!("Bundle model {} has no source content", model.id),
                    })?,
                bytes: model.bytes,
            })
        })
        .collect()
}

fn validate_snapshot(
    library: &LibraryState,
    bundle_id: &str,
    expected: &[BundleSnapshotMember],
) -> Result<Vec<Model>, AppError> {
    let current = bundle_models(library, bundle_id)?;
    let actual = snapshot(&current)?;
    if !snapshot_matches(&actual, expected) {
        return Err(AppError::Library {
            message: "Bundle membership or source hashes changed after preview".into(),
        });
    }
    Ok(current)
}

fn snapshot_matches(actual: &[BundleSnapshotMember], expected: &[BundleSnapshotMember]) -> bool {
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(actual, expected)| {
            actual.model_id == expected.model_id
                && actual.content_hash == expected.content_hash
                && actual.bytes == expected.bytes
        })
}

#[cfg(test)]
fn is_combined_bundle(models: &[Model]) -> bool {
    let Some(first) = models.first() else {
        return false;
    };
    first.source_archive_member.is_some()
        && models.iter().all(|model| {
            model.source_archive_member.is_some()
                && model.source_root_id == first.source_root_id
                && model.source_path == first.source_path
        })
}

#[cfg(test)]
fn layout_offsets(
    meshes: &[(&Model, stl_core::ParsedStl)],
    combined: bool,
) -> Result<Vec<[f32; 3]>, AppError> {
    if combined || meshes.len() <= 1 {
        return Ok(vec![[0.0, 0.0, 0.0]; meshes.len()]);
    }
    let bounds = meshes
        .iter()
        .map(|(_, mesh)| xy_bounds(mesh))
        .collect::<Vec<_>>();
    let cell_x = bounds
        .iter()
        .map(|bounds| bounds[2])
        .fold(0.0_f32, f32::max)
        + 10.0;
    let cell_y = bounds
        .iter()
        .map(|bounds| bounds[3])
        .fold(0.0_f32, f32::max)
        + 10.0;
    if !cell_x.is_finite()
        || !cell_y.is_finite()
        || bounds.iter().flatten().any(|value| !value.is_finite())
    {
        return Err(AppError::Library {
            message: "Bundle preview geometry is too large to lay out safely".into(),
        });
    }
    let columns = (meshes.len() as f32).sqrt().ceil() as usize;
    let offsets = (0..meshes.len())
        .map(|index| {
            let column = index % columns;
            let row = index / columns;
            [
                column as f32 * cell_x - bounds[index][0],
                row as f32 * cell_y - bounds[index][1],
                0.0,
            ]
        })
        .collect::<Vec<_>>();
    if offsets.iter().flatten().any(|value| !value.is_finite()) {
        return Err(AppError::Library {
            message: "Bundle preview grid offsets are not finite".into(),
        });
    }
    Ok(offsets)
}

#[cfg(test)]
fn xy_bounds(mesh: &stl_core::ParsedStl) -> [f32; 4] {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for point in mesh.positions.chunks_exact(3) {
        min[0] = min[0].min(point[0]);
        min[1] = min[1].min(point[1]);
        max[0] = max[0].max(point[0]);
        max[1] = max[1].max(point[1]);
    }
    if mesh.positions.is_empty() {
        [0.0, 0.0, 0.0, 0.0]
    } else {
        [
            min[0],
            min[1],
            (max[0] - min[0]).max(0.0),
            (max[1] - min[1]).max(0.0),
        ]
    }
}

#[cfg(test)]
fn load_model(library: &LibraryState, model: &Model) -> Result<stl_core::ParsedStl, AppError> {
    let (resolved, root_limit, library_root) = resolve_source(library, model)?;
    crate::library_commands::validate_indexed_source_limit(&resolved, root_limit)?;
    let bytes = source_bytes(&library_root, &resolved)?;
    stl_core::parse_model(&bytes).map_err(AppError::from)
}

fn resolve_source(
    library: &LibraryState,
    model: &Model,
) -> Result<(ResolvedModelSource, Option<u64>, PathBuf), AppError> {
    library.with_db(|db| {
        let root_limit = if model.source_kind == "indexed" {
            let root_id = model
                .source_root_id
                .as_deref()
                .ok_or_else(|| AppError::Library {
                    message: "Indexed bundle member has no source root".into(),
                })?;
            Some(
                db.list_index_roots()
                    .map_err(library_error)?
                    .into_iter()
                    .find(|root| root.id == root_id)
                    .ok_or_else(|| AppError::Library {
                        message: "Indexed bundle source root was removed".into(),
                    })?
                    .max_file_bytes,
            )
        } else {
            None
        };
        Ok((
            db.resolve_model_source_ref(&model.id)
                .map_err(library_error)?,
            root_limit,
            db.root().to_path_buf(),
        ))
    })
}

fn source_bytes(library_root: &Path, source: &ResolvedModelSource) -> Result<Vec<u8>, AppError> {
    if let Some(member) = source.archive_member.as_deref() {
        let temporary = crate::library_archives::extract_member_to_temporary(
            library_root,
            &source.outer_path,
            member,
            source
                .outer_fingerprint
                .as_deref()
                .ok_or_else(|| AppError::Library {
                    message: "Archive bundle member has no source fingerprint".into(),
                })?,
            &source.content_hash,
            source.bytes,
        )
        .map_err(|message| AppError::Library { message })?;
        read_verified_file(temporary.path(), source.bytes, &source.content_hash)
    } else {
        read_verified_file(&source.outer_path, source.bytes, &source.content_hash)
    }
}

fn read_verified_file(
    path: &Path,
    expected_bytes: u64,
    expected_hash: &str,
) -> Result<Vec<u8>, AppError> {
    if expected_bytes > stl_core::limits::MAX_FILE_BYTES {
        return Err(AppError::Library {
            message: "Bundle member exceeds the native file limit".into(),
        });
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|error| crate::error::map_io_error(&error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != expected_bytes
    {
        return Err(AppError::Library {
            message: "Bundle member source changed or is not a regular file".into(),
        });
    }
    let mut file = fs::File::open(path).map_err(|error| crate::error::map_io_error(&error))?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(expected_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| crate::error::map_io_error(&error))?;
    if bytes.len() as u64 != expected_bytes || hex_digest(&bytes) != expected_hash {
        return Err(AppError::Library {
            message: "Bundle member content hash changed".into(),
        });
    }
    Ok(bytes)
}

#[allow(clippy::too_many_arguments)]
fn send_bundle_to_slicer(
    library: &LibraryState,
    preview: &BundlePreviewState,
    bundle_id: &str,
    slicer_id: &str,
    token: &str,
    expected: &[BundleSnapshotMember],
    token_epoch: u64,
    library_generation: u64,
) -> Result<(), AppError> {
    send_bundle_with(
        library,
        bundle_id,
        slicer_id,
        token,
        expected,
        || token_is_current(library, preview, token_epoch, library_generation),
        crate::extension_api::installed_slicers,
        crate::handoff::launch_paths,
    )
    .map(|_| ())
}

#[allow(clippy::too_many_arguments)]
fn send_bundle_with(
    library: &LibraryState,
    bundle_id: &str,
    slicer_id: &str,
    token: &str,
    expected: &[BundleSnapshotMember],
    token_valid: impl Fn() -> bool,
    detect: impl FnOnce() -> Vec<crate::extension_api::SlicerApp>,
    launch: impl FnOnce(&crate::extension_api::SlicerApp, &[PathBuf]) -> Result<(), AppError>,
) -> Result<PathBuf, AppError> {
    if !token_valid() {
        return Err(AppError::Library {
            message: "Bundle preview token is stale".into(),
        });
    }
    let models = validate_snapshot(library, bundle_id, expected)?;
    validate_handoff_bounds(&models)?;

    let library_root = library.with_db(|db| Ok(db.root().to_path_buf()))?;
    let handoff_root = crate::handoff::stable_handoff_root(&library_root)?;
    create_real_directory(&handoff_root)?;
    let destination = handoff_root.join(token);
    fs::create_dir(&destination).map_err(|error| crate::error::map_io_error(&error))?;
    let result = (|| {
        let mut paths = Vec::with_capacity(models.len());
        for (index, model) in models.iter().enumerate() {
            if !token_valid() {
                return Err(AppError::Library {
                    message: "Bundle preview token is stale".into(),
                });
            }
            let (resolved, root_limit, root) = resolve_source(library, model)?;
            crate::library_commands::validate_indexed_source_limit(&resolved, root_limit)?;
            let bytes = source_bytes(&root, &resolved)?;
            let filename = format!("{index:04}-{}", sanitized_name(model));
            let path = destination.join(filename);
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| crate::error::map_io_error(&error))?;
            file.write_all(&bytes)
                .map_err(|error| crate::error::map_io_error(&error))?;
            paths.push(path);
        }
        if !token_valid() {
            return Err(AppError::Library {
                message: "Bundle preview token is stale".into(),
            });
        }
        validate_snapshot(library, bundle_id, expected)?;
        if !token_valid() {
            return Err(AppError::Library {
                message: "Bundle preview token is stale".into(),
            });
        }
        let installed = detect();
        crate::handoff::checked_paths_launch(&paths, slicer_id, &installed, launch)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&destination);
    }
    result.map(|_| destination)
}

fn validate_handoff_bounds(models: &[Model]) -> Result<(), AppError> {
    if models.is_empty() || models.len() > MAX_HANDOFF_MEMBERS {
        return Err(AppError::Library {
            message: format!("Bundle handoff supports 1 to {MAX_HANDOFF_MEMBERS} models"),
        });
    }
    let total = models
        .iter()
        .try_fold(0_u64, |total, model| total.checked_add(model.bytes));
    if total.is_none_or(|total| total > MAX_HANDOFF_BYTES) {
        return Err(AppError::Library {
            message: "Bundle handoff exceeds the 4 GiB admitted size limit".into(),
        });
    }
    Ok(())
}

fn token_is_current(
    library: &LibraryState,
    preview: &BundlePreviewState,
    token_epoch: u64,
    library_generation: u64,
) -> bool {
    preview.token_epoch.load(Ordering::SeqCst) == token_epoch
        && library.management_generation() == library_generation
        && !library.maintenance_active()
}

fn sanitized_name(model: &Model) -> String {
    let raw = model
        .source_archive_member
        .as_deref()
        .and_then(|path| Path::new(path).file_name()?.to_str())
        .or(model.original_name.as_deref())
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

fn create_real_directory(path: &Path) -> Result<(), AppError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(AppError::Library {
                message: "Bundle handoff namespace is not a real directory".into(),
            })
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| crate::error::map_io_error(&error))
        }
        Err(error) => Err(crate::error::map_io_error(&error)),
    }
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
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "gogglelab-bundle-{label}-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn triangle_stl(x: f32) -> Vec<u8> {
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&1_u32.to_le_bytes());
        for value in [
            0.0_f32,
            0.0,
            1.0,
            x,
            0.0,
            0.0,
            x + 10.0,
            0.0,
            0.0,
            x,
            10.0,
            0.0,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
        bytes
    }

    fn cube_3mf() -> Vec<u8> {
        let model = r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
<resources><object id="1" type="model"><mesh><vertices>
<vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="10" y="10" z="0"/><vertex x="0" y="10" z="0"/>
<vertex x="0" y="0" z="10"/><vertex x="10" y="0" z="10"/><vertex x="10" y="10" z="10"/><vertex x="0" y="10" z="10"/>
</vertices><triangles>
<triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="3" v3="2"/><triangle v1="4" v2="5" v3="6"/><triangle v1="4" v2="6" v3="7"/>
<triangle v1="0" v2="1" v3="5"/><triangle v1="0" v2="5" v3="4"/><triangle v1="1" v2="2" v3="6"/><triangle v1="1" v2="6" v3="5"/>
<triangle v1="2" v2="3" v3="7"/><triangle v1="2" v2="7" v3="6"/><triangle v1="3" v2="0" v3="4"/><triangle v1="3" v2="4" v3="7"/>
</triangles></mesh></object></resources><build><item objectid="1"/></build></model>"#;
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file("3D/3dmodel.model", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(model.as_bytes()).unwrap();
        archive.finish().unwrap().into_inner()
    }

    #[test]
    fn grid_layout_offsets_each_member_without_changing_z() {
        let model = Model {
            id: "m".into(),
            display_name: "M".into(),
            description: String::new(),
            folder_id: "root".into(),
            original_name: Some("m.stl".into()),
            format: Some("stl-binary".into()),
            bytes: 0,
            content_hash: Some("a".repeat(64)),
            storage_path: None,
            metadata_version: 1,
            created_at: "1".into(),
            updated_at: "1".into(),
            tags: Vec::new(),
            filament_ids: Vec::new(),
            source_kind: "managed".into(),
            source_path: None,
            source_root_id: None,
            source_missing: false,
            source_modified_at: None,
            thumbnail_path: None,
            metadata: Default::default(),
            bundle_id: Some("b".into()),
            source_archive_member: None,
            catalog_kind: "file".into(),
        };
        let mesh = stl_core::ParsedStl {
            format: stl_core::StlFormat::Binary,
            triangle_count: 1,
            positions: vec![0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0, 0.0],
            stored_normals: vec![0.0, 0.0, 1.0],
        };
        let offsets = layout_offsets(&[(&model, mesh.clone()), (&model, mesh)], false).unwrap();
        assert_eq!(offsets[0], [0.0, 0.0, 0.0]);
        assert!(offsets[1][0] > 10.0);
        assert_eq!(offsets[1][2], 0.0);
    }

    #[test]
    fn grid_layout_rejects_finite_inputs_whose_extent_overflows() {
        let model = test_model();
        let mesh = stl_core::ParsedStl {
            format: stl_core::StlFormat::Binary,
            triangle_count: 1,
            positions: vec![-f32::MAX, 0.0, 0.0, f32::MAX, 0.0, 0.0, 0.0, 1.0, 0.0],
            stored_normals: vec![0.0, 0.0, 1.0],
        };
        assert!(layout_offsets(&[(&model, mesh.clone()), (&model, mesh)], false).is_err());
    }

    #[test]
    fn preview_token_snapshot_rejects_membership_and_hash_changes() {
        let expected = vec![BundleSnapshotMember {
            model_id: "m1".into(),
            content_hash: "a".repeat(64),
            bytes: 84,
        }];
        let changed = vec![BundleSnapshotMember {
            model_id: "m1".into(),
            content_hash: "b".repeat(64),
            bytes: 84,
        }];
        assert!(!snapshot_matches(&changed, &expected));
        assert!(!snapshot_matches(&[], &expected));
        assert!(snapshot_matches(&expected, &expected));
    }

    #[test]
    fn larger_bundle_notice_never_implies_handoff_truncation() {
        let notice = preview_notice(33, 32).unwrap();
        assert!(notice.contains("32 of 33"));
        assert!(notice.contains("all 33"));
        assert!(preview_notice(32, 32).is_none());
    }

    #[test]
    fn handoff_bounds_fail_the_whole_bundle_without_truncation() {
        assert!(validate_handoff_bounds(&[]).is_err());
        let mut models = vec![test_model(); 41];
        for (index, model) in models.iter_mut().enumerate() {
            model.id = format!("m-{index}");
            model.bytes = 100 * 1024 * 1024;
        }
        assert!(validate_handoff_bounds(&models).is_err());
        assert_eq!(models.len(), 41);
    }

    #[test]
    fn sanitizes_unicode_and_shell_metacharacters_without_losing_extension() {
        let mut model = test_model();
        model.original_name = Some("模型; $(unsafe).3mf".into());
        let name = sanitized_name(&model);
        assert!(name.ends_with(".3mf"));
        assert!(!name.contains(';'));
        assert!(!name.contains('$'));
        assert!(!name.contains(' '));
    }

    #[test]
    fn prepares_real_multi_model_grid_without_replacing_main_viewer() {
        let catalog = temp_root("preview-catalog");
        let sources = temp_root("preview-sources");
        let first = sources.join("one.stl");
        let second = sources.join("two.3mf");
        fs::write(&first, triangle_stl(0.0)).unwrap();
        fs::write(&second, cube_3mf()).unwrap();
        let library = LibraryState::new(catalog.clone());
        let bundle_id = library
            .with_db(|db| {
                let ids = [first.clone(), second.clone()]
                    .iter()
                    .map(|path| {
                        db.import_file(path, None, None)
                            .map(|result| result.model.id)
                            .map_err(library_error)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                db.create_bundle(&ids).map_err(library_error)
            })
            .unwrap();
        let preview = BundlePreviewState::default();
        preview.latest_requested.store(7, Ordering::SeqCst);
        let library_generation = library.management_generation();
        let summary =
            prepare_bundle_preview(&library, &preview, &bundle_id, 7, library_generation).unwrap();
        assert_eq!(summary.member_count, 2);
        assert_eq!(summary.previewed_count, 2);
        assert_eq!(summary.omitted_count, 0);
        assert_eq!(summary.triangle_count, 13);
        assert_eq!(summary.layout, "grid");
        assert_ne!(summary.members[0].offset, summary.members[1].offset);
        let guard = preview.prepared.lock().unwrap();
        let prepared = guard.as_ref().unwrap();
        assert_eq!(&prepared.geometry[..4], b"STL2");
        assert_eq!(
            u32::from_le_bytes(prepared.geometry[8..12].try_into().unwrap()),
            13
        );
        if let Some(output) = std::env::var_os("GOGGLELAB_PHASE4_OUTPUT_DIR") {
            let output = PathBuf::from(output);
            fs::create_dir_all(&output).unwrap();
            fs::write(output.join("phase4-bundle.geometry"), &prepared.geometry).unwrap();
            fs::write(
                output.join("phase4-bundle.json"),
                serde_json::to_vec_pretty(&summary).unwrap(),
            )
            .unwrap();
        }
        drop(guard);
        preview.latest_requested.store(8, Ordering::SeqCst);
        assert!(matches!(
            prepare_bundle_preview(&library, &preview, &bundle_id, 7, library_generation),
            Err(AppError::Superseded)
        ));
        assert_eq!(
            preview.prepared.lock().unwrap().as_ref().unwrap().token,
            summary.token
        );
        let (prepared_epoch, prepared_library_generation) = {
            let guard = preview.prepared.lock().unwrap();
            let prepared = guard.as_ref().unwrap();
            (prepared.token_epoch, prepared.library_generation)
        };
        let maintenance = library.begin_maintenance().unwrap();
        maintenance.invalidate();
        drop(maintenance);
        assert!(!token_is_current(
            &library,
            &preview,
            prepared_epoch,
            prepared_library_generation
        ));
        let _ = fs::remove_dir_all(catalog);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn bundle_handoff_launches_all_verified_owned_copies_and_keeps_them_alive() {
        let catalog = temp_root("handoff-catalog");
        let sources = temp_root("handoff-sources");
        let first = sources.join("模型; first.stl");
        let second = sources.join("second $(unsafe).stl");
        let first_bytes = triangle_stl(0.0);
        let second_bytes = triangle_stl(20.0);
        fs::write(&first, &first_bytes).unwrap();
        fs::write(&second, &second_bytes).unwrap();
        let library = LibraryState::new(catalog.clone());
        let bundle_id = library
            .with_db(|db| {
                let ids = [first.clone(), second.clone()]
                    .iter()
                    .map(|path| {
                        db.import_file(path, None, None)
                            .map(|result| result.model.id)
                            .map_err(library_error)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                db.create_bundle(&ids).map_err(library_error)
            })
            .unwrap();
        let models = bundle_models(&library, &bundle_id).unwrap();
        let expected = snapshot(&models).unwrap();
        let slicer = crate::extension_api::SlicerApp {
            id: "com.bambulab.bambu-studio".into(),
            bundle_id: "com.bambulab.bambu-studio".into(),
            display_name: "Bambu Studio".into(),
            app_path: "/Applications/Bambu Studio.app".into(),
        };
        let mut launched = false;
        let io_guard = library.begin_io().unwrap();
        let destination = send_bundle_with(
            &library,
            &bundle_id,
            &slicer.id,
            "test-token",
            &expected,
            || true,
            || vec![slicer.clone()],
            |_, paths| {
                launched = true;
                assert!(library.begin_maintenance().is_err());
                assert_eq!(paths.len(), 2);
                let payloads = paths
                    .iter()
                    .map(|path| fs::read(path).unwrap())
                    .collect::<Vec<_>>();
                assert!(payloads.contains(&first_bytes));
                assert!(payloads.contains(&second_bytes));
                Ok(())
            },
        )
        .unwrap();
        drop(io_guard);
        assert!(launched);
        let retained = fs::read_dir(&destination)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        assert_eq!(retained.len(), 2);
        assert!(retained.iter().all(|path| path.is_file()));
        assert_eq!(fs::read(&first).unwrap(), first_bytes);
        assert_eq!(fs::read(&second).unwrap(), second_bytes);
        assert!(send_bundle_with(
            &library,
            &bundle_id,
            "unknown.slicer",
            "unknown-token",
            &expected,
            || true,
            Vec::new,
            |_, _| panic!("unknown slicer must never launch"),
        )
        .is_err());
        let handoff_root = crate::handoff::stable_handoff_root(&catalog).unwrap();
        assert!(!handoff_root.join("unknown-token").exists());
        library
            .with_db(|db| {
                db.set_model_bundle(&models[0].id, None)
                    .map(|_| ())
                    .map_err(library_error)
            })
            .unwrap();
        assert!(send_bundle_with(
            &library,
            &bundle_id,
            &slicer.id,
            "stale-token",
            &expected,
            || true,
            || panic!("stale membership must fail before slicer detection"),
            |_, _| panic!("stale membership must never launch"),
        )
        .is_err());
        assert!(!handoff_root.join("stale-token").exists());
        let _ = fs::remove_dir_all(handoff_root);
        let _ = fs::remove_dir_all(catalog);
        let _ = fs::remove_dir_all(sources);
    }

    fn test_model() -> Model {
        Model {
            id: "m".into(),
            display_name: "M".into(),
            description: String::new(),
            folder_id: "root".into(),
            original_name: Some("m.stl".into()),
            format: Some("stl-binary".into()),
            bytes: 84,
            content_hash: Some("a".repeat(64)),
            storage_path: None,
            metadata_version: 1,
            created_at: "1".into(),
            updated_at: "1".into(),
            tags: Vec::new(),
            filament_ids: Vec::new(),
            source_kind: "managed".into(),
            source_path: None,
            source_root_id: None,
            source_missing: false,
            source_modified_at: None,
            thumbnail_path: None,
            metadata: Default::default(),
            bundle_id: Some("b".into()),
            source_archive_member: None,
            catalog_kind: "file".into(),
        }
    }
}
