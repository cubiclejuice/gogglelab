use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use library_core::Model;
use library_protocol::{
    BundlePreviewMember, BundlePreviewSummary, ContentKind, ContentLease, RequestContext,
    RuntimeError, RuntimeErrorCode,
};
use sha2::{Digest, Sha256};

use crate::secure_fs::Directory;
use crate::{format_system_time, LibraryRuntime, ProcessingError};
use stl_core::package_bounds::PackageProcessingContext;
use stl_core::zip_bounds::ZipResourceObserver;

const MAX_PREVIEW_MEMBERS: usize = 32;
const MAX_PREVIEW_TRIANGLES: u32 = 2_000_000;
const MAX_PREVIEWS: usize = 4;
const MAX_CONCURRENT_PREPARATIONS: usize = 2;
const MAX_GEOMETRY_BYTES: usize = 256 * 1024 * 1024;
const MAX_SINGLE_GEOMETRY_BYTES: usize = 16 + MAX_PREVIEW_TRIANGLES as usize * 9 * 4;
const PREVIEW_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_BUNDLE_DOWNLOAD_MEMBERS: usize = 10_000;
const MAX_BUNDLE_DOWNLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub(crate) struct BundleArchive {
    pub(crate) path: PathBuf,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

#[derive(Clone, PartialEq, Eq)]
struct SnapshotMember {
    model_id: String,
    content_hash: String,
    bytes: u64,
}

struct PreparedBundle {
    summary: BundlePreviewSummary,
    snapshot: Vec<SnapshotMember>,
    geometry: Arc<[u8]>,
    principal_id: String,
    expires: SystemTime,
    catalog_revision: u64,
}

#[derive(Default)]
pub(crate) struct BundlePreviewStore {
    state: Mutex<StoreState>,
}

fn prune(state: &mut StoreState, epoch: u64) {
    let now = SystemTime::now();
    let mut retained_bytes = state.retained_bytes;
    state.prepared.retain(|_, item| {
        let keep = item.expires > now && item.summary.library_epoch == epoch;
        if !keep {
            retained_bytes = retained_bytes.saturating_sub(item.geometry.len());
        }
        keep
    });
    state.retained_bytes = retained_bytes;
}

#[derive(Default)]
struct StoreState {
    prepared: HashMap<String, PreparedBundle>,
    reservations: usize,
    retained_bytes: usize,
}

struct PreviewReservation<'a> {
    store: &'a BundlePreviewStore,
}

impl Drop for PreviewReservation<'_> {
    fn drop(&mut self) {
        let mut state = self.store.state.lock().unwrap();
        state.reservations = state.reservations.saturating_sub(1);
    }
}

impl BundlePreviewStore {
    pub(crate) fn invalidate_all(&self) {
        let mut state = self.state.lock().unwrap();
        state.prepared.clear();
        state.retained_bytes = 0;
    }

