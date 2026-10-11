#![recursion_limit = "256"]

mod bootstrap;
mod confirmation;
mod mcp;
mod security;

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Multipart, Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::oneshot;
use tokio::sync::Semaphore;

pub use bootstrap::{load_or_create_bearer_secret, BearerSecret};
pub use confirmation::{ConfirmationPlan, ConfirmationStore};
pub use library_protocol::{Capability, RequestContext};
use library_protocol::{
    CaptureEngine, CaptureEnvelope, CaptureImportResult, CatalogModel, ContentLease, Filament,
    FilamentEngine, FilamentInput, Folder, FolderEngine, LibraryEngine as ProtocolLibraryEngine,
    MetadataEngine, MetadataPatch, ModelAvailability, ModelFilamentAssignment,
    ModelQuery as ProtocolModelQuery, ModelSort, MutationVersion, PageRequest, PrintAttempt,
    PrintAttemptInput, PrintHistoryEngine, RuntimeError, RuntimeErrorCode, SortDirection, Tag,
    TagEngine, ThumbnailDescriptor, UploadIntent, UploadToken, VersionedTarget,
};
pub use security::{AllowedAuthority, AllowedOrigin, SecurityConfig};
use sha2::{Digest, Sha256};

pub const API_VERSION: &str = "1";
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
pub const MAX_JSON_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_MCP_BODY_BYTES: usize = 12 * 1024 * 1024;
pub const MAX_THUMBNAIL_UPLOAD_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PAGE_SIZE: u16 = 100;

pub struct ServerHandle {
    local_addr: std::net::SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Result<(), std::io::Error>>,
}

impl ServerHandle {
    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.local_addr
    }

    pub async fn stop(mut self) -> Result<(), String> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let mut task = self.task;
        match tokio::time::timeout(std::time::Duration::from_secs(5), &mut task).await {
            Ok(result) => result
                .map_err(|_| "Server task stopped unexpectedly".to_string())?
                .map_err(|_| "Server listener failed".to_string()),
            Err(_) => {
                task.abort();
                let _ = task.await;
                Ok(())
            }
        }
    }
}

pub async fn start_listener(
    listener: tokio::net::TcpListener,
    app: Router,
) -> Result<ServerHandle, String> {
    let local_addr = listener
        .local_addr()
        .map_err(|_| "Server listener address is unavailable")?;
    let (shutdown, receiver) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = receiver.await;
        })
        .await
    });
    Ok(ServerHandle {
        local_addr,
        shutdown: Some(shutdown),
        task,
    })
}

pub struct ServerState<E: LibraryEngine> {
    engine: Arc<E>,
    security: Arc<SecurityConfig>,
    admission: Arc<Semaphore>,
    content_admission: Arc<Semaphore>,
    content_send_timeout: std::time::Duration,
    mcp_sessions: Arc<mcp::SessionStore>,
}

