use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Barrier;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use library_protocol::ContentEngine;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;

#[derive(Default)]
struct FakeEngine {
    slow_probe: Option<Arc<SlowProducerProbe>>,
}

#[test]
fn printer_network_openapi_exposes_the_reviewed_interface() {
    let document = openapi_document();
    for (path, method) in [
        ("/api/v1/printer-network/policy", "get"),
        ("/api/v1/printer-network/discovery-plans", "post"),
        ("/api/v1/printer-network/discovery-jobs", "post"),
        ("/api/v1/printer-network/discovery-jobs/{jobId}", "get"),
        ("/api/v1/printer-network/discovery-jobs/{jobId}", "delete"),
        ("/api/v1/printers/{id}/endpoint-approval", "put"),
        ("/api/v1/printers/{id}/status-checks", "post"),
    ] {
        assert!(
            document["paths"][path][method].is_object(),
            "{method} {path}"
        );
    }
}

#[derive(Default)]
struct FakeHostSlicerAdapter {
    opened: Mutex<Vec<(String, u64, String)>>,
}

impl HostSlicerAdapter for FakeHostSlicerAdapter {
    fn list_slicers(&self, _: &RequestContext) -> Result<Vec<HostSlicerSummary>, EngineError> {
        Ok(vec![HostSlicerSummary {
            id: "com.bambulab.bambu-studio".into(),
            display_name: "Bambu Studio".into(),
        }])
    }

    fn open_in_slicer(
        &self,
        _: &RequestContext,
        model_id: &str,
        expected_version: u64,
        slicer_id: &str,
    ) -> Result<(), EngineError> {
        self.opened.lock().unwrap().push((
            model_id.to_owned(),
            expected_version,
            slicer_id.to_owned(),
        ));
        Ok(())
    }

    fn open_in_slicer_batch(
        &self,
        context: &RequestContext,
        targets: &[VersionedTarget],
        slicer_id: &str,
    ) -> Result<(), EngineError> {
        for target in targets {
            self.open_in_slicer(
                context,
                &target.model_id,
                target.expected_version,
                slicer_id,
            )?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SlowProducerEvent {
    WaitingToStart,
    Started,
    Finished,
}

struct SlowProducerProbe {
    active: AtomicUsize,
    events: tokio::sync::mpsc::UnboundedSender<SlowProducerEvent>,
    start_allowed: Mutex<bool>,
    start_changed: std::sync::Condvar,
}

impl SlowProducerProbe {
    fn release_start(&self) {
        *self.start_allowed.lock().unwrap() = true;
        self.start_changed.notify_all();
    }

    fn start(self: &Arc<Self>) -> ProducerGuard {
        let _ = self.events.send(SlowProducerEvent::WaitingToStart);
        let mut allowed = self.start_allowed.lock().unwrap();
        while !*allowed {
            allowed = self.start_changed.wait(allowed).unwrap();
        }
        self.active.fetch_add(1, Ordering::SeqCst);
        let _ = self.events.send(SlowProducerEvent::Started);
        ProducerGuard(Arc::clone(self))
    }
}

// Release gated blocking workers even if an assertion fails before explicit
// release; otherwise Tokio runtime shutdown could wait forever for the fixture.
struct ReleaseProducerStartOnDrop(Arc<SlowProducerProbe>);
impl Drop for ReleaseProducerStartOnDrop {
    fn drop(&mut self) {
        self.0.release_start();
    }
}

struct ProducerGuard(Arc<SlowProducerProbe>);
impl Drop for ProducerGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        let _ = self.0.events.send(SlowProducerEvent::Finished);
    }
}

fn fake_unsupported<T>() -> Result<T, EngineError> {
    Err(EngineError {
        code: "unsupported",
        message: "unsupported".into(),
        retryable: false,
        current_version: None,
        source_holds: Vec::new(),
    })
}

fn partial_source_action_error(action: &str) -> EngineError {
    EngineError {
        code: "io",
        message: format!("{action} stopped after retaining source bytes"),
        retryable: false,
        current_version: None,
        source_holds: vec![library_protocol::SourceHoldReceipt {
            model_id: "partial-model".into(),
            root_handle_id: "models".into(),
            relative_path: "nested/part.stl".into(),
            hold_name: ".part.stl.hold.data".into(),
            journal_name: ".part.stl.hold.json".into(),
            bytes: 19,
            sha256: "b".repeat(64),
        }],
    }
}
macro_rules! fake_extended_engine {
    () => {
        fn get_model(&self, _: &RequestContext, _: &str) -> Result<ModelSummary, EngineError> {
            fake_unsupported()
        }
        fn catalog_stats(
            &self,
            _: &RequestContext,
        ) -> Result<library_protocol::CatalogStats, EngineError> {
            fake_unsupported()
        }
        fn catalog_facets(
            &self,
            _: &RequestContext,
        ) -> Result<library_protocol::CatalogFacets, EngineError> {
            fake_unsupported()
        }
        fn list_index_root_settings(
            &self,
            _: &RequestContext,
        ) -> Result<Vec<library_protocol::IndexRootSettings>, EngineError> {
            fake_unsupported()
        }
        fn update_index_root_settings(
            &self,
            _: &RequestContext,
            _: &str,
            _: library_protocol::IndexRootSettingsPatch,
        ) -> Result<library_protocol::IndexRootSettings, EngineError> {
            fake_unsupported()
        }
        fn get_catalog_settings(
            &self,
            _: &RequestContext,
        ) -> Result<library_protocol::CatalogSettings, EngineError> {
            fake_unsupported()
        }
        fn update_catalog_settings(
            &self,
            _: &RequestContext,
            _: library_protocol::CatalogSettings,
        ) -> Result<library_protocol::CatalogSettings, EngineError> {
            fake_unsupported()
        }
        fn catalog_hash_status(
            &self,
            _: &RequestContext,
        ) -> Result<library_protocol::HashStatus, EngineError> {
            fake_unsupported()
        }
        fn check_model_sources(
            &self,
            _: &RequestContext,
            _: Vec<String>,
        ) -> Result<Vec<library_protocol::ModelSourceStatus>, EngineError> {
            fake_unsupported()
        }
        fn start_thumbnail_job(
            &self,
            _: &RequestContext,
            _: Vec<String>,
            _: bool,
        ) -> Result<library_protocol::ScanStatus, EngineError> {
            fake_unsupported()
        }
        fn integration_settings(
            &self,
            _: &RequestContext,
        ) -> Result<library_protocol::IntegrationSettings, EngineError> {
            fake_unsupported()
        }
        fn save_spoolman_config(
            &self,
            _: &RequestContext,
            _: Option<library_protocol::IntegrationSpoolmanConfig>,
        ) -> Result<library_protocol::IntegrationSettings, EngineError> {
            fake_unsupported()
        }
        fn pull_spoolman(
            &self,
            _: &RequestContext,
        ) -> Result<library_protocol::IntegrationSpoolmanPullResult, EngineError> {
            fake_unsupported()
        }
        fn save_ai_provider(
            &self,
            _: &RequestContext,
            _: library_protocol::AiProviderConfig,
            _: library_protocol::IntegrationCredentialUpdate,
        ) -> Result<library_protocol::AiProviderConfig, EngineError> {
            fake_unsupported()
        }
        fn delete_ai_provider(&self, _: &RequestContext, _: &str) -> Result<(), EngineError> {
            fake_unsupported()
        }
        fn suggest_ai_tags(
            &self,
            _: &RequestContext,
            _: &str,
            _: Vec<library_protocol::VersionedTarget>,
        ) -> Result<library_protocol::IntegrationAiSuggestionBatch, EngineError> {
            fake_unsupported()
        }
        fn apply_ai_review(
            &self,
            _: &RequestContext,
            _: &str,
            _: Vec<library_protocol::IntegrationAiReviewDecision>,
            _: library_protocol::IntegrationAiReviewStrategy,
        ) -> Result<Vec<library_protocol::MutationVersion>, EngineError> {
            fake_unsupported()
        }
        fn discard_ai_review(&self, _: &RequestContext, _: &str) -> Result<(), EngineError> {
            fake_unsupported()
        }
        fn update_metadata(
            &self,
            _: &RequestContext,
            _: library_protocol::VersionedTarget,
            _: library_protocol::MetadataPatch,
        ) -> Result<library_protocol::MutationVersion, EngineError> {
            fake_unsupported()
        }
        fn bulk_update_metadata(
            &self,
            _: &RequestContext,
            _: Vec<library_protocol::VersionedTarget>,
            _: library_protocol::MetadataPatch,
        ) -> Result<Vec<library_protocol::MutationVersion>, EngineError> {
            fake_unsupported()
        }
        fn list_tags(&self, _: &RequestContext) -> Result<Vec<library_protocol::Tag>, EngineError> {
            fake_unsupported()
        }
        fn create_tag(
            &self,
            _: &RequestContext,
            _: String,
            _: Option<String>,
        ) -> Result<library_protocol::Tag, EngineError> {
            fake_unsupported()
        }
        fn update_tag(
            &self,
            _: &RequestContext,
            _: &str,
            _: String,
            _: Option<String>,
        ) -> Result<library_protocol::Tag, EngineError> {
            fake_unsupported()
        }
        fn delete_tag(&self, _: &RequestContext, _: &str) -> Result<(), EngineError> {
            fake_unsupported()
        }
        fn list_folders(
            &self,
            _: &RequestContext,
        ) -> Result<Vec<library_protocol::Folder>, EngineError> {
            fake_unsupported()
        }
        fn create_folder(
            &self,
            _: &RequestContext,
            _: String,
            _: Option<String>,
        ) -> Result<library_protocol::Folder, EngineError> {
            fake_unsupported()
        }
        fn rename_folder(
            &self,
            _: &RequestContext,
            _: &str,
            _: String,
        ) -> Result<library_protocol::Folder, EngineError> {
            fake_unsupported()
        }
        fn move_folder(
            &self,
            _: &RequestContext,
            _: &str,
            _: &str,
        ) -> Result<library_protocol::Folder, EngineError> {
            fake_unsupported()
        }
        fn delete_folder(&self, _: &RequestContext, _: &str) -> Result<(), EngineError> {
            fake_unsupported()
        }
        fn list_filaments(
            &self,
            _: &RequestContext,
            _: bool,
        ) -> Result<Vec<library_protocol::Filament>, EngineError> {
            fake_unsupported()
        }
        fn create_filament(
            &self,
            _: &RequestContext,
            _: library_protocol::FilamentInput,
        ) -> Result<library_protocol::Filament, EngineError> {
            fake_unsupported()
        }
        fn update_filament(
            &self,
            _: &RequestContext,
            _: &str,
            _: u64,
            _: library_protocol::FilamentInput,
        ) -> Result<library_protocol::Filament, EngineError> {
            fake_unsupported()
        }
        fn delete_filament(
            &self,
            _: &RequestContext,
            _: &str,
            _: u64,
        ) -> Result<Option<library_protocol::Filament>, EngineError> {
            fake_unsupported()
        }
        fn bulk_assign_filaments(
            &self,
            _: &RequestContext,
            _: Vec<library_protocol::ModelFilamentAssignment>,
        ) -> Result<Vec<library_protocol::MutationVersion>, EngineError> {
            fake_unsupported()
        }
        fn list_history(
            &self,
            _: &RequestContext,
            _: &str,
        ) -> Result<Vec<library_protocol::PrintAttempt>, EngineError> {
            fake_unsupported()
        }
        fn create_history_batch(
            &self,
            _: &RequestContext,
            _: &str,
            _: Vec<library_protocol::PrintAttemptBatchItem>,
        ) -> Result<Vec<library_protocol::PrintAttempt>, EngineError> {
            fake_unsupported()
        }
        fn create_history(
            &self,
            _: &RequestContext,
            _: &str,
            _: library_protocol::PrintAttemptInput,
        ) -> Result<library_protocol::PrintAttempt, EngineError> {
            fake_unsupported()
        }
        fn update_history(
            &self,
            _: &RequestContext,
            _: &str,
            _: library_protocol::PrintAttemptInput,
        ) -> Result<library_protocol::PrintAttempt, EngineError> {
            fake_unsupported()
        }
        fn delete_history(&self, _: &RequestContext, _: &str) -> Result<(), EngineError> {
            fake_unsupported()
        }
        fn stage_upload(
            &self,
            _: &RequestContext,
            _: library_protocol::UploadIntent,
            _: Vec<u8>,
        ) -> Result<library_protocol::UploadToken, EngineError> {
            fake_unsupported()
        }
        fn import_capture(
            &self,
            _: &RequestContext,
            _: library_protocol::CaptureEnvelope,
            _: Vec<library_protocol::UploadToken>,
        ) -> Result<library_protocol::CaptureImportResult, EngineError> {
            fake_unsupported()
        }
        fn get_capture(
            &self,
            _: &RequestContext,
            _: &str,
        ) -> Result<library_protocol::CaptureImportResult, EngineError> {
            fake_unsupported()
        }
        fn roots(
            &self,
            _: &RequestContext,
        ) -> Result<Vec<library_protocol::RootHandle>, EngineError> {
            fake_unsupported()
        }
        fn start_scan(
            &self,
            _: &RequestContext,
            _: Option<String>,
        ) -> Result<library_protocol::ScanStatus, EngineError> {
            fake_unsupported()
        }
        fn cancel_scan(
            &self,
            _: &RequestContext,
        ) -> Result<library_protocol::ScanStatus, EngineError> {
            fake_unsupported()
        }
        fn scan_status(
            &self,
            _: &RequestContext,
        ) -> Result<library_protocol::ScanStatus, EngineError> {
            fake_unsupported()
        }
        fn prepare_bundle(
            &self,
            _: &RequestContext,
            _: &str,
        ) -> Result<library_protocol::BundlePreviewSummary, EngineError> {
            fake_unsupported()
        }
        fn describe_bundle_geometry(
            &self,
            _: &RequestContext,
            _: &str,
        ) -> Result<library_protocol::ContentLease, EngineError> {
            fake_unsupported()
        }
        fn stream_bundle_geometry(
            &self,
            _: &RequestContext,
            _: &str,
            _: &mut dyn std::io::Write,
        ) -> Result<(), EngineError> {
            fake_unsupported()
        }
        fn close_bundle(&self, _: &RequestContext, _: &str) -> Result<(), EngineError> {
            fake_unsupported()
        }
        fn issue_bundle_download(
            &self,
            _: &RequestContext,
            _: &str,
        ) -> Result<library_protocol::ContentLease, EngineError> {
            fake_unsupported()
        }
        fn issue_model_geometry(
            &self,
            _: &RequestContext,
            _: &str,
            _: Option<&str>,
            _: Option<u32>,
        ) -> Result<library_protocol::GeometryLease, EngineError> {
            fake_unsupported()
        }
        fn duplicate_groups(
            &self,
            _: &RequestContext,
            _: Vec<String>,
        ) -> Result<Vec<library_protocol::DuplicateGroup>, EngineError> {
            fake_unsupported()
        }
        fn read_embedded_metadata(
            &self,
            _: &RequestContext,
            _: &str,
            _: Option<&str>,
        ) -> Result<library_protocol::EmbeddedMetadataProposal, EngineError> {
            fake_unsupported()
        }
        fn apply_embedded_metadata(
            &self,
            _: &RequestContext,
            _: library_protocol::VersionedTarget,
            _: &str,
            _: library_protocol::EmbeddedMetadataSelection,
        ) -> Result<library_protocol::MutationVersion, EngineError> {
            fake_unsupported()
        }
        fn plan_management_action(
            &self,
            _: &RequestContext,
            _: library_protocol::ManagementAction,
        ) -> Result<library_protocol::ActionPlan, EngineError> {
            fake_unsupported()
        }
        fn execute_management_action(
            &self,
            _: &RequestContext,
            token: &str,
        ) -> Result<library_protocol::ManagementResult, EngineError> {
            match token {
                "partial-move" => Err(partial_source_action_error("Move")),
                "partial-quarantine" => Err(partial_source_action_error("Quarantine")),
                _ => fake_unsupported(),
            }
        }
        fn discard_management_action(
            &self,
            _: &RequestContext,
            token: &str,
        ) -> Result<(), EngineError> {
            if token == "discard-safe" {
                Ok(())
            } else {
                fake_unsupported()
            }
        }
        fn list_captures(
            &self,
            _: &RequestContext,
            _: library_protocol::PageRequest,
        ) -> Result<library_protocol::Page<library_protocol::CaptureSummary>, EngineError> {
            fake_unsupported()
        }
    };
}

impl LibraryEngine for FakeEngine {
    fn get_printer_network_policy(
        &self,
        _: &RequestContext,
    ) -> Result<PrinterNetworkPolicySummary, EngineError> {
        Ok(PrinterNetworkPolicySummary {
            enabled: true,
            discovery_available: true,
            status_checks_available: true,
            multicast_discovery_allowed: false,
            carbon_broadcast_allowed: false,
            max_discovery_addresses: 1022,
            max_suggestions: 33,
            maximum_duration_ms: 30000,
        })
    }
    fn plan_printer_discovery(
        &self,
        _: &RequestContext,
        range: &str,
        include_bambu: bool,
    ) -> Result<PrinterDiscoveryPlan, EngineError> {
        Ok(PrinterDiscoveryPlan {
            plan_token: "synthetic-plan".into(),
            normalized_range: range.into(),
            address_count: 1,
            include_bambu_discovery: include_bambu,
            multicast_disclosure: None,
            expires_at: "2026-10-19T12:00:00Z".into(),
        })
    }
    fn start_printer_discovery(
        &self,
        _: &RequestContext,
        _: &str,
        _: bool,
    ) -> Result<PrinterDiscoveryJob, EngineError> {
        Ok(fake_network_job(
            "synthetic-job",
            library_protocol::PrinterDiscoveryJobStatus::Running,
        ))
    }
    fn get_printer_discovery(
        &self,
        _: &RequestContext,
        id: &str,
    ) -> Result<PrinterDiscoveryJob, EngineError> {
        Ok(fake_network_job(
            id,
            library_protocol::PrinterDiscoveryJobStatus::Completed,
        ))
    }
    fn cancel_printer_discovery(
        &self,
        _: &RequestContext,
        id: &str,
    ) -> Result<PrinterDiscoveryJob, EngineError> {
        Ok(fake_network_job(
            id,
            library_protocol::PrinterDiscoveryJobStatus::Cancelled,
        ))
    }
    fake_extended_engine!();
    fn capabilities(&self, _: &RequestContext) -> Result<HostCapabilities, EngineError> {
        Ok(HostCapabilities {
            api_version: API_VERSION.into(),
            host_mode: HostMode::Headless,
            library_epoch: "7".into(),
            catalog_revision: "11".into(),
            features: FeatureSet {
                reveal_source: false,
                trash_source: false,
                purge_catalog: false,
                cleanup_orphans: false,
                launch_slicer: false,
                download_source: true,
                download_bundle: true,
                ai: true,
                spoolman: true,
                mcp: true,
                capture_inbox: true,
            },
            limits: LimitSet {
                page_size_max: MAX_PAGE_SIZE,
                thumbnail_upload_bytes: 8 * 1024 * 1024,
                model_upload_bytes: 100 * 1024 * 1024,
                active_previews_max: 4,
            },
        })
    }

    fn status(&self, _: &RequestContext) -> Result<RuntimeStatus, EngineError> {
        Ok(RuntimeStatus {
            library_epoch: "7".into(),
            catalog_revision: "11".into(),
            maintenance: false,
            jobs: Vec::new(),
        })
    }

    fn list_models(
        &self,
        _: &RequestContext,
        _: Option<&str>,
        _: u16,
        _: library_protocol::ModelQuery,
    ) -> Result<Page<ModelSummary>, EngineError> {
        Ok(Page {
            items: vec![ModelSummary {
                id: "model-1".into(),
                display_name: "Cube".into(),
                folder_id: "root".into(),
                source_root_id: None,
                bundle_id: None,
                archive_member: false,
                parent_model_id: None,
                created_at: "2026-10-11T00:00:00Z".into(),
                updated_at: "2026-10-11T00:00:00Z".into(),
                last_printed_at: None,
                print_count: "0".into(),
                metadata_version: "3".into(),
                format: Some("stl-binary".into()),
                bytes: "84".into(),
                content_hash: Some("a".repeat(64)),
                tags: vec!["fixture".into()],
                filament_ids: Vec::new(),
                source_kind: "managed".into(),
                availability: "available".into(),
                catalog_kind: "model".into(),
                thumbnail_id: None,
                designer: None,
                source_url: None,
                notes: None,
                model_license: None,
                print_status: "unprinted".into(),
                rating: 0,
                favorite: false,
            }],
            next_cursor: None,
            snapshot_revision: "11".into(),
        })
    }

    fn call_tool(
        &self,
        context: &RequestContext,
        name: &str,
        arguments: Value,
    ) -> Result<Value, EngineError> {
        if name == "execute_management_action" {
            let token = arguments
                .get("token")
                .and_then(Value::as_str)
                .ok_or_else(invalid_tool)?;
            return self
                .execute_management_action(context, token)
                .and_then(|result| serde_json::to_value(result).map_err(|_| internal_tool()));
        }
        Ok(json!({"tool":name,"ok":true}))
    }

    fn supports_tool(&self, _: &RequestContext, _: &str) -> bool {
        true
    }
    fn list_thumbnails(
        &self,
        _: &RequestContext,
        _: &str,
    ) -> Result<Vec<library_protocol::ThumbnailDescriptor>, EngineError> {
        Ok(vec![])
    }
    fn set_active_thumbnail(
        &self,
        _: &RequestContext,
        _: &str,
        _: u64,
        _: &str,
    ) -> Result<u64, EngineError> {
        Ok(2)
    }
    fn add_thumbnail(
        &self,
        _: &RequestContext,
        _: &str,
        _: u64,
        _: String,
        _: Vec<u8>,
    ) -> Result<u64, EngineError> {
        Ok(2)
    }
    fn delete_thumbnail(
        &self,
        _: &RequestContext,
        _: &str,
        _: u64,
        _: &str,
    ) -> Result<u64, EngineError> {
        Ok(2)
    }
    fn issue_thumbnail_media(
        &self,
        _: &RequestContext,
        _: &str,
        _: &str,
    ) -> Result<library_protocol::ContentLease, EngineError> {
        Err(EngineError {
            code: "unsupported",
            message: "unsupported".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })
    }
    fn issue_model_download(
        &self,
        _: &RequestContext,
        _: &str,
        _: Option<&str>,
    ) -> Result<library_protocol::ContentLease, EngineError> {
        Err(EngineError {
            code: "unsupported",
            message: "unsupported".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })
    }
    fn stream_content(
        &self,
        _: &RequestContext,
        lease: &str,
        sink: &mut dyn std::io::Write,
    ) -> Result<(), EngineError> {
        if lease == "slow" {
            let _guard = self.slow_probe.as_ref().map(SlowProducerProbe::start);
            for _ in 0..32 {
                sink.write_all(&vec![7_u8; 64 * 1024])
                    .map_err(|_| EngineError {
                        code: "io",
                        message: "closed".into(),
                        retryable: false,
                        current_version: None,
                        source_holds: Vec::new(),
                    })?;
            }
            return Ok(());
        }
        Err(EngineError {
            code: "unsupported",
            message: "unsupported".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })
    }
    fn describe_content(
        &self,
        _: &RequestContext,
        lease: &str,
    ) -> Result<library_protocol::ContentLease, EngineError> {
        if lease == "slow" {
            return Ok(library_protocol::ContentLease {
                id: "slow".into(),
                kind: library_protocol::ContentKind::Model,
                file_name: "slow.bin".into(),
                mime: "application/octet-stream".into(),
                bytes: 2 * 1024 * 1024,
                sha256: "a".repeat(64),
                library_epoch: 1,
                expires_at: "2026-10-11T00:00:00Z".into(),
            });
        }
        Err(EngineError {
            code: "unsupported",
            message: "unsupported".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })
    }
}

struct BlockingEngine {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}

impl LibraryEngine for BlockingEngine {
    fake_extended_engine!();
    fn capabilities(&self, context: &RequestContext) -> Result<HostCapabilities, EngineError> {
        self.entered.wait();
        self.release.wait();
        FakeEngine::default().capabilities(context)
    }

    fn status(&self, context: &RequestContext) -> Result<RuntimeStatus, EngineError> {
        FakeEngine::default().status(context)
    }