    pub(crate) fn prepare(
        &self,
        runtime: &LibraryRuntime,
        context: &RequestContext,
        bundle_id: &str,
    ) -> Result<BundlePreviewSummary, RuntimeError> {
        let _reservation = self.reserve(runtime.library_epoch())?;
        let models = runtime.read_db(|db| db.list_bundle_models(bundle_id))?;
        if models.is_empty() {
            return Err(not_found("Bundle contains no active models"));
        }
        let snapshot = snapshot(&models)?;
        let revision = runtime.catalog_revision();
        let previewed = models.len().min(MAX_PREVIEW_MEMBERS);
        let combined = is_combined_bundle(&models);
        let mut meshes = Vec::with_capacity(previewed);
        let mut triangle_count = 0_u32;
        for model in models.iter().take(previewed) {
            let parsed = load_model(runtime, model)?;
            triangle_count = triangle_count
                .checked_add(parsed.triangle_count)
                .ok_or_else(|| invalid("Bundle preview triangle count overflowed"))?;
            if triangle_count > MAX_PREVIEW_TRIANGLES {
                return Err(invalid("Bundle preview exceeds the triangle limit"));
            }
            meshes.push((model, parsed));
        }
        let offsets = layout_offsets(&meshes, combined)?;
        let mut positions = Vec::new();
        let mut members = Vec::with_capacity(meshes.len());
        for ((model, mesh), offset) in meshes.into_iter().zip(offsets) {
            if mesh.stored_normals.iter().any(|value| !value.is_finite()) {
                return Err(invalid(
                    "Bundle preview contains a non-finite stored normal",
                ));
            }
            for point in mesh.positions.chunks_exact(3) {
                let translated = [
                    point[0] + offset[0],
                    point[1] + offset[1],
                    point[2] + offset[2],
                ];
                if translated.iter().any(|value| !value.is_finite()) {
                    return Err(invalid("Bundle preview layout exceeds finite coordinates"));
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
        validate_snapshot(runtime, bundle_id, &snapshot, revision)?;
        let geometry = Arc::<[u8]>::from(build_geometry(triangle_count, &positions));
        let geometry_sha256 = format!("{:x}", Sha256::digest(geometry.as_ref()));
        let token = uuid::Uuid::new_v4().simple().to_string();
        let omitted_count = models.len() - previewed;
        let summary = BundlePreviewSummary {
            token: token.clone(),
            bundle_id: bundle_id.to_owned(),
            member_count: models.len(),
            previewed_count: previewed,
            omitted_count,
            triangle_count,
            layout: if combined { "combined" } else { "grid" }.into(),
            members,
            notice: (omitted_count > 0).then(|| {
                format!(
                    "Preview shows {previewed} of {} models. Bundle actions still include all models.",
                    models.len()
                )
            }),
            geometry_bytes: geometry.len() as u64,
            geometry_sha256,
            library_epoch: runtime.library_epoch(),
        };
        let expires = SystemTime::now() + PREVIEW_TTL;
        let mut state = self.state.lock().unwrap();
        prune(&mut state, runtime.library_epoch());
        if state.prepared.len() >= MAX_PREVIEWS
            || state.retained_bytes.saturating_add(geometry.len()) > MAX_GEOMETRY_BYTES
        {
            return Err(busy("Bundle preview capacity is full"));
        }
        state.retained_bytes += geometry.len();
        state.prepared.insert(
            token,
            PreparedBundle {
                summary: summary.clone(),
                snapshot,
                geometry,
                principal_id: context.principal_id.clone(),
                expires,
                catalog_revision: revision,
            },
        );
        Ok(summary)
    }

    pub(crate) fn describe(
        &self,
        runtime: &LibraryRuntime,
        principal_id: &str,
        token: &str,
    ) -> Result<ContentLease, RuntimeError> {
        let prepared = self.admit(runtime, principal_id, token)?;
        Ok(ContentLease {
            id: prepared.summary.token.clone(),
            kind: ContentKind::Geometry,
            file_name: format!("bundle-{}.stl2", prepared.summary.bundle_id),
            mime: "application/vnd.gogglelab.geometry".into(),
            bytes: prepared.summary.geometry_bytes,
            sha256: prepared.summary.geometry_sha256.clone(),
            library_epoch: prepared.summary.library_epoch,
            expires_at: format_system_time(prepared.expires)?,
        })
    }

    pub(crate) fn stream(
        &self,
        runtime: &LibraryRuntime,
        principal_id: &str,
        token: &str,
        sink: &mut dyn Write,
    ) -> Result<(), RuntimeError> {
        let prepared = self.admit(runtime, principal_id, token)?;
        sink.write_all(&prepared.geometry).map_err(|_| io_error())
    }

    pub(crate) fn close(&self, principal_id: &str, token: &str) -> Result<(), RuntimeError> {
        let mut state = self.state.lock().unwrap();
        if state
            .prepared
            .get(token)
            .is_some_and(|item| item.principal_id != principal_id)
        {
            return Err(forbidden());
        }
        if let Some(removed) = state.prepared.remove(token) {
            state.retained_bytes = state.retained_bytes.saturating_sub(removed.geometry.len());
        }
        Ok(())
    }

    pub(crate) fn bundle_id_for_download(
        &self,
        runtime: &LibraryRuntime,
        principal_id: &str,
        token: &str,
    ) -> Result<String, RuntimeError> {
        Ok(self.admit(runtime, principal_id, token)?.summary.bundle_id)
    }

    fn reserve(&self, epoch: u64) -> Result<PreviewReservation<'_>, RuntimeError> {
        let mut state = self.state.lock().unwrap();
        prune(&mut state, epoch);
        if state.reservations >= MAX_CONCURRENT_PREPARATIONS
            || state.prepared.len().saturating_add(state.reservations) >= MAX_PREVIEWS
            || state
                .retained_bytes
                .saturating_add((state.reservations + 1) * MAX_SINGLE_GEOMETRY_BYTES)
                > MAX_GEOMETRY_BYTES
        {
            return Err(busy("Bundle preview capacity is full"));
        }
        state.reservations += 1;
        Ok(PreviewReservation { store: self })
    }

    fn admit(
        &self,
        runtime: &LibraryRuntime,
        principal_id: &str,
        token: &str,
    ) -> Result<PreparedBundle, RuntimeError> {
        let prepared = self
            .state
            .lock()
            .unwrap()
            .prepared
            .get(token)
            .map(|item| PreparedBundle {
                summary: item.summary.clone(),
                snapshot: item.snapshot.clone(),
                geometry: Arc::clone(&item.geometry),
                principal_id: item.principal_id.clone(),
                expires: item.expires,
                catalog_revision: item.catalog_revision,
            })
            .ok_or_else(|| stale("Bundle preview token is unknown or expired"))?;
        if prepared.principal_id != principal_id {
            return Err(forbidden());
        }
        if prepared.expires <= SystemTime::now()
            || prepared.summary.library_epoch != runtime.library_epoch()
        {
            let mut state = self.state.lock().unwrap();
            if let Some(removed) = state.prepared.remove(token) {
                state.retained_bytes = state.retained_bytes.saturating_sub(removed.geometry.len());
            }
            return Err(stale("Bundle preview token is stale"));
        }
        validate_snapshot(
            runtime,
            &prepared.summary.bundle_id,
            &prepared.snapshot,
            prepared.catalog_revision,
        )?;
        Ok(prepared)
    }
}

pub(crate) fn create_bundle_archive(
    runtime: &LibraryRuntime,
    bundle_id: &str,
) -> Result<BundleArchive, RuntimeError> {
    let models = runtime.read_db(|db| db.list_bundle_models(bundle_id))?;
    if models.is_empty() || models.len() > MAX_BUNDLE_DOWNLOAD_MEMBERS {
        return Err(invalid(
            "Bundle download member count is outside the supported range",
        ));
    }
    let total = models
        .iter()
        .try_fold(0_u64, |total, model| total.checked_add(model.bytes))
        .ok_or_else(|| invalid("Bundle download size overflowed"))?;
    let archive_overhead = (models.len() as u64)
        .checked_mul(1024)
        .ok_or_else(|| invalid("Bundle archive overhead overflowed"))?;
    if total
        .checked_add(archive_overhead)
        .is_none_or(|bytes| bytes > MAX_BUNDLE_DOWNLOAD_BYTES)
    {
        return Err(invalid("Bundle download exceeds the 4 GiB limit"));
    }
    let snapshot = snapshot(&models)?;
    let revision = runtime.catalog_revision();
    let namespace = owned_bundle_namespace(runtime.host_library_root())?;
    let path = namespace.join(format!("bundle-{}.zip", uuid::Uuid::new_v4().simple()));
    let directory = Directory::open(&namespace).map_err(|_| io_error())?;
    let name = path.file_name().ok_or_else(|| io_error())?;
    let mut created = false;
    let result = (|| {
        let file = directory
            .create_new_file_read_write(name)
            .map_err(|_| io_error())?;
        created = true;
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (index, model) in models.iter().enumerate() {
            let bytes = load_source_bytes(runtime, model)?;
            let name = format!("{index:04}-{}", sanitized_name(model));
            archive.start_file(name, options).map_err(|_| io_error())?;
            archive.write_all(&bytes).map_err(|_| io_error())?;
        }
        let file = archive.finish().map_err(|_| io_error())?;
        file.sync_all().map_err(|_| io_error())?;
        let metadata = file.metadata().map_err(|_| io_error())?;
        if !metadata.is_file() || metadata.len() > MAX_BUNDLE_DOWNLOAD_BYTES {
            return Err(invalid("Generated bundle archive exceeds its owned limit"));
        }
        let expected_hash = hash_file(file.try_clone().map_err(|_| io_error())?, metadata.len())?;
        validate_snapshot(runtime, bundle_id, &snapshot, revision)?;
        let published = directory.open_file(name).map_err(|_| io_error())?;
        let published_metadata = published.metadata().map_err(|_| io_error())?;
        if published_metadata.len() != metadata.len()
            || hash_file(published, metadata.len())? != expected_hash
        {
            let _ = directory.hold_entry(name);
            return Err(stale("Generated bundle archive changed before publication"));
        }
        Ok(BundleArchive {
            path: path.clone(),
            bytes: metadata.len(),
            sha256: expected_hash,
        })
    })();
    if result.is_err() && created {
        let _ = directory.hold_entry(name);
    }
    result
}

pub(crate) fn bundle_archive_estimated_bytes(
    runtime: &LibraryRuntime,
    bundle_id: &str,
) -> Result<u64, RuntimeError> {
    let models = runtime.read_db(|db| db.list_bundle_models(bundle_id))?;
    if models.is_empty() || models.len() > MAX_BUNDLE_DOWNLOAD_MEMBERS {
        return Err(invalid(
            "Bundle download member count is outside the supported range",
        ));
    }
    let total = models
        .iter()
        .try_fold(0_u64, |total, model| total.checked_add(model.bytes))
        .ok_or_else(|| invalid("Bundle download size overflowed"))?;
    let overhead = (models.len() as u64)
        .checked_mul(1024)
        .ok_or_else(|| invalid("Bundle archive overhead overflowed"))?;
    total
        .checked_add(overhead)
        .filter(|bytes| *bytes <= MAX_BUNDLE_DOWNLOAD_BYTES)
        .ok_or_else(|| invalid("Bundle download exceeds the 4 GiB limit"))
}

fn owned_bundle_namespace(library_root: &Path) -> Result<PathBuf, RuntimeError> {
    let staging = library_root.join("staging");
    let metadata = fs::symlink_metadata(&staging).map_err(|_| io_error())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || staging.canonicalize().map_err(|_| io_error())? != staging
    {
        return Err(invalid("Bundle staging namespace is unsafe"));
    }
    let namespace = staging.join("bundle-downloads");
    match fs::symlink_metadata(&namespace) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(invalid("Bundle download namespace is unsafe"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&namespace).map_err(|_| io_error())?;
        }
        Err(_) => return Err(io_error()),
    }
    if namespace.canonicalize().map_err(|_| io_error())? != namespace {
        return Err(invalid("Bundle download namespace identity changed"));
    }
    Ok(namespace)
}

fn load_source_bytes(runtime: &LibraryRuntime, model: &Model) -> Result<Vec<u8>, RuntimeError> {
    runtime.verified_model_source_bytes(model)
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
    if sanitized.is_empty() || matches!(sanitized.as_str(), "." | "..") {
        "model.stl".into()
    } else {
        sanitized
    }
}

fn hash_file(mut file: fs::File, expected_bytes: u64) -> Result<String, RuntimeError> {
    file.seek(SeekFrom::Start(0)).map_err(|_| io_error())?;
    let mut digest = Sha256::new();
    let mut read = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|_| io_error())?;
        if count == 0 {
            break;
        }
        read = read
            .checked_add(count as u64)
            .ok_or_else(|| invalid("Bundle archive size overflowed"))?;
        if read > expected_bytes {
            return Err(stale("Generated bundle archive changed"));
        }
        digest.update(&buffer[..count]);
    }
    if read != expected_bytes {
        return Err(stale("Generated bundle archive changed"));
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn snapshot(models: &[Model]) -> Result<Vec<SnapshotMember>, RuntimeError> {
    models
        .iter()
        .map(|model| {
            Ok(SnapshotMember {
                model_id: model.id.clone(),
                content_hash: model
                    .content_hash
                    .clone()
                    .ok_or_else(|| invalid("Bundle model has no source content"))?,
                bytes: model.bytes,
            })
        })
        .collect()
}

fn validate_snapshot(
    runtime: &LibraryRuntime,
    bundle_id: &str,
    expected: &[SnapshotMember],
    revision: u64,
) -> Result<(), RuntimeError> {
    if runtime.catalog_revision() != revision {
        return Err(stale("Bundle changed while previewing"));
    }
    let current = runtime.read_db(|db| db.list_bundle_models(bundle_id))?;
    if snapshot(&current)? != expected {
        return Err(stale("Bundle membership or source hashes changed"));
    }
    Ok(())
}

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

fn load_model(
    runtime: &LibraryRuntime,
    model: &Model,
) -> Result<stl_core::ParsedStl, RuntimeError> {
    let mut context = PackageProcessingContext::new(ZipResourceObserver::new());
    load_model_in_context(runtime, model, &mut context)
}

fn load_model_in_context(
    runtime: &LibraryRuntime,
    model: &Model,
    context: &mut PackageProcessingContext,
) -> Result<stl_core::ParsedStl, RuntimeError> {
    let observer = context.observer();
    let result = (|| {
        let bytes = runtime.verified_model_source_bytes_in_context(model, context)?;
        stl_core::parse_model_in_context(&bytes, context).map_err(|error| {
            ProcessingError::from(error).into_runtime(invalid("Bundle member could not be parsed"))
        })
    })();
    observer.check().map_err(|error| {
        ProcessingError::from(error).into_runtime(invalid("Bundle member could not be parsed"))
    })?;
    result
}

fn layout_offsets(
    meshes: &[(&Model, stl_core::ParsedStl)],
    combined: bool,
) -> Result<Vec<[f32; 3]>, RuntimeError> {
    if combined || meshes.len() <= 1 {
        return Ok(vec![[0.0, 0.0, 0.0]; meshes.len()]);
    }
    let bounds = meshes
        .iter()
        .map(|(_, mesh)| xy_bounds(mesh))
        .collect::<Vec<_>>();
    let cell_x = bounds.iter().map(|value| value[2]).fold(0.0, f32::max) + 10.0;
    let cell_y = bounds.iter().map(|value| value[3]).fold(0.0, f32::max) + 10.0;
    if !cell_x.is_finite()
        || !cell_y.is_finite()
        || bounds.iter().flatten().any(|value| !value.is_finite())
    {
        return Err(invalid("Bundle geometry is too large to lay out safely"));
    }
    let columns = (meshes.len() as f32).sqrt().ceil() as usize;
    let offsets = (0..meshes.len())
        .map(|index| {
            [
                (index % columns) as f32 * cell_x - bounds[index][0],
                (index / columns) as f32 * cell_y - bounds[index][1],
                0.0,
            ]
        })
        .collect::<Vec<_>>();
    if offsets.iter().flatten().any(|value| !value.is_finite()) {
        return Err(invalid("Bundle preview offsets are not finite"));
    }
    Ok(offsets)
}

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
        [0.0; 4]
    } else {
        [
            min[0],
            min[1],
            (max[0] - min[0]).max(0.0),
            (max[1] - min[1]).max(0.0),
        ]
    }
}