impl<E: LibraryEngine> Clone for ServerState<E> {
    fn clone(&self) -> Self {
        Self {
            engine: Arc::clone(&self.engine),
            security: Arc::clone(&self.security),
            admission: Arc::clone(&self.admission),
            content_admission: Arc::clone(&self.content_admission),
            content_send_timeout: self.content_send_timeout,
            mcp_sessions: Arc::clone(&self.mcp_sessions),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostCapabilities {
    pub api_version: String,
    pub host_mode: HostMode,
    pub library_epoch: String,
    pub catalog_revision: String,
    pub features: FeatureSet,
    pub limits: LimitSet,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum HostMode {
    Desktop,
    Headless,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FeatureSet {
    pub reveal_source: bool,
    pub trash_source: bool,
    pub launch_slicer: bool,
    pub download_source: bool,
    pub download_bundle: bool,
    pub ai: bool,
    pub spoolman: bool,
    pub mcp: bool,
    pub capture_inbox: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LimitSet {
    pub page_size_max: u16,
    pub thumbnail_upload_bytes: u64,
    pub model_upload_bytes: u64,
    pub active_previews_max: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub library_epoch: String,
    pub catalog_revision: String,
    pub maintenance: bool,
    pub jobs: Vec<JobStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JobStatus {
    pub id: String,
    pub kind: String,
    pub state: String,
    pub completed: u64,
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelSummary {
    pub id: String,
    pub display_name: String,
    pub folder_id: String,
    pub metadata_version: String,
    pub format: Option<String>,
    pub bytes: String,
    pub content_hash: Option<String>,
    pub tags: Vec<String>,
    pub filament_ids: Vec<String>,
    pub source_kind: String,
    pub availability: String,
    pub catalog_kind: String,
    pub thumbnail_id: Option<String>,
    pub designer: Option<String>,
    pub source_url: Option<String>,
    pub notes: Option<String>,
    pub model_license: Option<String>,
    pub print_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub snapshot_revision: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelQuery {
    cursor: Option<String>,
    limit: Option<u16>,
    query: Option<String>,
    folder_id: Option<String>,
    sort: Option<ModelSort>,
    direction: Option<SortDirection>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireMutation {
    model_id: String,
    metadata_version: String,
}
impl From<MutationVersion> for WireMutation {
    fn from(v: MutationVersion) -> Self {
        Self {
            model_id: v.model_id,
            metadata_version: v.metadata_version.to_string(),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireFolder {
    id: String,
    name: String,
    parent_id: Option<String>,
    model_count: String,
}
impl From<Folder> for WireFolder {
    fn from(v: Folder) -> Self {
        Self {
            id: v.id,
            name: v.name,
            parent_id: v.parent_id,
            model_count: v.model_count.to_string(),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireFilament {
    id: String,
    version: String,
    brand: String,
    material: String,
    color: String,
    name: String,
    notes: String,
    origin: String,
    resource_kind: String,
    source_id: Option<String>,
    external_id: Option<String>,
    parent_filament_id: Option<String>,
    archived: bool,
    source_missing: bool,
}
impl From<Filament> for WireFilament {
    fn from(v: Filament) -> Self {
        Self {
            id: v.id,
            version: v.version.to_string(),
            brand: v.brand,
            material: v.material,
            color: v.color,
            name: v.name,
            notes: v.notes,
            origin: v.origin,
            resource_kind: v.resource_kind,
            source_id: v.source_id,
            external_id: v.external_id,
            parent_filament_id: v.parent_filament_id,
            archived: v.archived,
            source_missing: v.source_missing,
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireThumbnail {
    thumbnail_id: String,
    model_id: String,
    ordinal: u16,
    active: bool,
    custom: bool,
    mime: String,
    bytes: String,
    sha256: String,
}
impl From<ThumbnailDescriptor> for WireThumbnail {
    fn from(v: ThumbnailDescriptor) -> Self {
        Self {
            thumbnail_id: v.thumbnail_id,
            model_id: v.model_id,
            ordinal: v.ordinal,
            active: v.active,
            custom: v.custom,
            mime: v.mime,
            bytes: v.bytes.to_string(),
            sha256: v.sha256,
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireLease {
    id: String,
    file_name: String,
    mime: String,
    bytes: String,
    sha256: String,
    library_epoch: String,
    expires_at: String,
}
impl From<ContentLease> for WireLease {
    fn from(v: ContentLease) -> Self {
        Self {
            id: v.id,
            file_name: v.file_name,
            mime: v.mime,
            bytes: v.bytes.to_string(),
            sha256: v.sha256,
            library_epoch: v.library_epoch.to_string(),
            expires_at: v.expires_at,
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireUpload {
    id: String,
    item_id: String,
    capture_id: String,
    expires_at: String,
    library_epoch: String,
}
impl From<UploadToken> for WireUpload {
    fn from(v: UploadToken) -> Self {
        Self {
            id: v.id,
            item_id: v.item_id,
            capture_id: v.capture_id,
            expires_at: v.expires_at,
            library_epoch: v.library_epoch.to_string(),
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireTarget {
    model_id: String,
    expected_version: String,
}
impl WireTarget {
    fn parse(self) -> Result<VersionedTarget, ApiError> {
        Ok(VersionedTarget {
            model_id: self.model_id,
            expected_version: parse_decimal(&self.expected_version)?,
        })
    }
}
fn parse_decimal(value: &str) -> Result<u64, ApiError> {
    if value == "0" {
        return Ok(0);
    }
    if value.is_empty() || value.starts_with('0') || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(api_bad("invalidInput", "decimal string is invalid"));
    }
    value
        .parse()
        .map_err(|_| api_bad("invalidInput", "decimal string is out of range"))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireUploadIntent {
    item_id: String,
    capture_id: String,
    file_name: String,
    bytes: String,
    sha256: String,
}
impl WireUploadIntent {
    fn parse(self) -> Result<UploadIntent, ApiError> {
        Ok(UploadIntent {
            item_id: self.item_id,
            capture_id: self.capture_id,
            file_name: self.file_name,
            bytes: parse_decimal(&self.bytes)?,
            sha256: self.sha256,
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireUploadInput {
    id: String,
    item_id: String,
    capture_id: String,
    expires_at: String,
    library_epoch: String,
}
impl WireUploadInput {
    fn parse(self) -> Result<UploadToken, ApiError> {
        Ok(UploadToken {
            id: self.id,
            item_id: self.item_id,
            capture_id: self.capture_id,
            expires_at: self.expires_at,
            library_epoch: parse_decimal(&self.library_epoch)?,
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireAssignment {
    target: WireTarget,
    filament_ids: Vec<String>,
}
impl WireAssignment {
    fn parse(self) -> Result<ModelFilamentAssignment, ApiError> {
        Ok(ModelFilamentAssignment {
            target: self.target.parse()?,
            filament_ids: self.filament_ids,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
    pub current_version: Option<u64>,
}

pub trait LibraryEngine: Send + Sync + 'static {
    fn capabilities(&self, context: &RequestContext) -> Result<HostCapabilities, EngineError>;
    fn status(&self, context: &RequestContext) -> Result<RuntimeStatus, EngineError>;
    fn list_models(
        &self,
        context: &RequestContext,
        cursor: Option<&str>,
        limit: u16,
        query: ProtocolModelQuery,
    ) -> Result<Page<ModelSummary>, EngineError>;
    fn call_tool(
        &self,
        context: &RequestContext,
        name: &str,
        arguments: Value,
    ) -> Result<Value, EngineError>;

    fn supports_tool(&self, context: &RequestContext, name: &str) -> bool;

    fn list_thumbnails(
        &self,
        context: &RequestContext,
        model_id: &str,
    ) -> Result<Vec<ThumbnailDescriptor>, EngineError>;
    fn set_active_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        thumbnail_id: &str,
    ) -> Result<u64, EngineError>;
    fn add_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        file_name: String,
        bytes: Vec<u8>,
    ) -> Result<u64, EngineError>;
    fn delete_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        thumbnail_id: &str,
    ) -> Result<u64, EngineError>;
    fn issue_thumbnail_media(
        &self,
        context: &RequestContext,
        model_id: &str,
        thumbnail_id: &str,
    ) -> Result<ContentLease, EngineError>;
    fn issue_model_download(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_hash: Option<&str>,
    ) -> Result<ContentLease, EngineError>;
    fn stream_content(
        &self,
        context: &RequestContext,
        lease_id: &str,
        sink: &mut dyn std::io::Write,
    ) -> Result<(), EngineError>;
    fn describe_content(
        &self,
        context: &RequestContext,
        lease_id: &str,
    ) -> Result<ContentLease, EngineError>;
    fn update_metadata(
        &self,
        c: &RequestContext,
        t: VersionedTarget,
        p: MetadataPatch,
    ) -> Result<MutationVersion, EngineError>;
    fn bulk_update_metadata(
        &self,
        c: &RequestContext,
        t: Vec<VersionedTarget>,
        p: MetadataPatch,
    ) -> Result<Vec<MutationVersion>, EngineError>;
    fn list_tags(&self, c: &RequestContext) -> Result<Vec<Tag>, EngineError>;
    fn create_tag(
        &self,
        c: &RequestContext,
        n: String,
        k: Option<String>,
    ) -> Result<Tag, EngineError>;
    fn update_tag(
        &self,
        c: &RequestContext,
        id: &str,
        n: String,
        k: Option<String>,
    ) -> Result<Tag, EngineError>;
    fn delete_tag(&self, c: &RequestContext, id: &str) -> Result<(), EngineError>;
    fn list_folders(&self, c: &RequestContext) -> Result<Vec<Folder>, EngineError>;
    fn create_folder(
        &self,
        c: &RequestContext,
        n: String,
        p: Option<String>,
    ) -> Result<Folder, EngineError>;
    fn rename_folder(&self, c: &RequestContext, id: &str, n: String)
        -> Result<Folder, EngineError>;
    fn move_folder(&self, c: &RequestContext, id: &str, p: &str) -> Result<Folder, EngineError>;
    fn delete_folder(&self, c: &RequestContext, id: &str) -> Result<(), EngineError>;
    fn list_filaments(&self, c: &RequestContext, u: bool) -> Result<Vec<Filament>, EngineError>;
    fn create_filament(
        &self,
        c: &RequestContext,
        i: FilamentInput,
    ) -> Result<Filament, EngineError>;
    fn update_filament(
        &self,
        c: &RequestContext,
        id: &str,
        v: u64,
        i: FilamentInput,
    ) -> Result<Filament, EngineError>;
    fn delete_filament(
        &self,
        c: &RequestContext,
        id: &str,
        v: u64,
    ) -> Result<Option<Filament>, EngineError>;
    fn bulk_assign_filaments(
        &self,
        c: &RequestContext,
        a: Vec<ModelFilamentAssignment>,
    ) -> Result<Vec<MutationVersion>, EngineError>;
    fn list_history(&self, c: &RequestContext, m: &str) -> Result<Vec<PrintAttempt>, EngineError>;
    fn create_history(
        &self,
        c: &RequestContext,
        m: &str,
        i: PrintAttemptInput,
    ) -> Result<PrintAttempt, EngineError>;
    fn update_history(
        &self,
        c: &RequestContext,
        id: &str,
        i: PrintAttemptInput,
    ) -> Result<PrintAttempt, EngineError>;
    fn delete_history(&self, c: &RequestContext, id: &str) -> Result<(), EngineError>;
    fn stage_upload(
        &self,
        c: &RequestContext,
        i: UploadIntent,
        b: Vec<u8>,
    ) -> Result<UploadToken, EngineError>;
    fn import_capture(
        &self,
        c: &RequestContext,
        e: CaptureEnvelope,
        u: Vec<UploadToken>,
    ) -> Result<CaptureImportResult, EngineError>;
    fn get_capture(&self, c: &RequestContext, id: &str)
        -> Result<CaptureImportResult, EngineError>;
}

pub struct ProtocolEngineAdapter<
    E: ProtocolLibraryEngine
        + MetadataEngine
        + TagEngine
        + FolderEngine
        + FilamentEngine
        + PrintHistoryEngine
        + CaptureEngine,
> {
    inner: Arc<E>,
    host_mode: HostMode,
}

impl<
        E: ProtocolLibraryEngine
            + MetadataEngine
            + TagEngine
            + FolderEngine
            + FilamentEngine
            + PrintHistoryEngine
            + CaptureEngine,
    > ProtocolEngineAdapter<E>
{
    pub fn new(inner: Arc<E>, host_mode: HostMode) -> Self {
        Self { inner, host_mode }
    }
}

impl<
        E: ProtocolLibraryEngine
            + MetadataEngine
            + TagEngine
            + FolderEngine
            + FilamentEngine
            + PrintHistoryEngine
            + CaptureEngine
            + 'static,
    > LibraryEngine for ProtocolEngineAdapter<E>
{
    fn capabilities(&self, context: &RequestContext) -> Result<HostCapabilities, EngineError> {
        let info = self
            .inner
            .runtime_info(context)
            .map_err(map_runtime_error)?;
        Ok(HostCapabilities {
            api_version: API_VERSION.into(),
            host_mode: self.host_mode,
            library_epoch: info.library_epoch.to_string(),
            catalog_revision: info.catalog_revision.to_string(),
            features: FeatureSet {
                reveal_source: false,
                trash_source: false,
                launch_slicer: false,
                download_source: info.capabilities.contains(&Capability::MediaRead),
                download_bundle: false,
                ai: false,
                spoolman: false,
                mcp: true,
                capture_inbox: info.capabilities.contains(&Capability::CatalogWrite),
            },
            limits: LimitSet {
                page_size_max: MAX_PAGE_SIZE,
                thumbnail_upload_bytes: MAX_THUMBNAIL_UPLOAD_BYTES as u64,
                model_upload_bytes: 100 * 1024 * 1024,
                active_previews_max: 4,
            },
        })
    }

    fn status(&self, context: &RequestContext) -> Result<RuntimeStatus, EngineError> {
        let status = self
            .inner
            .runtime_status(context)
            .map_err(map_runtime_error)?;
        Ok(RuntimeStatus {
            library_epoch: status.library_epoch.to_string(),
            catalog_revision: status.catalog_revision.to_string(),
            maintenance: status.maintenance,
            jobs: Vec::new(),
        })
    }

    fn list_models(
        &self,
        context: &RequestContext,
        cursor: Option<&str>,
        limit: u16,
        query: ProtocolModelQuery,
    ) -> Result<Page<ModelSummary>, EngineError> {
        let page = self
            .inner
            .list_models(
                context,
                query,
                PageRequest {
                    cursor: cursor.map(str::to_owned),
                    limit,
                },
            )
            .map_err(map_runtime_error)?;
        Ok(Page {
            items: page.items.into_iter().map(http_model).collect(),
            next_cursor: page.next_cursor,
            snapshot_revision: page.snapshot_revision.to_string(),
        })
    }

    fn call_tool(
        &self,
        context: &RequestContext,
        name: &str,
        arguments: Value,
    ) -> Result<Value, EngineError> {
        macro_rules! encoded {
            ($value:expr) => {
                serde_json::to_value($value).map_err(|_| internal_tool())
            };
        }
        validate_tool_shape(name, &arguments)?;
        match name {
            "get_folder_tree" => {
                return encoded!(self
                    .list_folders(context)?
                    .into_iter()
                    .map(WireFolder::from)
                    .collect::<Vec<_>>())
            }
            "list_tags" => return encoded!(self.list_tags(context)?),
            "list_filaments" => {
                return encoded!(self
                    .list_filaments(
                        context,
                        arguments
                            .get("includeUnavailable")
                            .and_then(Value::as_bool)
                            .unwrap_or(true)
                    )?
                    .into_iter()
                    .map(WireFilament::from)
                    .collect::<Vec<_>>())
            }
            "update_model" => {
                let target: WireTarget = serde_json::from_value(
                    arguments.get("target").cloned().ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                let patch: MetadataPatch = serde_json::from_value(
                    arguments.get("patch").cloned().ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                return encoded!(WireMutation::from(self.update_metadata(
                    context,
                    target.parse().map_err(|_| invalid_tool())?,
                    patch
                )?));
            }
            "update_models_batch" => {
                let request: BulkMetadataRequest =
                    serde_json::from_value(arguments).map_err(|_| invalid_tool())?;
                let targets = request
                    .targets
                    .into_iter()
                    .map(|v| v.parse().map_err(|_| invalid_tool()))
                    .collect::<Result<Vec<_>, _>>()?;
                return encoded!(self
                    .bulk_update_metadata(context, targets, request.patch)?
                    .into_iter()
                    .map(WireMutation::from)
                    .collect::<Vec<_>>());
            }
            "add_tag" => {
                let value: NameCategory =
                    serde_json::from_value(arguments).map_err(|_| invalid_tool())?;
                return encoded!(self.create_tag(context, value.name, value.category)?);
            }
            "rename_tag" => {
                let id = arguments
                    .get("tagId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                let name = arguments
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?
                    .to_owned();
                let category = arguments
                    .get("category")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                return encoded!(self.update_tag(context, id, name, category)?);
            }
            "delete_tag" => {
                let id = arguments
                    .get("tagId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                self.delete_tag(context, id)?;
                return Ok(serde_json::json!({"ok":true}));
            }
            "save_filament" => {
                let input: FilamentInput = serde_json::from_value(
                    arguments.get("input").cloned().ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                if let Some(id) = arguments.get("filamentId").and_then(Value::as_str) {
                    let version = tool_version(&arguments)?;
                    return encoded!(WireFilament::from(
                        self.update_filament(context, id, version, input)?
                    ));
                }
                return encoded!(WireFilament::from(self.create_filament(context, input)?));
            }
            "delete_filament" => {
                let id = arguments
                    .get("filamentId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                return encoded!(self
                    .delete_filament(context, id, tool_version(&arguments)?)?
                    .map(WireFilament::from));
            }
            "set_model_filaments" => {
                let values: Assignments =
                    serde_json::from_value(arguments).map_err(|_| invalid_tool())?;
                let assignments = values
                    .assignments
                    .into_iter()
                    .map(|v| v.parse().map_err(|_| invalid_tool()))
                    .collect::<Result<Vec<_>, _>>()?;
                return encoded!(self
                    .bulk_assign_filaments(context, assignments)?
                    .into_iter()
                    .map(WireMutation::from)
                    .collect::<Vec<_>>());
            }
            "get_print_events" => {
                let id = arguments
                    .get("modelId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                return encoded!(self.list_history(context, id)?);
            }
            "log_print_event" => {
                let id = arguments
                    .get("modelId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                let input: PrintAttemptInput = serde_json::from_value(
                    arguments.get("input").cloned().ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                return encoded!(self.create_history(context, id, input)?);
            }
            "delete_print_event" => {
                let id = arguments
                    .get("attemptId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                self.delete_history(context, id)?;
                return Ok(serde_json::json!({"ok":true}));
            }
            _ => {}
        }
        if name != "search_models" {
            let model_id = arguments
                .get("modelId")
                .and_then(Value::as_str)
                .ok_or_else(invalid_tool)?
                .to_owned();
            return match name {
                "get_thumbnails" => serde_json::to_value(
                    self.list_thumbnails(context, &model_id)?
                        .into_iter()
                        .map(WireThumbnail::from)
                        .collect::<Vec<_>>(),
                )
                .map_err(|_| internal_tool()),
                "set_default_thumbnail" => {
                    let version = tool_version(&arguments)?;
                    let id = arguments
                        .get("thumbnailId")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid_tool)?;
                    Ok(
                        serde_json::json!({"metadataVersion":self.set_active_thumbnail(context,&model_id,version,id)?.to_string()}),
                    )
                }
                "delete_thumbnail" => {
                    let version = tool_version(&arguments)?;
                    let id = arguments
                        .get("thumbnailId")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid_tool)?;
                    Ok(
                        serde_json::json!({"metadataVersion":self.delete_thumbnail(context,&model_id,version,id)?.to_string()}),
                    )
                }
                "add_thumbnail" | "set_thumbnail" => {
                    let version = tool_version(&arguments)?;
                    let bytes = decode_tool_image(&arguments)?;
                    Ok(
                        serde_json::json!({"metadataVersion":self.add_thumbnail(context,&model_id,version,"thumbnail-upload".into(),bytes)?.to_string()}),
                    )
                }
                "download_model" => {
                    serde_json::to_value(WireLease::from(self.issue_model_download(
                        context,
                        &model_id,
                        arguments.get("expectedHash").and_then(Value::as_str),
                    )?))
                    .map_err(|_| internal_tool())
                }
                _ => Err(EngineError {
                    code: "unsupported",
                    message: "Tool is not connected to the runtime yet".into(),
                    retryable: false,
                    current_version: None,
                }),
            };
        }
        let object = arguments.as_object().ok_or_else(|| EngineError {
            code: "invalidInput",
            message: "Tool arguments must be an object".into(),
            retryable: false,
            current_version: None,
        })?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "query" | "cursor" | "limit"))
            || object
                .get("query")
                .is_some_and(|value| value.as_str().is_none_or(|value| value.len() > 256))
            || object
                .get("cursor")
                .is_some_and(|value| value.as_str().is_none_or(|value| value.len() > 512))
            || object.get("limit").is_some_and(|value| {
                value
                    .as_u64()
                    .is_none_or(|value| value == 0 || value > u64::from(MAX_PAGE_SIZE))
            })
        {
            return Err(EngineError {
                code: "invalidInput",
                message: "Search arguments are invalid".into(),
                retryable: false,
                current_version: None,
            });
        }
        let limit = arguments
            .get("limit")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(50)
            .min(MAX_PAGE_SIZE);
        let page = self.list_models(
            context,
            arguments.get("cursor").and_then(Value::as_str),
            limit,
            ProtocolModelQuery {
                search: arguments
                    .get("query")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                ..ProtocolModelQuery::default()
            },
        )?;
        serde_json::to_value(page).map_err(|_| EngineError {
            code: "internal",
            message: "Catalog page could not be encoded".into(),
            retryable: false,
            current_version: None,
        })
    }

    fn supports_tool(&self, context: &RequestContext, name: &str) -> bool {
        let connected = matches!(
            name,
            "search_models"
                | "get_thumbnails"
                | "set_default_thumbnail"
                | "delete_thumbnail"
                | "add_thumbnail"
                | "set_thumbnail"
                | "download_model"
                | "get_folder_tree"
                | "list_tags"
                | "add_tag"
                | "rename_tag"
                | "delete_tag"
                | "list_filaments"
                | "save_filament"
                | "delete_filament"
                | "set_model_filaments"
                | "get_print_events"
                | "log_print_event"
                | "delete_print_event"
                | "update_model"
                | "update_models_batch"
        );
        let write = matches!(
            name,
            "add_thumbnail"
                | "set_thumbnail"
                | "set_default_thumbnail"
                | "delete_thumbnail"
                | "update_model"
                | "update_models_batch"
                | "add_tag"
                | "rename_tag"
                | "delete_tag"
                | "save_filament"
                | "delete_filament"
                | "set_model_filaments"
                | "log_print_event"
                | "delete_print_event"
        );
        connected
            && context.capabilities.contains(if write {
                &Capability::CatalogWrite
            } else {
                &Capability::CatalogRead
            })
    }

    fn list_thumbnails(
        &self,
        context: &RequestContext,
        model_id: &str,
    ) -> Result<Vec<ThumbnailDescriptor>, EngineError> {
        self.inner
            .list_thumbnails(context, model_id)
            .map_err(map_runtime_error)
    }
    fn set_active_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        thumbnail_id: &str,
    ) -> Result<u64, EngineError> {
        self.inner
            .set_active_thumbnail(context, model_id, expected_version, thumbnail_id)
            .map_err(map_runtime_error)
    }
    fn add_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        file_name: String,
        bytes: Vec<u8>,
    ) -> Result<u64, EngineError> {
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        let intent = UploadIntent {
            item_id: model_id.into(),
            capture_id: uuid::Uuid::new_v4().simple().to_string(),
            file_name,
            bytes: bytes.len() as u64,
            sha256,
        };
        let token = self
            .inner
            .stage_upload(context, intent, &mut std::io::Cursor::new(bytes))
            .map_err(map_runtime_error)?;
        match self
            .inner
            .add_thumbnail(context, model_id, expected_version, &token.id)
        {
            Ok(version) => Ok(version),
            Err(error) => {
                let _ = self.inner.discard_upload(context, &token.id);
                Err(map_runtime_error(error))
            }
        }
    }
    fn delete_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        thumbnail_id: &str,
    ) -> Result<u64, EngineError> {
        self.inner
            .delete_thumbnail(context, model_id, expected_version, thumbnail_id)
            .map_err(map_runtime_error)
    }
    fn issue_thumbnail_media(
        &self,
        context: &RequestContext,
        model_id: &str,
        thumbnail_id: &str,
    ) -> Result<ContentLease, EngineError> {
        self.inner
            .issue_thumbnail_media(context, model_id, thumbnail_id)
            .map_err(map_runtime_error)
    }
    fn issue_model_download(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_hash: Option<&str>,
    ) -> Result<ContentLease, EngineError> {
        self.inner
            .issue_model_download(context, model_id, expected_hash)
            .map_err(map_runtime_error)
    }
    fn stream_content(
        &self,
        context: &RequestContext,
        lease_id: &str,
        sink: &mut dyn std::io::Write,
    ) -> Result<(), EngineError> {
        self.inner
            .stream_content(context, lease_id, sink)
            .map_err(map_runtime_error)
    }
    fn describe_content(
        &self,
        context: &RequestContext,
        lease_id: &str,
    ) -> Result<ContentLease, EngineError> {
        self.inner
            .describe_content(context, lease_id)
            .map_err(map_runtime_error)
    }
    fn update_metadata(
        &self,
        c: &RequestContext,
        t: VersionedTarget,
        p: MetadataPatch,
    ) -> Result<MutationVersion, EngineError> {
        self.inner
            .update_metadata(c, t, p)
            .map_err(map_runtime_error)
    }
    fn bulk_update_metadata(
        &self,
        c: &RequestContext,
        t: Vec<VersionedTarget>,
        p: MetadataPatch,
    ) -> Result<Vec<MutationVersion>, EngineError> {
        self.inner
            .bulk_update_metadata(c, t, p)
            .map_err(map_runtime_error)
    }
    fn list_tags(&self, c: &RequestContext) -> Result<Vec<Tag>, EngineError> {
        self.inner.list_tags(c).map_err(map_runtime_error)
    }
    fn create_tag(
        &self,
        c: &RequestContext,
        n: String,
        k: Option<String>,
    ) -> Result<Tag, EngineError> {
        self.inner.create_tag(c, n, k).map_err(map_runtime_error)
    }
    fn update_tag(
        &self,
        c: &RequestContext,
        id: &str,
        n: String,
        k: Option<String>,
    ) -> Result<Tag, EngineError> {
        self.inner
            .update_tag(c, id, n, k)
            .map_err(map_runtime_error)
    }
    fn delete_tag(&self, c: &RequestContext, id: &str) -> Result<(), EngineError> {
        self.inner.delete_tag(c, id).map_err(map_runtime_error)
    }
    fn list_folders(&self, c: &RequestContext) -> Result<Vec<Folder>, EngineError> {
        self.inner.list_folders(c).map_err(map_runtime_error)
    }
    fn create_folder(
        &self,
        c: &RequestContext,
        n: String,
        p: Option<String>,
    ) -> Result<Folder, EngineError> {
        self.inner.create_folder(c, n, p).map_err(map_runtime_error)
    }
    fn rename_folder(
        &self,
        c: &RequestContext,
        id: &str,
        n: String,
    ) -> Result<Folder, EngineError> {
        self.inner
            .rename_folder(c, id, n)
            .map_err(map_runtime_error)
    }
    fn move_folder(&self, c: &RequestContext, id: &str, p: &str) -> Result<Folder, EngineError> {
        self.inner.move_folder(c, id, p).map_err(map_runtime_error)
    }
    fn delete_folder(&self, c: &RequestContext, id: &str) -> Result<(), EngineError> {
        self.inner.delete_folder(c, id).map_err(map_runtime_error)
    }
    fn list_filaments(&self, c: &RequestContext, u: bool) -> Result<Vec<Filament>, EngineError> {
        self.inner.list_filaments(c, u).map_err(map_runtime_error)
    }
    fn create_filament(
        &self,
        c: &RequestContext,
        i: FilamentInput,
    ) -> Result<Filament, EngineError> {
        self.inner.create_filament(c, i).map_err(map_runtime_error)
    }
    fn update_filament(
        &self,
        c: &RequestContext,
        id: &str,
        v: u64,
        i: FilamentInput,
    ) -> Result<Filament, EngineError> {
        self.inner
            .update_filament(c, id, v, i)
            .map_err(map_runtime_error)
    }
    fn delete_filament(
        &self,
        c: &RequestContext,
        id: &str,
        v: u64,
    ) -> Result<Option<Filament>, EngineError> {
        self.inner
            .delete_filament(c, id, v)
            .map_err(map_runtime_error)
    }
    fn bulk_assign_filaments(
        &self,
        c: &RequestContext,
        a: Vec<ModelFilamentAssignment>,
    ) -> Result<Vec<MutationVersion>, EngineError> {
        self.inner
            .bulk_assign_model_filaments(c, a)
            .map_err(map_runtime_error)
    }
    fn list_history(&self, c: &RequestContext, m: &str) -> Result<Vec<PrintAttempt>, EngineError> {
        self.inner
            .list_print_attempts(c, m)
            .map_err(map_runtime_error)
    }
    fn create_history(
        &self,
        c: &RequestContext,
        m: &str,
        i: PrintAttemptInput,
    ) -> Result<PrintAttempt, EngineError> {
        self.inner
            .create_print_attempt(c, m, i)
            .map_err(map_runtime_error)
    }
    fn update_history(
        &self,
        c: &RequestContext,
        id: &str,
        i: PrintAttemptInput,
    ) -> Result<PrintAttempt, EngineError> {
        self.inner
            .update_print_attempt(c, id, i)
            .map_err(map_runtime_error)
    }
    fn delete_history(&self, c: &RequestContext, id: &str) -> Result<(), EngineError> {
        self.inner
            .delete_print_attempt(c, id)
            .map_err(map_runtime_error)
    }
    fn stage_upload(
        &self,
        c: &RequestContext,
        i: UploadIntent,
        b: Vec<u8>,
    ) -> Result<UploadToken, EngineError> {
        self.inner
            .stage_upload(c, i, &mut std::io::Cursor::new(b))
            .map_err(map_runtime_error)
    }
    fn import_capture(
        &self,
        c: &RequestContext,
        e: CaptureEnvelope,
        u: Vec<UploadToken>,
    ) -> Result<CaptureImportResult, EngineError> {
        self.inner
            .import_capture(c, e, u)
            .map_err(map_runtime_error)
    }
    fn get_capture(
        &self,
        c: &RequestContext,
        id: &str,
    ) -> Result<CaptureImportResult, EngineError> {
        self.inner
            .get_capture_result(c, id)
            .map_err(map_runtime_error)
    }
}

fn invalid_tool() -> EngineError {
    EngineError {
        code: "invalidInput",
        message: "Tool arguments are invalid".into(),
        retryable: false,
        current_version: None,
    }
}
fn validate_tool_shape(name: &str, value: &Value) -> Result<(), EngineError> {
    let object = value.as_object().ok_or_else(invalid_tool)?;
    let (allowed, required): (&[&str], &[&str]) = match name {
        "search_models" => (&["query", "cursor", "limit"], &[]),
        "get_folder_tree" | "list_tags" => (&[], &[]),
        "list_filaments" => (&["includeUnavailable"], &[]),
        "update_model" => (&["target", "patch"], &["target", "patch"]),
        "update_models_batch" => (&["targets", "patch"], &["targets", "patch"]),
        "add_tag" => (&["name", "category"], &["name"]),
        "rename_tag" => (&["tagId", "name", "category"], &["tagId", "name"]),
        "delete_tag" => (&["tagId"], &["tagId"]),
        "save_filament" => (&["filamentId", "expectedVersion", "input"], &["input"]),
        "delete_filament" => (
            &["filamentId", "expectedVersion"],
            &["filamentId", "expectedVersion"],
        ),
        "set_model_filaments" => (&["assignments"], &["assignments"]),
        "get_print_events" => (&["modelId"], &["modelId"]),
        "log_print_event" => (&["modelId", "input"], &["modelId", "input"]),
        "delete_print_event" => (&["attemptId"], &["attemptId"]),
        "get_thumbnails" => (&["modelId"], &["modelId"]),
        "set_default_thumbnail" | "delete_thumbnail" => (
            &["modelId", "expectedVersion", "thumbnailId"],
            &["modelId", "expectedVersion", "thumbnailId"],
        ),
        "set_thumbnail" | "add_thumbnail" => (
            &["modelId", "expectedVersion", "image", "mimeType"],
            &["modelId", "expectedVersion", "image"],
        ),
        "download_model" => (&["modelId", "expectedHash"], &["modelId"]),
        _ => return Ok(()),
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str()))
        || required.iter().any(|key| !object.contains_key(*key))
    {
        return Err(invalid_tool());
    }
    Ok(())
}
fn internal_tool() -> EngineError {
    EngineError {
        code: "internal",
        message: "Tool response could not be encoded".into(),
        retryable: false,
        current_version: None,
    }
}
fn tool_version(value: &Value) -> Result<u64, EngineError> {
    value
        .get("expectedVersion")
        .and_then(Value::as_str)
        .ok_or_else(invalid_tool)
        .and_then(|value| parse_decimal(value).map_err(|_| invalid_tool()))
}
fn decode_tool_image(value: &Value) -> Result<Vec<u8>, EngineError> {
    use base64::Engine as _;
    let image = value
        .get("image")
        .and_then(Value::as_str)
        .ok_or_else(invalid_tool)?;
    let encoded = image
        .strip_prefix("data:image/png;base64,")
        .or_else(|| image.strip_prefix("data:image/jpeg;base64,"))
        .unwrap_or(image);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| invalid_tool())?;
    if bytes.len() > MAX_THUMBNAIL_UPLOAD_BYTES {
        return Err(invalid_tool());
    }
    Ok(bytes)
}

fn http_model(model: CatalogModel) -> ModelSummary {
    ModelSummary {
        id: model.id,
        display_name: model.display_name,
        folder_id: model.folder_id,
        metadata_version: model.metadata_version.to_string(),
        format: model.format,
        bytes: model.bytes.to_string(),
        content_hash: model.content_hash,
        tags: model.tags,
        filament_ids: model.filament_ids,
        source_kind: model.source_kind,
        availability: match model.availability {
            ModelAvailability::Available => "available",
            ModelAvailability::Missing => "missing",
            ModelAvailability::Bookmark => "bookmark",
        }
        .into(),
        catalog_kind: model.catalog_kind,
        thumbnail_id: model.thumbnail_id,
        designer: model.designer,
        source_url: model.source_url,
        notes: model.notes,
        model_license: model.model_license,
        print_status: model.print_status,
    }
}

fn map_runtime_error(error: RuntimeError) -> EngineError {
    EngineError {
        code: match error.code {
            RuntimeErrorCode::InvalidInput => "invalidInput",
            RuntimeErrorCode::NotFound => "notFound",
            RuntimeErrorCode::Conflict => "conflict",
            RuntimeErrorCode::Stale => "stale",
            RuntimeErrorCode::Busy => "busy",
            RuntimeErrorCode::Maintenance => "maintenance",
            RuntimeErrorCode::Forbidden => "forbidden",
            RuntimeErrorCode::Io => "io",
            RuntimeErrorCode::Corrupt => "corrupt",
            RuntimeErrorCode::Unsupported => "unsupported",
            RuntimeErrorCode::Internal => "internal",
        },
        message: error.message,
        retryable: error.retryable,
        current_version: error.current_version,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    code: String,
    message: String,
    request_id: String,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_version: Option<String>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    body: ErrorBody,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(ErrorEnvelope { error: self.body })).into_response()
    }
}

impl ApiError {
    fn from_engine(error: EngineError, request_id: &str) -> Self {
        let status = match error.code {
            "notFound" => StatusCode::NOT_FOUND,
            "conflict" | "stale" => StatusCode::CONFLICT,
            "busy" => StatusCode::TOO_MANY_REQUESTS,
            "maintenance" => StatusCode::SERVICE_UNAVAILABLE,
            "forbidden" => StatusCode::FORBIDDEN,
            "unsupported" => StatusCode::NOT_IMPLEMENTED,
            "invalidInput" => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            body: ErrorBody {
                code: error.code.into(),
                message: error.message,
                request_id: request_id.into(),
                retryable: error.retryable,
                current_version: error.current_version.map(|value| value.to_string()),
            },
        }
    }
}

pub fn router<E: LibraryEngine>(engine: Arc<E>, security: SecurityConfig) -> Router {
    build_router(engine, security, None)
}

pub fn router_with_static<E: LibraryEngine>(
    engine: Arc<E>,
    security: SecurityConfig,
    web_root: &std::path::Path,
) -> Result<Router, String> {
    let metadata = std::fs::symlink_metadata(web_root)
        .map_err(|_| "Browser asset directory could not be inspected")?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("Browser asset directory must be a real directory".into());
    }
    let root = web_root
        .canonicalize()
        .map_err(|_| "Browser asset directory could not be resolved")?;
    Ok(build_router(engine, security, Some(root)))
}

fn build_router<E: LibraryEngine>(
    engine: Arc<E>,
    security: SecurityConfig,
    web_root: Option<std::path::PathBuf>,
) -> Router {
    let state = ServerState {
        engine,
        admission: Arc::new(Semaphore::new(security.max_in_flight)),
        content_admission: Arc::new(Semaphore::new(4)),
        content_send_timeout: security.content_send_timeout,
        mcp_sessions: Arc::new(mcp::SessionStore::new(
            security.max_mcp_sessions,
            security.mcp_session_ttl,
        )),
        security: Arc::new(security),
    };
    let mut routes = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/openapi.json", get(openapi))
        .route("/api/v1/capabilities", get(capabilities::<E>))
        .route("/api/v1/status", get(status::<E>))
        .route("/api/v1/models", get(models::<E>))
        .route(
            "/api/v1/models/metadata/bulk",
            axum::routing::post(bulk_metadata::<E>),
        )
        .route(
            "/api/v1/models/{model_id}/metadata",
            axum::routing::patch(update_metadata::<E>),
        )
        .route("/api/v1/tags", get(list_tags::<E>).post(create_tag::<E>))
        .route(
            "/api/v1/tags/{tag_id}",
            axum::routing::patch(update_tag::<E>).delete(delete_tag::<E>),
        )
        .route(
            "/api/v1/folders",
            get(list_folders::<E>).post(create_folder::<E>),
        )
        .route(
            "/api/v1/folders/{folder_id}",
            axum::routing::patch(rename_folder::<E>).delete(delete_folder::<E>),
        )
        .route(
            "/api/v1/folders/{folder_id}/move",
            axum::routing::post(move_folder::<E>),
        )
        .route(
            "/api/v1/filaments",
            get(list_filaments::<E>).post(create_filament::<E>),
        )
        .route(
            "/api/v1/filaments/{filament_id}",
            axum::routing::put(update_filament::<E>).delete(delete_filament::<E>),
        )
        .route(
            "/api/v1/models/filaments/bulk",
            axum::routing::post(bulk_assign_filaments::<E>),
        )
        .route(
            "/api/v1/models/{model_id}/history",
            get(list_history::<E>).post(create_history::<E>),
        )
        .route(
            "/api/v1/history/{attempt_id}",
            axum::routing::put(update_history::<E>).delete(delete_history::<E>),
        )
        .route(
            "/api/v1/captures/uploads",
            axum::routing::post(capture_upload::<E>)
                .layer(axum::extract::DefaultBodyLimit::max(101 * 1024 * 1024)),
        )
        .route(
            "/api/v1/captures/import",
            axum::routing::post(import_capture::<E>),
        )
        .route("/api/v1/captures/{item_id}", get(get_capture::<E>))
        .route(
            "/api/v1/models/{model_id}/thumbnails",
            get(list_thumbnails::<E>).post(add_thumbnail::<E>),
        )
        .route(
            "/api/v1/models/{model_id}/thumbnails/{thumbnail_id}/active",
            axum::routing::patch(set_active_thumbnail::<E>),
        )
        .route(
            "/api/v1/models/{model_id}/thumbnails/{thumbnail_id}",
            axum::routing::delete(delete_thumbnail::<E>),
        )
        .route(
            "/api/v1/models/{model_id}/thumbnails/{thumbnail_id}/media-lease",
            axum::routing::post(thumbnail_media_lease::<E>),
        )
        .route(
            "/api/v1/models/{model_id}/download-lease",
            axum::routing::post(model_download_lease::<E>),
        )
        .route("/api/v1/content/{lease_id}", get(content::<E>))
        .route("/api/v1/events", get(events::<E>))
        .route(
            "/mcp",
            axum::routing::post(mcp::post::<E>)
                .get(mcp::get)
                .delete(mcp::delete::<E>),
        );
    if let Some(root) = web_root {
        routes = routes.fallback_service(
            tower_http::services::ServeDir::new(root).append_index_html_on_directories(true),
        );
    }
    routes
        .layer(axum::extract::DefaultBodyLimit::max(MAX_MCP_BODY_BYTES))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(30),
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            security::admit::<E>,
        ))
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(serde_json::json!({"status":"ok","apiVersion":API_VERSION}))
}

async fn openapi() -> Json<Value> {
    Json(openapi_document())
}

pub fn openapi_document() -> Value {
    serde_json::json!({
        "openapi":"3.1.0",
        "info":{"title":"GoggleLab Library API","version":"1.0.0"},
        "servers":[{"url":"/"}],
        "components":{
            "securitySchemes":{"bearerAuth":{"type":"http","scheme":"bearer"}},
            "schemas":{
                "Error":{"type":"object","required":["error"],"properties":{"error":{"type":"object","required":["code","message","requestId","retryable"]}}}
            }
        },
        "security":[{"bearerAuth":[]}],
        "paths":{
            "/api/v1/health":{"get":{"security":[],"responses":{"200":{"description":"Liveness only"}}}},
            "/api/v1/openapi.json":{"get":{"responses":{"200":{"description":"Versioned API contract"}}}},
            "/api/v1/capabilities":{"get":{"responses":{"200":{"description":"Host capabilities"},"401":{"description":"Unauthenticated"},"403":{"description":"Rejected Host or Origin"}}}},
            "/api/v1/status":{"get":{"responses":{"200":{"description":"Library epochs and jobs"}}}},
            "/api/v1/models":{"get":{"parameters":[
                {"name":"cursor","in":"query","schema":{"type":"string","maxLength":512}},
                {"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":100}},
                {"name":"query","in":"query","schema":{"type":"string","maxLength":256}}
                ,{"name":"folderId","in":"query","schema":{"type":"string","maxLength":128}}
                ,{"name":"sort","in":"query","schema":{"type":"string","enum":["name","size","date"]}}
                ,{"name":"direction","in":"query","schema":{"type":"string","enum":["asc","desc"]}}
            ],"responses":{"200":{"description":"Revision-bound model page"},"409":{"description":"Cursor revision is stale"}}}},
            "/api/v1/models/{modelId}/thumbnails":{"get":{"responses":{"200":{"description":"Stable thumbnail gallery descriptors"}}},"post":{"requestBody":{"required":true,"content":{"multipart/form-data":{"schema":{"type":"object","required":["expectedVersion","file"],"properties":{"expectedVersion":{"type":"string","pattern":"^[0-9]+$"},"file":{"type":"string","contentEncoding":"binary"}}}}}},"responses":{"200":{"description":"Thumbnail appended and activated"},"409":{"description":"Stale model version"},"413":{"description":"Upload exceeds 8 MiB"}}}},
            "/api/v1/models/{modelId}/thumbnails/{thumbnailId}/active":{"patch":{"requestBody":{"required":true,"content":{"application/json":{"schema":{"type":"object","additionalProperties":false,"required":["expectedVersion"],"properties":{"expectedVersion":{"type":"string","pattern":"^[0-9]+$"}}}}}},"responses":{"200":{"description":"Active thumbnail selected"},"409":{"description":"Stale model version"}}}},
            "/api/v1/models/{modelId}/thumbnails/{thumbnailId}":{"delete":{"requestBody":{"required":true,"content":{"application/json":{"schema":{"type":"object","additionalProperties":false,"required":["expectedVersion"],"properties":{"expectedVersion":{"type":"string","pattern":"^[0-9]+$"}}}}}},"responses":{"200":{"description":"Thumbnail deleted with guarded fallback"},"409":{"description":"Stale model version"}}}},
            "/api/v1/models/{modelId}/thumbnails/{thumbnailId}/media-lease":{"post":{"responses":{"200":{"description":"Five-minute epoch-bound media lease"}}}},
            "/api/v1/models/{modelId}/download-lease":{"post":{"requestBody":{"required":true,"content":{"application/json":{"schema":{"type":"object","additionalProperties":false,"properties":{"expectedHash":{"type":["string","null"],"pattern":"^[0-9a-f]{64}$"}}}}}},"responses":{"200":{"description":"Five-minute verified source lease"},"409":{"description":"Source hash changed"}}}},
            "/api/v1/content/{leaseId}":{"get":{"responses":{"200":{"description":"Verified lease bytes","content":{"application/octet-stream":{}}},"404":{"description":"Unknown lease"},"409":{"description":"Expired or epoch-stale lease"}}}},
            "/api/v1/models/{modelId}/metadata":{"patch":{"responses":{"200":{"description":"Versioned metadata mutation"},"409":{"description":"Stale model version"}}}},
            "/api/v1/models/metadata/bulk":{"post":{"responses":{"200":{"description":"Atomic common-patch metadata batch"},"409":{"description":"Any stale target aborts all writes"}}}},
            "/api/v1/tags":{"get":{"responses":{"200":{"description":"Tag catalog"}}},"post":{"responses":{"200":{"description":"Created tag"}}}},
            "/api/v1/tags/{tagId}":{"patch":{"responses":{"200":{"description":"Updated tag"}}},"delete":{"responses":{"204":{"description":"Deleted tag"}}}},
            "/api/v1/folders":{"get":{"responses":{"200":{"description":"Folder tree records"}}},"post":{"responses":{"200":{"description":"Created folder"}}}},
            "/api/v1/folders/{folderId}":{"patch":{"responses":{"200":{"description":"Renamed folder"}}},"delete":{"responses":{"204":{"description":"Deleted empty folder"}}}},
            "/api/v1/folders/{folderId}/move":{"post":{"responses":{"200":{"description":"Moved folder"},"422":{"description":"Cycle rejected"}}}},
            "/api/v1/filaments":{"get":{"responses":{"200":{"description":"Filament catalog"}}},"post":{"responses":{"200":{"description":"Created manual filament"}}}},
            "/api/v1/filaments/{filamentId}":{"put":{"responses":{"200":{"description":"Updated manual filament"},"409":{"description":"Stale filament version"}}},"delete":{"responses":{"200":{"description":"Deleted or archived filament"}}}},
            "/api/v1/models/filaments/bulk":{"post":{"responses":{"200":{"description":"Atomic heterogeneous assignments"},"409":{"description":"Any stale target aborts all writes"}}}},
            "/api/v1/models/{modelId}/history":{"get":{"responses":{"200":{"description":"Print attempts"}}},"post":{"responses":{"200":{"description":"Created print attempt"}}}},
            "/api/v1/history/{attemptId}":{"put":{"responses":{"200":{"description":"Updated print attempt"}}},"delete":{"responses":{"204":{"description":"Deleted print attempt"}}}},
            "/api/v1/captures/uploads":{"post":{"requestBody":{"required":true,"content":{"multipart/form-data":{"schema":{"type":"object","required":["intent","file"],"properties":{"intent":{"type":"string","description":"UploadIntent JSON"},"file":{"type":"string","contentEncoding":"binary"}}}}}},"responses":{"200":{"description":"Epoch-bound staged upload token"},"413":{"description":"Upload exceeds 100 MiB"}}}},
            "/api/v1/captures/import":{"post":{"responses":{"200":{"description":"Idempotent capture import result"}}}},
            "/api/v1/captures/{itemId}":{"get":{"responses":{"200":{"description":"Persisted capture result"}}}},
            "/api/v1/events":{"get":{"responses":{"200":{"description":"Authenticated server-sent invalidation events","content":{"text/event-stream":{}}}}}},
            "/mcp":{"post":{"responses":{"200":{"description":"JSON-RPC response"},"202":{"description":"Notification accepted"}}},"get":{"responses":{"405":{"description":"Standalone SSE listener disabled in v1"}}},"delete":{"responses":{"204":{"description":"Session terminated"},"404":{"description":"Unknown session"}}}}
        }
    })
}

async fn capabilities<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
) -> Result<Json<HostCapabilities>, ApiError> {
    state
        .engine
        .capabilities(&context)
        .map(Json)
        .map_err(|error| ApiError::from_engine(error, &context.request_id))
}

async fn status<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
) -> Result<Json<RuntimeStatus>, ApiError> {
    state
        .engine
        .status(&context)
        .map(Json)
        .map_err(|error| ApiError::from_engine(error, &context.request_id))
}

async fn models<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    Query(query): Query<ModelQuery>,
) -> Result<Json<Page<ModelSummary>>, ApiError> {
    let limit = query.limit.unwrap_or(50);
    if !(1..=MAX_PAGE_SIZE).contains(&limit)
        || query.cursor.as_ref().is_some_and(|value| value.len() > 512)
        || query.query.as_ref().is_some_and(|value| value.len() > 256)
    {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            body: ErrorBody {
                code: "invalidInput".into(),
                message: "Model query is outside its limits".into(),
                request_id: context.request_id,
                retryable: false,
                current_version: None,
            },
        });
    }
    state
        .engine
        .list_models(
            &context,
            query.cursor.as_deref(),
            limit,
            ProtocolModelQuery {
                folder_id: query.folder_id,
                search: query.query,
                sort: query.sort.unwrap_or(ModelSort::Date),
                direction: query.direction.unwrap_or(SortDirection::Asc),
                ..ProtocolModelQuery::default()
            },
        )
        .map(Json)
        .map_err(|error| ApiError::from_engine(error, &context.request_id))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VersionBody {
    expected_version: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DownloadBody {
    expected_hash: Option<String>,
}
fn parse_version(value: &str) -> Result<u64, ApiError> {
    parse_decimal(value)
}
fn api_bad(code: &str, message: &str) -> ApiError {
    ApiError {
        status: StatusCode::BAD_REQUEST,
        body: ErrorBody {
            code: code.into(),
            message: message.into(),
            request_id: "request".into(),
            retryable: false,
            current_version: None,
        },
    }
}
async fn list_thumbnails<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(model): AxumPath<String>,
) -> Result<Json<Vec<WireThumbnail>>, ApiError> {
    s.engine
        .list_thumbnails(&c, &model)
        .map(|values| Json(values.into_iter().map(Into::into).collect()))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn set_active_thumbnail<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath((model, thumb)): AxumPath<(String, String)>,
    Json(body): Json<VersionBody>,
) -> Result<Json<Value>, ApiError> {
    let v = parse_version(&body.expected_version)?;
    s.engine
        .set_active_thumbnail(&c, &model, v, &thumb)
        .map(|v| Json(serde_json::json!({"metadataVersion":v.to_string()})))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn delete_thumbnail<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath((model, thumb)): AxumPath<(String, String)>,
    Json(body): Json<VersionBody>,
) -> Result<Json<Value>, ApiError> {
    let v = parse_version(&body.expected_version)?;
    s.engine
        .delete_thumbnail(&c, &model, v, &thumb)
        .map(|v| Json(serde_json::json!({"metadataVersion":v.to_string()})))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn add_thumbnail<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(model): AxumPath<String>,
    mut multipart: Multipart,
) -> Result<Json<Value>, ApiError> {
    let mut version = None;
    let mut file = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| api_bad("invalidInput", "multipart is invalid"))?
    {
        match field.name() {
            Some("expectedVersion") => {
                version = Some(
                    field
                        .text()
                        .await
                        .map_err(|_| api_bad("invalidInput", "expectedVersion is invalid"))?,
                )
            }
            Some("file") => {
                let name = field.file_name().unwrap_or("thumbnail.png").to_owned();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|_| api_bad("invalidInput", "file is invalid"))?;
                if bytes.len() > MAX_THUMBNAIL_UPLOAD_BYTES {
                    return Err(api_bad("tooLarge", "thumbnail exceeds 8 MiB"));
                }
                file = Some((name, bytes.to_vec()));
            }
            _ => return Err(api_bad("invalidInput", "unexpected multipart field")),
        }
    }
    let v = parse_version(
        version
            .as_deref()
            .ok_or_else(|| api_bad("invalidInput", "expectedVersion is required"))?,
    )?;
    let (name, bytes) = file.ok_or_else(|| api_bad("invalidInput", "file is required"))?;
    s.engine
        .add_thumbnail(&c, &model, v, name, bytes)
        .map(|v| Json(serde_json::json!({"metadataVersion":v.to_string()})))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn thumbnail_media_lease<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath((model, thumb)): AxumPath<(String, String)>,
) -> Result<Json<WireLease>, ApiError> {
    s.engine
        .issue_thumbnail_media(&c, &model, &thumb)
        .map(|v| Json(v.into()))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn model_download_lease<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(model): AxumPath<String>,
    Json(body): Json<DownloadBody>,
) -> Result<Json<WireLease>, ApiError> {
    if body.expected_hash.as_ref().is_some_and(|hash| {
        hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }) {
        return Err(api_bad("invalidInput", "expectedHash is invalid"));
    }
    s.engine
        .issue_model_download(&c, &model, body.expected_hash.as_deref())
        .map(|v| Json(v.into()))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn content<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(lease): AxumPath<String>,
) -> Response {
    let permit = match s.content_admission.clone().try_acquire_owned() {
        Ok(value) => value,
        Err(_) => {
            return ApiError {
                status: StatusCode::TOO_MANY_REQUESTS,
                body: ErrorBody {
                    code: "busy".into(),
                    message: "Content stream limit reached".into(),
                    request_id: c.request_id.clone(),
                    retryable: true,
                    current_version: None,
                },
            }
            .into_response()
        }
    };
    let descriptor = match s.engine.describe_content(&c, &lease) {
        Ok(value) => value,
        Err(error) => return ApiError::from_engine(error, &c.request_id).into_response(),
    };
    if descriptor.bytes > 100 * 1024 * 1024 {
        return api_bad("tooLarge", "content exceeds limit").into_response();
    }
    let engine = Arc::clone(&s.engine);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    tokio::task::spawn_blocking(move || {
        struct Sink {
            sender: tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
            blocked_since: Option<std::time::Instant>,
            timeout: std::time::Duration,
        }
        impl std::io::Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                for chunk in bytes.chunks(64 * 1024) {
                    let mut value = Ok(Bytes::copy_from_slice(chunk));
                    loop {
                        match self.sender.try_send(value) {
                            Ok(()) => {
                                self.blocked_since = None;
                                break;
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                return Err(std::io::Error::other("client disconnected"))
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Full(returned)) => {
                                let since = *self
                                    .blocked_since
                                    .get_or_insert_with(std::time::Instant::now);
                                if since.elapsed() >= self.timeout {
                                    return Err(std::io::Error::new(
                                        std::io::ErrorKind::TimedOut,
                                        "content consumer stalled",
                                    ));
                                }
                                value = returned;
                                std::thread::sleep(std::time::Duration::from_millis(10));
                            }
                        }
                    }
                }
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut sink = Sink {
            sender: tx.clone(),
            blocked_since: None,
            timeout: s.content_send_timeout,
        };
        if let Err(error) = engine.stream_content(&c, &lease, &mut sink) {
            let _ = tx.try_send(Err(std::io::Error::other(error.message)));
        }
    });
    use tokio_stream::StreamExt as _;
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx)
        .timeout(std::time::Duration::from_secs(30))
        .map(move |item| {
            let _hold = &permit;
            match item {
                Ok(value) => value,
                Err(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "content stream timed out",
                )),
            }
        });
    let body = axum::body::Body::from_stream(stream);
    let mut response = (StatusCode::OK, body).into_response();
    if let Ok(value) = axum::http::HeaderValue::from_str(&descriptor.mime) {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_TYPE, value);
    }
    if let Ok(value) = axum::http::HeaderValue::from_str(&descriptor.bytes.to_string()) {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_LENGTH, value);
    }
    if let Ok(value) = axum::http::HeaderValue::from_str(&format!("\"{}\"", descriptor.sha256)) {
        response
            .headers_mut()
            .insert(axum::http::header::ETAG, value);
    }
    if let Ok(value) = axum::http::HeaderValue::from_str(&format!(
        "attachment; filename=\"{}\"",
        descriptor.file_name.replace(['\"', '\\'], "_")
    )) {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_DISPOSITION, value);
    }
    response
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MetadataRequest {
    expected_version: String,
    patch: MetadataPatch,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BulkMetadataRequest {
    targets: Vec<WireTarget>,
    patch: MetadataPatch,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NameCategory {
    name: String,
    category: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FolderCreate {
    name: String,
    parent_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FolderRename {
    name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FolderMove {
    parent_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FilamentUpdate {
    expected_version: String,
    input: FilamentInput,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Assignments {
    assignments: Vec<WireAssignment>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CaptureImportRequest {
    envelope: CaptureEnvelope,
    uploads: Vec<WireUploadInput>,
}
async fn update_metadata<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
    Json(b): Json<MetadataRequest>,
) -> Result<Json<WireMutation>, ApiError> {
    let v = parse_version(&b.expected_version)?;
    s.engine
        .update_metadata(
            &c,
            VersionedTarget {
                model_id: id,
                expected_version: v,
            },
            b.patch,
        )
        .map(|v| Json(v.into()))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn bulk_metadata<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    Json(b): Json<BulkMetadataRequest>,
) -> Result<Json<Vec<WireMutation>>, ApiError> {
    let targets = b
        .targets
        .into_iter()
        .map(WireTarget::parse)
        .collect::<Result<Vec<_>, _>>()?;
    s.engine
        .bulk_update_metadata(&c, targets, b.patch)
        .map(|values| Json(values.into_iter().map(Into::into).collect()))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn list_tags<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
) -> Result<Json<Vec<Tag>>, ApiError> {
    s.engine
        .list_tags(&c)
        .map(Json)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn create_tag<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    Json(b): Json<NameCategory>,
) -> Result<Json<Tag>, ApiError> {
    s.engine
        .create_tag(&c, b.name, b.category)
        .map(Json)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn update_tag<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
    Json(b): Json<NameCategory>,
) -> Result<Json<Tag>, ApiError> {
    s.engine
        .update_tag(&c, &id, b.name, b.category)
        .map(Json)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn delete_tag<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    s.engine
        .delete_tag(&c, &id)
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn list_folders<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
) -> Result<Json<Vec<WireFolder>>, ApiError> {
    s.engine
        .list_folders(&c)
        .map(|values| Json(values.into_iter().map(Into::into).collect()))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn create_folder<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    Json(b): Json<FolderCreate>,
) -> Result<Json<WireFolder>, ApiError> {
    s.engine
        .create_folder(&c, b.name, b.parent_id)
        .map(|v| Json(v.into()))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn rename_folder<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
    Json(b): Json<FolderRename>,
) -> Result<Json<WireFolder>, ApiError> {
    s.engine
        .rename_folder(&c, &id, b.name)
        .map(|v| Json(v.into()))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn move_folder<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
    Json(b): Json<FolderMove>,
) -> Result<Json<WireFolder>, ApiError> {
    s.engine
        .move_folder(&c, &id, &b.parent_id)
        .map(|v| Json(v.into()))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn delete_folder<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    s.engine
        .delete_folder(&c, &id)
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FilamentQuery {
    include_unavailable: Option<bool>,
}
async fn list_filaments<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    Query(q): Query<FilamentQuery>,
) -> Result<Json<Vec<WireFilament>>, ApiError> {
    s.engine
        .list_filaments(&c, q.include_unavailable.unwrap_or(true))
        .map(|values| Json(values.into_iter().map(Into::into).collect()))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn create_filament<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    Json(i): Json<FilamentInput>,
) -> Result<Json<WireFilament>, ApiError> {
    s.engine
        .create_filament(&c, i)
        .map(|v| Json(v.into()))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn update_filament<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
    Json(b): Json<FilamentUpdate>,
) -> Result<Json<WireFilament>, ApiError> {
    let v = parse_version(&b.expected_version)?;
    s.engine
        .update_filament(&c, &id, v, b.input)
        .map(|v| Json(v.into()))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn delete_filament<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
    Json(b): Json<VersionBody>,
) -> Result<Json<Option<WireFilament>>, ApiError> {
    let v = parse_version(&b.expected_version)?;
    s.engine
        .delete_filament(&c, &id, v)
        .map(|v| Json(v.map(Into::into)))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn bulk_assign_filaments<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    Json(b): Json<Assignments>,
) -> Result<Json<Vec<WireMutation>>, ApiError> {
    let assignments = b
        .assignments
        .into_iter()
        .map(WireAssignment::parse)
        .collect::<Result<Vec<_>, _>>()?;
    s.engine
        .bulk_assign_filaments(&c, assignments)
        .map(|values| Json(values.into_iter().map(Into::into).collect()))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn list_history<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Vec<PrintAttempt>>, ApiError> {
    s.engine
        .list_history(&c, &id)
        .map(Json)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn create_history<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
    Json(i): Json<PrintAttemptInput>,
) -> Result<Json<PrintAttempt>, ApiError> {
    s.engine
        .create_history(&c, &id, i)
        .map(Json)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn update_history<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
    Json(i): Json<PrintAttemptInput>,
) -> Result<Json<PrintAttempt>, ApiError> {
    s.engine
        .update_history(&c, &id, i)
        .map(Json)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn delete_history<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    s.engine
        .delete_history(&c, &id)
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn capture_upload<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    mut m: Multipart,
) -> Result<Json<WireUpload>, ApiError> {
    let mut intent = None;
    let mut file = None;
    while let Some(f) = m
        .next_field()
        .await
        .map_err(|_| api_bad("invalidInput", "multipart is invalid"))?
    {
        match f.name() {
            Some("intent") => {
                intent = Some(
                    serde_json::from_str::<WireUploadIntent>(
                        &f.text()
                            .await
                            .map_err(|_| api_bad("invalidInput", "intent is invalid"))?,
                    )
                    .map_err(|_| api_bad("invalidInput", "intent is invalid"))?,
                )
            }
            Some("file") => {
                file = Some(
                    f.bytes()
                        .await
                        .map_err(|_| api_bad("invalidInput", "file is invalid"))?
                        .to_vec(),
                )
            }
            _ => return Err(api_bad("invalidInput", "unexpected multipart field")),
        }
    }
    s.engine
        .stage_upload(
            &c,
            intent
                .ok_or_else(|| api_bad("invalidInput", "intent is required"))?
                .parse()?,
            file.ok_or_else(|| api_bad("invalidInput", "file is required"))?,
        )
        .map(|v| Json(v.into()))
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn import_capture<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    Json(b): Json<CaptureImportRequest>,
) -> Result<Json<CaptureImportResult>, ApiError> {
    let uploads = b
        .uploads
        .into_iter()
        .map(WireUploadInput::parse)
        .collect::<Result<Vec<_>, _>>()?;
    s.engine
        .import_capture(&c, b.envelope, uploads)
        .map(Json)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn get_capture<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<CaptureImportResult>, ApiError> {
    s.engine
        .get_capture(&c, &id)
        .map(Json)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}

async fn events<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
) -> Result<
    Sse<
        impl futures_core::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
    >,
    ApiError,
> {
    let status = state
        .engine
        .status(&context)
        .map_err(|error| ApiError::from_engine(error, &context.request_id))?;
    let payload = serde_json::json!({
        "sequence":"0",
        "libraryEpoch":status.library_epoch,
        "catalogRevision":status.catalog_revision,
        "requestId":context.request_id,
        "reset":true
    });
    Ok(Sse::new(futures_util::stream::once(async move {
        Ok(axum::response::sse::Event::default()
            .event("library.reset")
            .id("0")
            .json_data(payload)
            .expect("static event is serializable"))
    })))
}

pub fn tool_names() -> &'static [&'static str] {
    &[
        "search_models",
        "get_model",
        "get_library_stats",
        "get_folder_tree",
        "list_tags",
        "add_tag",
        "rename_tag",
        "delete_tag",
        "add_model_tags",
        "remove_model_tags",
        "list_filaments",
        "save_filament",
        "delete_filament",
        "set_model_filaments",
        "sync_spoolman_filaments",
        "get_print_events",
        "log_print_event",
        "delete_print_event",
        "log_print_events_batch",
        "list_parent_models",
        "list_designers",
        "list_licenses",
        "get_all_metadata",
        "update_model",
        "update_models_batch",
        "get_models_missing_thumbnails",
        "get_thumbnails",
        "set_thumbnail",
        "add_thumbnail",
        "set_default_thumbnail",
        "delete_thumbnail",
        "find_duplicates",
        "get_hash_status",
        "refresh_hashes",
        "check_files_exist",
        "get_embedded_3mf_metadata",
        "apply_embedded_3mf_metadata",
        "generate_tags",
        "apply_tag_review",
        "scan_directory",
        "cancel_scan",
        "get_scan_status",
        "remove_model",
        "trash_file",
        "undo_trash",
        "move_files",
        "export_library",
        "backup_library",
        "restore_library",
        "list_slicers",
        "open_in_slicer",
        "prepare_bundle_preview",
        "download_model",
        "download_bundle",
    ]
}

pub fn tool_definitions() -> Vec<Value> {
    tool_names()
        .iter()
        .map(|name| {
            let input_schema = match *name {
                "search_models" => serde_json::json!({
                    "type":"object","additionalProperties":false,
                    "properties":{
                        "query":{"type":"string","maxLength":256},
                        "cursor":{"type":"string","maxLength":512},
                        "limit":{"type":"integer","minimum":1,"maximum":100}
                    }
                }),
                "get_folder_tree"|"list_tags" => serde_json::json!({"type":"object","additionalProperties":false}),
                "list_filaments" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"includeUnavailable":{"type":"boolean"}}}),
                "update_model" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"target":{"type":"object","required":["modelId","expectedVersion"],"properties":{"modelId":{"type":"string"},"expectedVersion":{"type":"string","pattern":"^(0|[1-9][0-9]*)$"}}},"patch":{"type":"object"}},"required":["target","patch"]}),
                "update_models_batch" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"targets":{"type":"array","maxItems":50000,"items":{"type":"object","required":["modelId","expectedVersion"]}},"patch":{"type":"object"}},"required":["targets","patch"]}),
                "add_tag" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"name":{"type":"string","maxLength":256},"category":{"type":["string","null"],"maxLength":120}},"required":["name"]}),
                "rename_tag" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"tagId":{"type":"string"},"name":{"type":"string","maxLength":256},"category":{"type":["string","null"],"maxLength":120}},"required":["tagId","name"]}),
                "delete_tag" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"tagId":{"type":"string"}},"required":["tagId"]}),
                "save_filament" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"filamentId":{"type":"string"},"expectedVersion":{"type":"string","pattern":"^(0|[1-9][0-9]*)$"},"input":{"type":"object"}},"required":["input"]}),
                "delete_filament" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"filamentId":{"type":"string"},"expectedVersion":{"type":"string","pattern":"^(0|[1-9][0-9]*)$"}},"required":["filamentId","expectedVersion"]}),
                "set_model_filaments" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"assignments":{"type":"array","maxItems":50000}},"required":["assignments"]}),
                "get_print_events" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"modelId":{"type":"string"}},"required":["modelId"]}),
                "log_print_event" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"modelId":{"type":"string"},"input":{"type":"object"}},"required":["modelId","input"]}),
                "delete_print_event" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"attemptId":{"type":"string"}},"required":["attemptId"]}),
                "set_thumbnail" | "add_thumbnail" => serde_json::json!({
                    "type":"object","additionalProperties":false,
                    "properties":{
                        "modelId":{"type":"string","minLength":1,"maxLength":128},
                        "expectedVersion":{"type":"string","pattern":"^[0-9]+$"},
                        "image":{"type":"string","maxLength":11500000},
                        "mimeType":{"type":"string","enum":["image/png","image/jpeg"]}
                    },
                    "required":["modelId","expectedVersion","image"]
                }),
                "get_thumbnails" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"modelId":{"type":"string","minLength":1,"maxLength":128}},"required":["modelId"]}),
                "set_default_thumbnail" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"modelId":{"type":"string","minLength":1,"maxLength":128},"expectedVersion":{"type":"string","pattern":"^[0-9]+$"},"thumbnailId":{"type":"string","minLength":1,"maxLength":128}},"required":["modelId","expectedVersion","thumbnailId"]}),
                "download_model" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"modelId":{"type":"string","minLength":1,"maxLength":128},"expectedHash":{"type":"string","pattern":"^[0-9a-f]{64}$"}},"required":["modelId"]}),
                "delete_thumbnail" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"modelId":{"type":"string","minLength":1,"maxLength":128},"expectedVersion":{"type":"string","pattern":"^[0-9]+$"},"thumbnailId":{"type":"string","minLength":1,"maxLength":128}},"required":["modelId","expectedVersion","thumbnailId"]}),
                "remove_model"|"trash_file"|"undo_trash"|"move_files"|"restore_library" => serde_json::json!({
                    "type":"object",
                    "properties":{"confirmationToken":{"type":"string","minLength":43,"maxLength":256}},
                    "required":["confirmationToken"]
                }),
                _ => serde_json::json!({"type":"object"}),
            };
            serde_json::json!({
                "name": name,
                "description": format!("GoggleLab {} operation", name.replace('_', " ")),
                "inputSchema": input_schema
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
