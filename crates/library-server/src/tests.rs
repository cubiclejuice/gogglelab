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
fn advertised_mutation_tools_have_strict_discoverable_schemas_and_argument_guards() {
    for name in ["update_model", "save_filament", "log_print_event"] {
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
    assert!(
        super::validate_tool_shape("save_filament", &json!({"input":{},"extra":true})).is_err()
    );
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
    runtime
        .mutate_db(|db| db.create_model("Runtime Cube", None).map(|_| ()))
        .unwrap();
    let adapter = Arc::new(ProtocolEngineAdapter::new(runtime, HostMode::Headless));
    let read_context = RequestContext {
        principal_id: "reader".into(),
        capabilities: BTreeSet::from([Capability::CatalogRead]),
        request_id: "read".into(),
    };
    assert!(adapter.supports_tool(&read_context, "search_models"));
    assert!(!adapter.supports_tool(&read_context, "update_model"));
    let app = router(adapter, security(8));
    let response = app
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
    assert!(body["items"][0].get("sourcePath").is_none());
    assert!(body["items"][0].get("storagePath").is_none());
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
    let app = router(
        Arc::new(ProtocolEngineAdapter::new(runtime, HostMode::Headless)),
        security(8),
    );
    let patch=app.clone().oneshot(request("PATCH",&format!("/api/v1/models/{}/metadata",model.id),true).header(header::CONTENT_TYPE,"application/json").body(Body::from(json!({"expectedVersion":model.metadata_version.to_string(),"patch":{"notes":"remote note"}}).to_string())).unwrap()).await.unwrap();
    assert_eq!(patch.status(), StatusCode::OK);
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
    let imported = app
        .oneshot(
            request("POST", "/api/v1/captures/import", true)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"envelope":envelope,"uploads":[token]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(imported.status(), StatusCode::OK);
}