fn build_geometry(triangle_count: u32, positions: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16 + positions.len() * 4);
    bytes.extend_from_slice(b"STL2");
    bytes.extend_from_slice(&2_u32.to_le_bytes());
    bytes.extend_from_slice(&triangle_count.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    for value in positions {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn invalid(message: &str) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::InvalidInput,
        message: message.into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    }
}

fn not_found(message: &str) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::NotFound,
        message: message.into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    }
}

fn stale(message: &str) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Stale,
        message: message.into(),
        retryable: true,
        current_version: None,
        source_holds: Vec::new(),
    }
}

fn forbidden() -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Forbidden,
        message: "Bundle preview belongs to another principal".into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    }
}

fn busy(message: &str) -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Busy,
        message: message.into(),
        retryable: true,
        current_version: None,
        source_holds: Vec::new(),
    }
}

fn io_error() -> RuntimeError {
    RuntimeError {
        code: RuntimeErrorCode::Io,
        message: "Bundle preview filesystem operation failed".into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservations_are_bounded_and_release_after_failure_scope() {
        let store = BundlePreviewStore::default();
        let first = store.reserve(1).unwrap();
        let second = store.reserve(1).unwrap();
        assert_eq!(store.reserve(1).err().unwrap().code, RuntimeErrorCode::Busy);
        drop(first);
        assert!(store.reserve(1).is_ok());
        drop(second);
        assert_eq!(store.state.lock().unwrap().reservations, 0);
    }
    #[test]
    fn bundle_model_nested_validation_and_parse_share_actual_package_counter() {
        for method in [
            zip::CompressionMethod::Stored,
            zip::CompressionMethod::Deflated,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let (runtime, model, _, xml_bytes) =
                crate::tests::nested_package_fixture(&temp, method);
            let observer = ZipResourceObserver::new();
            let mut context = PackageProcessingContext::new(observer.clone());
            let parsed = load_model_in_context(&runtime, &model, &mut context).unwrap();
            assert_eq!(parsed.triangle_count, 1);
            assert_eq!(context.usage().inflation_bytes, xml_bytes * 2);
            assert_eq!(context.usage().retained_input_bytes, 0);
            let _ = observer.stop_io(stl_core::ResourceStop::ReadWorkExceeded);
            let error = load_model_in_context(&runtime, &model, &mut context).unwrap_err();
            assert_eq!(error.code, RuntimeErrorCode::Unsupported);
            assert!(!error.retryable);
            assert_eq!(context.usage().inflation_bytes, xml_bytes * 2);
        }
    }
}