    fn list_models(
        &self,
        context: &RequestContext,
        cursor: Option<&str>,
        limit: u16,
        query: library_protocol::ModelQuery,
    ) -> Result<Page<ModelSummary>, EngineError> {
        FakeEngine::default().list_models(context, cursor, limit, query)
    }

    fn call_tool(
        &self,
        context: &RequestContext,
        name: &str,
        arguments: Value,
    ) -> Result<Value, EngineError> {
        FakeEngine::default().call_tool(context, name, arguments)
    }

    fn supports_tool(&self, context: &RequestContext, name: &str) -> bool {
        FakeEngine::default().supports_tool(context, name)
    }
    fn list_thumbnails(
        &self,
        c: &RequestContext,
        m: &str,
    ) -> Result<Vec<library_protocol::ThumbnailDescriptor>, EngineError> {
        FakeEngine::default().list_thumbnails(c, m)
    }
    fn set_active_thumbnail(
        &self,
        c: &RequestContext,
        m: &str,
        v: u64,
        t: &str,
    ) -> Result<u64, EngineError> {
        FakeEngine::default().set_active_thumbnail(c, m, v, t)
    }
    fn add_thumbnail(
        &self,
        c: &RequestContext,
        m: &str,
        v: u64,
        n: String,
        b: Vec<u8>,
    ) -> Result<u64, EngineError> {
        FakeEngine::default().add_thumbnail(c, m, v, n, b)
    }
    fn delete_thumbnail(
        &self,
        c: &RequestContext,
        m: &str,
        v: u64,
        t: &str,
    ) -> Result<u64, EngineError> {
        FakeEngine::default().delete_thumbnail(c, m, v, t)
    }
    fn issue_thumbnail_media(
        &self,
        c: &RequestContext,
        m: &str,
        t: &str,
    ) -> Result<library_protocol::ContentLease, EngineError> {
        FakeEngine::default().issue_thumbnail_media(c, m, t)
    }
    fn issue_model_download(
        &self,
        c: &RequestContext,
        m: &str,
        h: Option<&str>,
    ) -> Result<library_protocol::ContentLease, EngineError> {
        FakeEngine::default().issue_model_download(c, m, h)
    }
    fn stream_content(
        &self,
        c: &RequestContext,
        l: &str,
        sink: &mut dyn std::io::Write,
    ) -> Result<(), EngineError> {
        FakeEngine::default().stream_content(c, l, sink)
    }
    fn describe_content(
        &self,
        c: &RequestContext,
        l: &str,
    ) -> Result<library_protocol::ContentLease, EngineError> {
        FakeEngine::default().describe_content(c, l)
    }
}

fn test_router() -> axum::Router {
    router(Arc::new(FakeEngine::default()), security(8))
}

fn security(max_in_flight: usize) -> SecurityConfig {
    SecurityConfig {
        bearer_token: BearerSecret::from_string("A".repeat(43)).unwrap(),
        allowed_hosts: BTreeSet::from([AllowedAuthority::parse("127.0.0.1:3745").unwrap()]),
        allowed_origins: BTreeSet::from([AllowedOrigin::parse("http://127.0.0.1:3745").unwrap()]),
        principal_capabilities: BTreeSet::from([
            Capability::CatalogRead,
            Capability::CatalogWrite,
            Capability::MediaRead,
            Capability::MediaWrite,
            Capability::Scan,
        ]),
        max_in_flight,
        max_mcp_sessions: 4,
        mcp_session_ttl: Duration::from_secs(60),
        trusted_proxy_peers: BTreeSet::new(),
        public_base_url: None,
        content_send_timeout: Duration::from_millis(50),
    }
}

fn request(method: &str, uri: &str, authenticated: bool) -> http::request::Builder {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "127.0.0.1:3745");
    if authenticated {
        request = request.header(header::AUTHORIZATION, format!("Bearer {}", "A".repeat(43)));
    }
    request
}

