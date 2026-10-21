//! Bounded catalog diagnostics and shared thumbnail jobs; never accepts host paths.
use std::collections::BTreeSet;
use std::sync::Arc;

use library_core::{LibraryError, Model};
use library_protocol::{
    Capability, CatalogJobEngine, HashStatus, ModelSourceStatus, RequestContext, RuntimeError,
    RuntimeErrorCode, ScanStatus, SourceState,
};

use crate::{invalid, io_error, thumbnails, LibraryRuntime, ReservationKind};
use stl_core::{package_bounds::PackageProcessingContext, zip_bounds::ZipResourceObserver};

const MAX_JOB_MODELS: usize = 10_000;
const MAX_SOURCE_CHECK_BYTES: u64 = 256 * 1024 * 1024;

impl LibraryRuntime {
    /// Counts cataloged hashes. It does not claim that stored hashes verify today's bytes.
    pub fn catalog_hash_status(
        &self,
        context: &RequestContext,
    ) -> Result<HashStatus, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        let counts = self.read_db(|db| db.catalog_hash_counts())?;
        Ok(HashStatus {
            model_count: counts.total.to_string(),
            hashed_count: counts.hashed.to_string(),
            unhashed_count: counts.unhashed.to_string(),
            bookmark_count: counts.bookmarks.to_string(),
            missing_count: counts.missing.to_string(),
        })
    }

    /// Explicit, bounded source verification including archived members and admitted roots.
    pub fn check_model_sources(
        &self,
        context: &RequestContext,
        model_ids: Vec<String>,
    ) -> Result<Vec<ModelSourceStatus>, RuntimeError> {
        self.require(context, Capability::CatalogRead)?;
        self.require(context, Capability::MediaRead)?;
        let _io = self.begin_borrowed_io()?;
        let models = self.job_models(&model_ids)?;
        // Check the complete cost before touching any source. A per-model cap alone
        // still allowed a single request to read close to a terabyte.
        validate_source_check_budget(&models)?;
        Ok(models
            .into_iter()
            .map(|model| {
                let state = if model.catalog_kind == "bookmark" {
                    SourceState::Bookmark
                } else if model.content_hash.is_none() {
                    SourceState::Unhashed
                } else if model.source_missing {
                    SourceState::Missing
                } else {
                    match self.verified_model_source_bytes(&model) {
                        Ok(_) => SourceState::Available,
                        Err(error) => match error.code {
                            RuntimeErrorCode::Forbidden => SourceState::Unadmitted,
                            RuntimeErrorCode::Conflict | RuntimeErrorCode::Stale => {
                                SourceState::Changed
                            }
                            RuntimeErrorCode::NotFound => SourceState::Missing,
                            _ => SourceState::Unavailable,
                        },
                    }
                };
                ModelSourceStatus {
                    model_id: model.id,
                    stored_hash: model.content_hash,
                    state,
                }
            })
            .collect())
    }

    /// One cancellable job shares scan admission with startup and manual scans.
    pub fn start_thumbnail_job(
        &self,
        context: &RequestContext,
        model_ids: Vec<String>,
        force: bool,
    ) -> Result<ScanStatus, RuntimeError> {
        self.require(context, Capability::CatalogWrite)?;
        self.require(context, Capability::MediaWrite)?;
        let runtime = self
            .self_weak
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(|| crate::internal("Runtime is unavailable"))?;
        let io = runtime.begin_io()?;
        let models = self.job_models(&model_ids)?;
        if !self.scan_control().try_begin(0, "thumbnails") {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Busy,
                message: "A scan or thumbnail job is already running".into(),
                retryable: true,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        self.scan_control().set_thumbnail_progress(models.len());
        let settings = match self.read_db(|db| db.get_catalog_settings()) {
            Ok(settings) => settings,
            Err(error) => {
                self.scan_control().finish();
                return Err(error);
            }
        };
        let state = Arc::clone(&runtime);
        if std::thread::Builder::new()
            .name("gogglelab-thumbnail-job".into())
            .spawn(move || {
                let _io = io;
                for model in models {
                    if state.scan_control().cancelled() {
                        break;
                    }
                    if let Err(error) = render_model(&state, &model, &settings, force) {
                        state
                            .scan_control()
                            .error(format!("Thumbnail {}: {}", model.id, error.message));
                    }
                    state.scan_control().thumbnail_completed();
                }
                state.scan_control().finish();
            })
            .is_err()
        {
            runtime
                .scan_control()
                .failed_to_spawn("Could not start thumbnail job".into());
            return Err(io_error());
        }
        Ok(crate::scan_status(self.scan_control().status(), false))
    }

    fn job_models(&self, ids: &[String]) -> Result<Vec<Model>, RuntimeError> {
        if ids.is_empty()
            || ids.len() > MAX_JOB_MODELS
            || ids.iter().any(|id| id.is_empty() || id.len() > 128)
            || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
        {
            return Err(invalid(
                "Model selection must contain 1 to 10000 unique IDs",
            ));
        }
        self.read_db(|db| {
            ids.iter()
                .map(|id| {
                    db.get_model(id)?
                        .ok_or_else(|| LibraryError::ModelNotFound(id.clone()))
                })
                .collect()
        })
    }
}

