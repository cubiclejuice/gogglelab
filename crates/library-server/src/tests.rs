use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Barrier;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;

struct FakeEngine;
static SLOW_PRODUCERS: AtomicUsize = AtomicUsize::new(0);
struct ProducerGuard;
impl Drop for ProducerGuard {
    fn drop(&mut self) {
        SLOW_PRODUCERS.fetch_sub(1, Ordering::SeqCst);
    }
}

fn fake_unsupported<T>() -> Result<T, EngineError> {
    Err(EngineError {
        code: "unsupported",
        message: "unsupported".into(),
        retryable: false,
        current_version: None,
    })
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
            _: &str,
        ) -> Result<library_protocol::ManagementResult, EngineError> {
            fake_unsupported()
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

    fn call_tool(&self, _: &RequestContext, name: &str, _: Value) -> Result<Value, EngineError> {
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
        })
    }
    fn stream_content(
        &self,
        _: &RequestContext,
        lease: &str,
        sink: &mut dyn std::io::Write,
    ) -> Result<(), EngineError> {
        if lease == "slow" {
            SLOW_PRODUCERS.fetch_add(1, Ordering::SeqCst);
            let _guard = ProducerGuard;
            for _ in 0..32 {
                sink.write_all(&vec![7_u8; 64 * 1024])
                    .map_err(|_| EngineError {
                        code: "io",
                        message: "closed".into(),
                        retryable: false,
                        current_version: None,
                    })?;
            }
            return Ok(());
        }
        Err(EngineError {
            code: "unsupported",
            message: "unsupported".into(),
            retryable: false,
            current_version: None,
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
        FakeEngine.capabilities(context)
    }

    fn status(&self, context: &RequestContext) -> Result<RuntimeStatus, EngineError> {
        FakeEngine.status(context)
    }

    fn list_models(
        &self,
        context: &RequestContext,
        cursor: Option<&str>,
        limit: u16,
        query: library_protocol::ModelQuery,
    ) -> Result<Page<ModelSummary>, EngineError> {
        FakeEngine.list_models(context, cursor, limit, query)
    }

    fn call_tool(
        &self,
        context: &RequestContext,
        name: &str,
        arguments: Value,
    ) -> Result<Value, EngineError> {
        FakeEngine.call_tool(context, name, arguments)
    }

    fn supports_tool(&self, context: &RequestContext, name: &str) -> bool {
        FakeEngine.supports_tool(context, name)
    }
    fn list_thumbnails(
        &self,
        c: &RequestContext,
        m: &str,
    ) -> Result<Vec<library_protocol::ThumbnailDescriptor>, EngineError> {
        FakeEngine.list_thumbnails(c, m)
    }
    fn set_active_thumbnail(
        &self,
        c: &RequestContext,
        m: &str,
        v: u64,
        t: &str,
    ) -> Result<u64, EngineError> {
        FakeEngine.set_active_thumbnail(c, m, v, t)
    }
    fn add_thumbnail(
        &self,
        c: &RequestContext,
        m: &str,
        v: u64,
        n: String,
        b: Vec<u8>,
    ) -> Result<u64, EngineError> {
        FakeEngine.add_thumbnail(c, m, v, n, b)
    }
    fn delete_thumbnail(
        &self,
        c: &RequestContext,
        m: &str,
        v: u64,
        t: &str,
    ) -> Result<u64, EngineError> {
        FakeEngine.delete_thumbnail(c, m, v, t)
    }
    fn issue_thumbnail_media(
        &self,
        c: &RequestContext,
        m: &str,
        t: &str,
    ) -> Result<library_protocol::ContentLease, EngineError> {
        FakeEngine.issue_thumbnail_media(c, m, t)
    }
    fn issue_model_download(
        &self,
        c: &RequestContext,
        m: &str,
        h: Option<&str>,
    ) -> Result<library_protocol::ContentLease, EngineError> {
        FakeEngine.issue_model_download(c, m, h)
    }
    fn stream_content(
        &self,
        c: &RequestContext,
        l: &str,
        sink: &mut dyn std::io::Write,
    ) -> Result<(), EngineError> {
        FakeEngine.stream_content(c, l, sink)
    }
    fn describe_content(
        &self,
        c: &RequestContext,
        l: &str,
    ) -> Result<library_protocol::ContentLease, EngineError> {
        FakeEngine.describe_content(c, l)
    }
}

fn test_router() -> axum::Router {
    router(Arc::new(FakeEngine), security(8))
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
    let response = router(Arc::new(FakeEngine), policy)
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
    let app = test_router();
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
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(SLOW_PRODUCERS.load(Ordering::SeqCst), 0);
    drop(held);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let admitted = app
        .oneshot(
            request("GET", "/api/v1/content/slow", true)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admitted.status(), StatusCode::OK);
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
    for unsupported in [
        "quarantineLooseSource",
        "undoQuarantine",
        "moveLooseSource",
        "purgeCatalog",
        "cleanupOrphans",
    ] {
        assert!(!management["inputSchema"].to_string().contains(unsupported));
    }
    for name in [
        "update_model",
        "save_filament",
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
    let app = router(
        Arc::new(ProtocolEngineAdapter::new(runtime, HostMode::Headless)),
        security(8),
    );
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
    let app = router_with_static(Arc::new(FakeEngine), security(8), temp.path()).unwrap();
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
    let app = router_with_static(Arc::new(FakeEngine), security(8), &web).unwrap();
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
    let error = router_with_static(Arc::new(FakeEngine), security(8), temp.path())
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
async fn real_runtime_embedded_3mf_proposal_and_versioned_apply_work() {
    use base64::Engine as _;

    const MODEL_3MF: &str = "UEsDBBQAAAAAAK19S12dfITk7wEAAO8BAAAQAAAAM0QvM2Rtb2RlbC5tb2RlbDw/eG1sIHZlcnNpb249IjEuMCIgZW5jb2Rpbmc9IlVURi04Ij8+PG1vZGVsIHVuaXQ9Im1pbGxpbWV0ZXIiIHhtbG5zPSJodHRwOi8vc2NoZW1hcy5taWNyb3NvZnQuY29tLzNkbWFudWZhY3R1cmluZy9jb3JlLzIwMTUvMDIiPjxtZXRhZGF0YSBuYW1lPSJEZXNpZ25lciI+RW1iZWRkZWQgRGVzaWduZXI8L21ldGFkYXRhPjxtZXRhZGF0YSBuYW1lPSJMaWNlbnNlIj5DQzA8L21ldGFkYXRhPjxyZXNvdXJjZXM+PG9iamVjdCBpZD0iMSIgdHlwZT0ibW9kZWwiPjxtZXNoPjx2ZXJ0aWNlcz48dmVydGV4IHg9IjAiIHk9IjAiIHo9IjAiLz48dmVydGV4IHg9IjEiIHk9IjAiIHo9IjAiLz48dmVydGV4IHg9IjAiIHk9IjEiIHo9IjAiLz48L3ZlcnRpY2VzPjx0cmlhbmdsZXM+PHRyaWFuZ2xlIHYxPSIwIiB2Mj0iMSIgdjM9IjIiLz48L3RyaWFuZ2xlcz48L21lc2g+PC9vYmplY3Q+PC9yZXNvdXJjZXM+PGJ1aWxkPjxpdGVtIG9iamVjdGlkPSIxIi8+PC9idWlsZD48L21vZGVsPlBLAQIUAxQAAAAAAK19S12dfITk7wEAAO8BAAAQAAAAAAAAAAAAAACAAQAAAAAzRC8zZG1vZGVsLm1vZGVsUEsFBgAAAAABAAEAPgAAAB0CAAAAAA==";
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("metadata.3mf");
    std::fs::write(
        &source,
        base64::engine::general_purpose::STANDARD
            .decode(MODEL_3MF)
            .unwrap(),
    )
    .unwrap();
    let runtime = library_runtime::LibraryRuntime::open(library_runtime::RuntimeConfig::desktop(
        temp.path().join("library"),
        temp.path().join("integrations.json"),
    ))
    .unwrap();
    let model = runtime
        .mutate_db(|db| db.import_file(&source, None, None).map(|value| value.model))
        .unwrap();
    let app = router(
        Arc::new(ProtocolEngineAdapter::new(runtime, HostMode::Headless)),
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
                    "selection":{"designer":true,"sourceUrl":false,"notes":false,"modelLicense":false}
                })
                .to_string(),
            ))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(applied.status(), StatusCode::OK);
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
    let model = runtime
        .mutate_db(|db| db.create_model("JSON export model", None))
        .unwrap();
    let adapter = ProtocolEngineAdapter::new(runtime, HostMode::Headless);
    let context = RequestContext {
        principal_id: "backup-only-writer".into(),
        capabilities: BTreeSet::from([Capability::Maintenance, Capability::BackupWrite]),
        request_id: "export-library-json".into(),
    };

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
    assert_eq!(exported["models"].as_array().unwrap().len(), 1);
    assert_eq!(exported["models"][0]["fileName"], "JSON export model");
    assert!(exported["models"][0]["favorite"].is_boolean());
    assert!(exported["models"][0]["printEvents"].is_array());
    assert!(exported["models"][0].get("thumbnail").is_none());
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