#[tokio::test]
async fn host_origin_and_bearer_are_native_admission_boundaries() {
    let app = test_router();
    let health = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/health", false)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    let bad_host = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .header(header::HOST, "evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_host.status(), StatusCode::FORBIDDEN);
    let missing = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/capabilities", false)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
    let bad_origin = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/capabilities", true)
                .header(header::ORIGIN, "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_origin.status(), StatusCode::FORBIDDEN);
    let allowed = app
        .oneshot(
            request("GET", "/api/v1/capabilities", true)
                .header(header::ORIGIN, "http://127.0.0.1:3745")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
    assert_eq!(allowed.headers()[header::X_FRAME_OPTIONS], "DENY");
    assert_eq!(allowed.headers()[header::CACHE_CONTROL], "no-store");
    let rejected = test_router()
        .oneshot(
            request("GET", "/api/v1/status", false)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.headers()[header::X_FRAME_OPTIONS], "DENY");
    assert_eq!(rejected.headers()[header::CACHE_CONTROL], "no-store");
}

#[tokio::test]
async fn tls_proxy_mode_rejects_direct_or_spoofed_requests_without_trusted_peer_identity() {
    let mut policy = security(8);
    policy.public_base_url = Some(AllowedOrigin::parse("https://nas.example").unwrap());
    policy.trusted_proxy_peers = BTreeSet::from(["10.0.0.2".parse().unwrap()]);
    let response = router(Arc::new(FakeEngine::default()), policy)
        .oneshot(
            request("GET", "/api/v1/capabilities", true)
                .header("x-forwarded-for", "10.0.0.2")
                .header("x-forwarded-proto", "https")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rest_pagination_is_bounded_and_versions_are_strings() {
    let app = test_router();
    let bad = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/models?limit=101", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let response = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/models?limit=10", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["snapshotRevision"], "11");
    assert_eq!(body["items"][0]["metadataVersion"], "3");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_admission_refuses_concurrent_requests_without_queue_growth() {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let app = router(
        Arc::new(BlockingEngine {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }),
        security(1),
    );
    let first_app = app.clone();
    let first = tokio::spawn(async move {
        first_app
            .oneshot(
                request("GET", "/api/v1/capabilities", true)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    let entered_wait = Arc::clone(&entered);
    tokio::task::spawn_blocking(move || entered_wait.wait())
        .await
        .unwrap();
    let refused = app
        .oneshot(
            request("GET", "/api/v1/status", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.headers()[header::RETRY_AFTER], "1");
    let release_wait = Arc::clone(&release);
    tokio::task::spawn_blocking(move || release_wait.wait())
        .await
        .unwrap();
    assert_eq!(first.await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn content_stream_admission_is_held_until_body_drop() {
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let probe = Arc::new(SlowProducerProbe {
        active: AtomicUsize::new(0),
        events,
        start_allowed: Mutex::new(false),
        start_changed: std::sync::Condvar::new(),
    });
    let _release_on_failure = ReleaseProducerStartOnDrop(Arc::clone(&probe));
    let app = router(
        Arc::new(FakeEngine {
            slow_probe: Some(Arc::clone(&probe)),
        }),
        security(8),
    );
    let mut held = Vec::new();
    for _ in 0..4 {
        let response = app
            .clone()
            .oneshot(
                request("GET", "/api/v1/content/slow", true)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        held.push(response);
    }
    let refused = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/content/slow", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    // Deliberately delay every producer after HTTP admission. An active count
    // of zero at this point says nothing about whether streaming has completed.
    tokio::time::timeout(Duration::from_secs(5), async {
        for _ in 0..4 {
            assert_eq!(
                received.recv().await,
                Some(SlowProducerEvent::WaitingToStart)
            );
        }
    })
    .await
    .expect("all four producers should reach the start gate");
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    probe.release_start();
    wait_for_slow_producers(&mut received, 4, 0).await;
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);

    let still_held = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/content/slow", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(still_held.status(), StatusCode::TOO_MANY_REQUESTS);
    drop(held);
    let admitted = app
        .oneshot(
            request("GET", "/api/v1/content/slow", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admitted.status(), StatusCode::OK);
    drop(admitted);
    wait_for_slow_producers(&mut received, 1, 1).await;
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
}

async fn wait_for_slow_producers(
    received: &mut tokio::sync::mpsc::UnboundedReceiver<SlowProducerEvent>,
    expected: usize,
    expected_waiting: usize,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let (mut waiting, mut started, mut finished) = (0, 0, 0);
        while finished < expected {
            match received
                .recv()
                .await
                .expect("producer event channel closed")
            {
                SlowProducerEvent::WaitingToStart => waiting += 1,
                SlowProducerEvent::Started => started += 1,
                SlowProducerEvent::Finished => finished += 1,
            }
        }
        assert_eq!(waiting, expected_waiting);
        assert_eq!(started, expected);
        assert_eq!(finished, expected);
    })
    .await
    .expect("every admitted producer should start and finish");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn large_upload_admission_happens_before_multipart_buffering_and_releases_on_drop() {
    fn stalled_upload(app: axum::Router, authenticated: bool) -> tokio::task::JoinHandle<Response> {
        tokio::spawn(async move {
            app.oneshot(
                request("POST", "/api/v1/captures/uploads", authenticated)
                    .header(
                        header::CONTENT_TYPE,
                        "multipart/form-data; boundary=STALLED",
                    )
                    .body(Body::from_stream(futures_util::stream::pending::<
                        Result<Bytes, std::io::Error>,
                    >()))
                    .unwrap(),
            )
            .await
            .unwrap()
        })
    }

    let app = test_router();
    let unauthenticated = stalled_upload(app.clone(), false);
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(250), unauthenticated)
            .await
            .unwrap()
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let first = stalled_upload(app.clone(), true);
    let second = stalled_upload(app.clone(), true);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let refused = tokio::time::timeout(
        Duration::from_millis(250),
        app.clone().oneshot(
            request("POST", "/api/v1/captures/uploads", true)
                .header(header::CONTENT_TYPE, "multipart/form-data; boundary=THIRD")
                .body(Body::empty())
                .unwrap(),
        ),
    )
    .await
    .expect("third upload must be refused before reading its body")
    .unwrap();
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.headers()[header::RETRY_AFTER], "1");
    first.abort();
    second.abort();
    let _ = first.await;
    let _ = second.await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let released = app
        .oneshot(
            request("POST", "/api/v1/captures/uploads", true)
                .header(header::CONTENT_TYPE, "multipart/form-data; boundary=EMPTY")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(released.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn event_stream_reconnects_without_reset_and_emits_only_real_revision_changes() {
    use futures_util::StreamExt as _;

    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let app = router(
        Arc::new(ProtocolEngineAdapter::new(
            runtime.clone(),
            HostMode::Headless,
        )),
        security(8),
    );
    let initial = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/events", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initial.status(), StatusCode::OK);
    let mut initial_stream = initial.into_body().into_data_stream();
    let initial_event = tokio::time::timeout(Duration::from_millis(250), initial_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let initial_event = String::from_utf8(initial_event.to_vec()).unwrap();
    assert!(initial_event.contains("event: library.reset"));
    let event_id = initial_event
        .lines()
        .find_map(|line| line.strip_prefix("id: "))
        .unwrap()
        .to_owned();
    drop(initial_stream);

    let resumed = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/events", true)
                .header("last-event-id", &event_id)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resumed.status(), StatusCode::OK);
    let mut resumed_stream = resumed.into_body().into_data_stream();
    assert!(
        tokio::time::timeout(Duration::from_millis(250), resumed_stream.next())
            .await
            .is_err()
    );
    runtime
        .mutate_db(|db| db.create_model("Event change", None).map(|_| ()))
        .unwrap();
    let changed = tokio::time::timeout(Duration::from_secs(3), resumed_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8(changed.to_vec())
        .unwrap()
        .contains("event: library.changed"));
}

#[tokio::test]
async fn event_stream_admission_is_bounded_and_released_when_bodies_drop() {
    let app = test_router();
    let mut streams = Vec::new();
    for _ in 0..8 {
        let response = app
            .clone()
            .oneshot(
                request("GET", "/api/v1/events", true)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        streams.push(response);
    }
    let refused = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/events", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    drop(streams);
    let admitted = app
        .oneshot(
            request("GET", "/api/v1/events", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admitted.status(), StatusCode::OK);
}

#[tokio::test]
async fn event_streams_close_when_server_shutdown_is_signalled() {
    use futures_util::StreamExt as _;

    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let app = test_router().layer(axum::Extension(ServerShutdown(receiver)));
    let response = app
        .oneshot(
            request("GET", "/api/v1/events", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    assert!(stream.next().await.is_some());
    shutdown.send(true).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(250), stream.next())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn mcp_2025_11_25_session_lifecycle_and_headers_are_enforced() {
    let app = test_router();
    let initialize = json!({
        "jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":MCP_PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}
    });
    let missing_accept = app
        .clone()
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(initialize.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_accept.status(), StatusCode::NOT_ACCEPTABLE);
    let boolean_id = app
        .clone()
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .body(Body::from(
                    json!({
                        "jsonrpc":"2.0",
                        "id":true,
                        "method":"initialize",
                        "params":{"protocolVersion":MCP_PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(boolean_id.status(), StatusCode::BAD_REQUEST);
    let batch = app
        .clone()
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .body(Body::from(format!("[{initialize}]")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(batch.status(), StatusCode::BAD_REQUEST);
    let initialized = app
        .clone()
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .body(Body::from(initialize.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initialized.status(), StatusCode::OK);
    let session = initialized.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        initialized.headers()["mcp-protocol-version"],
        MCP_PROTOCOL_VERSION
    );
    let initialized_notice = app
        .clone()
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header("mcp-session-id", &session)
                .header("mcp-protocol-version", MCP_PROTOCOL_VERSION)
                .body(Body::from(
                    json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initialized_notice.status(), StatusCode::ACCEPTED);
    let tools = app
        .clone()
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header("mcp-session-id", &session)
                .header("mcp-protocol-version", MCP_PROTOCOL_VERSION)
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tools.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(tools.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    assert!(body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "get_thumbnails"));
    let get = app
        .clone()
        .oneshot(request("GET", "/mcp", true).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::METHOD_NOT_ALLOWED);
    let deleted = app
        .clone()
        .oneshot(
            request("DELETE", "/mcp", true)
                .header("mcp-session-id", &session)
                .header("mcp-protocol-version", MCP_PROTOCOL_VERSION)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let after_delete = app
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header("mcp-session-id", session)
                .header("mcp-protocol-version", MCP_PROTOCOL_VERSION)
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":3,"method":"ping"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(after_delete.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn mcp_source_action_aliases_require_confirm_and_route_to_the_named_tool() {
    let app = test_router();
    let initialize = app
        .clone()
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .body(Body::from(
                    json!({
                        "jsonrpc":"2.0","id":1,"method":"initialize",
                        "params":{"protocolVersion":MCP_PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let session = initialize.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let initialize_notice = app
        .clone()
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header("mcp-session-id", &session)
                .header("mcp-protocol-version", MCP_PROTOCOL_VERSION)
                .body(Body::from(
                    json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initialize_notice.status(), StatusCode::ACCEPTED);

    let listed = app
        .clone()
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header("mcp-session-id", &session)
                .header("mcp-protocol-version", MCP_PROTOCOL_VERSION)
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let listed: Value =
        serde_json::from_slice(&to_bytes(listed.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    for name in ["trash_file", "undo_trash", "move_files"] {
        assert!(listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == name));
    }

    let denied = app
        .clone()
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header("mcp-session-id", &session)
                .header("mcp-protocol-version", MCP_PROTOCOL_VERSION)
                .body(Body::from(
                    json!({
                        "jsonrpc":"2.0","id":3,"method":"tools/call",
                        "params":{"name":"trash_file","arguments":{"modelId":"model-1","expectedVersion":"3","confirm":false}}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let denied: Value =
        serde_json::from_slice(&to_bytes(denied.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    assert_eq!(denied["error"]["code"], -32602);

    for (id, name, arguments) in [
        (
            4,
            "trash_file",
            json!({"modelId":"model-1","expectedVersion":"3","confirm":true}),
        ),
        (
            5,
            "undo_trash",
            json!({"snapshotId":"snapshot-1","confirm":true}),
        ),
        (
            6,
            "move_files",
            json!({"modelId":"model-1","expectedVersion":"3","destinationHandleId":"models","fileName":"model.stl","confirm":true}),
        ),
    ] {
        let called = app
            .clone()
            .oneshot(
                request("POST", "/mcp", true)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .header("mcp-session-id", &session)
                    .header("mcp-protocol-version", MCP_PROTOCOL_VERSION)
                    .body(Body::from(
                        json!({
                            "jsonrpc":"2.0","id":id,"method":"tools/call",
                            "params":{"name":name,"arguments":arguments}
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let called: Value =
            serde_json::from_slice(&to_bytes(called.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(called["result"]["structuredContent"]["tool"], name);
    }
}

#[tokio::test]
async fn partial_source_action_errors_return_typed_recovery_holds_over_rest_and_mcp() {
    let app = test_router();
    let rest = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/management/actions/execute", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"token":"partial-move"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rest.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let rest: Value =
        serde_json::from_slice(&to_bytes(rest.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(rest["error"]["code"], "io");
    assert_eq!(
        rest["error"]["message"],
        "Move stopped after retaining source bytes"
    );
    assert_eq!(rest["error"]["retryable"], false);
    let rest_holds = rest["error"]["sourceHolds"].as_array().unwrap();
    assert_eq!(rest_holds.len(), 1);
    assert_eq!(rest_holds[0]["modelId"], "partial-model");
    assert_eq!(rest_holds[0]["rootHandleId"], "models");
    assert_eq!(rest_holds[0]["relativePath"], "nested/part.stl");
    assert_eq!(rest_holds[0]["holdName"], ".part.stl.hold.data");
    assert_eq!(rest_holds[0]["journalName"], ".part.stl.hold.json");
    assert_eq!(rest_holds[0]["bytes"], "19");
    assert_eq!(rest_holds[0]["sha256"], "b".repeat(64));
    let hold_schema = &openapi_document()["components"]["schemas"]["Error"]["properties"]["error"]
        ["properties"]["sourceHolds"];
    assert_eq!(hold_schema["type"], "array");
    assert!(hold_schema["items"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field == "sha256"));

    let initialized = app
        .clone()
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .body(Body::from(
                    json!({
                        "jsonrpc":"2.0","id":1,"method":"initialize",
                        "params":{"protocolVersion":MCP_PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let session = initialized.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let initialized_notice = app
        .clone()
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header("mcp-session-id", &session)
                .header("mcp-protocol-version", MCP_PROTOCOL_VERSION)
                .body(Body::from(
                    json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initialized_notice.status(), StatusCode::ACCEPTED);

    let mcp = app
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header("mcp-session-id", &session)
                .header("mcp-protocol-version", MCP_PROTOCOL_VERSION)
                .body(Body::from(
                    json!({
                        "jsonrpc":"2.0","id":2,"method":"tools/call",
                        "params":{"name":"execute_management_action","arguments":{"token":"partial-quarantine"}}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let mcp: Value =
        serde_json::from_slice(&to_bytes(mcp.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(mcp["result"]["isError"], true);
    assert_eq!(
        mcp["result"]["content"][0]["text"],
        "Quarantine stopped after retaining source bytes"
    );
    let error = &mcp["result"]["structuredContent"]["error"];
    assert_eq!(error["code"], "io");
    assert_eq!(error["retryable"], false);
    let mcp_holds = error["sourceHolds"].as_array().unwrap();
    assert_eq!(mcp_holds.len(), 1);
    assert_eq!(mcp_holds[0]["modelId"], "partial-model");
    assert_eq!(mcp_holds[0]["rootHandleId"], "models");
    assert_eq!(mcp_holds[0]["relativePath"], "nested/part.stl");
    assert_eq!(mcp_holds[0]["holdName"], ".part.stl.hold.data");
    assert_eq!(mcp_holds[0]["journalName"], ".part.stl.hold.json");
    assert_eq!(mcp_holds[0]["bytes"], "19");
    assert_eq!(mcp_holds[0]["sha256"], "b".repeat(64));
}

#[test]
fn confirmation_tokens_are_one_use_and_bound_to_principal_body_and_epoch() {
    let store = ConfirmationStore::new(Duration::from_secs(60), 4);
    let plan = ConfirmationPlan {
        action: "trash".into(),
        target_ids: vec!["model-1".into()],
        expected_versions: vec![3],
        library_epoch: 7,
        summary: "Move one source to recoverable Trash".into(),
    };
    let token = store.issue("operator", b"request-a", plan.clone()).unwrap();
    assert!(store.consume(&token, "other", b"request-a", 7).is_err());
    let token = store.issue("operator", b"request-a", plan.clone()).unwrap();
    assert_eq!(
        store.consume(&token, "operator", b"request-a", 7).unwrap(),
        plan
    );
    assert!(store.consume(&token, "operator", b"request-a", 7).is_err());
}

#[test]
fn transport_decimal_strings_preserve_values_above_javascript_precision() {
    assert_eq!(
        super::parse_decimal("9007199254740993").unwrap(),
        9_007_199_254_740_993
    );
    for invalid in ["", "01", "+1", "-1", "1e3", " 1", "1 "] {
        assert!(super::parse_decimal(invalid).is_err(), "accepted {invalid}");
    }
    assert_eq!(super::parse_decimal("0").unwrap(), 0);
}

#[test]
fn openapi_advertises_the_connected_catalog_scan_bundle_and_capture_contracts() {
    let document = openapi_document();
    let paths = document["paths"].as_object().unwrap();
    for path in [
        "/api/v1/models/{modelId}",
        "/api/v1/catalog/stats",
        "/api/v1/catalog/facets",
        "/api/v1/catalog/hash-status",
        "/api/v1/models/source-status",
        "/api/v1/thumbnails/jobs",
        "/api/v1/roots",
        "/api/v1/settings/roots",
        "/api/v1/settings/roots/{handleId}",
        "/api/v1/settings/catalog",
        "/api/v1/management/actions/plan",
        "/api/v1/management/actions/execute",
        "/api/v1/integrations/settings",
        "/api/v1/integrations/spoolman",
        "/api/v1/integrations/spoolman/pull",
        "/api/v1/integrations/ai/providers/{providerId}",
        "/api/v1/integrations/ai/suggestions",
        "/api/v1/integrations/ai/reviews/{reviewToken}/apply",
        "/api/v1/integrations/ai/reviews/{reviewToken}",
        "/api/v1/scan",
        "/api/v1/bundles/{bundleId}/preview",
        "/api/v1/bundle-previews/{token}/geometry",
        "/api/v1/bundle-previews/{token}/download-lease",
        "/api/v1/bundle-previews/{token}",
        "/api/v1/models/{modelId}/geometry-lease",
        "/api/v1/duplicates/query",
        "/api/v1/models/{modelId}/embedded-metadata/proposal",
        "/api/v1/models/{modelId}/embedded-metadata/apply",
        "/api/v1/captures",
        "/api/v1/captures/{itemId}/retry",
        "/api/v1/history/bulk",
    ] {
        assert!(paths.contains_key(path), "OpenAPI omitted {path}");
    }
    let parameters = paths["/api/v1/models"]["get"]["parameters"]
        .as_array()
        .unwrap();
    for name in [
        "folderId",
        "sort",
        "direction",
        "tagId",
        "category",
        "designer",
        "modelLicense",
        "printStatus",
        "everPrinted",
        "bundleId",
        "rootId",
        "filamentId",
        "sourceMissing",
        "duplicatesOnly",
        "sourceKind",
        "group",
    ] {
        assert!(
            parameters.iter().any(|parameter| parameter["name"] == name),
            "OpenAPI omitted model filter {name}"
        );
    }
}

#[test]
fn advertised_mutation_tools_have_strict_discoverable_schemas_and_argument_guards() {
    let management = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "plan_management_action")
        .unwrap();
    for unsupported in ["quarantineLooseSource", "undoQuarantine", "moveLooseSource"] {
        assert!(!management["inputSchema"].to_string().contains(unsupported));
    }
    for supported in ["purgeCatalog", "cleanupOrphans"] {
        assert!(management["inputSchema"].to_string().contains(supported));
    }
    assert!(
        serde_json::from_value::<super::WireManagementAction>(json!({
            "kind":"purgeCatalog",
            "backupDestinationHandleId":"backups",
            "backupFileName":"before-purge.zip"
        }))
        .is_err()
    );
    let purge_false_confirmation: super::WireManagementAction = serde_json::from_value(json!({
        "kind":"purgeCatalog",
        "backupDestinationHandleId":"backups",
        "backupFileName":"before-purge.zip",
        "confirm":false
    }))
    .unwrap();
    assert!(purge_false_confirmation.parse().is_err());
    let purge_with_confirmation: super::WireManagementAction = serde_json::from_value(json!({
        "kind":"purgeCatalog",
        "backupDestinationHandleId":"backups",
        "backupFileName":"before-purge.zip",
        "confirm":true
    }))
    .unwrap();
    assert!(matches!(
        purge_with_confirmation.parse().unwrap(),
        library_protocol::ManagementAction::PurgeCatalog { .. }
    ));
    assert!(
        serde_json::from_value::<super::WireManagementAction>(json!({
            "kind":"cleanupOrphans"
        }))
        .is_err()
    );
    let cleanup_false_confirmation: super::WireManagementAction =
        serde_json::from_value(json!({"kind":"cleanupOrphans","confirm":false})).unwrap();
    assert!(cleanup_false_confirmation.parse().is_err());
    let cleanup_with_confirmation: super::WireManagementAction =
        serde_json::from_value(json!({"kind":"cleanupOrphans","confirm":true})).unwrap();
    assert!(matches!(
        cleanup_with_confirmation.parse().unwrap(),
        library_protocol::ManagementAction::CleanupOrphans { .. }
    ));
    for name in [
        "update_model",
        "save_filament",
        "create_part_stock",
        "update_part_stock",
        "delete_part_stock",
        "log_print_event",
        "add_model_tags",
        "remove_model_tags",
        "remove_model",
    ] {
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == name)
            .unwrap();
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert!(tool["inputSchema"]["required"]
            .as_array()
            .is_some_and(|items| !items.is_empty()));
    }
    assert!(super::validate_tool_shape("update_model", &json!({"target":{},"patch":{}})).is_ok());
    assert!(super::validate_tool_shape("update_model", &json!({"target":{}})).is_err());
    let export = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "export_library")
        .unwrap();
    assert_eq!(export["inputSchema"]["additionalProperties"], false);
    assert!(export["description"]
        .as_str()
        .unwrap()
        .contains("without thumbnails"));
    assert!(super::validate_tool_shape("export_library", &json!({})).is_ok());
    assert!(
        super::validate_tool_shape("export_library", &json!({"destPath":"backup.json"})).is_ok()
    );
    assert!(super::validate_tool_shape("export_library", &json!({"destPath":42})).is_err());
    assert!(super::validate_tool_shape(
        "export_library",
        &json!({"destPath":"backup.json","extra":true})
    )
    .is_err());
    assert!(
        super::validate_tool_shape("save_filament", &json!({"input":{},"extra":true})).is_err()
    );
    let stock_input = json!({
        "name":"M3 insert",
        "category":"hardware",
        "quantity":25,
        "unit":"pcs",
        "notes":"brass",
        "lowStock":4
    });
    assert!(super::validate_tool_shape("list_part_stock", &json!({})).is_ok());
    assert!(super::validate_tool_shape("create_part_stock", &json!({"input":stock_input})).is_ok());
    assert!(super::validate_tool_shape(
        "update_part_stock",
        &json!({"partId":"part-1","expectedVersion":"3","input":stock_input})
    )
    .is_ok());
    assert!(super::validate_tool_shape(
        "delete_part_stock",
        &json!({"partId":"part-1","expectedVersion":"3"})
    )
    .is_ok());
    assert!(super::validate_tool_shape(
        "create_part_stock",
        &json!({"input":stock_input,"extra":true})
    )
    .is_err());
    let history_tool = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "log_print_event")
        .unwrap();
    assert_eq!(
        history_tool["inputSchema"]["properties"]["input"]["properties"]["parts"]["maxItems"],
        256
    );
    let event_input = json!({
        "printedAt":"2026-10-15T12:00:00Z",
        "quantity":2,
        "notes":"cancelled after setup",
        "filamentIds":[],
        "outcome":"cancelled",
        "parts":[{"id":"part-1","quantity":3}]
    });
    assert!(super::validate_tool_shape(
        "log_print_event",
        &json!({"modelId":"model-1","input":event_input})
    )
    .is_ok());
    assert!(super::validate_tool_shape(
        "log_print_event",
        &json!({"modelId":"model-1","input":{
            "printedAt":"2026-10-15T12:00:00Z",
            "quantity":2,
            "notes":"duplicate parts",
            "filamentIds":[],
            "outcome":"printed",
            "parts":[{"id":"part-1","quantity":1},{"id":"part-1","quantity":2}]
        }})
    )
    .is_err());
    for name in [
        "get_model",
        "get_hash_status",
        "calculate_missing_hashes",
        "check_files_exist",
        "refresh_hashes",
        "start_thumbnail_job",
        "get_models_missing_thumbnails",
        "get_models_with_default_thumbnails",
        "get_models_by_directory",
        "rename_metadata",
        "delete_metadata",
        "backup_database",
        "integration_get_settings",
        "integration_save_spoolman_config",
        "integration_save_ai_provider",
        "integration_delete_ai_provider",
        "sync_spoolman_filaments",
        "generate_tags",
        "apply_tag_review",
        "discard_tag_review",
        "plan_management_action",
        "execute_management_action",
        "get_index_root_settings",
        "update_index_root_settings",
        "get_catalog_settings",
        "update_catalog_settings",
        "get_library_stats",
        "list_roots",
        "scan_directory",
        "prepare_bundle_preview",
        "get_bundle_preview_geometry",
        "download_bundle",
        "get_model_geometry",
        "close_bundle_preview",
        "find_duplicates",
        "get_embedded_3mf_metadata",
        "pull_3mf_metadata",
        "apply_embedded_3mf_metadata",
        "list_capture_items",
        "retry_capture",
        "log_print_events_batch",
    ] {
        let tool = tool_definitions()
            .into_iter()
            .find(|tool| tool["name"] == name)
            .unwrap();
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    }
    assert!(super::validate_tool_shape(
        "rename_metadata",
        &json!({"type":"designer","oldName":"Old","newName":"New"}),
    )
    .is_ok());
    assert!(super::validate_tool_shape(
        "get_models_by_directory",
        &json!({"rootId":"admitted-root","directory":"parts/nested"}),
    )
    .is_ok());
    assert!(super::validate_tool_shape("get_model", &json!({"modelId":"model-1"})).is_ok());
    assert!(super::validate_tool_shape("get_model", &json!({"modelId":1})).is_err());
    assert!(super::validate_tool_shape("get_model", &json!({"modelId":"m","extra":true})).is_err());
}

#[test]
fn source_file_action_tools_require_confirmation_and_safe_handle_arguments() {
    let definitions = tool_definitions();
    for name in ["trash_file", "undo_trash", "move_files"] {
        let schema = definitions
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap()["inputSchema"]
            .clone();
        assert_eq!(schema["additionalProperties"], false);
        assert!(schema["required"]
            .as_array()
            .is_some_and(|required| required.iter().any(|value| value == "confirm")));
        assert_eq!(schema["properties"]["confirm"]["const"], true);
    }

    assert!(super::validate_tool_shape(
        "trash_file",
        &json!({"modelId":"model-1","expectedVersion":"3","confirm":true}),
    )
    .is_ok());
    assert!(super::validate_tool_shape(
        "trash_file",
        &json!({"modelId":"model-1","expectedVersion":"3"}),
    )
    .is_err());
    assert!(super::validate_tool_shape(
        "trash_file",
        &json!({"modelId":"model-1","expectedVersion":"3","confirm":false}),
    )
    .is_err());
    assert!(super::validate_tool_shape(
        "trash_file",
        &json!({"modelId":"model-1","expectedVersion":"3","confirm":true,"sourcePath":"/tmp/model.stl"}),
    )
    .is_err());
    assert!(super::validate_tool_shape(
        "undo_trash",
        &json!({"snapshotId":"snapshot-1","confirm":true}),
    )
    .is_ok());
    assert!(super::validate_tool_shape(
        "move_files",
        &json!({"modelId":"model-1","expectedVersion":"3","destinationHandleId":"writable-root","fileName":"model-copy.stl","confirm":true}),
    )
    .is_ok());
    assert!(super::validate_tool_shape(
        "move_files",
        &json!({"modelId":"model-1","expectedVersion":"3","destinationHandleId":"writable-root","fileName":"../model.stl","confirm":true}),
    )
    .is_err());
    assert!(super::validate_tool_shape(
        "plan_management_action",
        &json!({"action":{"kind":"quarantineLooseSource","target":{"modelId":"model-1","expectedVersion":"3"}}}),
    )
    .is_err());
}

#[test]
fn rest_physical_source_management_actions_require_confirmation() {
    for action in [
        json!({"kind":"quarantineLooseSource","target":{"modelId":"model-1","expectedVersion":"3"}}),
        json!({"kind":"undoQuarantine","snapshotId":"snapshot-1"}),
        json!({"kind":"moveLooseSource","target":{"modelId":"model-1","expectedVersion":"3"},"destinationHandleId":"root-1","fileName":"moved.stl"}),
    ] {
        assert!(serde_json::from_value::<super::WireManagementAction>(action.clone()).is_err());
        let mut rejected = action.clone();
        rejected["confirm"] = json!(false);
        assert!(
            serde_json::from_value::<super::WireManagementAction>(rejected)
                .unwrap()
                .parse()
                .is_err()
        );
        let mut approved = action;
        approved["confirm"] = json!(true);
        assert!(
            serde_json::from_value::<super::WireManagementAction>(approved)
                .unwrap()
                .parse()
                .is_ok()
        );
    }
}

#[test]
fn bootstrap_secret_is_256_bit_and_persisted_outside_catalog_contract() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let path = root.join("server-token");
    let first = load_or_create_bearer_secret(&path).unwrap();
    let second = load_or_create_bearer_secret(&path).unwrap();
    assert!(!first.matches(&"A".repeat(43)));
    assert_eq!(first.as_bytes(), second.as_bytes());
    assert_eq!(first.as_bytes().len(), 43);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(path).unwrap().mode() & 0o077, 0);
    }
}

#[tokio::test]
async fn protocol_adapter_pages_the_real_runtime_without_paths() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let model = runtime
        .mutate_db(|db| db.create_model("Runtime Cube", None))
        .unwrap();
    let adapter = Arc::new(ProtocolEngineAdapter::new(
        Arc::clone(&runtime),
        HostMode::Headless,
    ));
    let read_context = RequestContext {
        principal_id: "reader".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead]),
        request_id: "read".into(),
    };
    assert!(adapter.supports_tool(&read_context, "search_models"));
    assert!(adapter.supports_tool(&read_context, "get_model"));
    assert!(adapter.supports_tool(&read_context, "get_library_stats"));
    assert!(!adapter.supports_tool(&read_context, "prepare_bundle_preview"));
    assert!(!adapter.supports_tool(&read_context, "update_model"));
    let integration_reader = RequestContext {
        principal_id: "integration-reader".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::Integration]),
        request_id: "integration-read".into(),
    };
    assert!(adapter.supports_tool(&integration_reader, "integration_get_settings"));
    assert!(!adapter.supports_tool(&integration_reader, "generate_tags"));
    assert!(!adapter.supports_tool(&integration_reader, "sync_spoolman_filaments"));
    assert!(!adapter.supports_tool(&integration_reader, "rename_metadata"));
    assert!(!adapter.supports_tool(&integration_reader, "integration_save_spoolman_config"));
    let integration_writer = RequestContext {
        principal_id: "integration-writer".into(),
        capabilities: BTreeSet::from([
            Capability::CatalogRead,
            Capability::CatalogWrite,
            Capability::Integration,
        ]),
        request_id: "integration-write".into(),
    };
    assert!(adapter.supports_tool(&integration_writer, "rename_metadata"));
    assert!(adapter.supports_tool(&integration_writer, "get_models_by_directory"));
    assert!(adapter.supports_tool(&integration_writer, "get_models_with_default_thumbnails"));
    assert!(adapter.supports_tool(&integration_writer, "pull_3mf_metadata"));
    assert!(!adapter.supports_tool(&integration_writer, "backup_database"));
    let maintenance_writer = RequestContext {
        principal_id: "maintenance-writer".into(),
        capabilities: BTreeSet::from([
            Capability::CatalogWrite,
            Capability::Maintenance,
            Capability::PhysicalSourceWrite,
        ]),
        request_id: "maintenance-write".into(),
    };
    assert!(adapter.supports_tool(&maintenance_writer, "backup_database"));
    let backup_only_writer = RequestContext {
        principal_id: "backup-only-writer".into(),
        capabilities: BTreeSet::from([Capability::Maintenance, Capability::BackupWrite]),
        request_id: "backup-only-write".into(),
    };
    assert!(!backup_only_writer
        .capabilities
        .contains(&Capability::PhysicalSourceWrite));
    assert!(adapter.supports_tool(&backup_only_writer, "backup_database"));
    assert!(adapter.supports_tool(&backup_only_writer, "export_library"));
    assert!(adapter.supports_tool(&integration_writer, "integration_save_spoolman_config"));
    assert!(!adapter.supports_tool(&integration_writer, "sync_spoolman_filaments"));
    assert!(!adapter.supports_tool(&integration_writer, "generate_tags"));
    let integration_transport =
        Arc::new(library_runtime::integrations::RuntimeIntegrationTransport::http(false));
    runtime
        .install_shared_state(Arc::clone(&integration_transport))
        .unwrap();
    assert!(
        !adapter
            .capabilities(&integration_writer)
            .unwrap()
            .features
            .ai
    );
    assert!(
        !adapter
            .capabilities(&integration_writer)
            .unwrap()
            .features
            .spoolman
    );
    integration_transport.set_allow_network(true);
    assert!(adapter.supports_tool(&integration_writer, "sync_spoolman_filaments"));
    assert!(adapter.supports_tool(&integration_writer, "generate_tags"));
    let network_features = adapter.capabilities(&integration_writer).unwrap().features;
    assert!(network_features.ai);
    assert!(network_features.spoolman);
    let read_only_network_features = adapter.capabilities(&integration_reader).unwrap().features;
    assert!(read_only_network_features.ai);
    assert!(!read_only_network_features.spoolman);
    integration_transport.set_allow_network(false);
    assert!(!adapter.supports_tool(&integration_writer, "sync_spoolman_filaments"));
    assert!(!adapter.supports_tool(&integration_writer, "generate_tags"));
    let updated = adapter
        .update_metadata(
            &integration_writer,
            VersionedTarget {
                model_id: model.id.clone(),
                expected_version: model.metadata_version,
            },
            MetadataPatch {
                designer: Some("Original designer".into()),
                ..MetadataPatch::default()
            },
        )
        .unwrap();
    let rename = adapter
        .call_tool(
            &integration_writer,
            "rename_metadata",
            json!({"type":"designer","oldName":"Original designer","newName":"Renamed designer"}),
        )
        .unwrap();
    assert_eq!(rename["affectedModels"], "1");
    assert_eq!(
        adapter
            .get_model(&integration_writer, &model.id)
            .unwrap()
            .designer
            .as_deref(),
        Some("Renamed designer")
    );
    let delete = adapter
        .call_tool(
            &integration_writer,
            "delete_metadata",
            json!({"type":"designer","name":"Renamed designer"}),
        )
        .unwrap();
    assert_eq!(delete["affectedModels"], "1");
    assert_eq!(
        adapter
            .get_model(&integration_writer, &model.id)
            .unwrap()
            .designer,
        None
    );
    assert!(adapter
        .call_tool(
            &integration_reader,
            "search_models",
            json!({"rootId":"not-a-live-path","sourceDirectory":"../secret"}),
        )
        .is_err());
    assert!(updated.metadata_version > model.metadata_version);
    let app = router(adapter, security(8));
    let response = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/models?limit=10", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(body["items"][0]["displayName"], "Runtime Cube");
    assert!(body["items"][0]["printCount"].is_string());
    assert!(body["items"][0].get("sourcePath").is_none());
    assert!(body["items"][0].get("storagePath").is_none());
    let model_id = body["items"][0]["id"].as_str().unwrap();
    let detail = app
        .clone()
        .oneshot(
            request("GET", &format!("/api/v1/models/{model_id}"), true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    let detail: Value =
        serde_json::from_slice(&to_bytes(detail.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(detail["id"], model_id);
    assert!(detail.get("sourcePath").is_none());
    let stats = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/catalog/stats", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stats.status(), StatusCode::OK);
    let stats: Value =
        serde_json::from_slice(&to_bytes(stats.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(stats["totalModels"], "1");
    assert!(stats["totalBytes"].is_string());
    let facets = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/catalog/facets", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(facets.status(), StatusCode::OK);
    let catalog_settings = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/settings/catalog", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(catalog_settings.status(), StatusCode::OK);
    let updated_settings = app
        .clone()
        .oneshot(
            request("PUT", "/api/v1/settings/catalog", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"thumbnailSize":128,"thumbnailBackground":"#112233","thumbnailConcurrency":1,"scanBatchSize":64}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated_settings.status(), StatusCode::OK);
    let root_settings = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/settings/roots", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(root_settings.status(), StatusCode::OK);
    let roots = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/roots", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(roots.status(), StatusCode::OK);
    let scan = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/scan", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(scan.status(), StatusCode::OK);
    let started = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/scan", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK);
    let cancelled = app
        .clone()
        .oneshot(
            request("DELETE", "/api/v1/scan", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::OK);
    let planned = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/management/actions/plan", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"kind":"recoverableRemove","targets":[{"modelId":model_id,"expectedVersion":detail["metadataVersion"]}]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(planned.status(), StatusCode::OK);
    let plan: Value =
        serde_json::from_slice(&to_bytes(planned.into_body(), 65536).await.unwrap()).unwrap();
    assert!(plan["libraryEpoch"].is_string());
    assert!(plan["catalogRevision"].is_string());
    assert!(plan["targets"][0]["metadataVersion"].is_string());
    let removed = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/management/actions/execute", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"token":plan["token"]}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::OK);
    let removed: Value =
        serde_json::from_slice(&to_bytes(removed.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(removed["kind"], "removed");
    let replay = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/management/actions/execute", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"token":plan["token"]}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(replay.status(), StatusCode::OK);
    let restore_plan = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/management/actions/plan", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"kind":"restoreRemoval","snapshotId":removed["snapshotId"]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(restore_plan.status(), StatusCode::OK);
    let restore_plan: Value =
        serde_json::from_slice(&to_bytes(restore_plan.into_body(), 65536).await.unwrap()).unwrap();
    let restored = app
        .oneshot(
            request("POST", "/api/v1/management/actions/execute", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"token":restore_plan["token"]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(restored.status(), StatusCode::OK);
}

#[tokio::test]
async fn phase9_parts_rest_mcp_and_history_contracts_adjust_stock_once() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let model = runtime
        .mutate_db(|db| db.create_model("Parts API model", None))
        .unwrap();
    let adapter = Arc::new(ProtocolEngineAdapter::new(
        Arc::clone(&runtime),
        HostMode::Headless,
    ));
    let reader = RequestContext {
        principal_id: "part-reader".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead]),
        request_id: "part-reader".into(),
    };
    let writer = RequestContext {
        principal_id: "part-writer".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
        request_id: "part-writer".into(),
    };
    assert!(adapter.supports_tool(&reader, "list_part_stock"));
    assert!(!adapter.supports_tool(&reader, "create_part_stock"));
    assert!(!adapter.supports_tool(&reader, "update_part_stock"));
    assert!(!adapter.supports_tool(&reader, "delete_part_stock"));
    for name in [
        "create_part_stock",
        "update_part_stock",
        "delete_part_stock",
    ] {
        assert!(adapter.supports_tool(&writer, name));
    }
    assert_eq!(
        adapter
            .call_tool(
                &reader,
                "create_part_stock",
                json!({"input":{"name":"blocked","quantity":1,"unit":"pcs","notes":"","lowStock":0}}),
            )
            .unwrap_err()
            .code,
        "forbidden"
    );
    let created_mcp = adapter
        .call_tool(
            &writer,
            "create_part_stock",
            json!({"input":{"name":"M3 insert","category":"hardware","quantity":5,"unit":"pcs","notes":"brass","lowStock":1}}),
        )
        .unwrap();
    let part_id = created_mcp["id"].as_str().unwrap().to_owned();
    assert_eq!(created_mcp["metadataVersion"], "1");
    assert_eq!(created_mcp["quantity"], "5");
    let updated_mcp = adapter
        .call_tool(
            &writer,
            "update_part_stock",
            json!({"partId":part_id,"expectedVersion":"1","input":{"name":"M3 insert","category":"hardware","quantity":10,"unit":"pcs","notes":"brass","lowStock":2}}),
        )
        .unwrap();
    assert_eq!(updated_mcp["metadataVersion"], "2");
    assert_eq!(updated_mcp["quantity"], "10");
    let disposable_mcp = adapter
        .call_tool(
            &writer,
            "create_part_stock",
            json!({"input":{"name":"M2 nut","quantity":2,"unit":"pcs","notes":"","lowStock":0}}),
        )
        .unwrap();
    let disposable_id = disposable_mcp["id"].as_str().unwrap();
    let disposable_updated = adapter
        .call_tool(
            &writer,
            "update_part_stock",
            json!({"partId":disposable_id,"expectedVersion":"1","input":{"name":"M2 nut","quantity":3,"unit":"pcs","notes":"","lowStock":0}}),
        )
        .unwrap();
    assert_eq!(disposable_updated["metadataVersion"], "2");
    adapter
        .call_tool(
            &writer,
            "delete_part_stock",
            json!({"partId":disposable_id,"expectedVersion":"2"}),
        )
        .unwrap();
    let listed_mcp = adapter
        .call_tool(&reader, "list_part_stock", json!({}))
        .unwrap();
    assert_eq!(listed_mcp[0]["id"], part_id);
    assert_eq!(listed_mcp[0]["metadataVersion"], "2");
    assert_eq!(listed_mcp.as_array().unwrap().len(), 1);

    let app = router(adapter.clone(), security(8));
    let listed = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/parts", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed: Value =
        serde_json::from_slice(&to_bytes(listed.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(listed[0]["id"], part_id);
    assert_eq!(listed[0]["metadataVersion"], "2");

    let updated = app
        .clone()
        .oneshot(
            request("PUT", &format!("/api/v1/parts/{part_id}"), true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"expectedVersion":"2","input":{"name":"M3 insert","category":"hardware","quantity":10,"unit":"pcs","notes":"brass","lowStock":2}}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated: Value =
        serde_json::from_slice(&to_bytes(updated.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(updated["metadataVersion"], "3");
    assert_eq!(updated["quantity"], "10");
    let stale_update = app
        .clone()
        .oneshot(
            request("PUT", &format!("/api/v1/parts/{part_id}"), true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"expectedVersion":"2","input":{"name":"M3 insert","category":"hardware","quantity":9,"unit":"pcs","notes":"brass","lowStock":2}}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale_update.status(), StatusCode::CONFLICT);

    let created_event = app
        .clone()
        .oneshot(
            request("POST", &format!("/api/v1/models/{}/history", model.id), true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"printedAt":"2026-10-15T12:00:00Z","quantity":2,"notes":"cancelled setup","filamentIds":[],"outcome":"cancelled","parts":[{"id":part_id,"quantity":3}]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created_event.status(), StatusCode::OK);
    let created_event: Value = serde_json::from_slice(
        &to_bytes(created_event.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let event_id = created_event["id"].as_str().unwrap().to_owned();
    assert_eq!(created_event["outcome"], "cancelled");
    assert_eq!(created_event["parts"][0]["quantityPerCopy"], 3);
    assert_eq!(created_event["parts"][0]["quantity"], 6);

    let stock_after_create = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/parts", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let stock_after_create: Value = serde_json::from_slice(
        &to_bytes(stock_after_create.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(stock_after_create[0]["quantity"], "4");

    let updated_event = app
        .clone()
        .oneshot(
            request("PUT", &format!("/api/v1/history/{event_id}"), true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"printedAt":"2026-10-15T12:00:00Z","quantity":1,"notes":"corrected copies","filamentIds":[],"outcome":"cancelled"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated_event.status(), StatusCode::OK);
    let updated_event: Value = serde_json::from_slice(
        &to_bytes(updated_event.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(updated_event["parts"][0]["quantity"], 3);
    let stock_after_update: Value = serde_json::from_slice(
        &to_bytes(
            app.clone()
                .oneshot(
                    request("GET", "/api/v1/parts", true)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .into_body(),
            64 * 1024,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(stock_after_update[0]["quantity"], "7");

    let batch_payload = json!({
        "idempotencyKey":"parts-http-replay",
        "items":[{"modelId":model.id,"input":{
            "printedAt":"2026-10-15T13:00:00Z",
            "quantity":2,
            "notes":"batch",
            "filamentIds":[],
            "outcome":"failed",
            "parts":[{"id":part_id,"quantity":1}]
        }}]
    });
    let first_batch = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/history/bulk", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(batch_payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_batch.status(), StatusCode::OK);
    let first_batch: Value =
        serde_json::from_slice(&to_bytes(first_batch.into_body(), 64 * 1024).await.unwrap())
            .unwrap();
    let batch_event_id = first_batch[0]["id"].as_str().unwrap().to_owned();
    let replay = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/history/bulk", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(batch_payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let replay: Value =
        serde_json::from_slice(&to_bytes(replay.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(replay, first_batch);
    let stock_after_replay: Value = serde_json::from_slice(
        &to_bytes(
            app.clone()
                .oneshot(
                    request("GET", "/api/v1/parts", true)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .into_body(),
            64 * 1024,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(stock_after_replay[0]["quantity"], "5");

    for attempt_id in [&event_id, &batch_event_id] {
        let deleted = app
            .clone()
            .oneshot(
                request("DELETE", &format!("/api/v1/history/{attempt_id}"), true)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    }
    let stock_after_delete: Value = serde_json::from_slice(
        &to_bytes(
            app.clone()
                .oneshot(
                    request("GET", "/api/v1/parts", true)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .into_body(),
            64 * 1024,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(stock_after_delete[0]["quantity"], "10");
    let current_stock_version = stock_after_delete[0]["metadataVersion"]
        .as_str()
        .unwrap()
        .to_owned();
    let deleted_stock = app
        .clone()
        .oneshot(
            request("DELETE", &format!("/api/v1/parts/{part_id}"), true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"expectedVersion":"1"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted_stock.status(), StatusCode::CONFLICT);
    let deleted_stock = app
        .clone()
        .oneshot(
            request("DELETE", &format!("/api/v1/parts/{part_id}"), true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"expectedVersion":current_stock_version}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted_stock.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn opaque_root_handle_and_admitted_catalog_id_both_start_the_same_scan() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("library");
    let integration_settings = temp.path().join("integrations.json");
    let source_root = temp.path().join("models");
    std::fs::create_dir(&source_root).unwrap();
    let source = source_root.join("fixture.stl");
    let bytes=b"solid s\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid s\n";
    std::fs::write(&source, bytes).unwrap();
    let before_hash = format!(
        "{:x}",
        sha2::Sha256::digest(std::fs::read(&source).unwrap())
    );
    let bootstrap = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        library.clone(),
        integration_settings.clone(),
    ))
    .unwrap();
    let catalog_root = bootstrap
        .mutate_db(|db| db.add_index_root(&source_root, 60, 50 * 1024 * 1024))
        .unwrap();
    drop(bootstrap);
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig {
        library_root: library,
        integration_settings_path: integration_settings,
        roots: vec![library_runtime::RuntimeRootPolicy {
            id: "models".into(),
            catalog_root_id: Some(catalog_root.id.clone()),
            label: "Models".into(),
            path: source_root,
            access: library_protocol::RootAccess::ReadOnly,
        }],
        capabilities: BTreeSet::from([
            Capability::CatalogRead,
            Capability::CatalogWrite,
            Capability::MediaRead,
            Capability::MediaWrite,
            Capability::Scan,
        ]),
        local_catalog_root_access: false,
    })
    .unwrap();
    let runtime_for_scheduler = Arc::clone(&runtime);
    let engine = Arc::new(ProtocolEngineAdapter::new(runtime, HostMode::Headless));
    let app = router(Arc::clone(&engine), security(8));
    let started = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/scan", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"rootId":"models"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK);
    for _ in 0..100 {
        let status = app
            .clone()
            .oneshot(
                request("GET", "/api/v1/scan", true)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status: Value =
            serde_json::from_slice(&to_bytes(status.into_body(), 65536).await.unwrap()).unwrap();
        if status["running"] == false {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let models = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/models?limit=10", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let models: Value =
        serde_json::from_slice(&to_bytes(models.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(models["items"][0]["displayName"], "fixture");
    let indexed_model_id = models["items"][0]["id"].as_str().unwrap().to_owned();
    let search_context = RequestContext {
        principal_id: "root-search-reader".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead]),
        request_id: "root-search".into(),
    };
    for root_id in ["models", catalog_root.id.as_str()] {
        let page = engine
            .call_tool(
                &search_context,
                "search_models",
                json!({"rootId":root_id,"directory":"","limit":10}),
            )
            .unwrap();
        assert_eq!(page["items"][0]["id"], indexed_model_id);
    }
    let hash_status = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/catalog/hash-status", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hash_status.status(), StatusCode::OK);
    let hash_status: Value =
        serde_json::from_slice(&to_bytes(hash_status.into_body(), 65536).await.unwrap()).unwrap();
    assert!(hash_status["hashedCount"].is_string());
    let source_status = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/models/source-status", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"modelIds":[indexed_model_id]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(source_status.status(), StatusCode::OK);
    let source_status: Value =
        serde_json::from_slice(&to_bytes(source_status.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(source_status[0]["state"], "available");
    let thumbnail_job = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/thumbnails/jobs", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"modelIds":[indexed_model_id],"force":false}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(thumbnail_job.status(), StatusCode::OK);
    let catalog_scan = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/scan", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"rootId":catalog_root.id}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(catalog_scan.status(), StatusCode::OK);
    for _ in 0..100 {
        let status = app
            .clone()
            .oneshot(
                request("GET", "/api/v1/scan", true)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status: Value =
            serde_json::from_slice(&to_bytes(status.into_body(), 65536).await.unwrap()).unwrap();
        if status["running"] == false {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let startup_source = temp.path().join("models").join("startup.stl");
    std::fs::write(&startup_source, bytes).unwrap();
    let startup_before = format!(
        "{:x}",
        sha2::Sha256::digest(std::fs::read(&startup_source).unwrap())
    );
    library_runtime::scanner::start_background_scanner(&runtime_for_scheduler);
    let mut startup_indexed = false;
    for _ in 0..150 {
        let models = app
            .clone()
            .oneshot(
                request("GET", "/api/v1/models?limit=10", true)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let models: Value =
            serde_json::from_slice(&to_bytes(models.into_body(), 65536).await.unwrap()).unwrap();
        if models["items"]
            .as_array()
            .is_some_and(|items| items.len() == 2)
        {
            startup_indexed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        startup_indexed,
        "startup background scan did not index the new model"
    );
    let after_hash = format!(
        "{:x}",
        sha2::Sha256::digest(std::fs::read(&source).unwrap())
    );
    assert_eq!(before_hash, after_hash);
    let startup_after = format!(
        "{:x}",
        sha2::Sha256::digest(std::fs::read(&startup_source).unwrap())
    );
    assert_eq!(startup_before, startup_after);
}

#[tokio::test]
async fn static_browser_assets_are_loopback_host_guarded_without_embedding_the_token() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("index.html"), "<main>GoggleLab</main>").unwrap();
    let app =
        router_with_static(Arc::new(FakeEngine::default()), security(8), temp.path()).unwrap();
    let response = app
        .oneshot(request("GET", "/", false).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("default-src 'self'"));
    assert!(response.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("img-src 'self' data: blob:"));
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(!String::from_utf8(body.to_vec())
        .unwrap()
        .contains(&"A".repeat(43)));
}

#[cfg(unix)]
#[tokio::test]
async fn static_browser_assets_never_follow_symlinks_or_accept_broad_private_roots() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let web = temp.path().join("web");
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(web.join("index.html"), "<main>Safe</main>").unwrap();
    let secret = temp.path().join("server-token");
    std::fs::write(&secret, "do-not-serve").unwrap();
    symlink(&secret, web.join("assets/key.js")).unwrap();
    let app = router_with_static(Arc::new(FakeEngine::default()), security(8), &web).unwrap();
    let response = app
        .oneshot(
            request("GET", "/assets/key.js", false)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    std::fs::write(temp.path().join("index.html"), "<main>Too broad</main>").unwrap();
    let error = router_with_static(Arc::new(FakeEngine::default()), security(8), temp.path())
        .expect_err("a web root containing a secret must be rejected");
    assert!(!error.contains(temp.path().to_string_lossy().as_ref()));
    assert!(!error.contains("server-token"));
}

#[tokio::test]
async fn real_runtime_download_and_png_jpeg_gallery_routes_are_versioned() {
    use image::ImageFormat;
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cube.stl");
    let stl=b"solid c\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid c\n";
    std::fs::write(&source, stl).unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let model = runtime
        .mutate_db(|db| db.import_file(&source, None, None).map(|v| v.model))
        .unwrap();
    let app = router(
        Arc::new(ProtocolEngineAdapter::new(
            runtime.clone(),
            HostMode::Headless,
        )),
        security(8),
    );
    let bad_hash = app
        .clone()
        .oneshot(
            request(
                "POST",
                &format!("/api/v1/models/{}/download-lease", model.id),
                true,
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"expectedHash":"0".repeat(64)}).to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_hash.status(), StatusCode::CONFLICT);
    let lease_response = app
        .clone()
        .oneshot(
            request(
                "POST",
                &format!("/api/v1/models/{}/download-lease", model.id),
                true,
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"expectedHash":model.content_hash}).to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(lease_response.status(), StatusCode::OK);
    let lease: Value =
        serde_json::from_slice(&to_bytes(lease_response.into_body(), 65536).await.unwrap())
            .unwrap();
    assert_eq!(lease["kind"], "model");
    let other = RequestContext {
        principal_id: "other".into(),
        capabilities: BTreeSet::from([Capability::MediaRead]),
        request_id: "other-request".into(),
    };
    assert!(library_protocol::ContentEngine::describe_content(
        &*runtime,
        &other,
        lease["id"].as_str().unwrap()
    )
    .is_err());
    let content = app
        .clone()
        .oneshot(
            request(
                "GET",
                &format!("/api/v1/content/{}", lease["id"].as_str().unwrap()),
                true,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        to_bytes(content.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .as_ref(),
        stl
    );
    let owned_source = runtime
        .host_library_root()
        .join(model.storage_path.as_ref().unwrap());
    std::fs::write(&owned_source, b"tampered").unwrap();
    let tampered = app
        .clone()
        .oneshot(
            request(
                "GET",
                &format!("/api/v1/content/{}", lease["id"].as_str().unwrap()),
                true,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tampered.status(), StatusCode::CONFLICT);
    std::fs::write(&owned_source, stl).unwrap();
    let maintenance = runtime.begin_maintenance().unwrap();
    maintenance.invalidate();
    drop(maintenance);
    let stale_lease = app
        .clone()
        .oneshot(
            request(
                "GET",
                &format!("/api/v1/content/{}", lease["id"].as_str().unwrap()),
                true,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale_lease.status(), StatusCode::CONFLICT);

    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 2)
        .write_to(&mut png, ImageFormat::Png)
        .unwrap();
    let boundary = "BOUNDARY";
    let mut multipart=format!("--{boundary}\r\nContent-Disposition: form-data; name=\"expectedVersion\"\r\n\r\n{}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"preview.png\"\r\nContent-Type: image/png\r\n\r\n",model.metadata_version).into_bytes();
    multipart.extend_from_slice(png.get_ref());
    multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let added = app
        .clone()
        .oneshot(
            request(
                "POST",
                &format!("/api/v1/models/{}/thumbnails", model.id),
                true,
            )
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(multipart))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(added.status(), StatusCode::OK);
    let added: Value =
        serde_json::from_slice(&to_bytes(added.into_body(), 65536).await.unwrap()).unwrap();
    let mut jpeg = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut jpeg, ImageFormat::Jpeg)
        .unwrap();
    let mut multipart = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"expectedVersion\"\r\n\r\n{}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"preview.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n", added["metadataVersion"].as_str().unwrap()).into_bytes();
    multipart.extend_from_slice(jpeg.get_ref());
    multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let jpeg_added = app
        .clone()
        .oneshot(
            request(
                "POST",
                &format!("/api/v1/models/{}/thumbnails", model.id),
                true,
            )
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(multipart))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(jpeg_added.status(), StatusCode::OK);
    let gallery = app
        .clone()
        .oneshot(
            request(
                "GET",
                &format!("/api/v1/models/{}/thumbnails", model.id),
                true,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    let gallery: Value =
        serde_json::from_slice(&to_bytes(gallery.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(gallery.as_array().unwrap().len(), 2);
    assert_eq!(gallery[0]["mime"], "image/png");
    let stale = app
        .oneshot(
            request(
                "PATCH",
                &format!(
                    "/api/v1/models/{}/thumbnails/{}/active",
                    model.id,
                    gallery[0]["thumbnailId"].as_str().unwrap()
                ),
                true,
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"expectedVersion":"1"}).to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn real_runtime_metadata_catalog_history_and_capture_routes_work() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let model = runtime
        .mutate_db(|db| db.create_model("Bookmark", None))
        .unwrap();
    let adapter = Arc::new(ProtocolEngineAdapter::new(runtime, HostMode::Headless));
    let app = router(adapter.clone(), security(8));
    let patch=app.clone().oneshot(request("PATCH",&format!("/api/v1/models/{}/metadata",model.id),true).header(header::CONTENT_TYPE,"application/json").body(Body::from(json!({"expectedVersion":model.metadata_version.to_string(),"patch":{"notes":"remote note","rating":5,"favorite":true}}).to_string())).unwrap()).await.unwrap();
    assert_eq!(patch.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(
            request(
                "GET",
                "/api/v1/models?limit=10&minimumRating=5&favorite=true",
                true,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(body["items"][0]["rating"], 5);
    assert_eq!(body["items"][0]["favorite"], true);
    let tag = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/tags", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"name":"fixture","category":"type"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tag.status(), StatusCode::OK);
    let folder = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/folders", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"name":"Remote","parentId":"root"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(folder.status(), StatusCode::OK);
    let filament=app.clone().oneshot(request("POST","/api/v1/filaments",true).header(header::CONTENT_TYPE,"application/json").body(Body::from(json!({"brand":"Acme","material":"PLA","color":"#ffffff","name":"White","notes":""}).to_string())).unwrap()).await.unwrap();
    assert_eq!(filament.status(), StatusCode::OK);
    let history=app.clone().oneshot(request("POST",&format!("/api/v1/models/{}/history",model.id),true).header(header::CONTENT_TYPE,"application/json").body(Body::from(json!({"printedAt":"2026-10-11T00:00:00Z","quantity":1,"notes":"ok","filamentIds":[],"outcome":"printed"}).to_string())).unwrap()).await.unwrap();
    assert_eq!(history.status(), StatusCode::OK);
    let batch_body = json!({"idempotencyKey":"fixture-batch-1","items":[{"modelId":model.id,"input":{"printedAt":"2026-10-11T01:00:00Z","quantity":1,"notes":"batch","filamentIds":[],"outcome":"printed"}}]}).to_string();
    let batch = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/history/bulk", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(batch_body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(batch.status(), StatusCode::OK);
    let first_batch: Value =
        serde_json::from_slice(&to_bytes(batch.into_body(), 65536).await.unwrap()).unwrap();
    let replay = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/history/bulk", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(batch_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let replay: Value =
        serde_json::from_slice(&to_bytes(replay.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(first_batch[0]["id"], replay[0]["id"]);
    let conflict = app.clone().oneshot(request("POST","/api/v1/history/bulk",true).header(header::CONTENT_TYPE,"application/json").body(Body::from(json!({"idempotencyKey":"fixture-batch-1","items":[{"modelId":model.id,"input":{"printedAt":"2026-10-11T01:00:00Z","quantity":2,"notes":"changed","filamentIds":[],"outcome":"printed"}}]}).to_string())).unwrap()).await.unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let stl=b"solid q\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid q\n";
    let hash = format!("{:x}", sha2::Sha256::digest(stl));
    let intent = json!({"itemId":"item-1","captureId":"part-1","fileName":"part.stl","bytes":stl.len().to_string(),"sha256":hash});
    let boundary = "CAPTURE";
    let mut body=format!("--{boundary}\r\nContent-Disposition: form-data; name=\"intent\"\r\n\r\n{}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"part.stl\"\r\nContent-Type: application/octet-stream\r\n\r\n",intent).into_bytes();
    body.extend_from_slice(stl);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let upload = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/captures/uploads", true)
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(upload.status(), StatusCode::OK);
    let token: Value =
        serde_json::from_slice(&to_bytes(upload.into_body(), 65536).await.unwrap()).unwrap();
    let envelope = json!({"version":1,"itemId":"item-1","createdAt":"2026-10-11T00:00:00Z","page":{"url":"https://www.printables.com/model/1","title":"Captured","designer":"Maker","license":null,"notes":null,"parentExternalId":null},"downloads":[{"captureId":"part-1","fileName":"part.stl","relativeInboxPath":null,"sha256":hash,"bytes":stl.len()}]});
    let import_body = json!({"envelope":envelope.clone(),"uploads":[]}).to_string();
    let retry_body = json!({"envelope":envelope,"uploads":[token]}).to_string();
    let imported = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/captures/import", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(import_body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(imported.status(), StatusCode::OK);
    let retried = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/captures/item-1/retry", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(retry_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retried.status(), StatusCode::OK);
    let captures = app
        .oneshot(
            request("GET", "/api/v1/captures?limit=10", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(captures.status(), StatusCode::OK);
    let captures: Value =
        serde_json::from_slice(&to_bytes(captures.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(captures["items"].as_array().unwrap().len(), 1);
    assert!(captures["snapshotRevision"].is_string());
}

#[tokio::test]
async fn real_runtime_bundle_preview_geometry_stream_and_close_work() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        root.join("library"),
        root.join("integrations.json"),
    ))
    .unwrap();
    let bytes=b"solid b\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid b\n";
    let mut ids = Vec::new();
    for index in 0..2 {
        let path = root.join(format!("{index}.stl"));
        std::fs::write(&path, bytes).unwrap();
        ids.push(
            runtime
                .mutate_db(|db| db.import_file(&path, None, None).map(|v| v.model.id))
                .unwrap(),
        );
    }
    let bundle = runtime.mutate_db(|db| db.create_bundle(&ids)).unwrap();
    let adapter = Arc::new(ProtocolEngineAdapter::new(runtime, HostMode::Headless));
    let app = router(adapter.clone(), security(8));
    let listed = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/models?group=bundle&limit=10", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed: Value =
        serde_json::from_slice(&to_bytes(listed.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(listed["items"][0]["bundleId"], bundle);
    assert!(listed["items"][0]["sourceRootId"].is_null());
    assert_eq!(listed["items"][0]["archiveMember"], false);
    let media_context = RequestContext {
        principal_id: "fixture-mcp".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::MediaRead]),
        request_id: "fixture-mcp-request".into(),
    };
    assert!(
        adapter
            .capabilities(&media_context)
            .unwrap()
            .features
            .download_bundle
    );
    let mcp_summary = adapter
        .call_tool(
            &media_context,
            "prepare_bundle_preview",
            json!({"bundleId":bundle}),
        )
        .unwrap();
    let mcp_token = mcp_summary["token"].as_str().unwrap();
    let mcp_geometry = adapter
        .call_tool(
            &media_context,
            "get_bundle_preview_geometry",
            json!({"token":mcp_token}),
        )
        .unwrap();
    assert_eq!(mcp_geometry["lease"]["kind"], "geometry");
    assert_eq!(
        mcp_geometry["contentPath"],
        format!("/api/v1/bundle-previews/{mcp_token}/geometry")
    );
    adapter
        .call_tool(
            &media_context,
            "close_bundle_preview",
            json!({"token":mcp_token}),
        )
        .unwrap();
    let prepared = app
        .clone()
        .oneshot(
            request("POST", &format!("/api/v1/bundles/{bundle}/preview"), true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(prepared.status(), StatusCode::OK);
    let summary: Value =
        serde_json::from_slice(&to_bytes(prepared.into_body(), 65536).await.unwrap()).unwrap();
    assert!(summary["geometryBytes"].is_string());
    let token = summary["token"].as_str().unwrap();
    let geometry = app
        .clone()
        .oneshot(
            request(
                "GET",
                &format!("/api/v1/bundle-previews/{token}/geometry"),
                true,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(geometry.status(), StatusCode::OK);
    assert_eq!(
        geometry.headers()[header::CONTENT_TYPE],
        "application/vnd.gogglelab.geometry"
    );
    assert!(!to_bytes(geometry.into_body(), 128 * 1024 * 1024)
        .await
        .unwrap()
        .is_empty());
    let bundle_lease = app
        .clone()
        .oneshot(
            request(
                "POST",
                &format!("/api/v1/bundle-previews/{token}/download-lease"),
                true,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bundle_lease.status(), StatusCode::OK);
    let bundle_lease: Value =
        serde_json::from_slice(&to_bytes(bundle_lease.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(bundle_lease["kind"], "bundleArchive");
    let bundle_bytes = app
        .clone()
        .oneshot(
            request(
                "GET",
                &format!("/api/v1/content/{}", bundle_lease["id"].as_str().unwrap()),
                true,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bundle_bytes.status(), StatusCode::OK);
    assert!(to_bytes(bundle_bytes.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap()
        .starts_with(b"PK"));
    let model_geometry = app
        .clone()
        .oneshot(
            request(
                "POST",
                &format!("/api/v1/models/{}/geometry-lease", ids[0]),
                true,
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(model_geometry.status(), StatusCode::OK);
    let model_geometry: Value = serde_json::from_slice(
        &to_bytes(model_geometry.into_body(), 128 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(model_geometry["packetFormat"], "stl2");
    assert_eq!(model_geometry["packetVersion"], 2);
    assert_eq!(model_geometry["content"]["kind"], "geometry");
    let duplicates = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/duplicates/query", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"modelIds":ids}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicates.status(), StatusCode::OK);
    let duplicates: Value =
        serde_json::from_slice(&to_bytes(duplicates.into_body(), 128 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(duplicates[0]["models"].as_array().unwrap().len(), 2);
    let closed = app
        .oneshot(
            request("DELETE", &format!("/api/v1/bundle-previews/{token}"), true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(closed.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn pull_3mf_metadata_returns_bounded_path_free_batch_proposals() {
    use base64::Engine as _;

    const MODEL_3MF: &str = "UEsDBBQAAAAAAK19S12dfITk7wEAAO8BAAAQAAAAM0QvM2Rtb2RlbC5tb2RlbDw/eG1sIHZlcnNpb249IjEuMCIgZW5jb2Rpbmc9IlVURi04Ij8+PG1vZGVsIHVuaXQ9Im1pbGxpbWV0ZXIiIHhtbG5zPSJodHRwOi8vc2NoZW1hcy5taWNyb3NvZnQuY29tLzNkbWFudWZhY3R1cmluZy9jb3JlLzIwMTUvMDIiPjxtZXRhZGF0YSBuYW1lPSJEZXNpZ25lciI+RW1iZWRkZWQgRGVzaWduZXI8L21ldGFkYXRhPjxtZXRhZGF0YSBuYW1lPSJMaWNlbnNlIj5DQzA8L21ldGFkYXRhPjxyZXNvdXJjZXM+PG9iamVjdCBpZD0iMSIgdHlwZT0ibW9kZWwiPjxtZXNoPjx2ZXJ0aWNlcz48dmVydGV4IHg9IjAiIHk9IjAiIHo9IjAiLz48dmVydGV4IHg9IjEiIHk9IjAiIHo9IjAiLz48dmVydGV4IHg9IjAiIHk9IjEiIHo9IjAiLz48L3ZlcnRpY2VzPjx0cmlhbmdsZXM+PHRyaWFuZ2xlIHYxPSIwIiB2Mj0iMSIgdjM9IjIiLz48L3RyaWFuZ2xlcz48L21lc2g+PC9vYmplY3Q+PC9yZXNvdXJjZXM+PGJ1aWxkPjxpdGVtIG9iamVjdGlkPSIxIi8+PC9idWlsZD48L21vZGVsPlBLAQIUAxQAAAAAAK19S12dfITk7wEAAO8BAAAQAAAAAAAAAAAAAACAAQAAAAAzRC8zZG1vZGVsLm1vZGVsUEsFBgAAAAABAAEAPgAAAB0CAAAAAA==";
    let temp = tempfile::tempdir().unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(MODEL_3MF)
        .unwrap();
    let first_source = temp.path().join("first.3mf");
    let second_source = temp.path().join("second.3mf");
    std::fs::write(&first_source, &bytes).unwrap();
    std::fs::write(&second_source, &bytes).unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let (first, second) = runtime
        .mutate_db(|db| {
            Ok((
                db.import_file(&first_source, None, None)?.model,
                db.import_file(&second_source, None, None)?.model,
            ))
        })
        .unwrap();
    let adapter = ProtocolEngineAdapter::new(runtime, HostMode::Headless);
    let context = RequestContext {
        principal_id: "metadata-reader".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead]),
        request_id: "pull-3mf-metadata-batch".into(),
    };

    let batch = adapter
        .call_tool(
            &context,
            "pull_3mf_metadata",
            json!({"targets":[
                {"modelId":first.id,"expectedHash":first.content_hash},
                {"modelId":second.id,"expectedHash":second.content_hash}
            ]}),
        )
        .unwrap();

    assert_eq!(batch["items"].as_array().unwrap().len(), 2);
    assert_eq!(batch["items"][0]["modelId"], first.id);
    assert_eq!(batch["items"][1]["modelId"], second.id);
    assert_eq!(
        batch["items"][0]["embedded"]["designer"],
        "Embedded Designer"
    );
    assert!(adapter
        .call_tool(
            &context,
            "pull_3mf_metadata",
            json!({"targets":[
                {"modelId":first.id},
                {"modelId":first.id}
            ]}),
        )
        .is_err());
    assert_eq!(std::fs::read(&first_source).unwrap(), bytes);
    assert_eq!(std::fs::read(&second_source).unwrap(), bytes);
}

#[tokio::test]
async fn real_runtime_embedded_3mf_proposal_and_versioned_apply_work() {
    use std::io::Write as _;

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("metadata.3mf");
    let model_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
<metadata name="Designer">Embedded Designer</metadata>
<metadata name="ParentModel">Parent Family</metadata>
<resources><object id="1" type="model"><mesh><vertices>
<vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="0"/>
</vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources>
<build><item objectid="1"/></build></model>"#;
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    archive
        .start_file("3D/3dmodel.model", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(model_xml.as_bytes()).unwrap();
    std::fs::write(&source, archive.finish().unwrap().into_inner()).unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let parent = runtime
        .mutate_db(|db| db.create_model("Parent Family", None))
        .unwrap();
    let model = runtime
        .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
        .unwrap();
    let app = router(
        Arc::new(ProtocolEngineAdapter::new(
            Arc::clone(&runtime),
            HostMode::Headless,
        )),
        security(8),
    );
    let proposal = app
        .clone()
        .oneshot(
            request(
                "POST",
                &format!("/api/v1/models/{}/embedded-metadata/proposal", model.id),
                true,
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"expectedHash":model.content_hash}).to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(proposal.status(), StatusCode::OK);
    let proposal: Value =
        serde_json::from_slice(&to_bytes(proposal.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(proposal["embedded"]["designer"], "Embedded Designer");
    assert_eq!(proposal["embedded"]["parentModel"], "Parent Family");
    let applied = app
        .oneshot(
            request(
                "POST",
                &format!("/api/v1/models/{}/embedded-metadata/apply", model.id),
                true,
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "expectedVersion":model.metadata_version.to_string(),
                    "expectedSourceHash":proposal["sourceHash"],
                    "selection":{"designer":true,"sourceUrl":false,"notes":false,"modelLicense":false,"parentModel":true}
                })
                .to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(applied.status(), StatusCode::OK);
    let updated = runtime
        .read_db(|db| db.get_model(&model.id))
        .unwrap()
        .unwrap();
    assert_eq!(
        updated.metadata.parent_model_id.as_deref(),
        Some(parent.id.as_str())
    );
}

#[test]
fn protocol_adapter_adds_and_removes_model_tags_by_name_with_versions() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let model = runtime
        .mutate_db(|db| db.create_model("MCP tag target", None))
        .unwrap();
    let adapter = ProtocolEngineAdapter::new(Arc::clone(&runtime), HostMode::Headless);
    let writer = RequestContext {
        principal_id: "tag-writer".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
        request_id: "tag-write".into(),
    };

    assert!(adapter.supports_tool(&writer, "add_model_tags"));
    assert!(adapter.supports_tool(&writer, "remove_model_tags"));
    let added = adapter
        .call_tool(
            &writer,
            "add_model_tags",
            json!({
                "modelId":model.id,
                "expectedVersion":model.metadata_version.to_string(),
                "tags":["Mechanical","Decorative"]
            }),
        )
        .unwrap();
    let added_version: u64 = added["metadataVersion"].as_str().unwrap().parse().unwrap();
    let current = adapter.get_model(&writer, &model.id).unwrap();
    assert_eq!(current.metadata_version, added_version.to_string());
    assert_eq!(current.tags, ["Decorative", "Mechanical"]);

    let removed = adapter
        .call_tool(
            &writer,
            "remove_model_tags",
            json!({
                "modelId":model.id,
                "expectedVersion":added_version.to_string(),
                "tags":["mechanical"]
            }),
        )
        .unwrap();
    let removed_version: u64 = removed["metadataVersion"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let current = adapter.get_model(&writer, &model.id).unwrap();
    assert_eq!(current.metadata_version, removed_version.to_string());
    assert_eq!(current.tags, ["Decorative"]);
    assert!(adapter
        .list_tags(&writer)
        .unwrap()
        .iter()
        .any(|tag| tag.name == "Mechanical"));

    assert!(adapter
        .call_tool(
            &writer,
            "add_model_tags",
            json!({
                "modelId":model.id,
                "expectedVersion":added_version.to_string(),
                "tags":["Stale"]
            }),
        )
        .is_err());
    assert!(!adapter
        .list_tags(&writer)
        .unwrap()
        .iter()
        .any(|tag| tag.name == "Stale"));
}

#[test]
fn get_models_missing_thumbnails_excludes_render_failures_before_limiting() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let _models = runtime
        .mutate_db(|db| {
            Ok(vec![
                db.create_model("Render failure one", None)?,
                db.create_model("Render failure two", None)?,
                db.create_model("Renderable model", None)?,
            ])
        })
        .unwrap();
    let adapter = ProtocolEngineAdapter::new(runtime, HostMode::Headless);
    let reader = RequestContext {
        principal_id: "thumbnail-reader".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead]),
        request_id: "missing-thumbnails-exclude".into(),
    };

    let unfiltered = adapter
        .call_tool(
            &reader,
            "get_models_missing_thumbnails",
            json!({"limit":10}),
        )
        .unwrap();
    let ordered_ids = unfiltered["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let expected_id = ordered_ids.last().unwrap().clone();
    let excluded_ids = ordered_ids[..ordered_ids.len() - 1].to_vec();

    let page = adapter
        .call_tool(
            &reader,
            "get_models_missing_thumbnails",
            json!({"limit":1,"excludeIds":excluded_ids}),
        )
        .unwrap();

    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], expected_id);
    assert!(page["nextCursor"].is_null());
}

#[tokio::test]
async fn protocol_adapter_set_thumbnail_replaces_while_add_thumbnail_appends() {
    use base64::Engine as _;
    use image::ImageFormat;

    fn png(color: [u8; 3]) -> String {
        let mut output = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(1, 1, image::Rgb(color))
            .write_to(&mut output, ImageFormat::Png)
            .unwrap();
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(output.into_inner())
        )
    }

    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let model = runtime
        .mutate_db(|db| db.create_model("Thumbnail set target", None))
        .unwrap();
    let content_runtime = Arc::clone(&runtime);
    let adapter = Arc::new(ProtocolEngineAdapter::new(runtime, HostMode::Headless));
    let context = RequestContext {
        principal_id: "thumbnail-writer".into(),
        capabilities: BTreeSet::from([
            Capability::CatalogRead,
            Capability::MediaRead,
            Capability::MediaWrite,
        ]),
        request_id: "thumbnail-set-versus-add".into(),
    };
    let added = adapter
        .call_tool(
            &context,
            "add_thumbnail",
            json!({
                "modelId":model.id,
                "expectedVersion":model.metadata_version.to_string(),
                "image":png([255,0,0])
            }),
        )
        .unwrap();
    let added_version = added["metadataVersion"].as_str().unwrap();
    let replaced = adapter
        .call_tool(
            &context,
            "set_thumbnail",
            json!({
                "modelId":model.id,
                "expectedVersion":added_version,
                "image":png([0,0,255])
            }),
        )
        .unwrap();
    assert!(replaced["metadataVersion"].as_str().is_some());

    let thumbnails = adapter
        .call_tool(&context, "get_thumbnails", json!({"modelId":model.id}))
        .unwrap();
    let custom = thumbnails
        .as_array()
        .unwrap()
        .iter()
        .filter(|thumbnail| thumbnail["custom"] == true)
        .collect::<Vec<_>>();
    assert_eq!(custom.len(), 1);
    assert_eq!(custom[0]["active"], true);
    assert!(custom[0]["mediaLease"]["id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
    let media_id = custom[0]["mediaLease"]["id"].as_str().unwrap();
    library_protocol::ContentEngine::describe_content(content_runtime.as_ref(), &context, media_id)
        .unwrap();
    let mut content = Vec::new();
    library_protocol::ContentEngine::stream_content(
        content_runtime.as_ref(),
        &context,
        media_id,
        &mut content,
    )
    .unwrap();
    let image = image::load_from_memory(&content).unwrap().to_rgb8();
    assert_eq!(*image.get_pixel(0, 0), image::Rgb([0, 0, 255]));
}

#[test]
fn protocol_adapter_updates_heterogeneous_model_batches_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let (first, second) = runtime
        .mutate_db(|db| {
            Ok((
                db.create_model("Heterogeneous first", None)?,
                db.create_model("Heterogeneous second", None)?,
            ))
        })
        .unwrap();
    let writer = RequestContext {
        principal_id: "metadata-writer".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
        request_id: "heterogeneous-update-batch".into(),
    };
    let filament = runtime
        .create_filament(
            &writer,
            library_protocol::FilamentInput {
                brand: "Local".into(),
                material: "PLA".into(),
                color: "Blue".into(),
                name: "PLA Blue".into(),
                notes: String::new(),
            },
        )
        .unwrap();
    let adapter = ProtocolEngineAdapter::new(runtime, HostMode::Headless);

    let result = adapter
        .call_tool(
            &writer,
            "update_models_batch",
            json!({"updates":[
                {"modelId":first.id,"expectedVersion":first.metadata_version.to_string(),"patch":{"designer":"Ada","favorite":true},"tags":["Mechanical","Support"],"filamentIds":[filament.id]},
                {"modelId":second.id,"expectedVersion":second.metadata_version.to_string(),"patch":{"notes":"Resin prototype","rating":4},"tags":["Draft"],"filamentIds":[]}
            ]}),
        )
        .unwrap();
    assert_eq!(result.as_array().unwrap().len(), 2);
    let updated_first = adapter.get_model(&writer, &first.id).unwrap();
    let updated_second = adapter.get_model(&writer, &second.id).unwrap();
    assert_eq!(updated_first.designer.as_deref(), Some("Ada"));
    assert!(updated_first.favorite);
    assert_eq!(updated_first.tags, ["Mechanical", "Support"]);
    assert_eq!(updated_first.filament_ids, vec![filament.id.clone()]);
    assert_eq!(updated_second.notes.as_deref(), Some("Resin prototype"));
    assert_eq!(updated_second.rating, 4);
    assert_eq!(updated_second.tags, ["Draft"]);
    assert!(updated_second.filament_ids.is_empty());

    let stale_result = adapter.call_tool(
        &writer,
        "update_models_batch",
        json!({"updates":[
            {"modelId":first.id,"expectedVersion":updated_first.metadata_version.clone(),"patch":{"notes":"Must roll back"},"tags":["Must Roll Back"],"filamentIds":[]},
            {"modelId":second.id,"expectedVersion":"999999999","patch":{"designer":"Must not apply"},"tags":[],"filamentIds":[]}
        ]}),
    );
    assert!(stale_result.is_err());
    assert_eq!(
        adapter.get_model(&writer, &first.id).unwrap().notes,
        None,
        "a stale item must roll back every heterogeneous patch"
    );
    assert_eq!(
        adapter.get_model(&writer, &second.id).unwrap().designer,
        None
    );
    let unchanged_first = adapter.get_model(&writer, &first.id).unwrap();
    assert_eq!(unchanged_first.tags, ["Mechanical", "Support"]);
    assert_eq!(unchanged_first.filament_ids, vec![filament.id]);
    assert!(!adapter
        .list_tags(&writer)
        .unwrap()
        .iter()
        .any(|tag| tag.name == "Must Roll Back"));
}

#[test]
fn protocol_adapter_remove_model_accepts_multiple_versioned_targets() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let (first, second, keep) = runtime
        .mutate_db(|db| {
            Ok((
                db.create_model("Batch remove first", None)?,
                db.create_model("Batch remove second", None)?,
                db.create_model("Batch keep", None)?,
            ))
        })
        .unwrap();
    let adapter = ProtocolEngineAdapter::new(Arc::clone(&runtime), HostMode::Headless);
    let writer = RequestContext {
        principal_id: "catalog-writer".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
        request_id: "batch-remove-model".into(),
    };
    let targets = json!([
        {"modelId":first.id,"expectedVersion":first.metadata_version.to_string()},
        {"modelId":second.id,"expectedVersion":second.metadata_version.to_string()}
    ]);
    assert!(adapter
        .call_tool(
            &writer,
            "remove_model",
            json!({
                "targets":[
                    {"modelId":first.id,"expectedVersion":first.metadata_version.to_string()},
                    {"modelId":first.id,"expectedVersion":first.metadata_version.to_string()}
                ],
                "confirm":true
            }),
        )
        .is_err());
    assert!(adapter
        .call_tool(
            &writer,
            "remove_model",
            json!({
                "targets":[
                    {"modelId":first.id,"expectedVersion":first.metadata_version.to_string()},
                    {"modelId":second.id,"expectedVersion":(second.metadata_version + 1).to_string()}
                ],
                "confirm":true
            }),
        )
        .is_err());
    assert!(adapter.get_model(&writer, &first.id).is_ok());
    assert!(adapter.get_model(&writer, &second.id).is_ok());

    let plan = adapter
        .call_tool(
            &writer,
            "remove_model",
            json!({"targets":targets,"confirm":true}),
        )
        .unwrap();
    assert_eq!(plan["actionKind"], "recoverableRemove");
    assert_eq!(plan["targets"].as_array().unwrap().len(), 2);
    let removed = adapter
        .call_tool(
            &writer,
            "execute_management_action",
            json!({"token":plan["token"]}),
        )
        .unwrap();
    assert_eq!(removed["kind"], "removed");
    assert_eq!(removed["removedIds"].as_array().unwrap().len(), 2);

    let page = adapter
        .call_tool(&writer, "search_models", json!({"query":"Batch remove"}))
        .unwrap();
    assert!(page["items"].as_array().unwrap().is_empty());
    assert_eq!(
        adapter
            .call_tool(&writer, "get_model", json!({"modelId":keep.id}))
            .unwrap()["id"],
        keep.id
    );
    let restore = adapter
        .call_tool(
            &writer,
            "plan_management_action",
            json!({"action":{"kind":"restoreRemoval","snapshotId":removed["snapshotId"]}}),
        )
        .unwrap();
    let restored = adapter
        .call_tool(
            &writer,
            "execute_management_action",
            json!({"token":restore["token"]}),
        )
        .unwrap();
    assert_eq!(restored["kind"], "restoredRemoval");
    assert_eq!(restored["models"].as_array().unwrap().len(), 2);
}

#[test]
fn protocol_adapter_remove_model_is_recoverable_and_requires_a_second_step() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let model = runtime
        .mutate_db(|db| db.create_model("Recoverable MCP model", None))
        .unwrap();
    let adapter = ProtocolEngineAdapter::new(Arc::clone(&runtime), HostMode::Headless);
    let writer = RequestContext {
        principal_id: "catalog-writer".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
        request_id: "remove-model".into(),
    };

    assert!(adapter.supports_tool(&writer, "remove_model"));
    assert!(adapter
        .call_tool(
            &writer,
            "remove_model",
            json!({
                "modelId":model.id,
                "expectedVersion":model.metadata_version.to_string(),
                "confirm":false
            }),
        )
        .is_err());
    let plan = adapter
        .call_tool(
            &writer,
            "remove_model",
            json!({
                "modelId":model.id,
                "expectedVersion":model.metadata_version.to_string(),
                "confirm":true
            }),
        )
        .unwrap();
    let token = plan["token"].as_str().unwrap();
    assert_eq!(adapter.get_model(&writer, &model.id).unwrap().id, model.id);

    adapter
        .call_tool(&writer, "execute_management_action", json!({"token":token}))
        .unwrap();
    let visible = adapter
        .call_tool(
            &writer,
            "search_models",
            json!({"query":"Recoverable MCP model","limit":100}),
        )
        .unwrap();
    assert!(!visible.to_string().contains(&model.id));
    assert!(adapter
        .call_tool(&writer, "execute_management_action", json!({"token":token}))
        .is_err());
}

#[test]
fn source_management_aliases_require_source_write_capabilities_and_writable_roots() {
    fn runtime_at(
        root_path: &std::path::Path,
        access: library_protocol::RootAccess,
    ) -> Arc<library_runtime::LibraryRuntime> {
        std::fs::create_dir_all(root_path).unwrap();
        let parent = root_path.parent().unwrap();
        let mut config = library_runtime::RuntimeConfig::desktop(
            parent.join("library"),
            parent.join("integrations.json"),
        );
        config.local_catalog_root_access = false;
        config.roots.push(library_runtime::RuntimeRootPolicy {
            id: "models".into(),
            catalog_root_id: None,
            label: "Models".into(),
            path: root_path.to_owned(),
            access,
        });
        library_runtime::LibraryRuntime::open(config).unwrap()
    }

    let writable_temp = tempfile::tempdir().unwrap();
    let runtime = runtime_at(
        &writable_temp.path().join("source"),
        library_protocol::RootAccess::Writable,
    );
    let adapter = ProtocolEngineAdapter::new(runtime, HostMode::Headless);
    let batch_tool = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "update_models_batch")
        .unwrap();
    let update_properties =
        &batch_tool["inputSchema"]["properties"]["updates"]["items"]["properties"];
    assert_eq!(update_properties["tags"]["uniqueItems"], true);
    assert_eq!(update_properties["tags"]["items"]["minLength"], 1);
    assert_eq!(
        update_properties["tags"]["items"]["pattern"].as_str(),
        Some(r"\S")
    );
    assert_eq!(update_properties["filamentIds"]["uniqueItems"], true);
    let writer = RequestContext {
        principal_id: "source-writer".into(),
        capabilities: BTreeSet::from([
            Capability::CatalogRead,
            Capability::CatalogWrite,
            Capability::PhysicalSourceWrite,
        ]),
        request_id: "source-actions".into(),
    };
    for name in ["trash_file", "undo_trash", "move_files"] {
        assert!(
            adapter.supports_tool(&writer, name),
            "{name} should be enabled"
        );
    }

    let no_physical_write = RequestContext {
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
        ..writer.clone()
    };
    let no_catalog_write = RequestContext {
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::PhysicalSourceWrite]),
        ..writer.clone()
    };
    for name in ["trash_file", "undo_trash", "move_files"] {
        assert!(!adapter.supports_tool(&no_physical_write, name));
        assert!(!adapter.supports_tool(&no_catalog_write, name));
    }

    let readonly_temp = tempfile::tempdir().unwrap();
    let readonly_runtime = runtime_at(
        &readonly_temp.path().join("source"),
        library_protocol::RootAccess::ReadOnly,
    );
    let readonly_adapter = ProtocolEngineAdapter::new(readonly_runtime, HostMode::Headless);
    for name in ["trash_file", "undo_trash", "move_files"] {
        assert!(!readonly_adapter.supports_tool(&writer, name));
    }

    let trash = adapter
        .call_tool(
            &writer,
            "trash_file",
            json!({"modelId":"missing","expectedVersion":"1","confirm":true}),
        )
        .unwrap_err();
    assert_eq!(trash.code, "notFound");
    let undo = adapter
        .call_tool(
            &writer,
            "undo_trash",
            json!({"snapshotId":"missing","confirm":true}),
        )
        .unwrap_err();
    assert_eq!(undo.code, "notFound");
    let movement = adapter
        .call_tool(
            &writer,
            "move_files",
            json!({"modelId":"missing","expectedVersion":"1","destinationHandleId":"models","fileName":"moved.stl","confirm":true}),
        )
        .unwrap_err();
    assert_eq!(movement.code, "notFound");
}

#[test]
fn mcp_purge_and_orphan_cleanup_use_confirmed_one_use_management_plans() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("library");
    let backups = temp.path().join("backups");
    std::fs::create_dir(&backups).unwrap();
    let mut runtime_config = library_runtime::RuntimeConfig::desktop(
        library.clone(),
        temp.path().join("integrations.json"),
    );
    runtime_config
        .roots
        .push(library_runtime::RuntimeRootPolicy {
            id: "backups".into(),
            catalog_root_id: None,
            label: "Backups".into(),
            path: backups.clone(),
            access: library_protocol::RootAccess::BackupOnly,
        });
    runtime_config
        .capabilities
        .remove(&Capability::PhysicalSourceWrite);
    runtime_config.capabilities.insert(Capability::BackupWrite);
    let runtime = library_runtime::LibraryRuntime::open(runtime_config).unwrap();
    let adapter = ProtocolEngineAdapter::new(Arc::clone(&runtime), HostMode::Headless);
    let context = RequestContext {
        principal_id: "confirmed-management-client".into(),
        capabilities: BTreeSet::from([
            Capability::CatalogRead,
            Capability::CatalogWrite,
            Capability::Maintenance,
            Capability::BackupWrite,
            Capability::MediaWrite,
        ]),
        request_id: "management-mcp".into(),
    };
    assert!(adapter
        .call_tool(
            &context,
            "plan_management_action",
            json!({"action":{
                "kind":"purgeCatalog",
                "backupDestinationHandleId":"backups",
                "backupFileName":"missing-confirm.zip"
            }}),
        )
        .is_err());
    let purge_plan = adapter
        .call_tool(
            &context,
            "plan_management_action",
            json!({"action":{
                "kind":"purgeCatalog",
                "backupDestinationHandleId":"backups",
                "backupFileName":"mcp-before-purge.zip",
                "confirm":true
            }}),
        )
        .unwrap();
    assert_eq!(purge_plan["actionKind"], "purgeCatalog");
    let purge_result = adapter
        .call_tool(
            &context,
            "execute_management_action",
            json!({"token":purge_plan["token"]}),
        )
        .unwrap();
    assert_eq!(purge_result["kind"], "purged");
    assert!(backups.join("mcp-before-purge.zip").is_file());
    assert!(adapter
        .call_tool(
            &context,
            "execute_management_action",
            json!({"token":purge_plan["token"]}),
        )
        .is_err());

    let thumbnail_dir = library.join("thumbnails");
    std::fs::create_dir_all(&thumbnail_dir).unwrap();
    let orphan = thumbnail_dir.join(format!("{}-isometric-v1-256-ffffff.png", "c".repeat(64)));
    let orphan_bytes = b"generated orphan thumbnail".to_vec();
    std::fs::write(&orphan, &orphan_bytes).unwrap();
    assert!(adapter
        .call_tool(
            &context,
            "plan_management_action",
            json!({"action":{"kind":"cleanupOrphans"}}),
        )
        .is_err());
    let cleanup_plan = adapter
        .call_tool(
            &context,
            "plan_management_action",
            json!({"action":{"kind":"cleanupOrphans","confirm":true}}),
        )
        .unwrap();
    assert_eq!(cleanup_plan["actionKind"], "cleanupOrphans");
    let cleanup_result = adapter
        .call_tool(
            &context,
            "execute_management_action",
            json!({"token":cleanup_plan["token"]}),
        )
        .unwrap();
    assert_eq!(cleanup_result["kind"], "cleanupCompleted");
    assert_eq!(cleanup_result["files"], "0");
    assert_eq!(cleanup_result["bytes"], "0");
    assert_eq!(std::fs::read(&orphan).unwrap(), orphan_bytes);
    assert_eq!(
        cleanup_result["thumbnailProgress"]["review"]["eligibleBytes"],
        "0"
    );
    assert!(
        cleanup_result["thumbnailProgress"]["review"]["preservedFiles"]
            .as_u64()
            .unwrap()
            >= 1
    );
}

#[test]
fn browser_capabilities_keep_maintenance_actions_separate_from_source_writes() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("library");
    let models = temp.path().join("models");
    let backups = temp.path().join("backups");
    std::fs::create_dir(&models).unwrap();
    std::fs::create_dir(&backups).unwrap();
    let bootstrap = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        library.clone(),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let model_root = bootstrap
        .mutate_db(|db| db.add_index_root(&models, 60, 50 * 1024 * 1024))
        .unwrap();
    drop(bootstrap);

    let mut runtime_config =
        library_runtime::RuntimeConfig::desktop(library, temp.path().join("integrations.json"));
    runtime_config.local_catalog_root_access = false;
    runtime_config.roots = vec![
        library_runtime::RuntimeRootPolicy {
            id: "models".into(),
            catalog_root_id: Some(model_root.id),
            label: "Models".into(),
            path: models,
            access: library_protocol::RootAccess::Writable,
        },
        library_runtime::RuntimeRootPolicy {
            id: "backups".into(),
            catalog_root_id: None,
            label: "Backups".into(),
            path: backups,
            access: library_protocol::RootAccess::BackupOnly,
        },
    ];
    let granted = BTreeSet::from([
        Capability::CatalogRead,
        Capability::CatalogWrite,
        Capability::Maintenance,
        Capability::BackupWrite,
        Capability::MediaWrite,
        Capability::PhysicalSourceWrite,
    ]);
    runtime_config.capabilities = granted.clone();
    let runtime = library_runtime::LibraryRuntime::open(runtime_config).unwrap();
    let adapter = ProtocolEngineAdapter::new(runtime, HostMode::Headless);

    let context = |capabilities| RequestContext {
        principal_id: "feature-visibility".into(),
        capabilities,
        request_id: "feature-visibility".into(),
    };
    let full = adapter
        .capabilities(&context(granted.clone()))
        .unwrap()
        .features;
    assert!(full.trash_source);
    assert!(full.purge_catalog);
    assert!(full.cleanup_orphans);

    let without_source_write = BTreeSet::from([
        Capability::CatalogRead,
        Capability::CatalogWrite,
        Capability::Maintenance,
        Capability::BackupWrite,
        Capability::MediaWrite,
    ]);
    let maintenance_only = adapter
        .capabilities(&context(without_source_write))
        .unwrap()
        .features;
    assert!(!maintenance_only.trash_source);
    assert!(maintenance_only.purge_catalog);
    assert!(maintenance_only.cleanup_orphans);

    let without_backup_write = BTreeSet::from([
        Capability::CatalogRead,
        Capability::CatalogWrite,
        Capability::Maintenance,
        Capability::MediaWrite,
        Capability::PhysicalSourceWrite,
    ]);
    let cleanup_only = adapter
        .capabilities(&context(without_backup_write))
        .unwrap()
        .features;
    assert!(!cleanup_only.purge_catalog);
    assert!(cleanup_only.cleanup_orphans);
}

#[test]
fn get_all_metadata_returns_counted_rows_without_arguments_and_keeps_get_model_separate() {
    assert!(super::validate_tool_shape("get_all_metadata", &json!({})).is_ok());
    assert!(super::validate_tool_shape("get_all_metadata", &json!({"modelId":"model-1"})).is_err());
    assert!(super::validate_tool_shape("get_model", &json!({"modelId":"model-1"})).is_ok());

    let metadata_tool = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "get_all_metadata")
        .unwrap();
    assert_eq!(metadata_tool["inputSchema"]["type"], "object");
    assert_eq!(metadata_tool["inputSchema"]["additionalProperties"], false);
    assert_eq!(metadata_tool["inputSchema"]["properties"], json!({}));
    assert!(metadata_tool["inputSchema"].get("required").is_none());

    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let parent = runtime
        .mutate_db(|db| db.create_model("Parent model", None))
        .unwrap();
    let child_one = runtime
        .mutate_db(|db| db.create_model("Child one", None))
        .unwrap();
    let child_two = runtime
        .mutate_db(|db| db.create_model("Child two", None))
        .unwrap();
    let adapter = ProtocolEngineAdapter::new(Arc::clone(&runtime), HostMode::Headless);
    let writer = RequestContext {
        principal_id: "metadata-writer".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
        request_id: "metadata-write".into(),
    };
    adapter
        .update_metadata(
            &writer,
            VersionedTarget {
                model_id: parent.id.clone(),
                expected_version: parent.metadata_version,
            },
            MetadataPatch {
                designer: Some("Alice".into()),
                model_license: Some("CC0".into()),
                ..MetadataPatch::default()
            },
        )
        .unwrap();
    adapter
        .update_metadata(
            &writer,
            VersionedTarget {
                model_id: child_one.id.clone(),
                expected_version: child_one.metadata_version,
            },
            MetadataPatch {
                designer: Some("Alice".into()),
                model_license: Some("GPL".into()),
                parent_model_id: Some(parent.id.clone()),
                ..MetadataPatch::default()
            },
        )
        .unwrap();
    adapter
        .update_metadata(
            &writer,
            VersionedTarget {
                model_id: child_two.id.clone(),
                expected_version: child_two.metadata_version,
            },
            MetadataPatch {
                designer: Some("Bob".into()),
                model_license: Some("CC0".into()),
                parent_model_id: Some(parent.id.clone()),
                ..MetadataPatch::default()
            },
        )
        .unwrap();

    let reader = RequestContext {
        capabilities: BTreeSet::from([Capability::CatalogRead]),
        ..writer
    };
    assert!(adapter
        .call_tool(
            &reader,
            "get_all_metadata",
            json!({"modelId":parent.id.clone()})
        )
        .is_err());
    let all_metadata = adapter
        .call_tool(&reader, "get_all_metadata", json!({}))
        .unwrap();
    assert_eq!(
        all_metadata,
        json!([
            {"type":"designer","name":"Alice","model_count":2},
            {"type":"designer","name":"Bob","model_count":1},
            {"type":"license","name":"CC0","model_count":2},
            {"type":"license","name":"GPL","model_count":1},
            {"type":"parentModel","name":"Parent model","model_count":2}
        ])
    );
    assert_eq!(
        adapter
            .call_tool(&reader, "get_model", json!({"modelId":parent.id}))
            .unwrap()["displayName"],
        "Parent model"
    );

    let removed_parent = runtime
        .mutate_db(|db| db.create_model("Removed parent", None))
        .unwrap();
    let remaining_child = runtime
        .mutate_db(|db| db.create_model("Remaining child", None))
        .unwrap();
    let remover = RequestContext {
        principal_id: "metadata-parent-remover".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
        request_id: "remove-metadata-parent".into(),
    };
    adapter
        .update_metadata(
            &remover,
            VersionedTarget {
                model_id: remaining_child.id,
                expected_version: remaining_child.metadata_version,
            },
            MetadataPatch {
                parent_model_id: Some(removed_parent.id.clone()),
                ..MetadataPatch::default()
            },
        )
        .unwrap();
    let removal_plan = adapter
        .call_tool(
            &remover,
            "remove_model",
            json!({
                "modelId":removed_parent.id,
                "expectedVersion":removed_parent.metadata_version.to_string(),
                "confirm":true
            }),
        )
        .unwrap();
    adapter
        .call_tool(
            &remover,
            "execute_management_action",
            json!({"token":removal_plan["token"]}),
        )
        .unwrap();
    let metadata_after_parent_removal = adapter
        .call_tool(&reader, "get_all_metadata", json!({}))
        .unwrap();
    assert!(metadata_after_parent_removal
        .as_array()
        .unwrap()
        .iter()
        .any(|row| {
            row == &json!({"type":"parentModel","name":"Removed parent","model_count":1})
        }));
}

#[test]
fn check_files_exist_allows_a_bounded_library_scan_without_paths() {
    assert!(super::validate_tool_shape("check_files_exist", &json!({})).is_ok());
    assert!(super::validate_tool_shape(
        "check_files_exist",
        &json!({"limit":2000,"missingOnly":true})
    )
    .is_ok());
    assert!(super::validate_tool_shape("check_files_exist", &json!({"limit":2001})).is_err());

    let tool = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "check_files_exist")
        .unwrap();
    assert_eq!(tool["inputSchema"]["properties"]["limit"]["maximum"], 2000);
    assert_eq!(
        tool["inputSchema"]["properties"]["modelIds"]["uniqueItems"],
        true
    );
    assert!(tool["description"].as_str().unwrap().contains("256 MiB"));
    assert!(tool["inputSchema"]["required"]
        .as_array()
        .is_none_or(|required| !required.contains(&json!("modelIds"))));
}

#[test]
fn check_files_exist_scans_only_the_requested_catalog_limit() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("library");
    let source_root = temp.path().join("models");
    std::fs::create_dir(&source_root).unwrap();
    let present_source = source_root.join("present.stl");
    let missing_source = source_root.join("missing.stl");
    let stl = b"solid check\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid check\n";
    std::fs::write(&present_source, stl).unwrap();
    std::fs::write(&missing_source, stl).unwrap();
    let mut bootstrap_config = library_runtime::RuntimeConfig::desktop(
        library.clone(),
        temp.path().join("integrations.json"),
    );
    bootstrap_config.capabilities =
        BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]);
    let bootstrap = library_runtime::LibraryRuntime::open(bootstrap_config).unwrap();
    let index_root = bootstrap
        .mutate_db(|db| db.add_index_root(&source_root, 60, 50 * 1024 * 1024))
        .unwrap();
    drop(bootstrap);
    let mut runtime_config =
        library_runtime::RuntimeConfig::desktop(library, temp.path().join("integrations.json"));
    runtime_config.local_catalog_root_access = false;
    runtime_config.roots = vec![library_runtime::RuntimeRootPolicy {
        id: "models".into(),
        catalog_root_id: Some(index_root.id.clone()),
        label: "Models".into(),
        path: source_root.clone(),
        access: library_protocol::RootAccess::ReadOnly,
    }];
    runtime_config.capabilities = BTreeSet::from([
        Capability::CatalogRead,
        Capability::CatalogWrite,
        Capability::MediaRead,
        Capability::Scan,
    ]);
    let runtime = library_runtime::LibraryRuntime::open(runtime_config).unwrap();
    library_runtime::scanner::scan_root(&runtime, &index_root).unwrap();
    let models = runtime.read_db(|db| db.list_models(None, None)).unwrap();
    let missing_model_id = models
        .iter()
        .find(|model| model.original_name.as_deref() == Some("missing.stl"))
        .unwrap()
        .id
        .clone();
    std::fs::remove_file(&missing_source).unwrap();
    library_runtime::scanner::scan_root(&runtime, &index_root).unwrap();
    let adapter = ProtocolEngineAdapter::new(Arc::clone(&runtime), HostMode::Headless);
    let reader = RequestContext {
        principal_id: "catalog-source-reader".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::MediaRead]),
        request_id: "catalog-source-check".into(),
    };

    let bounded = adapter
        .call_tool(
            &reader,
            "check_files_exist",
            json!({"limit":1,"missingOnly":false}),
        )
        .unwrap();
    assert_eq!(bounded.as_array().unwrap().len(), 1);
    let default_scan = adapter
        .call_tool(&reader, "check_files_exist", json!({}))
        .unwrap();
    assert_eq!(default_scan.as_array().unwrap().len(), 1);
    assert_eq!(default_scan[0]["modelId"], missing_model_id);
    let explicit = adapter
        .call_tool(
            &reader,
            "check_files_exist",
            json!({"modelIds":models.iter().map(|model| model.id.clone()).collect::<Vec<_>>(),"missingOnly":false}),
        )
        .unwrap();
    assert_eq!(explicit.as_array().unwrap().len(), 2);
}

#[test]
fn protocol_adapter_source_action_aliases_plan_and_execute_without_client_paths() {
    let temp = tempfile::tempdir().unwrap();
    let source_root = temp.path().join("models");
    let destination_root = temp.path().join("destination");
    std::fs::create_dir(&source_root).unwrap();
    std::fs::create_dir(&destination_root).unwrap();
    let source = source_root.join("part.stl");
    let second_source = source_root.join("part-two.stl");
    let mut source_bytes = vec![0_u8; 84];
    source_bytes[80..84].copy_from_slice(&1_u32.to_le_bytes());
    for value in [
        0.0_f32, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0,
    ] {
        source_bytes.extend_from_slice(&value.to_le_bytes());
    }
    source_bytes.extend_from_slice(&0_u16.to_le_bytes());
    std::fs::write(&source, &source_bytes).unwrap();
    std::fs::write(&second_source, &source_bytes).unwrap();

    let capabilities = BTreeSet::from([
        Capability::CatalogRead,
        Capability::CatalogWrite,
        Capability::Scan,
        Capability::PhysicalSourceWrite,
    ]);
    let mut config = library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    );
    config.local_catalog_root_access = false;
    config.roots = vec![
        library_runtime::RuntimeRootPolicy {
            id: "models".into(),
            catalog_root_id: None,
            label: "Models".into(),
            path: source_root.clone(),
            access: library_protocol::RootAccess::Writable,
        },
        library_runtime::RuntimeRootPolicy {
            id: "destination".into(),
            catalog_root_id: None,
            label: "Destination".into(),
            path: destination_root.clone(),
            access: library_protocol::RootAccess::Writable,
        },
    ];
    config.capabilities = capabilities.clone();
    let runtime = library_runtime::LibraryRuntime::open(config).unwrap();
    let context = RequestContext {
        principal_id: "source-manager".into(),
        capabilities,
        request_id: "source-alias-actions".into(),
    };
    let roots = runtime.root_handles(&context).unwrap();
    let source_root_info = roots.iter().find(|root| root.label == "Models").unwrap();
    let source_catalog_id = source_root_info.catalog_root_id.as_deref().unwrap();
    let source_catalog = runtime
        .read_db(|db| {
            db.list_index_roots().map(|roots| {
                roots
                    .into_iter()
                    .find(|root| root.id == source_catalog_id)
                    .unwrap()
            })
        })
        .unwrap();
    library_runtime::scanner::scan_root(&runtime, &source_catalog).unwrap();
    let models = runtime.read_db(|db| db.list_models(None, None)).unwrap();
    let model = models
        .iter()
        .find(|candidate| candidate.display_name == "part")
        .unwrap()
        .clone();
    let second_model = models
        .iter()
        .find(|candidate| candidate.display_name == "part-two")
        .unwrap()
        .clone();
    let source_handle_id = source_root_info.id.clone();
    let adapter = ProtocolEngineAdapter::new(runtime.clone(), HostMode::Headless);

    fn assert_source_hold_receipt(
        hold: &Value,
        model_id: &str,
        root_handle_id: &str,
        relative_path: &str,
        root: &std::path::Path,
        bytes: &[u8],
    ) {
        assert_eq!(hold["modelId"], model_id);
        assert_eq!(hold["rootHandleId"], root_handle_id);
        assert_eq!(hold["relativePath"], relative_path);
        assert_eq!(hold["bytes"], bytes.len().to_string());
        assert_eq!(hold["sha256"], format!("{:x}", sha2::Sha256::digest(bytes)));
        let hold_name = hold["holdName"].as_str().unwrap();
        let journal_name = hold["journalName"].as_str().unwrap();
        assert_eq!(std::fs::read(root.join(hold_name)).unwrap(), bytes);
        assert!(root.join(journal_name).is_file());
        assert!(!hold.to_string().contains(root.to_string_lossy().as_ref()));
    }

    let trash_plan = adapter
        .call_tool(
            &context,
            "trash_file",
            json!({
                "modelId":model.id,
                "expectedVersion":model.metadata_version.to_string(),
                "confirm":true
            }),
        )
        .unwrap();
    assert_eq!(trash_plan["actionKind"], "quarantineLooseSource");
    let trash_token = trash_plan["token"].as_str().unwrap();

    let move_plan = adapter
        .call_tool(
            &context,
            "move_files",
            json!({
                "modelId":model.id,
                "expectedVersion":model.metadata_version.to_string(),
                "destinationHandleId":source_handle_id,
                "fileName":"part-copy.stl",
                "confirm":true
            }),
        )
        .unwrap();
    assert_eq!(move_plan["actionKind"], "moveLooseSource");
    assert!(!move_plan["token"].as_str().unwrap().is_empty());
    assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
    assert!(!source_root.join("part-copy.stl").exists());

    let quarantined = adapter
        .call_tool(
            &context,
            "execute_management_action",
            json!({"token":trash_token}),
        )
        .unwrap();
    assert_eq!(quarantined["kind"], "quarantined");
    let source_holds = quarantined["sourceHolds"].as_array().unwrap();
    assert_eq!(source_holds.len(), 1);
    assert_source_hold_receipt(
        &source_holds[0],
        &model.id,
        &source_handle_id,
        "part.stl",
        &source_root,
        &source_bytes,
    );
    assert!(!source.exists());

    let undo_plan = adapter
        .call_tool(
            &context,
            "undo_trash",
            json!({"snapshotId":quarantined["snapshotId"],"confirm":true}),
        )
        .unwrap();
    assert_eq!(undo_plan["actionKind"], "undoQuarantine");
    assert!(!undo_plan["token"].as_str().unwrap().is_empty());
    let restored = adapter
        .call_tool(
            &context,
            "execute_management_action",
            json!({"token":undo_plan["token"]}),
        )
        .unwrap();
    assert_eq!(restored["kind"], "quarantineUndone");
    assert_eq!(std::fs::read(&source).unwrap(), source_bytes);

    let active_models = runtime.read_db(|db| db.list_models(None, None)).unwrap();
    let active_first = active_models
        .iter()
        .find(|candidate| candidate.id == model.id)
        .unwrap();
    let active_second = active_models
        .iter()
        .find(|candidate| candidate.id == second_model.id)
        .unwrap();
    let batch_targets = json!([
        {"modelId":active_first.id,"expectedVersion":active_first.metadata_version.to_string()},
        {"modelId":active_second.id,"expectedVersion":active_second.metadata_version.to_string()}
    ]);
    let batch_trash_plan = adapter
        .call_tool(
            &context,
            "trash_file",
            json!({"targets":batch_targets,"confirm":true}),
        )
        .unwrap();
    assert_eq!(batch_trash_plan["actionKind"], "quarantineLooseSources");
    assert_eq!(batch_trash_plan["targets"].as_array().unwrap().len(), 2);
    let batch_trash_result = adapter
        .call_tool(
            &context,
            "execute_management_action",
            json!({"token":batch_trash_plan["token"]}),
        )
        .unwrap();
    assert_eq!(batch_trash_result["kind"], "quarantinedMany");
    let batch_trash_holds = batch_trash_result["sourceHolds"].as_array().unwrap();
    assert_eq!(batch_trash_holds.len(), 2);
    for hold in batch_trash_holds {
        let (id, relative_path) = if hold["modelId"] == model.id {
            (model.id.as_str(), "part.stl")
        } else {
            assert_eq!(hold["modelId"], second_model.id);
            (second_model.id.as_str(), "part-two.stl")
        };
        assert_source_hold_receipt(
            hold,
            id,
            &source_handle_id,
            relative_path,
            &source_root,
            &source_bytes,
        );
    }
    assert!(!source.exists());
    assert!(!second_source.exists());

    let batch_undo_plan = adapter
        .call_tool(
            &context,
            "undo_trash",
            json!({"snapshotId":batch_trash_result["snapshotId"],"confirm":true}),
        )
        .unwrap();
    let batch_undo_result = adapter
        .call_tool(
            &context,
            "execute_management_action",
            json!({"token":batch_undo_plan["token"]}),
        )
        .unwrap();
    assert_eq!(batch_undo_result["kind"], "quarantineUndoneMany");
    assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
    assert_eq!(std::fs::read(&second_source).unwrap(), source_bytes);

    let restored_models = runtime.read_db(|db| db.list_models(None, None)).unwrap();
    let restored_first = restored_models
        .iter()
        .find(|candidate| candidate.id == model.id)
        .unwrap();
    let restored_second = restored_models
        .iter()
        .find(|candidate| candidate.id == second_model.id)
        .unwrap();
    let batch_move_plan = adapter
        .call_tool(
            &context,
            "move_files",
            json!({
                "destinationHandleId":"destination",
                "targets":[
                    {"modelId":restored_first.id,"expectedVersion":restored_first.metadata_version.to_string(),"fileName":"part-moved.stl"},
                    {"modelId":restored_second.id,"expectedVersion":restored_second.metadata_version.to_string(),"fileName":"part-two-moved.stl"}
                ],
                "confirm":true
            }),
        )
        .unwrap();
    assert_eq!(batch_move_plan["actionKind"], "moveLooseSources");
    assert_eq!(batch_move_plan["targets"].as_array().unwrap().len(), 2);
    let batch_move_result = adapter
        .call_tool(
            &context,
            "execute_management_action",
            json!({"token":batch_move_plan["token"]}),
        )
        .unwrap();
    assert_eq!(batch_move_result["kind"], "movedMany");
    let batch_move_holds = batch_move_result["sourceHolds"].as_array().unwrap();
    assert_eq!(batch_move_holds.len(), 2);
    for hold in batch_move_holds {
        let (id, relative_path) = if hold["modelId"] == model.id {
            (model.id.as_str(), "part.stl")
        } else {
            assert_eq!(hold["modelId"], second_model.id);
            (second_model.id.as_str(), "part-two.stl")
        };
        assert_source_hold_receipt(
            hold,
            id,
            &source_handle_id,
            relative_path,
            &source_root,
            &source_bytes,
        );
    }
    assert!(!source.exists());
    assert!(!second_source.exists());
    assert_eq!(
        std::fs::read(destination_root.join("part-moved.stl")).unwrap(),
        source_bytes
    );
    assert_eq!(
        std::fs::read(destination_root.join("part-two-moved.stl")).unwrap(),
        source_bytes
    );

    let active_after_batch_move = runtime.read_db(|db| db.list_models(None, None)).unwrap();
    let active_first = active_after_batch_move
        .iter()
        .find(|candidate| candidate.id == model.id)
        .unwrap();
    let single_move_plan = adapter
        .call_tool(
            &context,
            "move_files",
            json!({
                "modelId":active_first.id,
                "expectedVersion":active_first.metadata_version.to_string(),
                "destinationHandleId":source_handle_id,
                "fileName":"part-returned.stl",
                "confirm":true
            }),
        )
        .unwrap();
    let single_move_result = adapter
        .call_tool(
            &context,
            "execute_management_action",
            json!({"token":single_move_plan["token"]}),
        )
        .unwrap();
    assert_eq!(single_move_result["kind"], "moved");
    let single_move_holds = single_move_result["sourceHolds"].as_array().unwrap();
    assert_eq!(single_move_holds.len(), 1);
    assert_source_hold_receipt(
        &single_move_holds[0],
        &model.id,
        "destination",
        "part-moved.stl",
        &destination_root,
        &source_bytes,
    );
    assert_eq!(
        std::fs::read(source_root.join("part-returned.stl")).unwrap(),
        source_bytes
    );
}

#[test]
fn search_models_accepts_search_designer_and_print_state_filters() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let models = runtime
        .mutate_db(|db| {
            Ok(vec![
                db.create_model("Designer match printed", None)?,
                db.create_model("Designer match failed", None)?,
                db.create_model("Other designer printed", None)?,
            ])
        })
        .unwrap();
    let adapter = ProtocolEngineAdapter::new(runtime, HostMode::Headless);
    let writer = RequestContext {
        principal_id: "search-writer".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
        request_id: "search-pinned-filters".into(),
    };
    for (model, designer, print_status) in [
        (&models[0], "Ada", "printed"),
        (&models[1], "Ada", "failed"),
        (&models[2], "Grace", "printed"),
    ] {
        adapter
            .call_tool(
                &writer,
                "update_model",
                json!({
                    "target":{"modelId":model.id,"expectedVersion":model.metadata_version.to_string()},
                    "patch":{"designer":designer,"printStatus":print_status}
                }),
            )
            .unwrap();
    }

    let page = adapter
        .call_tool(
            &writer,
            "search_models",
            json!({
                "search":"Designer match",
                "designer":"Ada",
                "printed":"printed",
                "limit":10
            }),
        )
        .unwrap();

    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], models[0].id);
}

#[test]
fn scan_directory_accepts_only_root_relative_subdirectory_scopes() {
    assert!(validate_tool_shape("scan_directory", &json!({})).is_ok());
    assert!(validate_tool_shape("scan_directory", &json!({"rootId":"admitted-root"})).is_ok());
    assert!(validate_tool_shape(
        "scan_directory",
        &json!({"rootId":"admitted-root","directory":"parts/nested"})
    )
    .is_ok());
    assert!(validate_tool_shape("scan_directory", &json!({"directory":"parts/nested"})).is_err());
    assert!(validate_tool_shape(
        "scan_directory",
        &json!({"rootId":"admitted-root","directory":1})
    )
    .is_err());

    let temp = tempfile::tempdir().unwrap();
    let source_root = temp.path().join("models");
    std::fs::create_dir_all(source_root.join("parts/nested")).unwrap();
    std::fs::create_dir_all(source_root.join("other")).unwrap();
    let triangle = b"solid part\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid part\n";
    std::fs::write(source_root.join("parts/nested/inside.stl"), triangle).unwrap();
    std::fs::write(source_root.join("other/outside.stl"), triangle).unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig {
        library_root: temp.path().join("library"),
        integration_settings_path: temp.path().join("integrations.json"),
        roots: vec![library_runtime::RuntimeRootPolicy {
            id: "models".into(),
            catalog_root_id: None,
            label: "Models".into(),
            path: source_root,
            access: library_protocol::RootAccess::ReadOnly,
        }],
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::Scan]),
        local_catalog_root_access: false,
    })
    .unwrap();
    let context = RequestContext {
        principal_id: "subdirectory-reader".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::Scan]),
        request_id: "safe-subdirectory-scan".into(),
    };
    let handle = runtime.root_handles(&context).unwrap().remove(0);
    let adapter = ProtocolEngineAdapter::new(Arc::clone(&runtime), HostMode::Headless);
    assert!(adapter
        .call_tool(
            &context,
            "scan_directory",
            json!({"rootId":handle.id.clone(),"directory":"../other"}),
        )
        .is_err());
    adapter
        .call_tool(
            &context,
            "scan_directory",
            json!({"rootId":handle.id,"directory":"parts"}),
        )
        .unwrap();
    for _ in 0..100 {
        if !runtime.scan_control().status().running {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!runtime.scan_control().status().running);
    let models = runtime.read_db(|db| db.list_models(None, None)).unwrap();
    assert_eq!(models.len(), 1);
    assert!(models[0]
        .source_path
        .as_deref()
        .is_some_and(|path| path.ends_with("inside.stl")));
}

#[test]
fn duplicates_and_hash_status_apply_bounded_catalog_filters() {
    let temp = tempfile::tempdir().unwrap();
    let first_path = temp.path().join("Duplicate A.stl");
    let second_path = temp.path().join("Duplicate B.stl");
    let unrelated_path = temp.path().join("Unrelated.stl");
    let mut duplicate_bytes = vec![0_u8; 80];
    duplicate_bytes.extend_from_slice(&1_u32.to_le_bytes());
    for value in [
        0.0_f32, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0,
    ] {
        duplicate_bytes.extend_from_slice(&value.to_le_bytes());
    }
    duplicate_bytes.extend_from_slice(&[0, 0]);
    let mut unrelated_bytes = duplicate_bytes.clone();
    unrelated_bytes[0] = 0x5a;
    std::fs::write(&first_path, &duplicate_bytes).unwrap();
    std::fs::write(&second_path, &duplicate_bytes).unwrap();
    std::fs::write(&unrelated_path, unrelated_bytes).unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let (first, second) = runtime
        .mutate_db(|db| {
            let first = db.import_file(&first_path, None, None)?.model;
            let second = db.import_file(&second_path, None, None)?.model;
            let _unrelated = db.import_file(&unrelated_path, None, None)?.model;
            Ok((first, second))
        })
        .unwrap();
    let context = RequestContext {
        principal_id: "duplicate-reader".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
        request_id: "filtered-duplicates".into(),
    };
    let adapter = ProtocolEngineAdapter::new(runtime, HostMode::Headless);
    adapter
        .call_tool(
            &context,
            "add_tag",
            json!({"name":"hinge","category":"part"}),
        )
        .unwrap();
    for model in [&first, &second] {
        adapter
            .call_tool(
                &context,
                "add_model_tags",
                json!({
                    "modelId":model.id,
                    "expectedVersion":model.metadata_version.to_string(),
                    "tags":["hinge"]
                }),
            )
            .unwrap();
    }
    let filter = json!({
        "search":"Duplicate",
        "tags":["hinge"],
        "fileType":".STL",
        "printed":"all"
    });
    let groups = adapter
        .call_tool(
            &context,
            "find_duplicates",
            json!({"search":filter["search"],"tags":filter["tags"],"fileType":filter["fileType"],"printed":filter["printed"],"limit":1}),
        )
        .unwrap();
    assert_eq!(groups.as_array().unwrap().len(), 1);
    assert_eq!(groups[0]["models"].as_array().unwrap().len(), 2);
    let ids = groups[0]["models"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model["id"].as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids, BTreeSet::from([first.id.as_str(), second.id.as_str()]));

    let status = adapter
        .call_tool(&context, "get_hash_status", filter.clone())
        .unwrap();
    assert_eq!(status["modelCount"], "2");
    assert_eq!(status["hashedCount"], "2");
    assert_eq!(status["unhashedCount"], "0");
    let no_work = adapter
        .call_tool(&context, "calculate_missing_hashes", filter)
        .unwrap();
    assert_eq!(no_work["started"], false);
    assert_eq!(no_work["total"], "0");

    assert!(validate_tool_shape(
        "find_duplicates",
        &json!({"directory":"parts/nested","limit":20})
    )
    .is_err());
    assert!(validate_tool_shape("find_duplicates", &json!({"modelIds":["one","one"]})).is_err());
    let oversized_duplicate_scope = (0..1_001)
        .map(|index| Value::String(format!("model-{index}")))
        .collect::<Vec<_>>();
    assert!(validate_tool_shape(
        "find_duplicates",
        &json!({"modelIds":oversized_duplicate_scope})
    )
    .is_err());
}

#[test]
fn refresh_hashes_with_explicit_empty_root_discovers_new_sources() {
    let temp = tempfile::tempdir().unwrap();
    let source_root = temp.path().join("models");
    std::fs::create_dir_all(&source_root).unwrap();
    let triangle = b"solid part\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid part\n";
    std::fs::write(source_root.join("new-part.stl"), triangle).unwrap();
    let library = temp.path().join("library");
    let integration_settings = temp.path().join("integrations.json");
    let bootstrap = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        library.clone(),
        integration_settings.clone(),
    ))
    .unwrap();
    let root = bootstrap
        .mutate_db(|db| db.add_index_root(&source_root, 60, 50 * 1024 * 1024))
        .unwrap();
    drop(bootstrap);
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig {
        library_root: library,
        integration_settings_path: integration_settings,
        roots: vec![library_runtime::RuntimeRootPolicy {
            id: "models".into(),
            catalog_root_id: Some(root.id),
            label: "Models".into(),
            path: source_root,
            access: library_protocol::RootAccess::ReadOnly,
        }],
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::Scan]),
        local_catalog_root_access: false,
    })
    .unwrap();
    let context = RequestContext {
        principal_id: "empty-root-refresh".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::Scan]),
        request_id: "refresh-empty-root".into(),
    };
    let adapter = ProtocolEngineAdapter::new(Arc::clone(&runtime), HostMode::Headless);
    let result = adapter
        .call_tool(&context, "refresh_hashes", json!({"rootId":"models"}))
        .unwrap();
    assert_eq!(result["jobKind"], "scan");
    for _ in 0..100 {
        if !runtime.scan_control().status().running {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!runtime.scan_control().status().running);
    let models = runtime.read_db(|db| db.list_models(None, None)).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].display_name, "new-part");
}

#[test]
fn search_models_reaches_rating_and_favorite_filters() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let (favorite_match, low_rating_match, not_favorite_match) = runtime
        .mutate_db(|db| {
            let favorite_match = db.create_model("Favorite high rating", None)?;
            let low_rating_match = db.create_model("Favorite low rating", None)?;
            let not_favorite_match = db.create_model("Not favorite", None)?;
            Ok((favorite_match, low_rating_match, not_favorite_match))
        })
        .unwrap();
    let context = RequestContext {
        principal_id: "catalog-reader".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
        request_id: "rating-favorite-search".into(),
    };
    let adapter = ProtocolEngineAdapter::new(runtime, HostMode::Headless);
    for (model, rating, favorite) in [
        (&favorite_match, 5, true),
        (&low_rating_match, 2, true),
        (&not_favorite_match, 5, false),
    ] {
        adapter
            .call_tool(
                &context,
                "update_model",
                json!({
                    "target":{"modelId":model.id,"expectedVersion":model.metadata_version.to_string()},
                    "patch":{"rating":rating,"favorite":favorite}
                }),
            )
            .unwrap();
    }

    let page = adapter
        .call_tool(
            &context,
            "search_models",
            json!({"minimumRating":4,"favorite":true}),
        )
        .unwrap();

    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], favorite_match.id);
    assert_ne!(items[0]["id"], low_rating_match.id);
    assert_ne!(items[0]["id"], not_favorite_match.id);
}

#[test]
fn search_models_filters_by_all_tags_file_type_and_offset_continuation() {
    fn synthetic_stl() -> Vec<u8> {
        let mut bytes = vec![0; 80];
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        for value in [
            0.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
        bytes
    }

    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let models = runtime
        .mutate_db(|db| {
            let mut models = Vec::new();
            for name in ["Alpha filter", "Beta filter", "Gamma filter"] {
                let source = temp.path().join(format!("{}.stl", name.replace(' ', "-")));
                std::fs::write(&source, synthetic_stl()).unwrap();
                models.push(db.import_file(&source, None, Some(name))?.model);
            }
            db.add_model_tags_by_name(
                &models[0].id,
                models[0].metadata_version,
                &["Industrial".into(), "Precision".into()],
            )?;
            db.add_model_tags_by_name(
                &models[1].id,
                models[1].metadata_version,
                &["Industrial".into()],
            )?;
            db.add_model_tags_by_name(
                &models[2].id,
                models[2].metadata_version,
                &["Precision".into()],
            )?;
            Ok(models)
        })
        .unwrap();
    let adapter = ProtocolEngineAdapter::new(runtime, HostMode::Headless);
    let reader = RequestContext {
        principal_id: "search-reader".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
        request_id: "search-tags-file-type-offset".into(),
    };

    let tagged = adapter
        .call_tool(
            &reader,
            "search_models",
            json!({"tags":["industrial","PRECISION"],"fileType":"STL","limit":10}),
        )
        .unwrap();
    let tagged_items = tagged["items"].as_array().unwrap();
    assert_eq!(tagged_items.len(), 1);
    assert_eq!(tagged_items[0]["id"], models[0].id);
    let wrong_file_type = adapter
        .call_tool(
            &reader,
            "search_models",
            json!({"tags":["industrial","precision"],"fileType":"3mf","limit":10}),
        )
        .unwrap();
    assert!(wrong_file_type["items"].as_array().unwrap().is_empty());
    assert!(adapter
        .call_tool(
            &reader,
            "search_models",
            json!({"rootId":"unmatched-root","directory":"Parts"}),
        )
        .is_err());
    assert!(adapter
        .call_tool(&reader, "search_models", json!({"directory":"Parts"}))
        .is_err());

    let first = adapter
        .call_tool(
            &reader,
            "search_models",
            json!({"search":"filter","fileType":".stl","offset":0,"limit":1}),
        )
        .unwrap();
    let second = adapter
        .call_tool(
            &reader,
            "search_models",
            json!({"search":"filter","fileType":"stl","offset":1,"limit":1}),
        )
        .unwrap();
    assert_ne!(first["items"][0]["id"], second["items"][0]["id"]);
    assert!(second["nextCursor"].is_string());
    let third = adapter
        .call_tool(
            &reader,
            "search_models",
            json!({"search":"filter","fileType":"stl","cursor":second["nextCursor"],"limit":1}),
        )
        .unwrap();
    assert_ne!(third["items"][0]["id"], first["items"][0]["id"]);
    assert_ne!(third["items"][0]["id"], second["items"][0]["id"]);

    assert!(adapter
        .call_tool(
            &reader,
            "search_models",
            json!({"cursor":second["nextCursor"],"offset":1}),
        )
        .is_err());
    assert!(adapter
        .call_tool(&reader, "search_models", json!({"offset":100001}))
        .is_err());
}

#[test]
fn export_library_tool_plans_json_only_inside_backup_root() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("library");
    let backups = temp.path().join("backups");
    std::fs::create_dir(&backups).unwrap();
    let mut config = library_runtime::RuntimeConfig::desktop(
        library.clone(),
        temp.path().join("integrations.json"),
    );
    config.capabilities.insert(Capability::BackupWrite);
    config.roots.push(library_runtime::RuntimeRootPolicy {
        id: "backups".into(),
        catalog_root_id: None,
        label: "Backups".into(),
        path: backups.clone(),
        access: library_protocol::RootAccess::BackupOnly,
    });
    let runtime = library_runtime::LibraryRuntime::open(config).unwrap();
    let (parent, model) = runtime
        .mutate_db(|db| {
            let parent = db.create_model("Parent model", None)?;
            db.create_tag("Mechanical", Some("Assembly"))?;
            let model = db.create_model("JSON export model", None)?;
            let model = db.add_model_tags_by_name(
                &model.id,
                model.metadata_version,
                &["Mechanical".to_owned()],
            )?;
            Ok((parent, model))
        })
        .unwrap();
    let context = RequestContext {
        principal_id: "backup-only-writer".into(),
        capabilities: BTreeSet::from([
            Capability::CatalogWrite,
            Capability::Maintenance,
            Capability::BackupWrite,
        ]),
        request_id: "export-library-json".into(),
    };
    runtime
        .update_metadata(
            &context,
            VersionedTarget {
                model_id: model.id.clone(),
                expected_version: model.metadata_version,
            },
            MetadataPatch {
                parent_model_id: Some(parent.id.clone()),
                print_status: Some(PrintStatus::Queued),
                rating: Some(4),
                favorite: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    let adapter = ProtocolEngineAdapter::new(runtime, HostMode::Headless);

    assert!(adapter.supports_tool(&context, "export_library"));
    let plan = adapter
        .call_tool(
            &context,
            "export_library",
            json!({"destPath":"backups/library.json"}),
        )
        .unwrap();
    assert_eq!(plan["actionKind"], "exportLibrary");
    let result = adapter
        .call_tool(
            &context,
            "execute_management_action",
            json!({"token":plan["token"]}),
        )
        .unwrap();
    assert_eq!(result["kind"], "libraryExported");
    assert_eq!(result["destinationHandleId"], "backups");
    assert_eq!(result["fileName"], "library.json");

    let output_path = backups.join("library.json");
    let before = std::fs::read(&output_path).unwrap();
    let exported: Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(exported["version"], "1.0");
    let exported_models = exported["models"].as_array().unwrap();
    assert_eq!(exported_models.len(), 2);
    let exported_model = exported_models
        .iter()
        .find(|model| model["fileName"] == "JSON export model")
        .unwrap();
    assert_eq!(exported_model["parentModel"], "Parent model");
    assert_eq!(exported_model["gogglelabParentModelId"], parent.id);
    assert_eq!(exported_model["print_status"], "queued");
    assert_eq!(exported_model["rating"], 4);
    assert_eq!(exported_model["favorite"], true);
    assert_eq!(exported_model["tags"], json!(["Mechanical"]));
    assert_eq!(
        exported_model["gogglelabTags"],
        json!([{"name":"Mechanical","category":"Assembly"}])
    );
    assert!(exported_model["printEvents"].is_array());
    assert!(exported_model.get("thumbnail").is_none());
    assert!(!result.to_string().contains(temp.path().to_str().unwrap()));

    let raced = adapter
        .call_tool(
            &context,
            "export_library",
            json!({"destPath":"backups/race.json"}),
        )
        .unwrap();
    let raced_path = backups.join("race.json");
    std::fs::write(&raced_path, b"keep existing file").unwrap();
    assert!(adapter
        .call_tool(
            &context,
            "execute_management_action",
            json!({"token":raced["token"]}),
        )
        .is_err());
    assert_eq!(std::fs::read(&output_path).unwrap(), before);
    assert_eq!(std::fs::read(&raced_path).unwrap(), b"keep existing file");
    assert!(!library.join("library.json").exists());
    assert!(adapter
        .call_tool(
            &context,
            "export_library",
            json!({"destPath":"../escape.json"})
        )
        .is_err());
    assert_eq!(model.display_name, "JSON export model");
}

#[test]
fn desktop_slicer_tools_are_capability_gated_and_never_expose_host_paths() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let model = runtime
        .mutate_db(|db| db.create_model("Slicer handoff", None))
        .unwrap();
    let slicers = Arc::new(FakeHostSlicerAdapter::default());
    let adapter = ProtocolEngineAdapter::new(Arc::clone(&runtime), HostMode::Desktop)
        .with_host_slicer(slicers.clone());
    let context = RequestContext {
        principal_id: "desktop-user".into(),
        capabilities: BTreeSet::from([
            Capability::CatalogRead,
            Capability::MediaRead,
            Capability::HostSlicer,
        ]),
        request_id: "slicer".into(),
    };
    let catalog_reader = RequestContext {
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::MediaRead]),
        ..context.clone()
    };

    assert!(!adapter.supports_tool(&catalog_reader, "list_slicers"));
    assert!(!adapter.supports_tool(&catalog_reader, "open_in_slicer"));
    assert!(adapter.supports_tool(&context, "list_slicers"));
    assert!(adapter.supports_tool(&context, "open_in_slicer"));
    assert!(
        adapter
            .capabilities(&context)
            .unwrap()
            .features
            .launch_slicer
    );

    let slicer_list = adapter
        .call_tool(&context, "list_slicers", json!({}))
        .unwrap();
    assert_eq!(slicer_list[0]["id"], "com.bambulab.bambu-studio");
    assert_eq!(slicer_list[0]["displayName"], "Bambu Studio");
    assert!(slicer_list[0].get("appPath").is_none());
    assert!(slicer_list[0].get("bundleId").is_none());

    let result = adapter
        .call_tool(
            &context,
            "open_in_slicer",
            json!({
                "modelId":model.id,
                "expectedVersion":model.metadata_version.to_string(),
                "slicerId":"com.bambulab.bambu-studio"
            }),
        )
        .unwrap();
    assert_eq!(result["opened"], true);
    assert_eq!(
        *slicers.opened.lock().unwrap(),
        vec![(
            model.id.clone(),
            model.metadata_version,
            "com.bambulab.bambu-studio".into()
        )]
    );

    let second_model = runtime
        .mutate_db(|db| db.create_model("Second slicer handoff", None))
        .unwrap();
    let batch_result = adapter
        .call_tool(
            &context,
            "open_in_slicer",
            json!({
                "targets":[
                    {"modelId":model.id,"expectedVersion":model.metadata_version.to_string()},
                    {"modelId":second_model.id,"expectedVersion":second_model.metadata_version.to_string()}
                ],
                "slicerId":"com.bambulab.bambu-studio"
            }),
        )
        .unwrap();
    assert_eq!(batch_result["opened"], true);
    assert_eq!(batch_result["modelIds"].as_array().unwrap().len(), 2);
    assert_eq!(slicers.opened.lock().unwrap().len(), 3);

    assert!(validate_tool_shape(
        "open_in_slicer",
        &json!({
            "modelId":"model-1",
            "expectedVersion":"1",
            "slicerId":"com.bambulab.bambu-studio",
            "filePath":"/private/model.stl"
        })
    )
    .is_err());
    assert!(validate_tool_shape(
        "open_in_slicer",
        &json!({
            "targets":[
                {"modelId":"model-1","expectedVersion":"1"},
                {"modelId":"model-2","expectedVersion":"2"}
            ],
            "slicerId":"com.bambulab.bambu-studio"
        })
    )
    .is_ok());
    assert!(validate_tool_shape(
        "open_in_slicer",
        &json!({
            "targets":[
                {"modelId":"model-1","expectedVersion":"1"},
                {"modelId":"model-1","expectedVersion":"2"}
            ],
            "slicerId":"com.bambulab.bambu-studio"
        })
    )
    .is_err());
    assert!(validate_tool_shape(
        "open_in_slicer",
        &json!({
            "modelId":"model-1","expectedVersion":"1",
            "targets":[{"modelId":"model-2","expectedVersion":"2"}],
            "slicerId":"com.bambulab.bambu-studio"
        })
    )
    .is_err());
    assert!(validate_tool_shape(
        "open_in_slicer",
        &json!({"targets":[],"slicerId":"com.bambulab.bambu-studio"})
    )
    .is_err());
    assert!(validate_tool_shape(
        "open_in_slicer",
        &json!({
            "modelId":"model-1",
            "expectedVersion":"9".repeat(21),
            "slicerId":"com.bambulab.bambu-studio"
        })
    )
    .is_err());
    let open_schema = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "open_in_slicer")
        .unwrap();
    assert_eq!(open_schema["inputSchema"]["additionalProperties"], false);
    assert_eq!(open_schema["inputSchema"]["required"], json!(["slicerId"]));

    let headless =
        ProtocolEngineAdapter::new(runtime, HostMode::Headless).with_host_slicer(slicers);
    assert!(!headless.supports_tool(&context, "list_slicers"));
    assert!(headless.supports_tool(&context, "open_in_slicer"));
    assert!(
        !headless
            .capabilities(&context)
            .unwrap()
            .features
            .launch_slicer
    );
}

#[test]
fn headless_slicer_handoff_returns_path_free_verified_client_commands() {
    let temp = tempfile::tempdir().unwrap();
    let library_root = temp.path().join("library");
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        library_root,
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let mut bytes = vec![0_u8; 80];
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    for value in [
        0.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&[0, 0]);
    let first_path = temp.path().join("first.stl");
    let second_path = temp.path().join("second.stl");
    std::fs::write(&first_path, &bytes).unwrap();
    std::fs::write(&second_path, &bytes).unwrap();
    let first = runtime
        .mutate_db(|db| {
            db.import_file(&first_path, None, None)
                .map(|result| result.model)
        })
        .unwrap();
    let second = runtime
        .mutate_db(|db| {
            db.import_file(&second_path, None, None)
                .map(|result| result.model)
        })
        .unwrap();
    let context = RequestContext {
        principal_id: "remote-client".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::MediaRead]),
        request_id: "remote-slicer-command".into(),
    };
    let adapter = ProtocolEngineAdapter::new(Arc::clone(&runtime), HostMode::Headless);
    assert!(adapter.supports_tool(&context, "open_in_slicer"));
    assert!(!adapter.supports_tool(&context, "list_slicers"));

    let command = adapter
        .call_tool(
            &context,
            "open_in_slicer",
            json!({
                "targets":[
                    {"modelId":first.id,"expectedVersion":first.metadata_version.to_string()},
                    {"modelId":second.id,"expectedVersion":second.metadata_version.to_string()}
                ],
                "slicerId":"com.bambulab.bambu-studio"
            }),
        )
        .unwrap();
    assert_eq!(command["kind"], "clientCommand");
    assert_eq!(command["command"], "openInSlicer");
    assert_eq!(command["targets"].as_array().unwrap().len(), 2);
    assert!(!command
        .to_string()
        .contains(temp.path().to_string_lossy().as_ref()));
    for target in command["targets"].as_array().unwrap() {
        let lease = &target["downloadLease"];
        assert_eq!(
            target["contentRoute"],
            format!("/api/v1/content/{}", lease["id"].as_str().unwrap())
        );
        assert_eq!(lease["kind"], "model");
        assert_eq!(lease["sha256"].as_str().unwrap().len(), 64);
        let mut downloaded = Vec::new();
        runtime
            .stream_content(&context, lease["id"].as_str().unwrap(), &mut downloaded)
            .unwrap();
        assert_eq!(downloaded, bytes);
    }
}

#[tokio::test]
async fn desktop_slicer_tools_are_advertised_and_invokable_over_authenticated_mcp() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let slicers = Arc::new(FakeHostSlicerAdapter::default());
    let engine = Arc::new(
        ProtocolEngineAdapter::new(runtime, HostMode::Desktop).with_host_slicer(slicers.clone()),
    );
    let mut config = security(8);
    config.principal_capabilities.insert(Capability::HostSlicer);
    let app = router(engine, config);
    let initialized = app
        .clone()
        .oneshot(
            request("POST", "/mcp", true)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .body(Body::from(
                    json!({
                        "jsonrpc":"2.0","id":1,"method":"initialize",
                        "params":{"protocolVersion":MCP_PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initialized.status(), StatusCode::OK);
    let session = initialized.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let headers = |builder: http::request::Builder| {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header("mcp-session-id", &session)
            .header("mcp-protocol-version", MCP_PROTOCOL_VERSION)
    };
    let notice = app
        .clone()
        .oneshot(
            headers(request("POST", "/mcp", true))
                .body(Body::from(
                    json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(notice.status(), StatusCode::ACCEPTED);

    let tools = app
        .clone()
        .oneshot(
            headers(request("POST", "/mcp", true))
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let tool_body: Value =
        serde_json::from_slice(&to_bytes(tools.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    let names = tool_body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"list_slicers"));
    assert!(names.contains(&"open_in_slicer"));

    let list = app
        .clone()
        .oneshot(
            headers(request("POST", "/mcp", true))
                .body(Body::from(
                    json!({
                        "jsonrpc":"2.0","id":3,"method":"tools/call",
                        "params":{"name":"list_slicers","arguments":{}}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let list_body: Value =
        serde_json::from_slice(&to_bytes(list.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    assert_eq!(
        list_body["result"]["structuredContent"][0]["displayName"],
        "Bambu Studio"
    );
    assert!(list_body["result"]["structuredContent"][0]
        .get("appPath")
        .is_none());

    let open = app
        .oneshot(
            headers(request("POST", "/mcp", true))
                .body(Body::from(
                    json!({
                        "jsonrpc":"2.0","id":4,"method":"tools/call",
                        "params":{"name":"open_in_slicer","arguments":{
                            "modelId":"model-1","expectedVersion":"7","slicerId":"com.bambulab.bambu-studio"
                        }}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let open_body: Value =
        serde_json::from_slice(&to_bytes(open.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    assert_eq!(open_body["result"]["structuredContent"]["opened"], true);
    assert_eq!(
        *slicers.opened.lock().unwrap(),
        vec![("model-1".into(), 7, "com.bambulab.bambu-studio".into())]
    );
}

#[tokio::test]
async fn browser_printer_fleet_auth_versions_and_completion_are_atomic() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let adapter = Arc::new(ProtocolEngineAdapter::new(
        Arc::clone(&runtime),
        HostMode::Headless,
    ));
    let app = router(adapter.clone(), security(8));
    let input = json!({"nickname":"Bench printer","make":"Elegoo","model":"Centauri Carbon",
        "printerType":"fdm","firmware":null,"host":"192.0.2.10","webUrl":null,
        "bedMm":[256,256,256],"notes":"Synthetic metadata only"});
    let denied = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/printers", false)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let mut read_security = security(8);
    read_security.principal_capabilities = BTreeSet::from([Capability::CatalogRead]);
    let reader = router(adapter.clone(), read_security);
    let denied = reader
        .clone()
        .oneshot(
            request("POST", "/api/v1/printers", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(input.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let injected = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/printers", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"nickname":"bad","notes":"", "endpointApproved":true}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(injected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let response = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/printers", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(input.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let printer: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(printer["metadataVersion"], "1");
    assert_eq!(printer["printCount"], "0");
    assert_eq!(printer["endpointApproved"], false);
    assert_eq!(printer["jobStatus"], "unknown");
    let id = printer["id"].as_str().unwrap();
    let response = app
        .clone()
        .oneshot(
            request("PUT", &format!("/api/v1/printers/{id}"), true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"expectedVersion":"1","input":input}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let stale = app
        .clone()
        .oneshot(
            request("DELETE", &format!("/api/v1/printers/{id}"), true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"expectedVersion":"1"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let response = app.clone().oneshot(request("POST","/api/v1/printer-reminders",true)
        .header(header::CONTENT_TYPE,"application/json").body(Body::from(json!({"printerId":id,"input":{"title":"Lubricate","notes":"test","dueAt":"2026-10-19T00:00:00Z","intervalDays":7}}).to_string())).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let reminder: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    let reminder_id = reminder["id"].as_str().unwrap();
    let complete =
        json!({"expectedVersion":"1","completedAt":"2026-10-19T12:00:00Z","notes":"Done"});
    let response = app
        .clone()
        .oneshot(
            request(
                "POST",
                &format!("/api/v1/printer-reminders/{reminder_id}/complete"),
                true,
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(complete.to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let completed: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(completed["reminder"]["metadataVersion"], "2");
    assert_eq!(completed["maintenanceLog"]["metadataVersion"], "1");
    let replay = app
        .clone()
        .oneshot(
            request(
                "POST",
                &format!("/api/v1/printer-reminders/{reminder_id}/complete"),
                true,
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(complete.to_string()))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::CONFLICT);
    let response = reader
        .oneshot(
            request(
                "GET",
                &format!("/api/v1/printers/{id}/maintenance-logs"),
                true,
            )
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let logs: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(logs.as_array().unwrap().len(), 1);
    let maintenance_guard = runtime.begin_maintenance().unwrap();
    let blocked = app
        .oneshot(
            request("POST", "/api/v1/printers", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(input.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::SERVICE_UNAVAILABLE);
    drop(maintenance_guard);
}

#[tokio::test]
async fn browser_fleet_openapi_mcp_and_endpoint_trust_contract() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let adapter = Arc::new(ProtocolEngineAdapter::new(
        Arc::clone(&runtime),
        HostMode::Headless,
    ));
    let reader = RequestContext {
        principal_id: "fleet-reader".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead]),
        request_id: "reader".into(),
    };
    let writer = RequestContext {
        principal_id: "fleet-writer".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
        request_id: "writer".into(),
    };
    for name in [
        "list_printers",
        "get_printer",
        "list_printer_maintenance_logs",
        "list_printer_reminders",
    ] {
        assert!(adapter.supports_tool(&reader, name), "{name}");
    }
    for name in [
        "create_printer",
        "update_printer",
        "delete_printer",
        "create_printer_maintenance_log",
        "update_printer_maintenance_log",
        "delete_printer_maintenance_log",
        "create_printer_reminder",
        "update_printer_reminder",
        "delete_printer_reminder",
        "complete_printer_reminder",
        "set_printer_reminder_paused",
        "reschedule_printer_reminder",
    ] {
        assert!(!adapter.supports_tool(&reader, name), "{name}");
        assert!(adapter.supports_tool(&writer, name), "{name}");
    }
    let input = json!({"nickname":"Synthetic","notes":"","host":"192.0.2.10","webUrl":"http://192.0.2.10/private?token=discarded"});
    assert_eq!(
        adapter
            .call_tool(&reader, "create_printer", json!({"input":input}))
            .unwrap_err()
            .code,
        "forbidden"
    );
    let printer = adapter
        .call_tool(&writer, "create_printer", json!({"input":input}))
        .unwrap();
    assert_eq!(printer["webUrl"], "http://192.0.2.10");
    let id = printer["id"].as_str().unwrap();
    let edit = json!({"printerId":id,"expectedVersion":"1","input":{"nickname":"Updated","notes":"","host":"192.0.2.11","webUrl":"https://192.0.2.11"}});
    let changed = adapter
        .call_tool(&writer, "update_printer", edit.clone())
        .unwrap();
    assert_eq!(changed["metadataVersion"], "2");
    assert_eq!(changed["endpointApproved"], false);
    let conflict = adapter
        .call_tool(&writer, "update_printer", edit)
        .unwrap_err();
    assert_eq!(conflict.code, "conflict");
    assert_eq!(conflict.current_version, Some(2));
    assert!(adapter.call_tool(&writer,"create_printer",json!({"input":{"nickname":"bad","notes":"","webUrl":"https://user:password@192.0.2.10"}})).is_err());
    assert!(adapter.call_tool(&writer,"update_printer",json!({"printerId":id,"expectedVersion":"2","input":{"nickname":"bad","notes":"","jobStatus":"printing"}})).is_err());
    let log=adapter.call_tool(&writer,"create_printer_maintenance_log",json!({"printerId":id,"input":{"title":"Check","performedAt":"2026-10-19T12:00:00Z","notes":""}})).unwrap();
    let log_id = log["id"].as_str().unwrap();
    let edited_log=adapter.call_tool(&writer,"update_printer_maintenance_log",json!({"logId":log_id,"expectedVersion":"1","input":{"title":"Check complete","performedAt":"2026-10-19T12:00:00Z","notes":""}})).unwrap();
    assert_eq!(edited_log["metadataVersion"], "2");
    assert!(adapter
        .call_tool(
            &writer,
            "delete_printer_maintenance_log",
            json!({"logId":log_id,"expectedVersion":"1"})
        )
        .is_err());
    let reminder=adapter.call_tool(&writer,"create_printer_reminder",json!({"printerId":id,"input":{"title":"Clean","notes":"","dueAt":"2026-10-20T12:00:00Z","intervalDays":null}})).unwrap();
    let reminder_id = reminder["id"].as_str().unwrap();
    let paused = adapter
        .call_tool(
            &writer,
            "set_printer_reminder_paused",
            json!({"reminderId":reminder_id,"expectedVersion":"1","paused":true}),
        )
        .unwrap();
    assert_eq!(paused["status"], "paused");
    let rejected=adapter.call_tool(&writer,"complete_printer_reminder",json!({"reminderId":reminder_id,"expectedVersion":"2","completedAt":"2026-10-20T12:00:00Z","notes":""}));
    assert!(rejected.is_err());
    let logs = adapter
        .call_tool(
            &reader,
            "list_printer_maintenance_logs",
            json!({"printerId":id}),
        )
        .unwrap();
    assert_eq!(
        logs.as_array().unwrap().len(),
        1,
        "invalid completion adds no log"
    );
    let resumed = adapter
        .call_tool(
            &writer,
            "reschedule_printer_reminder",
            json!({"reminderId":reminder_id,"expectedVersion":"2","dueAt":"2026-10-21T12:00:00Z"}),
        )
        .unwrap();
    assert_eq!(resumed["status"], "active");
    assert_eq!(resumed["metadataVersion"], "3");
    adapter
        .call_tool(
            &writer,
            "delete_printer",
            json!({"printerId":id,"expectedVersion":"2"}),
        )
        .unwrap();
    let history = adapter
        .call_tool(&reader, "list_printer_maintenance_logs", json!({}))
        .unwrap();
    assert_eq!(history[0]["printerId"], Value::Null);
    assert_eq!(history[0]["printerNickname"], "Updated");
    assert_eq!(history[0]["metadataVersion"], "3");
    let app = router(adapter, security(8));
    let response = app
        .oneshot(
            request("GET", "/api/v1/openapi.json", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let spec: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    for path in [
        "/api/v1/printers",
        "/api/v1/printer-maintenance-logs",
        "/api/v1/printer-reminders",
        "/api/v1/printer-reminders/{id}/complete",
        "/api/v1/printer-reminders/{id}/pause",
        "/api/v1/printer-reminders/{id}/reschedule",
    ] {
        assert!(spec["paths"].get(path).is_some(), "{path}");
    }
    assert_eq!(
        spec["paths"]["/api/v1/printers"]["post"]["requestBody"]["content"]["application/json"]
            ["schema"]["additionalProperties"],
        false
    );
    assert!(
        spec["paths"]["/api/v1/printers"]["post"]["requestBody"]["content"]["application/json"]
            ["schema"]["properties"]
            .get("endpointApproved")
            .is_none()
    );
    assert_eq!(
        spec["components"]["schemas"]["Printer"]["properties"]["printCount"]["type"],
        "string"
    );
    assert_eq!(
        spec["components"]["schemas"]["Printer"]["properties"]["metadataVersion"]["type"],
        "string"
    );
}

#[test]
fn fleet_mcp_shapes_and_u64_wire_contracts_are_strict() {
    for arguments in [
        json!({"printerId":"x","expectedVersion":1,"input":{"nickname":"x","notes":""}}),
        json!({"printerId":"x","expectedVersion":"01","input":{"nickname":"x","notes":""}}),
        json!({"printerId":"x","expectedVersion":"1","input":{"nickname":"x","notes":"","endpointApproved":true}}),
        json!({"printerId":"x","expectedVersion":"1","input":{"nickname":"x","notes":""},"credentials":"secret"}),
    ] {
        assert!(super::validate_tool_shape("update_printer", &arguments).is_err());
    }
    assert!(super::validate_tool_shape(
        "list_printer_reminders",
        &json!({"includeCompleted":"yes"})
    )
    .is_err());
    let log = library_protocol::PrinterMaintenanceLog {
        id: "log".into(),
        metadata_version: u64::MAX,
        printer_id: None,
        printer_nickname: "".into(),
        title: "History".into(),
        performed_at: "2026-10-19T00:00:00Z".into(),
        notes: "".into(),
        created_at: "2026-10-19T00:00:00Z".into(),
    };
    let mut wire = serde_json::to_value(&log).unwrap();
    assert_eq!(wire["metadataVersion"], u64::MAX.to_string());
    let restored: library_protocol::PrinterMaintenanceLog =
        serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(restored.metadata_version, u64::MAX);
    wire["metadataVersion"] = json!(9007199254740993_u64);
    assert!(serde_json::from_value::<library_protocol::PrinterMaintenanceLog>(wire).is_err());
}

#[test]
fn printer_network_tool_shapes_require_exact_review_and_versions() {
    for (name, valid) in [
        ("get_printer_network_policy", json!({})),
        (
            "plan_printer_discovery",
            json!({"range":"192.0.2.10","includeBambuDiscovery":false}),
        ),
        (
            "start_printer_discovery",
            json!({"planToken":"opaque-plan","confirm":true}),
        ),
        ("get_printer_discovery", json!({"jobId":"opaque-job"})),
        ("cancel_printer_discovery", json!({"jobId":"opaque-job"})),
        (
            "set_printer_endpoint_approval",
            json!({"printerId":"printer-1","expectedVersion":"18446744073709551615","approved":false}),
        ),
        (
            "test_printer_connection",
            json!({"printerId":"printer-1","expectedVersion":"9007199254740993","discoverMainboardId":false,"confirm":true}),
        ),
    ] {
        assert!(validate_tool_shape(name, &valid).is_ok(), "{name}");
        let mut extra = valid.clone();
        extra["mainboardId"] = json!("must-never-be-accepted");
        assert!(validate_tool_shape(name, &extra).is_err(), "{name}");
        for key in valid.as_object().unwrap().keys() {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(validate_tool_shape(name, &missing).is_err(), "{name}:{key}");
        }
    }
    for invalid in [
        json!("01"),
        json!(9007199254740993_u64),
        json!("18446744073709551616"),
        json!("-1"),
    ] {
        assert!(validate_tool_shape(
            "set_printer_endpoint_approval",
            &json!({"printerId":"p","expectedVersion":invalid,"approved":true})
        )
        .is_err());
    }
    assert!(validate_tool_shape(
        "start_printer_discovery",
        &json!({"planToken":"p","confirm":false})
    )
    .is_err());
}

#[tokio::test]
async fn printer_network_routes_require_permission_and_disabled_policy_never_networks() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let adapter = Arc::new(ProtocolEngineAdapter::new(runtime, HostMode::Headless));
    let app = router(adapter.clone(), security(8));
    let unauth = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/printer-network/policy", false)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);
    let policy = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/printer-network/policy", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(policy.status(), StatusCode::OK);
    let value: Value =
        serde_json::from_slice(&to_bytes(policy.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    assert_eq!(value["enabled"], false);
    let denied = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/printer-network/discovery-plans", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"range":"192.0.2.10","includeBambuDiscovery":false}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let mut net_security = security(8);
    net_security
        .principal_capabilities
        .insert(Capability::PrinterNetwork);
    let app = router(adapter.clone(), net_security);
    let hidden = app
        .clone()
        .oneshot(
            request("GET", "/api/v1/openapi.json", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let document: Value =
        serde_json::from_slice(&to_bytes(hidden.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    assert!(document["paths"]["/api/v1/printer-network/policy"].is_object());
    assert!(document["paths"]
        .get("/api/v1/printer-network/discovery-plans")
        .is_none());
    for (uri, body) in [
        (
            "/api/v1/printer-network/discovery-plans",
            json!({"range":"192.0.2.10","includeBambuDiscovery":false,"credentials":"no"}),
        ),
        (
            "/api/v1/printers/p/status-checks",
            json!({"expectedVersion":"1","discoverMainboardId":false,"confirm":true,"mainboardId":"never"}),
        ),
    ] {
        let rejected = app
            .clone()
            .oneshot(
                request("POST", uri, true)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    let reader = RequestContext {
        principal_id: "reader".into(),
        request_id: "test".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead]),
    };
    assert!(adapter.supports_tool(&reader, "get_printer_network_policy"));
    assert!(!adapter.supports_tool(&reader, "plan_printer_discovery"));
    assert!(!adapter.supports_tool(&reader, "test_printer_connection"));
}

#[test]
fn printer_network_permission_matrix_never_uses_an_id_as_authority() {
    for (capabilities, policy_ok, network_ok, write_ok) in [
        (BTreeSet::new(), false, false, false),
        (
            BTreeSet::from([Capability::CatalogRead]),
            true,
            false,
            false,
        ),
        (
            BTreeSet::from([Capability::CatalogRead, Capability::PrinterNetwork]),
            true,
            true,
            false,
        ),
        (
            BTreeSet::from([Capability::CatalogRead, Capability::CatalogWrite]),
            true,
            false,
            false,
        ),
        (
            BTreeSet::from([
                Capability::CatalogRead,
                Capability::CatalogWrite,
                Capability::PrinterNetwork,
            ]),
            true,
            true,
            true,
        ),
    ] {
        let context = RequestContext {
            principal_id: "same-owner".into(),
            request_id: "test".into(),
            capabilities,
        };
        assert_eq!(
            network_permissions(&context, false, true).is_ok(),
            policy_ok
        );
        assert_eq!(
            network_permissions(&context, false, false).is_ok(),
            network_ok
        );
        assert_eq!(network_permissions(&context, true, false).is_ok(), write_ok);
    }
}

fn fake_network_job(
    id: &str,
    status: library_protocol::PrinterDiscoveryJobStatus,
) -> PrinterDiscoveryJob {
    PrinterDiscoveryJob {
        job_id: id.into(),
        status,
        suggestions: Vec::new(),
        normalized_range: "192.0.2.10".into(),
        error_code: None,
    }
}
#[tokio::test]
async fn printer_network_synthetic_discovery_routes_preserve_review_and_job_shapes() {
    let mut config = security(8);
    config
        .principal_capabilities
        .insert(Capability::PrinterNetwork);
    let app = router(Arc::new(FakeEngine::default()), config);
    for (method, path, body, expected) in [
        (
            "POST",
            "/api/v1/printer-network/discovery-plans",
            Some(json!({"range":"192.0.2.10","includeBambuDiscovery":false})),
            json!({"planToken":"synthetic-plan"}),
        ),
        (
            "POST",
            "/api/v1/printer-network/discovery-jobs",
            Some(json!({"planToken":"synthetic-plan","confirm":true})),
            json!({"status":"running"}),
        ),
        (
            "GET",
            "/api/v1/printer-network/discovery-jobs/synthetic-job",
            None,
            json!({"status":"completed"}),
        ),
        (
            "DELETE",
            "/api/v1/printer-network/discovery-jobs/synthetic-job",
            None,
            json!({"status":"cancelled"}),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                request(method, path, true)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(
                        body.map(|body| Body::from(body.to_string()))
                            .unwrap_or_else(Body::empty),
                    )
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{method} {path}");
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        for (field, expected) in expected.as_object().unwrap() {
            assert_eq!(&value[field], expected);
        }
    }
    let rejected = app
        .oneshot(
            request("POST", "/api/v1/printer-network/discovery-jobs", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"planToken":"synthetic-plan","confirm":false}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
}

#[test]
fn thumbnail_cleanup_transport_discard_is_distinct_and_owner_checked() {
    assert!(openapi_document()["paths"]["/api/v1/management/actions/discard"]["post"].is_object());
    assert!(tool_names().contains(&"discard_management_action"));
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("library");
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        library.clone(),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let sentinel = library.join("thumbnails/custom-original.png");
    std::fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    std::fs::write(&sentinel, b"preserve original synthetic bytes").unwrap();
    let adapter = ProtocolEngineAdapter::new(Arc::clone(&runtime), HostMode::Headless);
    let owner = RequestContext {
        principal_id: "cleanup-owner".into(),
        request_id: "cleanup-discard".into(),
        capabilities: BTreeSet::from([
            Capability::CatalogRead,
            Capability::CatalogWrite,
            Capability::Maintenance,
            Capability::MediaWrite,
        ]),
    };
    let plan = adapter
        .call_tool(
            &owner,
            "plan_management_action",
            json!({"action":{"kind":"cleanupOrphans","confirm":true}}),
        )
        .unwrap();
    assert_eq!(plan["thumbnailReview"]["eligibleBytes"], "0");
    let mut other = owner.clone();
    other.principal_id = "another-owner".into();
    assert!(adapter
        .call_tool(
            &other,
            "discard_management_action",
            json!({"token":plan["token"]})
        )
        .is_err());
    assert!(adapter
        .call_tool(
            &owner,
            "discard_management_action",
            json!({"token":plan["token"],"execute":true})
        )
        .is_err());
    let mut reduced = owner.clone();
    reduced.capabilities.clear();
    assert_eq!(
        adapter
            .call_tool(
                &reduced,
                "discard_management_action",
                json!({"token":plan["token"]})
            )
            .unwrap(),
        json!({"discarded":true})
    );
    assert!(adapter
        .call_tool(
            &owner,
            "execute_management_action",
            json!({"token":plan["token"]})
        )
        .is_err());
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"preserve original synthetic bytes"
    );
}

#[tokio::test]
async fn thumbnail_cleanup_transport_discard_rest_is_authenticated_strict_and_nonexecuting() {
    let app = test_router();
    let denied = app
        .clone()
        .oneshot(
            request("POST", "/api/v1/management/actions/discard", false)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"token":"discard-safe"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    for body in [
        json!({"token":""}),
        json!({"token":"x".repeat(257)}),
        json!({"token":"discard-safe","execute":true}),
    ] {
        let rejected = app
            .clone()
            .oneshot(
                request("POST", "/api/v1/management/actions/discard", true)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(rejected.status().is_client_error(), "{}", rejected.status());
    }
    // Fake execution rejects this token. Only the dedicated discard method accepts it.
    let discarded = app
        .oneshot(
            request("POST", "/api/v1/management/actions/discard", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"token":"discard-safe"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(discarded.status(), StatusCode::NO_CONTENT);
    assert!(to_bytes(discarded.into_body(), 1024)
        .await
        .unwrap()
        .is_empty());
}

#[test]
fn thumbnail_cleanup_transport_review_controls_are_explicit_and_bounded() {
    let document = openapi_document();
    let variants = document["paths"]["/api/v1/management/actions/plan"]["post"]["requestBody"]
        ["content"]["application/json"]["schema"]["oneOf"]
        .as_array()
        .unwrap();
    let cleanup = variants
        .iter()
        .find(|schema| schema["properties"]["kind"]["const"] == "cleanupOrphans")
        .unwrap();
    assert_eq!(cleanup["additionalProperties"], false);
    assert_eq!(cleanup["properties"]["continuation"]["maxLength"], 256);
    let reset_schema = variants
        .iter()
        .find(|schema| schema["properties"]["kind"]["const"] == "resetDefaultThumbnails")
        .unwrap();
    assert_eq!(reset_schema["properties"]["confirm"]["const"], true);
    let continued: super::WireManagementAction = serde_json::from_value(json!({
        "kind":"cleanupOrphans", "confirm":true, "continuation":"opaque-cursor"
    }))
    .unwrap();
    assert!(matches!(continued.parse().unwrap(),
        library_protocol::ManagementAction::CleanupOrphans { continuation: Some(value) } if value == "opaque-cursor"));
    for continuation in ["".to_owned(), "x".repeat(257)] {
        let invalid: super::WireManagementAction = serde_json::from_value(json!({
            "kind":"cleanupOrphans", "confirm":true, "continuation":continuation
        }))
        .unwrap();
        assert!(invalid.parse().is_err());
    }
    let reset: super::WireManagementAction = serde_json::from_value(json!({
        "kind":"resetDefaultThumbnails", "confirm":true
    }))
    .unwrap();
    assert_eq!(
        reset.parse().unwrap(),
        library_protocol::ManagementAction::ResetDefaultThumbnails
    );
    for body in [
        json!({"kind":"resetDefaultThumbnails"}),
        json!({"kind":"resetDefaultThumbnails","confirm":true,"path":"/outside"}),
        json!({"kind":"resetDefaultThumbnails","confirm":true,"continuation":"other-mode"}),
    ] {
        assert!(serde_json::from_value::<super::WireManagementAction>(body).is_err());
    }
    let denied: super::WireManagementAction = serde_json::from_value(json!({
        "kind":"resetDefaultThumbnails", "confirm":false
    }))
    .unwrap();
    assert!(denied.parse().is_err());
}