fn validate_source_check_budget(models: &[Model]) -> Result<(), RuntimeError> {
    let total_bytes = models
        .iter()
        .filter(|model| model.catalog_kind != "bookmark" && !model.source_missing)
        .try_fold(0_u64, |total, model| total.checked_add(model.bytes))
        .ok_or_else(|| invalid("Source verification byte total overflowed"))?;
    if total_bytes > MAX_SOURCE_CHECK_BYTES {
        return Err(invalid(
            "Source verification is limited to 256 MiB per request; split the selection",
        ));
    }
    Ok(())
}

impl CatalogJobEngine for LibraryRuntime {
    fn catalog_hash_status(&self, context: &RequestContext) -> Result<HashStatus, RuntimeError> {
        LibraryRuntime::catalog_hash_status(self, context)
    }
    fn check_model_sources(
        &self,
        context: &RequestContext,
        model_ids: Vec<String>,
    ) -> Result<Vec<ModelSourceStatus>, RuntimeError> {
        LibraryRuntime::check_model_sources(self, context, model_ids)
    }
    fn start_thumbnail_job(
        &self,
        context: &RequestContext,
        model_ids: Vec<String>,
        force: bool,
    ) -> Result<ScanStatus, RuntimeError> {
        LibraryRuntime::start_thumbnail_job(self, context, model_ids, force)
    }
}

fn render_model(
    runtime: &Arc<LibraryRuntime>,
    model: &Model,
    settings: &library_core::CatalogSettings,
    force: bool,
) -> Result<(), RuntimeError> {
    render_model_with_renderer(
        runtime,
        model,
        settings,
        force,
        |bytes, hash, settings, force, processing| {
            thumbnails::ensure_thumbnail_from_bytes_in_context_typed(
                &runtime.root,
                bytes,
                hash,
                bytes.len() as u64,
                settings,
                force,
                processing,
            )
        },
    )
}

fn render_model_with_renderer(
    runtime: &Arc<LibraryRuntime>,
    model: &Model,
    settings: &library_core::CatalogSettings,
    force: bool,
    render: impl FnOnce(
        &[u8],
        &str,
        &library_core::CatalogSettings,
        bool,
        &mut PackageProcessingContext,
    ) -> Result<thumbnails::ThumbnailArtifact, crate::ProcessingError>,
) -> Result<(), RuntimeError> {
    if model.catalog_kind == "bookmark" {
        return Ok(());
    }
    if !force
        && runtime
            .read_db(|db| db.thumbnail_cache_entry_for_model(&model.id))?
            .is_some()
    {
        return Ok(());
    }
    let hash = model
        .content_hash
        .as_deref()
        .ok_or_else(|| invalid("Model has no content hash"))?;
    let _reservation = runtime.reserve_temp(ReservationKind::Owned, model.bytes)?;
    let observer = ZipResourceObserver::new();
    let mut processing = PackageProcessingContext::new(observer.clone());
    let bytes = runtime.verified_model_source_bytes_in_context(model, &mut processing)?;
    let result = render(&bytes, hash, settings, force, &mut processing);
    let artifact = observer
        .finish(result)
        .map_err(|error| error.into_runtime(invalid("Thumbnail render failed")))?;
    if !runtime.scan_control().cancelled() {
        runtime.mutate_db(|db| {
            db.set_thumbnail_for_source_and_settings(
                &model.id,
                hash,
                settings,
                &artifact.png_hash,
                &artifact.relative_path,
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RuntimeConfig, RuntimeRootPolicy};
    use library_protocol::RootAccess;
    use std::fs;

    fn fixture() -> (
        tempfile::TempDir,
        Arc<LibraryRuntime>,
        RequestContext,
        Model,
        std::path::PathBuf,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let sources = temp.path().join("sources");
        fs::create_dir(&sources).unwrap();
        let path = sources.join("part.stl");
        fs::write(&path, b"solid part\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 10 0 0\nvertex 0 10 0\nendloop\nendfacet\nendsolid part\n").unwrap();
        let capabilities = BTreeSet::from([
            Capability::CatalogRead,
            Capability::CatalogWrite,
            Capability::MediaRead,
            Capability::MediaWrite,
            Capability::Scan,
        ]);
        let runtime = LibraryRuntime::open(RuntimeConfig {
            library_root: temp.path().join("library"),
            integration_settings_path: temp.path().join("integrations.json"),
            roots: vec![RuntimeRootPolicy {
                id: "models".into(),
                label: "Models".into(),
                path: sources,
                access: RootAccess::ReadOnly,
                catalog_root_id: None,
            }],
            capabilities: capabilities.clone(),
            local_catalog_root_access: false,
        })
        .unwrap();
        let root = runtime
            .read_db(|db| db.list_index_roots())
            .unwrap()
            .remove(0);
        crate::scanner::scan_root(&runtime, &root).unwrap();
        let model = runtime
            .read_db(|db| db.list_models(None, None))
            .unwrap()
            .remove(0);
        let context = RequestContext {
            principal_id: "fixture".into(),
            capabilities,
            request_id: "jobs".into(),
        };
        (temp, runtime, context, model, path)
    }

    #[test]
    fn source_checks_distinguish_stored_hash_from_verified_current_bytes() {
        let (_temp, runtime, context, model, path) = fixture();
        let status = runtime.catalog_hash_status(&context).unwrap();
        assert_eq!(status.hashed_count, "1");
        assert_eq!(
            runtime
                .check_model_sources(&context, vec![model.id.clone()])
                .unwrap()[0]
                .state,
            SourceState::Available
        );
        let original = fs::read(&path).unwrap();
        let mut changed = original.clone();
        changed[0] = b'S';
        fs::write(&path, changed).unwrap();
        assert_eq!(
            runtime
                .check_model_sources(&context, vec![model.id.clone()])
                .unwrap()[0]
                .state,
            SourceState::Changed
        );
        assert_eq!(
            runtime.catalog_hash_status(&context).unwrap().hashed_count,
            "1"
        );
        fs::write(path, original).unwrap();
    }

    #[test]
    fn diagnostics_and_jobs_fail_closed_for_capabilities_bounds_and_unknown_ids() {
        let (_temp, runtime, mut context, model, _path) = fixture();
        assert_eq!(
            runtime
                .check_model_sources(&context, vec![model.id.clone(), model.id.clone()])
                .unwrap_err()
                .code,
            RuntimeErrorCode::InvalidInput
        );
        assert_eq!(
            runtime
                .check_model_sources(&context, vec!["unknown".into()])
                .unwrap_err()
                .code,
            RuntimeErrorCode::NotFound
        );
        context.capabilities.remove(&Capability::MediaRead);
        assert_eq!(
            runtime
                .check_model_sources(&context, vec![model.id.clone()])
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
        context.capabilities.remove(&Capability::MediaWrite);
        assert_eq!(
            runtime
                .start_thumbnail_job(&context, vec![model.id], true)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Forbidden
        );
    }

    #[test]
    fn source_verification_rejects_oversized_selection_before_any_reads() {
        let (_temp, _runtime, _context, model, _path) = fixture();
        let mut oversized = vec![model.clone(); 3];
        for item in &mut oversized {
            item.bytes = 90 * 1024 * 1024;
        }
        let error = validate_source_check_budget(&oversized).unwrap_err();
        assert_eq!(error.code, RuntimeErrorCode::InvalidInput);

        let mut within_budget = vec![model.clone(); 2];
        for item in &mut within_budget {
            item.bytes = 100 * 1024 * 1024;
        }
        assert!(validate_source_check_budget(&within_budget).is_ok());

        let bookmark = Model {
            catalog_kind: "bookmark".into(),
            bytes: u64::MAX,
            ..model
        };
        assert!(validate_source_check_budget(&[bookmark]).is_ok());
    }

    #[test]
    fn explicit_regeneration_is_single_flight_and_preserves_originals_and_default_ids() {
        let (_temp, runtime, context, model, path) = fixture();
        let bytes = fs::read(&path).unwrap();
        let before = runtime
            .read_db(|db| db.thumbnail_cache_entry_for_model(&model.id))
            .unwrap()
            .unwrap();
        assert!(runtime.scan_control().try_begin(0, "scan"));
        assert_eq!(
            runtime
                .start_thumbnail_job(&context, vec![model.id.clone()], true)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Busy
        );
        runtime.scan_control().finish();
        runtime
            .start_thumbnail_job(&context, vec![model.id.clone()], true)
            .unwrap();
        for _ in 0..100 {
            if !runtime.scan_control().status().running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let status = runtime.scan_control().status();
        assert!(!status.running);
        assert!(status.errors.is_empty(), "{:?}", status.errors);
        assert_eq!(status.thumbnails_completed, 1);
        let after = runtime
            .read_db(|db| db.thumbnail_cache_entry_for_model(&model.id))
            .unwrap()
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(fs::read_dir(runtime.root.join("staging"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("thumbnail-source-")));
    }
    #[test]
    fn typed_catalog_render_errors_preserve_category_without_thumbnail_commit() {
        let failures = vec![
            crate::ProcessingError::ResourceStopped(stl_core::ResourceStop::InputLimitExceeded),
            crate::ProcessingError::ResourceStopped(stl_core::ResourceStop::ReadWorkExceeded),
            crate::ProcessingError::ResourceStopped(stl_core::ResourceStop::EnvelopeChanged),
            crate::ProcessingError::AllocationFailed,
        ];
        for failure in failures {
            let (_temp, runtime, _context, model, path) = fixture();
            let original = fs::read(&path).unwrap();
            let expected = failure.to_string();
            let (code, retryable) = match &failure {
                crate::ProcessingError::ResourceStopped(
                    stl_core::ResourceStop::EnvelopeChanged,
                ) => (RuntimeErrorCode::Stale, true),
                crate::ProcessingError::AllocationFailed => (RuntimeErrorCode::Internal, false),
                _ => (RuntimeErrorCode::Unsupported, false),
            };
            let before = runtime
                .read_db(|db| db.thumbnail_cache_entry_for_model(&model.id))
                .unwrap();
            let settings = runtime.read_db(|db| db.get_catalog_settings()).unwrap();
            let called = std::cell::Cell::new(false);
            let error =
                render_model_with_renderer(&runtime, &model, &settings, true, |_, _, _, _, _| {
                    called.set(true);
                    Err(failure)
                })
                .unwrap_err();
            assert!(called.get());
            assert_eq!(error.message, expected);
            assert_eq!(error.code, code);
            assert_eq!(error.retryable, retryable);
            assert_eq!(
                runtime
                    .read_db(|db| db.thumbnail_cache_entry_for_model(&model.id))
                    .unwrap(),
                before
            );
            assert_eq!(fs::read(&path).unwrap(), original);
        }
    }
    #[test]
    fn thumbnail_job_keeps_archived_validation_and_render_usage_before_catalog_commit() {
        use std::io::{Cursor, Write};
        let (_temp, runtime, _context, _model, path) = fixture();
        let xml = br#"<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources><object id="1"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources><build><item objectid="1"/></build></model>"#;
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[20, 40, 60, 255]).unwrap();
            writer.finish().unwrap();
        }
        for method in [
            zip::CompressionMethod::Stored,
            zip::CompressionMethod::Deflated,
        ] {
            let mut model_zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let options = zip::write::SimpleFileOptions::default().compression_method(method);
            for (name, bytes) in [
                ("3D/3dmodel.model", xml.as_slice()),
                ("Metadata/thumbnail.png", png.as_slice()),
            ] {
                model_zip.start_file(name, options).unwrap();
                model_zip.write_all(bytes).unwrap();
            }
            let model_bytes = model_zip.finish().unwrap().into_inner();
            let mut outer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            outer.start_file("part.3mf", options).unwrap();
            outer.write_all(&model_bytes).unwrap();
            let outer_bytes = outer.finish().unwrap().into_inner();
            let archive_path = path.parent().unwrap().join(format!("parts-{method:?}.zip"));
            fs::write(&archive_path, outer_bytes).unwrap();
            let mut root = runtime
                .read_db(|db| db.list_index_roots())
                .unwrap()
                .remove(0);
            root.scan_zip = true;
            let root = runtime.mutate_db(|db| db.update_index_root(&root)).unwrap();
            crate::scanner::scan_root(&runtime, &root).unwrap();
            let models = runtime.read_db(|db| db.list_models(None, None)).unwrap();
            let archived = models
                .iter()
                .find(|model| {
                    model.source_archive_member.as_deref() == Some("part.3mf")
                        && model.source_path.as_deref().is_some_and(|source| {
                            source.ends_with(archive_path.file_name().unwrap().to_str().unwrap())
                        })
                })
                .unwrap();
            let settings = runtime.read_db(|db| db.get_catalog_settings()).unwrap();
            render_model_with_renderer(
                &runtime,
                archived,
                &settings,
                true,
                |bytes, hash, settings, force, processing| {
                    assert_eq!(processing.usage().inflation_bytes, xml.len() as u64);
                    assert_eq!(processing.usage().retained_input_bytes, 0);
                    let artifact = thumbnails::ensure_thumbnail_from_bytes_in_context_typed(
                        &runtime.root,
                        bytes,
                        hash,
                        bytes.len() as u64,
                        settings,
                        force,
                        processing,
                    )?;
                    assert_eq!(
                        processing.usage().inflation_bytes,
                        2 * xml.len() as u64 + png.len() as u64
                    );
                    assert_eq!(processing.usage().retained_input_bytes, 0);
                    Ok(artifact)
                },
            )
            .unwrap();
            assert!(runtime
                .read_db(|db| db.thumbnail_cache_entry_for_model(&archived.id))
                .unwrap()
                .is_some());
        }
    }

    #[test]
    fn thumbnail_job_swallowed_renderer_stop_does_not_commit_success() {
        let (_temp, runtime, _context, model, path) = fixture();
        let original = fs::read(&path).unwrap();
        let before = runtime
            .read_db(|db| db.thumbnail_cache_entry_for_model(&model.id))
            .unwrap();
        let settings = runtime.read_db(|db| db.get_catalog_settings()).unwrap();
        let error = render_model_with_renderer(
            &runtime,
            &model,
            &settings,
            true,
            |_, _, _, _, processing| {
                let _ = processing
                    .observer()
                    .stop_io(stl_core::ResourceStop::ReadWorkExceeded);
                Ok(thumbnails::ThumbnailArtifact {
                    relative_path: "thumbnails/unpublished.png".into(),
                    png_hash: "0".repeat(64),
                })
            },
        )
        .unwrap_err();
        assert_eq!(error.code, RuntimeErrorCode::Unsupported);
        assert!(!error.retryable);
        assert_eq!(
            runtime
                .read_db(|db| db.thumbnail_cache_entry_for_model(&model.id))
                .unwrap(),
            before
        );
        assert_eq!(fs::read(path).unwrap(), original);
    }
}
