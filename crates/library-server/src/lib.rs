#![recursion_limit = "256"]

mod bootstrap;
mod confirmation;
mod mcp;
mod security;

use std::collections::BTreeMap;
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
use library_protocol::{
    ActionPlan, AiProviderConfig, BundleEngine, BundlePreviewSummary, CaptureEngine,
    CaptureEnvelope, CaptureImportResult, CaptureSummary, CatalogFacets, CatalogJobEngine,
    CatalogModel, CatalogSettings, CatalogStats, ContentKind, ContentLease, DuplicateEngine,
    DuplicateGroup, EmbeddedMetadataEngine, EmbeddedMetadataProposal, EmbeddedMetadataSelection,
    Filament, FilamentEngine, FilamentInput, Folder, FolderEngine, GeometryEngine, GeometryLease,
    HashStatus, IndexRootSettings, IndexRootSettingsPatch, IntegrationAiReviewDecision,
    IntegrationAiReviewStrategy, IntegrationAiSuggestionBatch, IntegrationCredentialUpdate,
    IntegrationEngine, IntegrationSettings, IntegrationSpoolmanConfig,
    IntegrationSpoolmanPullResult, LibraryEngine as ProtocolLibraryEngine, ManagementAction,
    ManagementEngine, ManagementResult, MetadataEngine, MetadataPatch, MetadataValueKind,
    ModelAvailability, ModelFilamentAssignment, ModelGroupFilter, ModelQuery as ProtocolModelQuery,
    ModelSort, ModelSourceStatus, MutationVersion, PageRequest, PrintAttempt,
    PrintAttemptBatchItem, PrintAttemptInput, PrintHistoryEngine, PrintStatus, RootHandle,
    RuntimeError, RuntimeErrorCode, ScanStatus, SettingsEngine, SortDirection, Tag, TagEngine,
    ThumbnailDescriptor, UploadIntent, UploadToken, VersionedTarget,
};
pub use library_protocol::{Capability, RequestContext};
pub use security::{AllowedAuthority, AllowedOrigin, SecurityConfig};
use sha2::{Digest, Sha256};

pub const API_VERSION: &str = "1";
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
pub const MAX_JSON_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_MCP_BODY_BYTES: usize = 12 * 1024 * 1024;
pub const MAX_THUMBNAIL_UPLOAD_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PAGE_SIZE: u16 = 100;
const MAX_STATIC_FILES: usize = 2_048;
const MAX_STATIC_ASSET_BYTES: u64 = 8 * 1024 * 1024;
const MAX_STATIC_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone)]
struct StaticAsset {
    bytes: Bytes,
    content_type: &'static str,
}

#[derive(Clone)]
struct StaticAssets {
    files: Arc<BTreeMap<String, StaticAsset>>,
}

pub struct ServerHandle {
    local_addr: std::net::SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    stream_shutdown: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), std::io::Error>>,
}

impl ServerHandle {
    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.local_addr
    }

    pub async fn stop(mut self) -> Result<(), String> {
        let _ = self.stream_shutdown.send(true);
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
    let (stream_shutdown, stream_receiver) = tokio::sync::watch::channel(false);
    let app = app.layer(axum::Extension(ServerShutdown(stream_receiver)));
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
        stream_shutdown,
        task,
    })
}

#[derive(Clone)]
struct ServerShutdown(tokio::sync::watch::Receiver<bool>);

pub struct ServerState<E: LibraryEngine> {
    engine: Arc<E>,
    security: Arc<SecurityConfig>,
    admission: Arc<Semaphore>,
    content_admission: Arc<Semaphore>,
    large_upload_admission: Arc<Semaphore>,
    event_admission: Arc<Semaphore>,
    content_send_timeout: std::time::Duration,
    mcp_sessions: Arc<mcp::SessionStore>,
    static_assets: Option<StaticAssets>,
}

struct EventStreamState<E: LibraryEngine> {
    engine: Arc<E>,
    context: RequestContext,
    _permit: tokio::sync::OwnedSemaphorePermit,
    library_epoch: String,
    catalog_revision: String,
    initial_reset: bool,
    first: bool,
    last_heartbeat: std::time::Instant,
    shutdown: Option<tokio::sync::watch::Receiver<bool>>,
}

impl<E: LibraryEngine> Clone for ServerState<E> {
    fn clone(&self) -> Self {
        Self {
            engine: Arc::clone(&self.engine),
            security: Arc::clone(&self.security),
            admission: Arc::clone(&self.admission),
            content_admission: Arc::clone(&self.content_admission),
            large_upload_admission: Arc::clone(&self.large_upload_admission),
            event_admission: Arc::clone(&self.event_admission),
            content_send_timeout: self.content_send_timeout,
            mcp_sessions: Arc::clone(&self.mcp_sessions),
            static_assets: self.static_assets.clone(),
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
    pub source_root_id: Option<String>,
    pub bundle_id: Option<String>,
    pub archive_member: bool,
    pub parent_model_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_printed_at: Option<String>,
    pub print_count: String,
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
    pub rating: u8,
    pub favorite: bool,
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
    tag_id: Option<String>,
    category: Option<String>,
    designer: Option<String>,
    model_license: Option<String>,
    print_status: Option<PrintStatus>,
    ever_printed: Option<bool>,
    bundle_id: Option<String>,
    root_id: Option<String>,
    filament_id: Option<String>,
    source_missing: Option<bool>,
    duplicates_only: Option<bool>,
    source_kind: Option<String>,
    group: Option<ModelGroupFilter>,
    thumbnail_mode: Option<String>,
    source_directory: Option<String>,
    minimum_rating: Option<u8>,
    favorite: Option<bool>,
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
    kind: ContentKind,
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
            kind: v.kind,
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
struct WireGeometryLease {
    content: WireLease,
    packet_format: String,
    packet_version: u32,
    model_format: String,
    triangle_count: u32,
    plates: Vec<library_protocol::GeometryPlate>,
    parts: Vec<library_protocol::GeometryPart>,
    materials: Vec<library_protocol::GeometryMaterial>,
    embedded_filaments: Vec<library_protocol::EmbeddedFilament>,
    part_warning: Option<String>,
    plate_warning: Option<String>,
}
impl From<GeometryLease> for WireGeometryLease {
    fn from(v: GeometryLease) -> Self {
        Self {
            content: v.content.into(),
            packet_format: v.packet_format,
            packet_version: v.packet_version,
            model_format: v.model_format,
            triangle_count: v.triangle_count,
            plates: v.plates,
            parts: v.parts,
            materials: v.materials,
            embedded_filaments: v.embedded_filaments,
            part_warning: v.part_warning,
            plate_warning: v.plate_warning,
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireDuplicateGroup {
    content_hash: String,
    models: Vec<ModelSummary>,
}
impl From<DuplicateGroup> for WireDuplicateGroup {
    fn from(v: DuplicateGroup) -> Self {
        Self {
            content_hash: v.content_hash,
            models: v.models.into_iter().map(http_model).collect(),
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
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum WireManagementAction {
    RecoverableRemove {
        targets: Vec<WireTarget>,
    },
    RestoreRemoval {
        snapshot_id: String,
    },
    CreateBackup {
        destination_handle_id: String,
        file_name: String,
    },
    RestoreBackup {
        source_handle_id: String,
        file_name: String,
        root_mappings: Vec<library_protocol::RestoreRootMapping>,
    },
    ExportSelection {
        targets: Vec<WireTarget>,
        destination_handle_id: String,
        file_name: String,
    },
    QuarantineLooseSource {
        target: WireTarget,
    },
    UndoQuarantine {
        snapshot_id: String,
    },
    MoveLooseSource {
        target: WireTarget,
        destination_handle_id: String,
        file_name: String,
    },
    PurgeCatalog {
        backup_destination_handle_id: String,
        backup_file_name: String,
    },
    CleanupOrphans,
}
impl WireManagementAction {
    fn parse(self) -> Result<ManagementAction, ApiError> {
        Ok(match self {
            Self::RecoverableRemove { targets } => ManagementAction::RecoverableRemove {
                targets: targets
                    .into_iter()
                    .map(WireTarget::parse)
                    .collect::<Result<_, _>>()?,
            },
            Self::RestoreRemoval { snapshot_id } => {
                ManagementAction::RestoreRemoval { snapshot_id }
            }
            Self::CreateBackup {
                destination_handle_id,
                file_name,
            } => ManagementAction::CreateBackup {
                destination_handle_id,
                file_name,
            },
            Self::RestoreBackup {
                source_handle_id,
                file_name,
                root_mappings,
            } => ManagementAction::RestoreBackup {
                source_handle_id,
                file_name,
                root_mappings,
            },
            Self::ExportSelection {
                targets,
                destination_handle_id,
                file_name,
            } => ManagementAction::ExportSelection {
                targets: targets
                    .into_iter()
                    .map(WireTarget::parse)
                    .collect::<Result<_, _>>()?,
                destination_handle_id,
                file_name,
            },
            Self::QuarantineLooseSource { target } => ManagementAction::QuarantineLooseSource {
                target: target.parse()?,
            },
            Self::UndoQuarantine { snapshot_id } => {
                ManagementAction::UndoQuarantine { snapshot_id }
            }
            Self::MoveLooseSource {
                target,
                destination_handle_id,
                file_name,
            } => ManagementAction::MoveLooseSource {
                target: target.parse()?,
                destination_handle_id,
                file_name,
            },
            Self::PurgeCatalog {
                backup_destination_handle_id,
                backup_file_name,
            } => ManagementAction::PurgeCatalog {
                backup_destination_handle_id,
                backup_file_name,
            },
            Self::CleanupOrphans => ManagementAction::CleanupOrphans,
        })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireActionTargetSnapshot {
    model_id: String,
    metadata_version: String,
    content_hash: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireActionPlan {
    token: String,
    action_kind: String,
    expires_at: String,
    library_epoch: String,
    catalog_revision: String,
    targets: Vec<WireActionTargetSnapshot>,
    impact: Vec<String>,
    input_bytes: Option<String>,
    input_sha256: Option<String>,
}
impl From<ActionPlan> for WireActionPlan {
    fn from(value: ActionPlan) -> Self {
        Self {
            token: value.token,
            action_kind: value.action_kind,
            expires_at: value.expires_at,
            library_epoch: value.library_epoch.to_string(),
            catalog_revision: value.catalog_revision.to_string(),
            targets: value
                .targets
                .into_iter()
                .map(|target| WireActionTargetSnapshot {
                    model_id: target.model_id,
                    metadata_version: target.metadata_version.to_string(),
                    content_hash: target.content_hash,
                })
                .collect(),
            impact: value.impact,
            input_bytes: value.input_bytes.map(|bytes| bytes.to_string()),
            input_sha256: value.input_sha256,
        }
    }
}
fn wire_management_result(value: ManagementResult) -> Value {
    match value {
        ManagementResult::Removed {
            snapshot_id,
            removed_ids,
        } => {
            serde_json::json!({"kind":"removed","snapshotId":snapshot_id,"removedIds":removed_ids})
        }
        ManagementResult::RestoredRemoval { models } => serde_json::json!({
            "kind":"restoredRemoval",
            "models":models.into_iter().map(WireMutation::from).collect::<Vec<_>>()
        }),
        ManagementResult::BackupCreated {
            destination_handle_id,
            file_name,
            files,
            bytes,
            sha256,
        } => serde_json::json!({
            "kind":"backupCreated","destinationHandleId":destination_handle_id,
            "fileName":file_name,"files":files,"bytes":bytes.to_string(),"sha256":sha256
        }),
        ManagementResult::BackupRestored {
            models,
            library_epoch,
        } => serde_json::json!({
            "kind":"backupRestored","models":models,"libraryEpoch":library_epoch.to_string()
        }),
        ManagementResult::SelectionExported {
            destination_handle_id,
            file_name,
            models,
            bytes,
        } => serde_json::json!({
            "kind":"selectionExported","destinationHandleId":destination_handle_id,
            "fileName":file_name,"models":models,"bytes":bytes.to_string()
        }),
        ManagementResult::LibraryExported {
            destination_handle_id,
            file_name,
            models,
            bytes,
        } => serde_json::json!({
            "kind":"libraryExported","destinationHandleId":destination_handle_id,
            "fileName":file_name,"models":models,"bytes":bytes.to_string()
        }),
        ManagementResult::Quarantined {
            snapshot_id,
            model_id,
        } => serde_json::json!({"kind":"quarantined","snapshotId":snapshot_id,"modelId":model_id}),
        ManagementResult::QuarantineUndone { model } => serde_json::json!({
            "kind":"quarantineUndone","model":WireMutation::from(model)
        }),
        ManagementResult::Moved { model } => serde_json::json!({
            "kind":"moved","model":WireMutation::from(model)
        }),
        ManagementResult::Purged {
            backup_destination_handle_id,
            backup_file_name,
            library_epoch,
        } => serde_json::json!({
            "kind":"purged","backupDestinationHandleId":backup_destination_handle_id,
            "backupFileName":backup_file_name,"libraryEpoch":library_epoch.to_string()
        }),
        ManagementResult::CleanupCompleted { files, bytes } => serde_json::json!({
            "kind":"cleanupCompleted","files":files.to_string(),"bytes":bytes.to_string()
        }),
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
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireBundleSummary {
    token: String,
    bundle_id: String,
    member_count: usize,
    previewed_count: usize,
    omitted_count: usize,
    triangle_count: u32,
    layout: String,
    members: Vec<library_protocol::BundlePreviewMember>,
    notice: Option<String>,
    geometry_bytes: String,
    geometry_sha256: String,
    library_epoch: String,
}
impl From<BundlePreviewSummary> for WireBundleSummary {
    fn from(v: BundlePreviewSummary) -> Self {
        Self {
            token: v.token,
            bundle_id: v.bundle_id,
            member_count: v.member_count,
            previewed_count: v.previewed_count,
            omitted_count: v.omitted_count,
            triangle_count: v.triangle_count,
            layout: v.layout,
            members: v.members,
            notice: v.notice,
            geometry_bytes: v.geometry_bytes.to_string(),
            geometry_sha256: v.geometry_sha256,
            library_epoch: v.library_epoch.to_string(),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireAiSuggestionTarget {
    model_id: String,
    expected_version: String,
    display_name: String,
    existing_tags: Vec<library_protocol::IntegrationAiExistingTag>,
    suggestions: Vec<library_protocol::IntegrationAiSuggestion>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireAiSuggestionBatch {
    review_token: String,
    provider_id: String,
    model: String,
    category_sharing: library_protocol::IntegrationCategorySharing,
    targets: Vec<WireAiSuggestionTarget>,
    failures: Vec<library_protocol::IntegrationAiSuggestionFailure>,
}
impl From<IntegrationAiSuggestionBatch> for WireAiSuggestionBatch {
    fn from(value: IntegrationAiSuggestionBatch) -> Self {
        Self {
            review_token: value.review_token,
            provider_id: value.provider_id,
            model: value.model,
            category_sharing: value.category_sharing,
            targets: value
                .targets
                .into_iter()
                .map(|target| WireAiSuggestionTarget {
                    model_id: target.model_id,
                    expected_version: target.expected_version.to_string(),
                    display_name: target.display_name,
                    existing_tags: target.existing_tags,
                    suggestions: target.suggestions,
                })
                .collect(),
            failures: value.failures,
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireCatalogStats {
    total_models: String,
    available_models: String,
    missing_models: String,
    bookmarks: String,
    total_bytes: String,
    bundle_count: String,
    duplicate_group_count: String,
}
impl From<CatalogStats> for WireCatalogStats {
    fn from(v: CatalogStats) -> Self {
        Self {
            total_models: v.total_models.to_string(),
            available_models: v.available_models.to_string(),
            missing_models: v.missing_models.to_string(),
            bookmarks: v.bookmarks.to_string(),
            total_bytes: v.total_bytes.to_string(),
            bundle_count: v.bundle_count.to_string(),
            duplicate_group_count: v.duplicate_group_count.to_string(),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireIndexRootSettings {
    handle_id: String,
    catalog_root_id: String,
    label: String,
    access: library_protocol::RootAccess,
    enabled: bool,
    last_scanned_at: Option<String>,
    scan_interval_minutes: u32,
    max_file_bytes: String,
    scan_zip: bool,
}
impl From<IndexRootSettings> for WireIndexRootSettings {
    fn from(v: IndexRootSettings) -> Self {
        Self {
            handle_id: v.handle_id,
            catalog_root_id: v.catalog_root_id,
            label: v.label,
            access: v.access,
            enabled: v.enabled,
            last_scanned_at: v.last_scanned_at,
            scan_interval_minutes: v.scan_interval_minutes,
            max_file_bytes: v.max_file_bytes.to_string(),
            scan_zip: v.scan_zip,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireIndexRootSettingsPatch {
    enabled: bool,
    scan_interval_minutes: u32,
    max_file_bytes: String,
    scan_zip: bool,
}
impl WireIndexRootSettingsPatch {
    fn parse(self) -> Result<IndexRootSettingsPatch, ApiError> {
        Ok(IndexRootSettingsPatch {
            enabled: self.enabled,
            scan_interval_minutes: self.scan_interval_minutes,
            max_file_bytes: parse_decimal(&self.max_file_bytes)?,
            scan_zip: self.scan_zip,
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
    fn get_model(
        &self,
        context: &RequestContext,
        model_id: &str,
    ) -> Result<ModelSummary, EngineError>;
    fn catalog_stats(&self, context: &RequestContext) -> Result<CatalogStats, EngineError>;
    fn catalog_facets(&self, context: &RequestContext) -> Result<CatalogFacets, EngineError>;
    fn list_index_root_settings(
        &self,
        context: &RequestContext,
    ) -> Result<Vec<IndexRootSettings>, EngineError>;
    fn update_index_root_settings(
        &self,
        context: &RequestContext,
        handle_id: &str,
        patch: IndexRootSettingsPatch,
    ) -> Result<IndexRootSettings, EngineError>;
    fn get_catalog_settings(
        &self,
        context: &RequestContext,
    ) -> Result<CatalogSettings, EngineError>;
    fn update_catalog_settings(
        &self,
        context: &RequestContext,
        settings: CatalogSettings,
    ) -> Result<CatalogSettings, EngineError>;
    fn catalog_hash_status(&self, context: &RequestContext) -> Result<HashStatus, EngineError>;
    fn check_model_sources(
        &self,
        context: &RequestContext,
        model_ids: Vec<String>,
    ) -> Result<Vec<ModelSourceStatus>, EngineError>;
    fn start_thumbnail_job(
        &self,
        context: &RequestContext,
        model_ids: Vec<String>,
        force: bool,
    ) -> Result<ScanStatus, EngineError>;
    fn integration_settings(
        &self,
        context: &RequestContext,
    ) -> Result<IntegrationSettings, EngineError>;
    fn save_spoolman_config(
        &self,
        context: &RequestContext,
        config: Option<IntegrationSpoolmanConfig>,
    ) -> Result<IntegrationSettings, EngineError>;
    fn pull_spoolman(
        &self,
        context: &RequestContext,
    ) -> Result<IntegrationSpoolmanPullResult, EngineError>;
    fn save_ai_provider(
        &self,
        context: &RequestContext,
        config: AiProviderConfig,
        credential: IntegrationCredentialUpdate,
    ) -> Result<AiProviderConfig, EngineError>;
    fn delete_ai_provider(
        &self,
        context: &RequestContext,
        provider_id: &str,
    ) -> Result<(), EngineError>;
    fn suggest_ai_tags(
        &self,
        context: &RequestContext,
        provider_id: &str,
        targets: Vec<VersionedTarget>,
    ) -> Result<IntegrationAiSuggestionBatch, EngineError>;
    fn apply_ai_review(
        &self,
        context: &RequestContext,
        review_token: &str,
        decisions: Vec<IntegrationAiReviewDecision>,
        strategy: IntegrationAiReviewStrategy,
    ) -> Result<Vec<MutationVersion>, EngineError>;
    fn discard_ai_review(
        &self,
        context: &RequestContext,
        review_token: &str,
    ) -> Result<(), EngineError>;
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
    fn rename_metadata_global(
        &self,
        context: &RequestContext,
        kind: MetadataValueKind,
        old_name: &str,
        new_name: &str,
    ) -> Result<u64, EngineError> {
        let _ = (context, kind, old_name, new_name);
        Err(EngineError {
            code: "unsupported",
            message: "Global metadata rename is unavailable".into(),
            retryable: false,
            current_version: None,
        })
    }
    fn delete_metadata_global(
        &self,
        context: &RequestContext,
        kind: MetadataValueKind,
        name: &str,
    ) -> Result<u64, EngineError> {
        let _ = (context, kind, name);
        Err(EngineError {
            code: "unsupported",
            message: "Global metadata delete is unavailable".into(),
            retryable: false,
            current_version: None,
        })
    }
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
    fn create_history_batch(
        &self,
        c: &RequestContext,
        idempotency_key: &str,
        items: Vec<PrintAttemptBatchItem>,
    ) -> Result<Vec<PrintAttempt>, EngineError>;
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
    fn roots(&self, c: &RequestContext) -> Result<Vec<RootHandle>, EngineError>;
    fn start_scan(&self, c: &RequestContext, id: Option<String>)
        -> Result<ScanStatus, EngineError>;
    fn cancel_scan(&self, c: &RequestContext) -> Result<ScanStatus, EngineError>;
    fn scan_status(&self, c: &RequestContext) -> Result<ScanStatus, EngineError>;
    fn prepare_bundle(
        &self,
        c: &RequestContext,
        id: &str,
    ) -> Result<BundlePreviewSummary, EngineError>;
    fn describe_bundle_geometry(
        &self,
        c: &RequestContext,
        t: &str,
    ) -> Result<ContentLease, EngineError>;
    fn stream_bundle_geometry(
        &self,
        c: &RequestContext,
        t: &str,
        s: &mut dyn std::io::Write,
    ) -> Result<(), EngineError>;
    fn close_bundle(&self, c: &RequestContext, t: &str) -> Result<(), EngineError>;
    fn issue_bundle_download(
        &self,
        c: &RequestContext,
        token: &str,
    ) -> Result<ContentLease, EngineError>;
    fn issue_model_geometry(
        &self,
        c: &RequestContext,
        model_id: &str,
        expected_hash: Option<&str>,
        plate_id: Option<u32>,
    ) -> Result<GeometryLease, EngineError>;
    fn duplicate_groups(
        &self,
        c: &RequestContext,
        model_ids: Vec<String>,
    ) -> Result<Vec<DuplicateGroup>, EngineError>;
    fn read_embedded_metadata(
        &self,
        c: &RequestContext,
        model_id: &str,
        expected_hash: Option<&str>,
    ) -> Result<EmbeddedMetadataProposal, EngineError>;
    fn apply_embedded_metadata(
        &self,
        c: &RequestContext,
        target: VersionedTarget,
        expected_source_hash: &str,
        selection: EmbeddedMetadataSelection,
    ) -> Result<MutationVersion, EngineError>;
    fn plan_management_action(
        &self,
        c: &RequestContext,
        action: ManagementAction,
    ) -> Result<ActionPlan, EngineError>;
    fn plan_library_export(
        &self,
        c: &RequestContext,
        dest_path: Option<&str>,
    ) -> Result<ActionPlan, EngineError> {
        let _ = (c, dest_path);
        Err(EngineError {
            code: "unsupported",
            message: "Library JSON export is unavailable".into(),
            retryable: false,
            current_version: None,
        })
    }
    fn execute_management_action(
        &self,
        c: &RequestContext,
        token: &str,
    ) -> Result<ManagementResult, EngineError>;
    fn list_captures(
        &self,
        c: &RequestContext,
        p: PageRequest,
    ) -> Result<library_protocol::Page<CaptureSummary>, EngineError>;
}

pub struct ProtocolEngineAdapter<
    E: ProtocolLibraryEngine
        + MetadataEngine
        + TagEngine
        + FolderEngine
        + FilamentEngine
        + PrintHistoryEngine
        + CaptureEngine
        + BundleEngine
        + GeometryEngine
        + DuplicateEngine
        + EmbeddedMetadataEngine
        + ManagementEngine
        + CatalogJobEngine
        + IntegrationEngine
        + SettingsEngine,
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
            + CaptureEngine
            + BundleEngine
            + GeometryEngine
            + DuplicateEngine
            + EmbeddedMetadataEngine
            + ManagementEngine
            + CatalogJobEngine
            + IntegrationEngine
            + SettingsEngine,
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
            + BundleEngine
            + GeometryEngine
            + DuplicateEngine
            + EmbeddedMetadataEngine
            + ManagementEngine
            + CatalogJobEngine
            + IntegrationEngine
            + SettingsEngine
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
                download_bundle: info.capabilities.contains(&Capability::MediaRead),
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

    fn get_model(
        &self,
        context: &RequestContext,
        model_id: &str,
    ) -> Result<ModelSummary, EngineError> {
        self.inner
            .get_model(context, model_id)
            .map(http_model)
            .map_err(map_runtime_error)
    }

    fn catalog_stats(&self, context: &RequestContext) -> Result<CatalogStats, EngineError> {
        self.inner.catalog_stats(context).map_err(map_runtime_error)
    }

    fn catalog_facets(&self, context: &RequestContext) -> Result<CatalogFacets, EngineError> {
        self.inner
            .catalog_facets(context)
            .map_err(map_runtime_error)
    }

    fn list_index_root_settings(
        &self,
        context: &RequestContext,
    ) -> Result<Vec<IndexRootSettings>, EngineError> {
        self.inner
            .list_index_root_settings(context)
            .map_err(map_runtime_error)
    }

    fn update_index_root_settings(
        &self,
        context: &RequestContext,
        handle_id: &str,
        patch: IndexRootSettingsPatch,
    ) -> Result<IndexRootSettings, EngineError> {
        self.inner
            .update_index_root_settings(context, handle_id, patch)
            .map_err(map_runtime_error)
    }

    fn get_catalog_settings(
        &self,
        context: &RequestContext,
    ) -> Result<CatalogSettings, EngineError> {
        self.inner
            .get_catalog_settings(context)
            .map_err(map_runtime_error)
    }

    fn update_catalog_settings(
        &self,
        context: &RequestContext,
        settings: CatalogSettings,
    ) -> Result<CatalogSettings, EngineError> {
        self.inner
            .update_catalog_settings(context, settings)
            .map_err(map_runtime_error)
    }

    fn catalog_hash_status(&self, context: &RequestContext) -> Result<HashStatus, EngineError> {
        self.inner
            .catalog_hash_status(context)
            .map_err(map_runtime_error)
    }

    fn check_model_sources(
        &self,
        context: &RequestContext,
        model_ids: Vec<String>,
    ) -> Result<Vec<ModelSourceStatus>, EngineError> {
        self.inner
            .check_model_sources(context, model_ids)
            .map_err(map_runtime_error)
    }

    fn start_thumbnail_job(
        &self,
        context: &RequestContext,
        model_ids: Vec<String>,
        force: bool,
    ) -> Result<ScanStatus, EngineError> {
        self.inner
            .start_thumbnail_job(context, model_ids, force)
            .map_err(map_runtime_error)
    }

    fn integration_settings(
        &self,
        context: &RequestContext,
    ) -> Result<IntegrationSettings, EngineError> {
        self.inner
            .integration_settings(context)
            .map_err(map_runtime_error)
    }
    fn save_spoolman_config(
        &self,
        context: &RequestContext,
        config: Option<IntegrationSpoolmanConfig>,
    ) -> Result<IntegrationSettings, EngineError> {
        self.inner
            .save_spoolman_config(context, config)
            .map_err(map_runtime_error)
    }
    fn pull_spoolman(
        &self,
        context: &RequestContext,
    ) -> Result<IntegrationSpoolmanPullResult, EngineError> {
        self.inner.pull_spoolman(context).map_err(map_runtime_error)
    }
    fn save_ai_provider(
        &self,
        context: &RequestContext,
        config: AiProviderConfig,
        credential: IntegrationCredentialUpdate,
    ) -> Result<AiProviderConfig, EngineError> {
        self.inner
            .save_ai_provider(context, config, credential)
            .map_err(map_runtime_error)
    }
    fn delete_ai_provider(
        &self,
        context: &RequestContext,
        provider_id: &str,
    ) -> Result<(), EngineError> {
        self.inner
            .delete_ai_provider(context, provider_id)
            .map_err(map_runtime_error)
    }
    fn suggest_ai_tags(
        &self,
        context: &RequestContext,
        provider_id: &str,
        targets: Vec<VersionedTarget>,
    ) -> Result<IntegrationAiSuggestionBatch, EngineError> {
        self.inner
            .suggest_ai_tags(context, provider_id, targets)
            .map_err(map_runtime_error)
    }
    fn apply_ai_review(
        &self,
        context: &RequestContext,
        review_token: &str,
        decisions: Vec<IntegrationAiReviewDecision>,
        strategy: IntegrationAiReviewStrategy,
    ) -> Result<Vec<MutationVersion>, EngineError> {
        self.inner
            .apply_ai_review(context, review_token, decisions, strategy)
            .map_err(map_runtime_error)
    }
    fn discard_ai_review(
        &self,
        context: &RequestContext,
        review_token: &str,
    ) -> Result<(), EngineError> {
        self.inner
            .discard_ai_review(context, review_token)
            .map_err(map_runtime_error)
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
            "integration_get_settings" => return encoded!(self.integration_settings(context)?),
            "integration_save_spoolman_config" => {
                let config = match arguments.get("config") {
                    Some(Value::Null) => None,
                    Some(value) => {
                        Some(serde_json::from_value(value.clone()).map_err(|_| invalid_tool())?)
                    }
                    None => return Err(invalid_tool()),
                };
                return encoded!(self.save_spoolman_config(context, config)?);
            }
            "sync_spoolman_filaments" => return encoded!(self.pull_spoolman(context)?),
            "integration_save_ai_provider" => {
                let config: AiProviderConfig = serde_json::from_value(
                    arguments.get("config").cloned().ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                let credential: IntegrationCredentialUpdate = serde_json::from_value(
                    arguments
                        .get("credentialUpdate")
                        .cloned()
                        .ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                return encoded!(self.save_ai_provider(context, config, credential)?);
            }
            "integration_delete_ai_provider" => {
                let provider_id = arguments
                    .get("providerId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                self.delete_ai_provider(context, provider_id)?;
                return Ok(serde_json::json!({"ok":true}));
            }
            "generate_tags" => {
                let provider_id = arguments
                    .get("providerId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                let targets: Vec<WireTarget> = serde_json::from_value(
                    arguments.get("targets").cloned().ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                let targets = targets
                    .into_iter()
                    .map(WireTarget::parse)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| invalid_tool())?;
                return encoded!(WireAiSuggestionBatch::from(self.suggest_ai_tags(
                    context,
                    provider_id,
                    targets,
                )?));
            }
            "apply_tag_review" => {
                let review_token = arguments
                    .get("reviewToken")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                let decisions: Vec<IntegrationAiReviewDecision> = serde_json::from_value(
                    arguments
                        .get("decisions")
                        .cloned()
                        .ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                let strategy: IntegrationAiReviewStrategy = serde_json::from_value(
                    arguments
                        .get("strategy")
                        .cloned()
                        .ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                return encoded!(self
                    .apply_ai_review(context, review_token, decisions, strategy)?
                    .into_iter()
                    .map(WireMutation::from)
                    .collect::<Vec<_>>());
            }
            "discard_tag_review" => {
                let review_token = arguments
                    .get("reviewToken")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                self.discard_ai_review(context, review_token)?;
                return Ok(serde_json::json!({"ok":true}));
            }
            "get_hash_status" => return encoded!(self.catalog_hash_status(context)?),
            "check_files_exist" => {
                let model_ids: Vec<String> = serde_json::from_value(
                    arguments
                        .get("modelIds")
                        .cloned()
                        .ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                return encoded!(self.check_model_sources(context, model_ids)?);
            }
            "calculate_missing_hashes" | "refresh_hashes" => {
                return encoded!(self.start_scan(context, None)?)
            }
            "start_thumbnail_job" => {
                let model_ids: Vec<String> = serde_json::from_value(
                    arguments
                        .get("modelIds")
                        .cloned()
                        .ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                let force = arguments
                    .get("force")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                return encoded!(self.start_thumbnail_job(context, model_ids, force)?);
            }
            "get_models_missing_thumbnails" | "get_models_with_default_thumbnails" => {
                let limit = arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .and_then(|value| u16::try_from(value).ok())
                    .unwrap_or(50);
                let thumbnail_mode = if name == "get_models_missing_thumbnails" {
                    "missing"
                } else {
                    "default"
                };
                return encoded!(self.list_models(
                    context,
                    arguments.get("cursor").and_then(Value::as_str),
                    limit,
                    ProtocolModelQuery {
                        thumbnail_mode: Some(thumbnail_mode.into()),
                        ..ProtocolModelQuery::default()
                    }
                )?);
            }
            "get_models_by_directory" => {
                let directory = arguments
                    .get("directory")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                let root_id = arguments
                    .get("rootId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                let limit = arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .and_then(|value| u16::try_from(value).ok())
                    .unwrap_or(50)
                    .min(MAX_PAGE_SIZE);
                return encoded!(self.list_models(
                    context,
                    arguments.get("cursor").and_then(Value::as_str),
                    limit,
                    ProtocolModelQuery {
                        root_id: Some(root_id.to_owned()),
                        source_directory: Some(directory.to_owned()),
                        ..ProtocolModelQuery::default()
                    }
                )?);
            }
            "rename_metadata" | "delete_metadata" => {
                let is_rename = name == "rename_metadata";
                let kind: MetadataValueKind = serde_json::from_value(
                    arguments.get("type").cloned().ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                let value_name = arguments
                    .get(if is_rename { "oldName" } else { "name" })
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                let count = if is_rename {
                    let new_name = arguments
                        .get("newName")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid_tool)?;
                    self.rename_metadata_global(context, kind, value_name, new_name)?
                } else {
                    self.delete_metadata_global(context, kind, value_name)?
                };
                return Ok(serde_json::json!({"affectedModels":count.to_string()}));
            }
            "add_model_tags" | "remove_model_tags" => {
                let model_id = arguments
                    .get("modelId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?
                    .to_owned();
                let names: Vec<String> = serde_json::from_value(
                    arguments.get("tags").cloned().ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                let target = VersionedTarget {
                    model_id,
                    expected_version: tool_version(&arguments)?,
                };
                let result = if name == "add_model_tags" {
                    self.inner.add_model_tags_by_name(context, target, names)
                } else {
                    self.inner.remove_model_tags_by_name(context, target, names)
                }
                .map_err(map_runtime_error)?;
                return encoded!(WireMutation::from(result));
            }
            "remove_model" => {
                if arguments.get("confirm").and_then(Value::as_bool) != Some(true) {
                    return Err(invalid_tool());
                }
                let model_id = arguments
                    .get("modelId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?
                    .to_owned();
                let target = VersionedTarget {
                    model_id,
                    expected_version: tool_version(&arguments)?,
                };
                return encoded!(WireActionPlan::from(self.plan_management_action(
                    context,
                    ManagementAction::RecoverableRemove {
                        targets: vec![target],
                    },
                )?));
            }
            "backup_database" => {
                let action = ManagementAction::CreateBackup {
                    destination_handle_id: arguments
                        .get("destinationHandleId")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid_tool)?
                        .to_owned(),
                    file_name: arguments
                        .get("fileName")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid_tool)?
                        .to_owned(),
                };
                return encoded!(WireActionPlan::from(
                    self.plan_management_action(context, action)?
                ));
            }
            "export_library" => {
                return encoded!(WireActionPlan::from(
                    self.inner
                        .plan_library_export(
                            context,
                            arguments.get("destPath").and_then(Value::as_str),
                        )
                        .map_err(map_runtime_error)?,
                ));
            }
            "pull_3mf_metadata" => {
                let model_id = arguments
                    .get("modelId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                return encoded!(self.read_embedded_metadata(
                    context,
                    model_id,
                    arguments.get("expectedHash").and_then(Value::as_str),
                )?);
            }
            "plan_management_action" => {
                let action: WireManagementAction = serde_json::from_value(
                    arguments.get("action").cloned().ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                return encoded!(WireActionPlan::from(self.plan_management_action(
                    context,
                    action.parse().map_err(|_| invalid_tool())?
                )?));
            }
            "execute_management_action" => {
                let token = arguments
                    .get("token")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                return Ok(wire_management_result(
                    self.execute_management_action(context, token)?,
                ));
            }
            "get_index_root_settings" => {
                return encoded!(self
                    .list_index_root_settings(context)?
                    .into_iter()
                    .map(WireIndexRootSettings::from)
                    .collect::<Vec<_>>())
            }
            "update_index_root_settings" => {
                let handle_id = arguments
                    .get("handleId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                let settings: WireIndexRootSettingsPatch = serde_json::from_value(
                    arguments
                        .get("settings")
                        .cloned()
                        .ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                let settings = settings.parse().map_err(|_| invalid_tool())?;
                return encoded!(WireIndexRootSettings::from(
                    self.update_index_root_settings(context, handle_id, settings)?
                ));
            }
            "get_catalog_settings" => return encoded!(self.get_catalog_settings(context)?),
            "update_catalog_settings" => {
                let settings: CatalogSettings = serde_json::from_value(
                    arguments
                        .get("settings")
                        .cloned()
                        .ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                return encoded!(self.update_catalog_settings(context, settings)?);
            }
            "get_model" | "get_all_metadata" => {
                let id = arguments
                    .get("modelId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                return encoded!(self.get_model(context, id)?);
            }
            "get_library_stats" => {
                return encoded!(WireCatalogStats::from(self.catalog_stats(context)?))
            }
            "list_parent_models" => return encoded!(self.catalog_facets(context)?.parents),
            "list_designers" => return encoded!(self.catalog_facets(context)?.designers),
            "list_licenses" => return encoded!(self.catalog_facets(context)?.licenses),
            "list_roots" => return encoded!(self.roots(context)?),
            "scan_directory" => {
                return encoded!(self.start_scan(
                    context,
                    arguments
                        .get("rootId")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                )?)
            }
            "cancel_scan" => return encoded!(self.cancel_scan(context)?),
            "get_scan_status" => return encoded!(self.scan_status(context)?),
            "prepare_bundle_preview" => {
                let id = arguments
                    .get("bundleId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                return encoded!(WireBundleSummary::from(self.prepare_bundle(context, id)?));
            }
            "get_bundle_preview_geometry" => {
                let token = arguments
                    .get("token")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                let lease = WireLease::from(self.describe_bundle_geometry(context, token)?);
                return encoded!(serde_json::json!({
                    "lease": lease,
                    "contentPath": format!("/api/v1/bundle-previews/{token}/geometry")
                }));
            }
            "download_bundle" => {
                let token = arguments
                    .get("token")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                return encoded!(WireLease::from(self.issue_bundle_download(context, token)?));
            }
            "get_model_geometry" => {
                let model_id = arguments
                    .get("modelId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                let plate_id = arguments
                    .get("plateId")
                    .and_then(Value::as_u64)
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|_| invalid_tool())?;
                return encoded!(WireGeometryLease::from(self.issue_model_geometry(
                    context,
                    model_id,
                    arguments.get("expectedHash").and_then(Value::as_str),
                    plate_id,
                )?));
            }
            "find_duplicates" => {
                let model_ids: Vec<String> = serde_json::from_value(
                    arguments
                        .get("modelIds")
                        .cloned()
                        .ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                return encoded!(self
                    .duplicate_groups(context, model_ids)?
                    .into_iter()
                    .map(WireDuplicateGroup::from)
                    .collect::<Vec<_>>());
            }
            "get_embedded_3mf_metadata" => {
                let model_id = arguments
                    .get("modelId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                return encoded!(self.read_embedded_metadata(
                    context,
                    model_id,
                    arguments.get("expectedHash").and_then(Value::as_str),
                )?);
            }
            "apply_embedded_3mf_metadata" => {
                let model_id = arguments
                    .get("modelId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                let expected_source_hash = arguments
                    .get("expectedSourceHash")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                let selection: EmbeddedMetadataSelection = serde_json::from_value(
                    arguments
                        .get("selection")
                        .cloned()
                        .ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                return encoded!(WireMutation::from(self.apply_embedded_metadata(
                    context,
                    VersionedTarget {
                        model_id: model_id.to_owned(),
                        expected_version: tool_version(&arguments)?,
                    },
                    expected_source_hash,
                    selection,
                )?));
            }
            "close_bundle_preview" => {
                let token = arguments
                    .get("token")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                self.close_bundle(context, token)?;
                return Ok(serde_json::json!({"ok":true}));
            }
            "list_capture_items" => {
                let limit = arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .and_then(|value| u16::try_from(value).ok())
                    .unwrap_or(50);
                if !(1..=MAX_PAGE_SIZE).contains(&limit) {
                    return Err(invalid_tool());
                }
                let page = self.list_captures(
                    context,
                    PageRequest {
                        cursor: arguments
                            .get("cursor")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        limit,
                    },
                )?;
                return Ok(serde_json::json!({
                    "items": page.items,
                    "nextCursor": page.next_cursor,
                    "snapshotRevision": page.snapshot_revision.to_string()
                }));
            }
            "retry_capture" => {
                let item_id = arguments
                    .get("itemId")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                let envelope: CaptureEnvelope = serde_json::from_value(
                    arguments
                        .get("envelope")
                        .cloned()
                        .ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                if envelope.item_id != item_id {
                    return Err(invalid_tool());
                }
                let uploads: Vec<WireUploadInput> = serde_json::from_value(
                    arguments.get("uploads").cloned().ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                let uploads = uploads
                    .into_iter()
                    .map(WireUploadInput::parse)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| invalid_tool())?;
                return encoded!(self.import_capture(context, envelope, uploads)?);
            }
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
            "log_print_events_batch" => {
                let idempotency_key = arguments
                    .get("idempotencyKey")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid_tool)?;
                let items: Vec<PrintAttemptBatchItem> = serde_json::from_value(
                    arguments.get("items").cloned().ok_or_else(invalid_tool)?,
                )
                .map_err(|_| invalid_tool())?;
                return encoded!(self.create_history_batch(context, idempotency_key, items)?);
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
        if object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "query" | "cursor" | "limit" | "thumbnailMode" | "rootId" | "sourceDirectory"
            )
        }) || object
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
            || object.get("thumbnailMode").is_some_and(|value| {
                value
                    .as_str()
                    .is_none_or(|value| !matches!(value, "missing" | "default" | "custom"))
            })
            || object
                .get("rootId")
                .is_some_and(|value| value.as_str().is_none_or(|value| value.len() > 128))
            || object
                .get("minimumRating")
                .is_some_and(|value| value.as_u64().is_none_or(|rating| rating > 5))
            || object
                .get("favorite")
                .is_some_and(|value| !value.is_boolean())
            || object.get("sourceDirectory").is_some_and(|value| {
                value.as_str().is_none_or(|value| {
                    value.len() > 1024 || value.contains('\\') || value.contains('\0')
                })
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
                thumbnail_mode: arguments
                    .get("thumbnailMode")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                root_id: arguments
                    .get("rootId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                source_directory: arguments
                    .get("sourceDirectory")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                minimum_rating: arguments
                    .get("minimumRating")
                    .and_then(Value::as_u64)
                    .and_then(|rating| u8::try_from(rating).ok()),
                favorite: arguments.get("favorite").and_then(Value::as_bool),
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
                | "integration_get_settings"
                | "integration_save_spoolman_config"
                | "integration_save_ai_provider"
                | "integration_delete_ai_provider"
                | "discard_tag_review"
                | "get_hash_status"
                | "check_files_exist"
                | "refresh_hashes"
                | "calculate_missing_hashes"
                | "start_thumbnail_job"
                | "get_models_missing_thumbnails"
                | "get_models_with_default_thumbnails"
                | "get_models_by_directory"
                | "backup_database"
                | "export_library"
                | "rename_metadata"
                | "delete_metadata"
                | "add_model_tags"
                | "remove_model_tags"
                | "sync_spoolman_filaments"
                | "generate_tags"
                | "apply_tag_review"
                | "plan_management_action"
                | "execute_management_action"
                | "get_index_root_settings"
                | "update_index_root_settings"
                | "get_catalog_settings"
                | "update_catalog_settings"
                | "get_model"
                | "get_all_metadata"
                | "get_library_stats"
                | "remove_model"
                | "list_parent_models"
                | "list_designers"
                | "list_licenses"
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
                | "log_print_events_batch"
                | "delete_print_event"
                | "update_model"
                | "update_models_batch"
                | "list_roots"
                | "scan_directory"
                | "cancel_scan"
                | "get_scan_status"
                | "prepare_bundle_preview"
                | "get_bundle_preview_geometry"
                | "download_bundle"
                | "get_model_geometry"
                | "close_bundle_preview"
                | "find_duplicates"
                | "get_embedded_3mf_metadata"
                | "pull_3mf_metadata"
                | "apply_embedded_3mf_metadata"
                | "list_capture_items"
                | "retry_capture"
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
                | "add_model_tags"
                | "remove_model_tags"
                | "remove_model"
                | "save_filament"
                | "delete_filament"
                | "set_model_filaments"
                | "log_print_event"
                | "log_print_events_batch"
                | "delete_print_event"
                | "update_catalog_settings"
                | "rename_metadata"
                | "delete_metadata"
                | "apply_embedded_3mf_metadata"
                | "retry_capture"
        );
        if name == "backup_database" {
            return connected
                && context.capabilities.contains(&Capability::Maintenance)
                && (context.capabilities.contains(&Capability::BackupWrite)
                    || context
                        .capabilities
                        .contains(&Capability::PhysicalSourceWrite));
        }
        if name == "export_library" {
            return connected
                && context.capabilities.contains(&Capability::Maintenance)
                && context.capabilities.contains(&Capability::BackupWrite);
        }
        if matches!(name, "plan_management_action" | "execute_management_action") {
            return connected
                && (context.capabilities.contains(&Capability::CatalogWrite)
                    || context.capabilities.contains(&Capability::Maintenance));
        }
        if name == "check_files_exist" {
            return connected
                && context.capabilities.contains(&Capability::CatalogRead)
                && context.capabilities.contains(&Capability::MediaRead);
        }
        if name == "start_thumbnail_job" {
            return connected
                && context.capabilities.contains(&Capability::CatalogWrite)
                && context.capabilities.contains(&Capability::MediaWrite);
        }
        if name == "integration_save_spoolman_config" {
            return connected
                && context.capabilities.contains(&Capability::Integration)
                && context.capabilities.contains(&Capability::CatalogWrite);
        }
        if name == "sync_spoolman_filaments" {
            return connected
                && self.inner.integration_network_enabled(context)
                && context.capabilities.contains(&Capability::Integration)
                && context.capabilities.contains(&Capability::CatalogWrite);
        }
        if name == "generate_tags" {
            return connected
                && self.inner.integration_network_enabled(context)
                && context.capabilities.contains(&Capability::Integration)
                && context.capabilities.contains(&Capability::CatalogRead);
        }
        if matches!(
            name,
            "integration_get_settings"
                | "integration_save_ai_provider"
                | "integration_delete_ai_provider"
                | "discard_tag_review"
        ) {
            return connected && context.capabilities.contains(&Capability::Integration);
        }
        if name == "apply_tag_review" {
            return connected
                && context.capabilities.contains(&Capability::Integration)
                && context.capabilities.contains(&Capability::CatalogWrite);
        }
        let required = if matches!(
            name,
            "scan_directory"
                | "cancel_scan"
                | "update_index_root_settings"
                | "refresh_hashes"
                | "calculate_missing_hashes"
        ) {
            &Capability::Scan
        } else if matches!(
            name,
            "prepare_bundle_preview"
                | "get_bundle_preview_geometry"
                | "download_bundle"
                | "get_model_geometry"
                | "close_bundle_preview"
        ) {
            &Capability::MediaRead
        } else if write {
            &Capability::CatalogWrite
        } else {
            &Capability::CatalogRead
        };
        connected && context.capabilities.contains(required)
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
    fn rename_metadata_global(
        &self,
        context: &RequestContext,
        kind: MetadataValueKind,
        old_name: &str,
        new_name: &str,
    ) -> Result<u64, EngineError> {
        self.inner
            .rename_metadata_value(context, kind, old_name, new_name)
            .map_err(map_runtime_error)
    }
    fn delete_metadata_global(
        &self,
        context: &RequestContext,
        kind: MetadataValueKind,
        name: &str,
    ) -> Result<u64, EngineError> {
        self.inner
            .delete_metadata_value(context, kind, name)
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
    fn create_history_batch(
        &self,
        c: &RequestContext,
        idempotency_key: &str,
        items: Vec<PrintAttemptBatchItem>,
    ) -> Result<Vec<PrintAttempt>, EngineError> {
        self.inner
            .create_print_attempts(c, idempotency_key, items)
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
    fn roots(&self, c: &RequestContext) -> Result<Vec<RootHandle>, EngineError> {
        self.inner
            .runtime_info(c)
            .map(|v| v.roots)
            .map_err(map_runtime_error)
    }
    fn start_scan(
        &self,
        c: &RequestContext,
        id: Option<String>,
    ) -> Result<ScanStatus, EngineError> {
        let catalog_root_id = if let Some(handle_id) = id {
            let mut matches = self
                .inner
                .runtime_info(c)
                .map_err(map_runtime_error)?
                .roots
                .into_iter()
                .filter(|root| {
                    root.id == handle_id
                        || root.catalog_root_id.as_deref() == Some(handle_id.as_str())
                })
                .filter_map(|root| root.catalog_root_id)
                .collect::<std::collections::BTreeSet<_>>();
            if matches.len() != 1 {
                return Err(EngineError {
                    code: "forbidden",
                    message: "Root handle is not admitted for scanning".into(),
                    retryable: false,
                    current_version: None,
                });
            }
            matches.pop_first()
        } else {
            None
        };
        self.inner
            .start_scan(c, catalog_root_id)
            .map_err(map_runtime_error)
    }
    fn cancel_scan(&self, c: &RequestContext) -> Result<ScanStatus, EngineError> {
        self.inner.cancel_scan(c).map_err(map_runtime_error)
    }
    fn scan_status(&self, c: &RequestContext) -> Result<ScanStatus, EngineError> {
        self.inner.scan_status(c).map_err(map_runtime_error)
    }
    fn prepare_bundle(
        &self,
        c: &RequestContext,
        id: &str,
    ) -> Result<BundlePreviewSummary, EngineError> {
        self.inner
            .prepare_bundle_preview(c, id)
            .map_err(map_runtime_error)
    }
    fn describe_bundle_geometry(
        &self,
        c: &RequestContext,
        t: &str,
    ) -> Result<ContentLease, EngineError> {
        self.inner
            .describe_bundle_geometry(c, t)
            .map_err(map_runtime_error)
    }
    fn stream_bundle_geometry(
        &self,
        c: &RequestContext,
        t: &str,
        s: &mut dyn std::io::Write,
    ) -> Result<(), EngineError> {
        self.inner
            .stream_bundle_geometry(c, t, s)
            .map_err(map_runtime_error)
    }
    fn close_bundle(&self, c: &RequestContext, t: &str) -> Result<(), EngineError> {
        self.inner
            .close_bundle_preview(c, t)
            .map_err(map_runtime_error)
    }
    fn issue_bundle_download(
        &self,
        c: &RequestContext,
        token: &str,
    ) -> Result<ContentLease, EngineError> {
        self.inner
            .issue_bundle_download(c, token)
            .map_err(map_runtime_error)
    }
    fn issue_model_geometry(
        &self,
        c: &RequestContext,
        model_id: &str,
        expected_hash: Option<&str>,
        plate_id: Option<u32>,
    ) -> Result<GeometryLease, EngineError> {
        self.inner
            .issue_model_geometry(c, model_id, expected_hash, plate_id)
            .map_err(map_runtime_error)
    }
    fn duplicate_groups(
        &self,
        c: &RequestContext,
        model_ids: Vec<String>,
    ) -> Result<Vec<DuplicateGroup>, EngineError> {
        self.inner
            .duplicate_groups(c, model_ids)
            .map_err(map_runtime_error)
    }
    fn read_embedded_metadata(
        &self,
        c: &RequestContext,
        model_id: &str,
        expected_hash: Option<&str>,
    ) -> Result<EmbeddedMetadataProposal, EngineError> {
        self.inner
            .read_embedded_metadata(c, model_id, expected_hash)
            .map_err(map_runtime_error)
    }
    fn apply_embedded_metadata(
        &self,
        c: &RequestContext,
        target: VersionedTarget,
        expected_source_hash: &str,
        selection: EmbeddedMetadataSelection,
    ) -> Result<MutationVersion, EngineError> {
        self.inner
            .apply_embedded_metadata(c, target, expected_source_hash, selection)
            .map_err(map_runtime_error)
    }
    fn plan_management_action(
        &self,
        c: &RequestContext,
        action: ManagementAction,
    ) -> Result<ActionPlan, EngineError> {
        self.inner.plan_action(c, action).map_err(map_runtime_error)
    }
    fn plan_library_export(
        &self,
        c: &RequestContext,
        dest_path: Option<&str>,
    ) -> Result<ActionPlan, EngineError> {
        self.inner
            .plan_library_export(c, dest_path)
            .map_err(map_runtime_error)
    }
    fn execute_management_action(
        &self,
        c: &RequestContext,
        token: &str,
    ) -> Result<ManagementResult, EngineError> {
        self.inner
            .execute_action(c, token)
            .map_err(map_runtime_error)
    }
    fn list_captures(
        &self,
        c: &RequestContext,
        p: PageRequest,
    ) -> Result<library_protocol::Page<CaptureSummary>, EngineError> {
        self.inner
            .list_capture_items(c, p)
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
        "search_models" => (
            &[
                "query",
                "cursor",
                "limit",
                "thumbnailMode",
                "rootId",
                "sourceDirectory",
                "minimumRating",
                "favorite",
            ],
            &[],
        ),
        "integration_get_settings" | "sync_spoolman_filaments" => (&[], &[]),
        "integration_save_spoolman_config" => (&["config"], &["config"]),
        "integration_save_ai_provider" => (
            &["config", "credentialUpdate"],
            &["config", "credentialUpdate"],
        ),
        "integration_delete_ai_provider" => (&["providerId"], &["providerId"]),
        "generate_tags" => (&["providerId", "targets"], &["providerId", "targets"]),
        "apply_tag_review" => (
            &["reviewToken", "decisions", "strategy"],
            &["reviewToken", "decisions", "strategy"],
        ),
        "discard_tag_review" => (&["reviewToken"], &["reviewToken"]),
        "get_hash_status" | "refresh_hashes" => (&[], &[]),
        "calculate_missing_hashes" => (&[], &[]),
        "check_files_exist" => (&["modelIds"], &["modelIds"]),
        "start_thumbnail_job" => (&["modelIds", "force"], &["modelIds"]),
        "get_models_missing_thumbnails" | "get_models_with_default_thumbnails" => {
            (&["cursor", "limit"], &[])
        }
        "get_models_by_directory" => (
            &["rootId", "directory", "cursor", "limit"],
            &["rootId", "directory"],
        ),
        "rename_metadata" => (
            &["type", "oldName", "newName"],
            &["type", "oldName", "newName"],
        ),
        "delete_metadata" => (&["type", "name"], &["type", "name"]),
        "add_model_tags" | "remove_model_tags" => (
            &["modelId", "expectedVersion", "tags"],
            &["modelId", "expectedVersion", "tags"],
        ),
        "remove_model" => (
            &["modelId", "expectedVersion", "confirm"],
            &["modelId", "expectedVersion", "confirm"],
        ),
        "backup_database" => (
            &["destinationHandleId", "fileName"],
            &["destinationHandleId", "fileName"],
        ),
        "export_library" => (&["destPath"], &[]),
        "plan_management_action" => (&["action"], &["action"]),
        "execute_management_action" => (&["token"], &["token"]),
        "get_index_root_settings" | "get_catalog_settings" => (&[], &[]),
        "update_index_root_settings" => (&["handleId", "settings"], &["handleId", "settings"]),
        "update_catalog_settings" => (&["settings"], &["settings"]),
        "get_model" | "get_all_metadata" => (&["modelId"], &["modelId"]),
        "get_library_stats" | "list_parent_models" | "list_designers" | "list_licenses"
        | "list_roots" | "cancel_scan" | "get_scan_status" => (&[], &[]),
        "scan_directory" => (&["rootId"], &[]),
        "prepare_bundle_preview" => (&["bundleId"], &["bundleId"]),
        "get_bundle_preview_geometry" | "download_bundle" | "close_bundle_preview" => {
            (&["token"], &["token"])
        }
        "get_model_geometry" => (&["modelId", "expectedHash", "plateId"], &["modelId"]),
        "find_duplicates" => (&["modelIds"], &["modelIds"]),
        "get_embedded_3mf_metadata" | "pull_3mf_metadata" => {
            (&["modelId", "expectedHash"], &["modelId"])
        }
        "apply_embedded_3mf_metadata" => (
            &[
                "modelId",
                "expectedVersion",
                "expectedSourceHash",
                "selection",
            ],
            &[
                "modelId",
                "expectedVersion",
                "expectedSourceHash",
                "selection",
            ],
        ),
        "list_capture_items" => (&["cursor", "limit"], &[]),
        "retry_capture" => (
            &["itemId", "envelope", "uploads"],
            &["itemId", "envelope", "uploads"],
        ),
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
        "log_print_events_batch" => (&["idempotencyKey", "items"], &["idempotencyKey", "items"]),
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
    let string_field_valid = |key: &str| object.get(key).is_none_or(Value::is_string);
    let valid_types = match name {
        "get_model" | "get_all_metadata" => string_field_valid("modelId"),
        "search_models" => {
            string_field_valid("query")
                && string_field_valid("cursor")
                && object.get("limit").is_none_or(Value::is_u64)
                && string_field_valid("thumbnailMode")
                && string_field_valid("rootId")
                && string_field_valid("sourceDirectory")
                && object
                    .get("minimumRating")
                    .is_none_or(|value| value.as_u64().is_some_and(|rating| rating <= 5))
                && object.get("favorite").is_none_or(Value::is_boolean)
        }
        "integration_save_spoolman_config" => object
            .get("config")
            .is_some_and(|value| value.is_null() || value.is_object()),
        "integration_save_ai_provider" => {
            object.get("config").is_some_and(Value::is_object)
                && object.get("credentialUpdate").is_some_and(Value::is_object)
        }
        "integration_delete_ai_provider" => string_field_valid("providerId"),
        "generate_tags" => {
            string_field_valid("providerId") && object.get("targets").is_some_and(Value::is_array)
        }
        "apply_tag_review" => {
            string_field_valid("reviewToken")
                && object.get("decisions").is_some_and(Value::is_array)
                && object.get("strategy").is_some_and(Value::is_string)
        }
        "discard_tag_review" => string_field_valid("reviewToken"),
        "check_files_exist" => object.get("modelIds").is_some_and(Value::is_array),
        "start_thumbnail_job" => {
            object.get("modelIds").is_some_and(Value::is_array)
                && object.get("force").is_none_or(Value::is_boolean)
        }
        "get_models_missing_thumbnails" | "get_models_with_default_thumbnails" => {
            string_field_valid("cursor") && object.get("limit").is_none_or(Value::is_u64)
        }
        "get_models_by_directory" => {
            string_field_valid("rootId")
                && string_field_valid("directory")
                && string_field_valid("cursor")
                && object.get("limit").is_none_or(Value::is_u64)
        }
        "rename_metadata" => {
            string_field_valid("type")
                && string_field_valid("oldName")
                && string_field_valid("newName")
        }
        "delete_metadata" => string_field_valid("type") && string_field_valid("name"),
        "add_model_tags" | "remove_model_tags" => {
            string_field_valid("modelId")
                && string_field_valid("expectedVersion")
                && object.get("tags").is_some_and(Value::is_array)
        }
        "remove_model" => {
            string_field_valid("modelId")
                && string_field_valid("expectedVersion")
                && object.get("confirm").is_some_and(Value::is_boolean)
        }
        "backup_database" => {
            string_field_valid("destinationHandleId") && string_field_valid("fileName")
        }
        "export_library" => string_field_valid("destPath"),
        "plan_management_action" => object.get("action").is_some_and(Value::is_object),
        "execute_management_action" => string_field_valid("token"),
        "update_index_root_settings" => {
            string_field_valid("handleId") && object.get("settings").is_some_and(Value::is_object)
        }
        "update_catalog_settings" => object.get("settings").is_some_and(Value::is_object),
        "scan_directory" => string_field_valid("rootId"),
        "prepare_bundle_preview" => string_field_valid("bundleId"),
        "get_bundle_preview_geometry" | "download_bundle" | "close_bundle_preview" => {
            string_field_valid("token")
        }
        "get_model_geometry" => {
            string_field_valid("modelId")
                && string_field_valid("expectedHash")
                && object.get("plateId").is_none_or(Value::is_u64)
        }
        "find_duplicates" => object.get("modelIds").is_some_and(Value::is_array),
        "get_embedded_3mf_metadata" | "pull_3mf_metadata" => {
            string_field_valid("modelId") && string_field_valid("expectedHash")
        }
        "apply_embedded_3mf_metadata" => {
            string_field_valid("modelId")
                && string_field_valid("expectedVersion")
                && string_field_valid("expectedSourceHash")
                && object.get("selection").is_some_and(Value::is_object)
        }
        "list_capture_items" => {
            string_field_valid("cursor") && object.get("limit").is_none_or(Value::is_u64)
        }
        "retry_capture" => {
            string_field_valid("itemId")
                && object.get("envelope").is_some_and(Value::is_object)
                && object.get("uploads").is_some_and(Value::is_array)
        }
        "log_print_events_batch" => {
            string_field_valid("idempotencyKey") && object.get("items").is_some_and(Value::is_array)
        }
        _ => true,
    };
    if !valid_types {
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
        source_root_id: model.root_id,
        bundle_id: model.bundle_id,
        archive_member: model.archive_member,
        parent_model_id: model.parent_model_id,
        created_at: model.created_at,
        updated_at: model.updated_at,
        last_printed_at: model.last_printed_at,
        print_count: model.print_count.to_string(),
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
        rating: model.rating,
        favorite: model.favorite,
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

async fn blocking_engine<E, T, F>(
    engine: Arc<E>,
    request_id: &str,
    operation: F,
) -> Result<T, ApiError>
where
    E: LibraryEngine,
    T: Send + 'static,
    F: FnOnce(Arc<E>) -> Result<T, EngineError> + Send + 'static,
{
    let request_id = request_id.to_owned();
    match tokio::task::spawn_blocking(move || operation(engine)).await {
        Ok(result) => result.map_err(|error| ApiError::from_engine(error, &request_id)),
        Err(_) => Err(ApiError::from_engine(
            EngineError {
                code: "internal",
                message: "The operation stopped unexpectedly".into(),
                retryable: true,
                current_version: None,
            },
            &request_id,
        )),
    }
}

fn static_content_type(extension: &str) -> Option<&'static str> {
    match extension {
        "html" => Some("text/html; charset=utf-8"),
        "css" => Some("text/css; charset=utf-8"),
        "js" => Some("text/javascript; charset=utf-8"),
        "json" => Some("application/json"),
        "wasm" => Some("application/wasm"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        "ico" => Some("image/x-icon"),
        "woff" => Some("font/woff"),
        "woff2" => Some("font/woff2"),
        "ttf" => Some("font/ttf"),
        "otf" => Some("font/otf"),
        _ => None,
    }
}

fn load_static_assets(web_root: &std::path::Path) -> Result<StaticAssets, String> {
    use std::io::Read as _;

    let root_metadata = std::fs::symlink_metadata(web_root)
        .map_err(|_| "Browser asset directory could not be inspected")?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err("Browser asset directory must be a real directory".into());
    }
    let root = web_root
        .canonicalize()
        .map_err(|_| "Browser asset directory could not be resolved")?;
    let mut pending = vec![root.clone()];
    let mut files = BTreeMap::new();
    let mut total = 0_u64;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)
            .map_err(|_| "Browser asset directory could not be read")?
        {
            let entry = entry.map_err(|_| "Browser asset directory contains an invalid entry")?;
            let path = entry.path();
            let relative = path
                .strip_prefix(&root)
                .map_err(|_| "Browser asset escaped its directory")?;
            if relative.components().count() > 8
                || relative.components().any(|component| match component {
                    std::path::Component::Normal(value) => value
                        .to_str()
                        .is_none_or(|name| name.starts_with('.') || name.is_empty()),
                    _ => true,
                })
            {
                return Err("Browser asset path is invalid".into());
            }
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|_| "Browser asset could not be inspected")?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() || files.len() >= MAX_STATIC_FILES {
                return Err("Browser asset inventory is invalid or too large".into());
            }
            let extension = path
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .map(str::to_ascii_lowercase)
                .ok_or_else(|| "Browser asset has an unsupported file type".to_string())?;
            let content_type = static_content_type(&extension)
                .ok_or_else(|| "Browser asset has an unsupported file type".to_string())?;
            if metadata.len() > MAX_STATIC_ASSET_BYTES {
                return Err("Browser asset exceeds its size limit".into());
            }
            let canonical = path
                .canonicalize()
                .map_err(|_| "Browser asset could not be resolved")?;
            if !canonical.starts_with(&root) || canonical != path {
                return Err("Browser asset escaped its directory".into());
            }
            let file =
                std::fs::File::open(&path).map_err(|_| "Browser asset could not be opened")?;
            let opened = file
                .metadata()
                .map_err(|_| "Browser asset could not be inspected")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt as _;
                if metadata.dev() != opened.dev() || metadata.ino() != opened.ino() {
                    return Err("Browser asset changed during validation".into());
                }
            }
            let mut bytes = Vec::with_capacity(usize::try_from(opened.len()).unwrap_or(0));
            file.take(MAX_STATIC_ASSET_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "Browser asset could not be read")?;
            if bytes.len() as u64 != opened.len() || bytes.len() as u64 > MAX_STATIC_ASSET_BYTES {
                return Err("Browser asset changed during validation".into());
            }
            let after = std::fs::symlink_metadata(&path)
                .map_err(|_| "Browser asset changed during validation")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt as _;
                if after.file_type().is_symlink()
                    || after.dev() != opened.dev()
                    || after.ino() != opened.ino()
                {
                    return Err("Browser asset changed during validation".into());
                }
            }
            total = total
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| "Browser asset inventory is too large".to_string())?;
            if total > MAX_STATIC_TOTAL_BYTES {
                return Err("Browser asset inventory is too large".into());
            }
            let key = format!("/{}", relative.to_string_lossy().replace('\\', "/"));
            files.insert(
                key,
                StaticAsset {
                    bytes: Bytes::from(bytes),
                    content_type,
                },
            );
        }
    }
    if !files.contains_key("/index.html") {
        return Err("Browser asset directory is missing index.html".into());
    }
    Ok(StaticAssets {
        files: Arc::new(files),
    })
}

async fn static_asset<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
) -> Response {
    let Some(assets) = state.static_assets else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let path = uri.path();
    let key = if path == "/" {
        "/index.html"
    } else if assets.files.contains_key(path) {
        path
    } else if !path.starts_with("/api/")
        && path != "/mcp"
        && !path.rsplit('/').next().unwrap_or_default().contains('.')
    {
        "/index.html"
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(asset) = assets.files.get(key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = (StatusCode::OK, asset.bytes.clone()).into_response();
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static(asset.content_type),
    );
    response
}

async fn admit_large_upload<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let request_id = request
        .extensions()
        .get::<RequestContext>()
        .map(|context| context.request_id.clone())
        .unwrap_or_else(|| "request".into());
    let permit = match state.large_upload_admission.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(axum::http::header::RETRY_AFTER, "1")],
                Json(serde_json::json!({"error":{
                    "code":"busy",
                    "message":"Large upload limit reached",
                    "requestId":request_id,
                    "retryable":true
                }})),
            )
                .into_response()
        }
    };
    let response = next.run(request).await;
    drop(permit);
    response
}

pub fn router<E: LibraryEngine>(engine: Arc<E>, security: SecurityConfig) -> Router {
    build_router(engine, security, None)
}

pub fn router_with_static<E: LibraryEngine>(
    engine: Arc<E>,
    security: SecurityConfig,
    web_root: &std::path::Path,
) -> Result<Router, String> {
    let assets = load_static_assets(web_root)?;
    Ok(build_router(engine, security, Some(assets)))
}

fn build_router<E: LibraryEngine>(
    engine: Arc<E>,
    security: SecurityConfig,
    static_assets: Option<StaticAssets>,
) -> Router {
    let state = ServerState {
        engine,
        admission: Arc::new(Semaphore::new(security.max_in_flight)),
        content_admission: Arc::new(Semaphore::new(4)),
        large_upload_admission: Arc::new(Semaphore::new(2)),
        event_admission: Arc::new(Semaphore::new(8)),
        content_send_timeout: security.content_send_timeout,
        mcp_sessions: Arc::new(mcp::SessionStore::new(
            security.max_mcp_sessions,
            security.mcp_session_ttl,
        )),
        static_assets,
        security: Arc::new(security),
    };
    let mut routes = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/openapi.json", get(openapi))
        .route("/api/v1/capabilities", get(capabilities::<E>))
        .route("/api/v1/status", get(status::<E>))
        .route("/api/v1/models", get(models::<E>))
        .route("/api/v1/models/{model_id}", get(model::<E>))
        .route("/api/v1/catalog/stats", get(catalog_stats::<E>))
        .route("/api/v1/catalog/facets", get(catalog_facets::<E>))
        .route("/api/v1/catalog/hash-status", get(catalog_hash_status::<E>))
        .route(
            "/api/v1/models/source-status",
            axum::routing::post(model_source_status::<E>),
        )
        .route(
            "/api/v1/thumbnails/jobs",
            axum::routing::post(start_thumbnail_job::<E>),
        )
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
            "/api/v1/history/bulk",
            axum::routing::post(create_history_batch::<E>),
        )
        .route(
            "/api/v1/history/{attempt_id}",
            axum::routing::put(update_history::<E>).delete(delete_history::<E>),
        )
        .route(
            "/api/v1/captures/uploads",
            axum::routing::post(capture_upload::<E>)
                .layer(axum::extract::DefaultBodyLimit::max(101 * 1024 * 1024))
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    admit_large_upload::<E>,
                )),
        )
        .route(
            "/api/v1/captures/import",
            axum::routing::post(import_capture::<E>),
        )
        .route(
            "/api/v1/captures/{item_id}/retry",
            axum::routing::post(retry_capture::<E>),
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
        .route("/api/v1/roots", get(roots::<E>))
        .route("/api/v1/settings/roots", get(index_root_settings::<E>))
        .route(
            "/api/v1/settings/roots/{handle_id}",
            axum::routing::patch(update_index_root_settings::<E>),
        )
        .route(
            "/api/v1/settings/catalog",
            get(catalog_settings::<E>).put(update_catalog_settings::<E>),
        )
        .route(
            "/api/v1/management/actions/plan",
            axum::routing::post(plan_management_action::<E>),
        )
        .route(
            "/api/v1/management/actions/execute",
            axum::routing::post(execute_management_action::<E>),
        )
        .route(
            "/api/v1/integrations/settings",
            get(integration_settings::<E>),
        )
        .route(
            "/api/v1/integrations/spoolman",
            axum::routing::put(save_spoolman_config::<E>),
        )
        .route(
            "/api/v1/integrations/spoolman/pull",
            axum::routing::post(pull_spoolman::<E>),
        )
        .route(
            "/api/v1/integrations/ai/providers/{provider_id}",
            axum::routing::put(save_ai_provider::<E>).delete(delete_ai_provider::<E>),
        )
        .route(
            "/api/v1/integrations/ai/suggestions",
            axum::routing::post(suggest_ai_tags::<E>),
        )
        .route(
            "/api/v1/integrations/ai/reviews/{review_token}/apply",
            axum::routing::post(apply_ai_review::<E>),
        )
        .route(
            "/api/v1/integrations/ai/reviews/{review_token}",
            axum::routing::delete(discard_ai_review::<E>),
        )
        .route(
            "/api/v1/scan",
            get(scan_status::<E>)
                .post(start_scan::<E>)
                .delete(cancel_scan::<E>),
        )
        .route(
            "/api/v1/bundles/{bundle_id}/preview",
            axum::routing::post(prepare_bundle::<E>),
        )
        .route(
            "/api/v1/bundle-previews/{token}/geometry",
            get(bundle_geometry::<E>),
        )
        .route(
            "/api/v1/bundle-previews/{token}/download-lease",
            axum::routing::post(bundle_download::<E>),
        )
        .route(
            "/api/v1/bundle-previews/{token}",
            axum::routing::delete(close_bundle::<E>),
        )
        .route(
            "/api/v1/models/{model_id}/geometry-lease",
            axum::routing::post(model_geometry::<E>),
        )
        .route(
            "/api/v1/duplicates/query",
            axum::routing::post(duplicates::<E>),
        )
        .route(
            "/api/v1/models/{model_id}/embedded-metadata/proposal",
            axum::routing::post(embedded_metadata::<E>),
        )
        .route(
            "/api/v1/models/{model_id}/embedded-metadata/apply",
            axum::routing::post(apply_embedded_metadata::<E>),
        )
        .route("/api/v1/captures", get(list_captures::<E>))
        .route(
            "/mcp",
            axum::routing::post(mcp::post::<E>)
                .get(mcp::get)
                .delete(mcp::delete::<E>),
        );
    if state.static_assets.is_some() {
        routes = routes.fallback(static_asset::<E>);
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
                ,{"name":"tagId","in":"query","schema":{"type":"string","maxLength":128}}
                ,{"name":"category","in":"query","schema":{"type":"string","maxLength":120}}
                ,{"name":"designer","in":"query","schema":{"type":"string","maxLength":256}}
                ,{"name":"modelLicense","in":"query","schema":{"type":"string","maxLength":256}}
                ,{"name":"printStatus","in":"query","schema":{"type":"string","enum":["unprinted","want","queued","printing","printed","failed"]}}
                ,{"name":"everPrinted","in":"query","schema":{"type":"boolean"}}
                ,{"name":"bundleId","in":"query","schema":{"type":"string","maxLength":128}}
                ,{"name":"rootId","in":"query","schema":{"type":"string","maxLength":128}}
                ,{"name":"filamentId","in":"query","schema":{"type":"string","maxLength":128}}
                ,{"name":"sourceMissing","in":"query","schema":{"type":"boolean"}}
                ,{"name":"duplicatesOnly","in":"query","schema":{"type":"boolean"}}
                ,{"name":"sourceKind","in":"query","schema":{"type":"string","maxLength":64}}
                ,{"name":"group","in":"query","schema":{"type":"string","enum":["standalone","bundle","archive"]}}
                ,{"name":"thumbnailMode","in":"query","schema":{"type":"string","enum":["missing","default","custom"]}}
                ,{"name":"sourceDirectory","in":"query","schema":{"type":"string","maxLength":1024,"description":"Relative directory prefix within rootId"}}
                ,{"name":"minimumRating","in":"query","schema":{"type":"integer","minimum":0,"maximum":5}}
                ,{"name":"favorite","in":"query","schema":{"type":"boolean"}}
            ],"responses":{"200":{"description":"Revision-bound model page"},"409":{"description":"Cursor revision is stale"}}}},
            "/api/v1/models/{modelId}":{"get":{"responses":{"200":{"description":"Path-free model detail"},"404":{"description":"Unknown model"}}}},
            "/api/v1/catalog/stats":{"get":{"responses":{"200":{"description":"Catalog totals with decimal-string counters and bytes"}}}},
            "/api/v1/catalog/facets":{"get":{"responses":{"200":{"description":"Bounded designer, license, category, parent, folder, and root facets"}}}},
            "/api/v1/catalog/hash-status":{"get":{"responses":{"200":{"description":"Decimal-string catalog hash coverage counts"}}}},
            "/api/v1/models/source-status":{"post":{"responses":{"200":{"description":"Path-free verified source states for up to 10000 models"}}}},
            "/api/v1/thumbnails/jobs":{"post":{"responses":{"200":{"description":"Started bounded shared thumbnail generation job"},"409":{"description":"Another scan or thumbnail job is active"}}}},
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
            "/api/v1/history/bulk":{"post":{"requestBody":{"required":true,"content":{"application/json":{"schema":{"type":"object","additionalProperties":false,"required":["idempotencyKey","items"],"properties":{"idempotencyKey":{"type":"string","minLength":1,"maxLength":128},"items":{"type":"array","minItems":1,"maxItems":10000,"items":{"type":"object","required":["modelId","input"]}}}}}}},"responses":{"200":{"description":"Atomic idempotent print attempt batch"},"409":{"description":"Idempotency key payload conflict or stale model"}}}},
            "/api/v1/history/{attemptId}":{"put":{"responses":{"200":{"description":"Updated print attempt"}}},"delete":{"responses":{"204":{"description":"Deleted print attempt"}}}},
            "/api/v1/captures/uploads":{"post":{"requestBody":{"required":true,"content":{"multipart/form-data":{"schema":{"type":"object","required":["intent","file"],"properties":{"intent":{"type":"string","description":"UploadIntent JSON"},"file":{"type":"string","contentEncoding":"binary"}}}}}},"responses":{"200":{"description":"Epoch-bound staged upload token"},"413":{"description":"Upload exceeds 100 MiB"}}}},
            "/api/v1/captures/import":{"post":{"responses":{"200":{"description":"Idempotent capture import result"}}}},
            "/api/v1/captures/{itemId}/retry":{"post":{"responses":{"200":{"description":"Retry using the exact original envelope and replacement upload tokens"},"422":{"description":"Envelope itemId does not match route"}}}},
            "/api/v1/captures/{itemId}":{"get":{"responses":{"200":{"description":"Persisted capture result"}}}},
            "/api/v1/captures":{"get":{"parameters":[{"name":"cursor","in":"query","schema":{"type":"string","maxLength":512}},{"name":"limit","in":"query","schema":{"type":"integer","minimum":1,"maximum":100}}],"responses":{"200":{"description":"Bounded capture summary page"}}}},
            "/api/v1/roots":{"get":{"responses":{"200":{"description":"Opaque admitted root handles"}}}},
            "/api/v1/settings/roots":{"get":{"responses":{"200":{"description":"Path-free index root settings; maxFileBytes is a decimal string"}}}},
            "/api/v1/settings/roots/{handleId}":{"patch":{"requestBody":{"required":true,"content":{"application/json":{"schema":{"type":"object","additionalProperties":false,"required":["enabled","scanIntervalMinutes","maxFileBytes","scanZip"],"properties":{"enabled":{"type":"boolean"},"scanIntervalMinutes":{"type":"integer","minimum":1,"maximum":1440},"maxFileBytes":{"type":"string","pattern":"^(0|[1-9][0-9]*)$"},"scanZip":{"type":"boolean"}}}}}},"responses":{"200":{"description":"Validated index root settings update"},"403":{"description":"Scan capability required"}}}},
            "/api/v1/settings/catalog":{"get":{"responses":{"200":{"description":"Catalog thumbnail and scan settings"}}},"put":{"requestBody":{"required":true,"content":{"application/json":{"schema":{"type":"object","additionalProperties":false,"required":["thumbnailSize","thumbnailBackground","thumbnailConcurrency","scanBatchSize"],"properties":{"thumbnailSize":{"type":"integer","enum":[128,256,512]},"thumbnailBackground":{"type":"string","pattern":"^#[0-9A-Fa-f]{6}$"},"thumbnailConcurrency":{"type":"integer","minimum":1,"maximum":2},"scanBatchSize":{"type":"integer","minimum":16,"maximum":256}}}}}},"responses":{"200":{"description":"Validated catalog settings update"},"403":{"description":"CatalogWrite capability required"}}}},
            "/api/v1/management/actions/plan":{"post":{"responses":{"200":{"description":"Five-minute principal, epoch, revision, version, and source-hash-bound action plan"},"403":{"description":"Required catalog-write or maintenance capability is absent"}}}},
            "/api/v1/management/actions/execute":{"post":{"requestBody":{"required":true,"content":{"application/json":{"schema":{"type":"object","additionalProperties":false,"required":["token"],"properties":{"token":{"type":"string","minLength":1,"maxLength":256}}}}}},"responses":{"200":{"description":"One-use planned action result"},"409":{"description":"Plan is stale or already consumed"}}}},
            "/api/v1/integrations/settings":{"get":{"responses":{"200":{"description":"Redacted integration settings; credentials are never returned"}}}},
            "/api/v1/integrations/spoolman":{"put":{"responses":{"200":{"description":"Validated nonsecret Spoolman configuration"}}}},
            "/api/v1/integrations/spoolman/pull":{"post":{"responses":{"200":{"description":"Atomic filament and spool snapshot result"},"501":{"description":"Network transport is not enabled by this host"}}}},
            "/api/v1/integrations/ai/providers/{providerId}":{"put":{"responses":{"200":{"description":"Validated provider with session-only credential action"}}},"delete":{"responses":{"204":{"description":"Provider and session credential removed"}}}},
            "/api/v1/integrations/ai/suggestions":{"post":{"responses":{"200":{"description":"Human-review-only bounded tag suggestions"},"501":{"description":"Network transport is not enabled by this host"}}}},
            "/api/v1/integrations/ai/reviews/{reviewToken}/apply":{"post":{"responses":{"200":{"description":"Atomic versioned accepted tag mutations"},"409":{"description":"Review is stale or replayed"}}}},
            "/api/v1/integrations/ai/reviews/{reviewToken}":{"delete":{"responses":{"204":{"description":"Review discarded"}}}},
            "/api/v1/scan":{"get":{"responses":{"200":{"description":"Scan status"}}},"post":{"requestBody":{"required":true,"content":{"application/json":{"schema":{"type":"object","additionalProperties":false,"properties":{"rootId":{"type":["string","null"],"maxLength":128}}}}}},"responses":{"200":{"description":"Started admitted-root scan"},"403":{"description":"Root handle is not admitted"}}},"delete":{"responses":{"200":{"description":"Cancellation requested"}}}},
            "/api/v1/bundles/{bundleId}/preview":{"post":{"responses":{"200":{"description":"Prepared bounded bundle preview summary"}}}},
            "/api/v1/bundle-previews/{token}/geometry":{"get":{"responses":{"200":{"description":"Existing GoggleLab binary geometry packet","content":{"application/vnd.gogglelab.geometry":{}}}}}},
            "/api/v1/bundle-previews/{token}/download-lease":{"post":{"responses":{"200":{"description":"Verified all-member ZIP content lease"}}}},
            "/api/v1/bundle-previews/{token}":{"delete":{"responses":{"204":{"description":"Preview closed"}}}},
            "/api/v1/models/{modelId}/geometry-lease":{"post":{"responses":{"200":{"description":"Verified STL2 v2 model geometry lease"},"409":{"description":"Source hash changed"}}}},
            "/api/v1/duplicates/query":{"post":{"responses":{"200":{"description":"Path-free duplicate groups for the requested models"}}}},
            "/api/v1/models/{modelId}/embedded-metadata/proposal":{"post":{"responses":{"200":{"description":"Read-only embedded 3MF metadata proposal"},"409":{"description":"Source hash changed"}}}},
            "/api/v1/models/{modelId}/embedded-metadata/apply":{"post":{"responses":{"200":{"description":"Versioned embedded metadata selection applied"},"409":{"description":"Model version or source hash changed"}}}},
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
        || query
            .folder_id
            .as_ref()
            .is_some_and(|value| value.len() > 128)
        || query.tag_id.as_ref().is_some_and(|value| value.len() > 128)
        || query
            .category
            .as_ref()
            .is_some_and(|value| value.len() > 120)
        || query
            .designer
            .as_ref()
            .is_some_and(|value| value.len() > 256)
        || query
            .model_license
            .as_ref()
            .is_some_and(|value| value.len() > 256)
        || query
            .bundle_id
            .as_ref()
            .is_some_and(|value| value.len() > 128)
        || query
            .root_id
            .as_ref()
            .is_some_and(|value| value.len() > 128)
        || query
            .filament_id
            .as_ref()
            .is_some_and(|value| value.len() > 128)
        || query
            .source_kind
            .as_ref()
            .is_some_and(|value| value.len() > 64)
        || query
            .thumbnail_mode
            .as_ref()
            .is_some_and(|value| !matches!(value.as_str(), "missing" | "default" | "custom"))
        || query
            .source_directory
            .as_ref()
            .is_some_and(|value| value.len() > 1024 || value.contains('\\') || value.contains('\0'))
        || query.minimum_rating.is_some_and(|rating| rating > 5)
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
    let cursor = query.cursor;
    let protocol_query = ProtocolModelQuery {
        folder_id: query.folder_id,
        search: query.query,
        sort: query.sort.unwrap_or(ModelSort::Date),
        direction: query.direction.unwrap_or(SortDirection::Asc),
        tag_id: query.tag_id,
        category: query.category,
        designer: query.designer,
        model_license: query.model_license,
        print_status: query.print_status,
        ever_printed: query.ever_printed,
        bundle_id: query.bundle_id,
        root_id: query.root_id,
        filament_id: query.filament_id,
        source_missing: query.source_missing,
        duplicates_only: query.duplicates_only.unwrap_or(false),
        source_kind: query.source_kind,
        group: query.group,
        thumbnail_mode: query.thumbnail_mode,
        source_directory: query.source_directory,
        minimum_rating: query.minimum_rating,
        favorite: query.favorite,
    };
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.list_models(&request, cursor.as_deref(), limit, protocol_query)
    })
    .await
    .map(Json)
}

async fn model<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    AxumPath(model_id): AxumPath<String>,
) -> Result<Json<ModelSummary>, ApiError> {
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.get_model(&request, &model_id)
    })
    .await
    .map(Json)
}

async fn catalog_stats<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
) -> Result<Json<WireCatalogStats>, ApiError> {
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.catalog_stats(&request)
    })
    .await
    .map(|value| Json(value.into()))
}

async fn catalog_facets<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
) -> Result<Json<CatalogFacets>, ApiError> {
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.catalog_facets(&request)
    })
    .await
    .map(Json)
}
async fn catalog_hash_status<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
) -> Result<Json<HashStatus>, ApiError> {
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.catalog_hash_status(&request)
    })
    .await
    .map(Json)
}
async fn model_source_status<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    Json(body): Json<ModelIdsRequest>,
) -> Result<Json<Vec<ModelSourceStatus>>, ApiError> {
    validate_model_ids(&body.model_ids, 10_000)?;
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.check_model_sources(&request, body.model_ids)
    })
    .await
    .map(Json)
}
async fn start_thumbnail_job<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    Json(body): Json<ThumbnailJobRequest>,
) -> Result<Json<ScanStatus>, ApiError> {
    validate_model_ids(&body.model_ids, 10_000)?;
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.start_thumbnail_job(&request, body.model_ids, body.force)
    })
    .await
    .map(Json)
}

fn validate_model_ids(model_ids: &[String], maximum: usize) -> Result<(), ApiError> {
    let mut unique = std::collections::BTreeSet::new();
    if model_ids.is_empty()
        || model_ids.len() > maximum
        || model_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 128 || !unique.insert(id.as_str()))
    {
        return Err(api_bad("invalidInput", "modelIds are outside their limits"));
    }
    Ok(())
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
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GeometryRequest {
    expected_hash: Option<String>,
    plate_id: Option<u32>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DuplicateRequest {
    model_ids: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmbeddedApplyRequest {
    expected_version: String,
    expected_source_hash: String,
    selection: EmbeddedMetadataSelection,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExecuteActionRequest {
    token: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelIdsRequest {
    model_ids: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ThumbnailJobRequest {
    model_ids: Vec<String>,
    force: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SpoolmanConfigRequest {
    config: Option<IntegrationSpoolmanConfig>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AiProviderRequest {
    config: AiProviderConfig,
    credential_update: IntegrationCredentialUpdate,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AiSuggestRequest {
    provider_id: String,
    targets: Vec<WireTarget>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AiApplyRequest {
    decisions: Vec<IntegrationAiReviewDecision>,
    strategy: IntegrationAiReviewStrategy,
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
    let request = c.clone();
    blocking_engine(s.engine, &c.request_id, move |engine| {
        engine.issue_model_download(&request, &model, body.expected_hash.as_deref())
    })
    .await
    .map(|value| Json(value.into()))
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
    let describe_engine = Arc::clone(&s.engine);
    let describe_context = c.clone();
    let describe_lease = lease.clone();
    let descriptor = match blocking_engine(describe_engine, &c.request_id, move |engine| {
        engine.describe_content(&describe_context, &describe_lease)
    })
    .await
    {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let byte_limit = match descriptor.kind {
        ContentKind::Thumbnail => 4 * 1024 * 1024,
        ContentKind::Model => 100 * 1024 * 1024,
        ContentKind::Geometry => 128 * 1024 * 1024,
        ContentKind::BundleArchive => 4 * 1024 * 1024 * 1024,
    };
    if descriptor.bytes > byte_limit {
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
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrintBatchRequest {
    idempotency_key: String,
    items: Vec<PrintAttemptBatchItem>,
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
async fn create_history_batch<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    Json(body): Json<PrintBatchRequest>,
) -> Result<Json<Vec<PrintAttempt>>, ApiError> {
    if body.idempotency_key.is_empty()
        || body.idempotency_key.len() > 128
        || body.items.is_empty()
        || body.items.len() > 10_000
    {
        return Err(api_bad("invalidInput", "Print batch is outside its limits"));
    }
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.create_history_batch(&request, &body.idempotency_key, body.items)
    })
    .await
    .map(Json)
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
    let intent = intent
        .ok_or_else(|| api_bad("invalidInput", "intent is required"))?
        .parse()?;
    let file = file.ok_or_else(|| api_bad("invalidInput", "file is required"))?;
    let request = c.clone();
    blocking_engine(s.engine, &c.request_id, move |engine| {
        engine.stage_upload(&request, intent, file)
    })
    .await
    .map(|value| Json(value.into()))
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
    let request = c.clone();
    blocking_engine(s.engine, &c.request_id, move |engine| {
        engine.import_capture(&request, b.envelope, uploads)
    })
    .await
    .map(Json)
}
async fn retry_capture<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(item_id): AxumPath<String>,
    Json(b): Json<CaptureImportRequest>,
) -> Result<Json<CaptureImportResult>, ApiError> {
    if b.envelope.item_id != item_id {
        return Err(api_bad(
            "invalidInput",
            "Capture item does not match the retry route",
        ));
    }
    let uploads = b
        .uploads
        .into_iter()
        .map(WireUploadInput::parse)
        .collect::<Result<Vec<_>, _>>()?;
    let request = c.clone();
    blocking_engine(s.engine, &c.request_id, move |engine| {
        engine.import_capture(&request, b.envelope, uploads)
    })
    .await
    .map(Json)
}
async fn get_capture<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<CaptureImportResult>, ApiError> {
    let request = c.clone();
    blocking_engine(s.engine, &c.request_id, move |engine| {
        engine.get_capture(&request, &id)
    })
    .await
    .map(Json)
}

async fn roots<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
) -> Result<Json<Vec<RootHandle>>, ApiError> {
    s.engine
        .roots(&c)
        .map(Json)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn index_root_settings<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
) -> Result<Json<Vec<WireIndexRootSettings>>, ApiError> {
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.list_index_root_settings(&request)
    })
    .await
    .map(|settings| Json(settings.into_iter().map(Into::into).collect()))
}
async fn update_index_root_settings<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    AxumPath(handle_id): AxumPath<String>,
    Json(body): Json<WireIndexRootSettingsPatch>,
) -> Result<Json<WireIndexRootSettings>, ApiError> {
    if handle_id.is_empty() || handle_id.len() > 128 {
        return Err(api_bad("invalidInput", "handleId is outside its limits"));
    }
    let patch = body.parse()?;
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.update_index_root_settings(&request, &handle_id, patch)
    })
    .await
    .map(|settings| Json(settings.into()))
}
async fn catalog_settings<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
) -> Result<Json<CatalogSettings>, ApiError> {
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.get_catalog_settings(&request)
    })
    .await
    .map(Json)
}
async fn update_catalog_settings<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    Json(settings): Json<CatalogSettings>,
) -> Result<Json<CatalogSettings>, ApiError> {
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.update_catalog_settings(&request, settings)
    })
    .await
    .map(Json)
}
async fn plan_management_action<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    Json(action): Json<WireManagementAction>,
) -> Result<Json<WireActionPlan>, ApiError> {
    let action = action.parse()?;
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.plan_management_action(&request, action)
    })
    .await
    .map(|plan| Json(plan.into()))
}
async fn execute_management_action<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    Json(body): Json<ExecuteActionRequest>,
) -> Result<Json<Value>, ApiError> {
    if body.token.is_empty() || body.token.len() > 256 {
        return Err(api_bad(
            "invalidInput",
            "Action token is outside its limits",
        ));
    }
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.execute_management_action(&request, &body.token)
    })
    .await
    .map(|result| Json(wire_management_result(result)))
}
async fn integration_settings<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
) -> Result<Json<IntegrationSettings>, ApiError> {
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.integration_settings(&request)
    })
    .await
    .map(Json)
}
async fn save_spoolman_config<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    Json(body): Json<SpoolmanConfigRequest>,
) -> Result<Json<IntegrationSettings>, ApiError> {
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.save_spoolman_config(&request, body.config)
    })
    .await
    .map(Json)
}
async fn pull_spoolman<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
) -> Result<Json<IntegrationSpoolmanPullResult>, ApiError> {
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.pull_spoolman(&request)
    })
    .await
    .map(Json)
}
async fn save_ai_provider<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    AxumPath(provider_id): AxumPath<String>,
    Json(body): Json<AiProviderRequest>,
) -> Result<Json<AiProviderConfig>, ApiError> {
    if provider_id != body.config.id {
        return Err(api_bad(
            "invalidInput",
            "Provider route and configuration IDs differ",
        ));
    }
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.save_ai_provider(&request, body.config, body.credential_update)
    })
    .await
    .map(Json)
}
async fn delete_ai_provider<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    AxumPath(provider_id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.delete_ai_provider(&request, &provider_id)
    })
    .await
    .map(|_| StatusCode::NO_CONTENT)
}
async fn suggest_ai_tags<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    Json(body): Json<AiSuggestRequest>,
) -> Result<Json<WireAiSuggestionBatch>, ApiError> {
    let targets = body
        .targets
        .into_iter()
        .map(WireTarget::parse)
        .collect::<Result<Vec<_>, _>>()?;
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.suggest_ai_tags(&request, &body.provider_id, targets)
    })
    .await
    .map(|batch| Json(batch.into()))
}
async fn apply_ai_review<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    AxumPath(review_token): AxumPath<String>,
    Json(body): Json<AiApplyRequest>,
) -> Result<Json<Vec<WireMutation>>, ApiError> {
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.apply_ai_review(&request, &review_token, body.decisions, body.strategy)
    })
    .await
    .map(|versions| Json(versions.into_iter().map(Into::into).collect()))
}
async fn discard_ai_review<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    AxumPath(review_token): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.discard_ai_review(&request, &review_token)
    })
    .await
    .map(|_| StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScanRequest {
    root_id: Option<String>,
}
async fn start_scan<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    Json(b): Json<ScanRequest>,
) -> Result<Json<ScanStatus>, ApiError> {
    if b.root_id.as_ref().is_some_and(|value| value.len() > 128) {
        return Err(api_bad("invalidInput", "rootId is outside its limits"));
    }
    let request = c.clone();
    blocking_engine(s.engine, &c.request_id, move |engine| {
        engine.start_scan(&request, b.root_id)
    })
    .await
    .map(Json)
}
async fn cancel_scan<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
) -> Result<Json<ScanStatus>, ApiError> {
    s.engine
        .cancel_scan(&c)
        .map(Json)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn scan_status<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
) -> Result<Json<ScanStatus>, ApiError> {
    s.engine
        .scan_status(&c)
        .map(Json)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
async fn prepare_bundle<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<WireBundleSummary>, ApiError> {
    if id.is_empty() || id.len() > 128 {
        return Err(api_bad("invalidInput", "bundleId is outside its limits"));
    }
    let request = c.clone();
    blocking_engine(s.engine, &c.request_id, move |engine| {
        engine.prepare_bundle(&request, &id)
    })
    .await
    .map(|value| Json(value.into()))
}
async fn close_bundle<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(token): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    if token.is_empty() || token.len() > 256 {
        return Err(api_bad(
            "invalidInput",
            "preview token is outside its limits",
        ));
    }
    s.engine
        .close_bundle(&c, &token)
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| ApiError::from_engine(e, &c.request_id))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CaptureQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}
async fn list_captures<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    Query(q): Query<CaptureQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = q.limit.unwrap_or(50);
    if !(1..=MAX_PAGE_SIZE).contains(&limit)
        || q.cursor.as_ref().is_some_and(|value| value.len() > 512)
    {
        return Err(api_bad(
            "invalidInput",
            "Capture query is outside its limits",
        ));
    }
    let request = c.clone();
    let page = blocking_engine(s.engine, &c.request_id, move |engine| {
        engine.list_captures(
            &request,
            PageRequest {
                cursor: q.cursor,
                limit,
            },
        )
    })
    .await?;
    Ok(Json(
        serde_json::json!({"items":page.items,"nextCursor":page.next_cursor,"snapshotRevision":page.snapshot_revision.to_string()}),
    ))
}
async fn bundle_geometry<E: LibraryEngine>(
    State(s): State<ServerState<E>>,
    axum::Extension(c): axum::Extension<RequestContext>,
    AxumPath(token): AxumPath<String>,
) -> Response {
    if token.is_empty() || token.len() > 256 {
        return api_bad("invalidInput", "preview token is outside its limits").into_response();
    }
    let permit = match s.content_admission.clone().try_acquire_owned() {
        Ok(v) => v,
        Err(_) => return StatusCode::TOO_MANY_REQUESTS.into_response(),
    };
    let describe_engine = Arc::clone(&s.engine);
    let describe_context = c.clone();
    let describe_token = token.clone();
    let descriptor = match blocking_engine(describe_engine, &c.request_id, move |engine| {
        engine.describe_bundle_geometry(&describe_context, &describe_token)
    })
    .await
    {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    if descriptor.kind != ContentKind::Geometry
        || descriptor.mime != "application/vnd.gogglelab.geometry"
    {
        return ApiError::from_engine(
            EngineError {
                code: "corrupt",
                message: "Bundle geometry descriptor is invalid".into(),
                retryable: false,
                current_version: None,
            },
            &c.request_id,
        )
        .into_response();
    }
    if descriptor.bytes > 128 * 1024 * 1024 {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let engine = Arc::clone(&s.engine);
    let timeout = s.content_send_timeout;
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    tokio::task::spawn_blocking(move || {
        struct Sink {
            tx: tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
            blocked: Option<std::time::Instant>,
            timeout: std::time::Duration,
        }
        impl std::io::Write for Sink {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                for chunk in b.chunks(64 * 1024) {
                    let mut value = Ok(Bytes::copy_from_slice(chunk));
                    loop {
                        match self.tx.try_send(value) {
                            Ok(()) => {
                                self.blocked = None;
                                break;
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                return Err(std::io::Error::other("closed"))
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Full(v)) => {
                                let since =
                                    *self.blocked.get_or_insert_with(std::time::Instant::now);
                                if since.elapsed() >= self.timeout {
                                    return Err(std::io::Error::new(
                                        std::io::ErrorKind::TimedOut,
                                        "stalled",
                                    ));
                                }
                                value = v;
                                std::thread::sleep(std::time::Duration::from_millis(10));
                            }
                        }
                    }
                }
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut sink = Sink {
            tx: tx.clone(),
            blocked: None,
            timeout,
        };
        if let Err(e) = engine.stream_bundle_geometry(&c, &token, &mut sink) {
            let _ = tx.try_send(Err(std::io::Error::other(e.message)));
        }
    });
    use tokio_stream::StreamExt as _;
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx)
        .timeout(std::time::Duration::from_secs(30))
        .map(move |v| {
            let _hold = &permit;
            v.unwrap_or_else(|_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "geometry timeout",
                ))
            })
        });
    let mut response = (StatusCode::OK, axum::body::Body::from_stream(stream)).into_response();
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/vnd.gogglelab.geometry"),
    );
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
    response
}

async fn bundle_download<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    AxumPath(token): AxumPath<String>,
) -> Result<Json<WireLease>, ApiError> {
    if token.is_empty() || token.len() > 256 {
        return Err(api_bad(
            "invalidInput",
            "preview token is outside its limits",
        ));
    }
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.issue_bundle_download(&request, &token)
    })
    .await
    .map(|lease| Json(lease.into()))
}

async fn model_geometry<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    AxumPath(model_id): AxumPath<String>,
    Json(body): Json<GeometryRequest>,
) -> Result<Json<WireGeometryLease>, ApiError> {
    if body.expected_hash.as_ref().is_some_and(|hash| {
        hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }) {
        return Err(api_bad("invalidInput", "expectedHash is invalid"));
    }
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.issue_model_geometry(
            &request,
            &model_id,
            body.expected_hash.as_deref(),
            body.plate_id,
        )
    })
    .await
    .map(|lease| Json(lease.into()))
}

async fn duplicates<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    Json(body): Json<DuplicateRequest>,
) -> Result<Json<Vec<WireDuplicateGroup>>, ApiError> {
    if body.model_ids.is_empty()
        || body.model_ids.len() > 50_000
        || body
            .model_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 128)
    {
        return Err(api_bad("invalidInput", "modelIds are outside their limits"));
    }
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.duplicate_groups(&request, body.model_ids)
    })
    .await
    .map(|groups| Json(groups.into_iter().map(Into::into).collect()))
}

async fn embedded_metadata<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    AxumPath(model_id): AxumPath<String>,
    Json(body): Json<DownloadBody>,
) -> Result<Json<EmbeddedMetadataProposal>, ApiError> {
    if body.expected_hash.as_ref().is_some_and(|hash| {
        hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }) {
        return Err(api_bad("invalidInput", "expectedHash is invalid"));
    }
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.read_embedded_metadata(&request, &model_id, body.expected_hash.as_deref())
    })
    .await
    .map(Json)
}

async fn apply_embedded_metadata<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    AxumPath(model_id): AxumPath<String>,
    Json(body): Json<EmbeddedApplyRequest>,
) -> Result<Json<WireMutation>, ApiError> {
    let expected_version = parse_version(&body.expected_version)?;
    if body.expected_source_hash.len() != 64
        || !body
            .expected_source_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(api_bad("invalidInput", "expectedSourceHash is invalid"));
    }
    let request = context.clone();
    blocking_engine(state.engine, &context.request_id, move |engine| {
        engine.apply_embedded_metadata(
            &request,
            VersionedTarget {
                model_id,
                expected_version,
            },
            &body.expected_source_hash,
            body.selection,
        )
    })
    .await
    .map(|version| Json(version.into()))
}

async fn events<E: LibraryEngine>(
    State(state): State<ServerState<E>>,
    axum::Extension(context): axum::Extension<RequestContext>,
    headers: axum::http::HeaderMap,
    shutdown: Option<axum::Extension<ServerShutdown>>,
) -> Response {
    let permit = match state.event_admission.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(axum::http::header::RETRY_AFTER, "1")],
                Json(serde_json::json!({"error":{
                    "code":"busy",
                    "message":"Event stream limit reached",
                    "requestId":context.request_id,
                    "retryable":true
                }})),
            )
                .into_response()
        }
    };
    let request = context.clone();
    let status = match blocking_engine(
        Arc::clone(&state.engine),
        &context.request_id,
        move |engine| engine.status(&request),
    )
    .await
    {
        Ok(status) => status,
        Err(error) => return error.into_response(),
    };
    let current_id = format!("{}:{}", status.library_epoch, status.catalog_revision);
    let last_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok());
    let stream_state = EventStreamState {
        engine: state.engine,
        context,
        _permit: permit,
        library_epoch: status.library_epoch,
        catalog_revision: status.catalog_revision,
        initial_reset: last_id != Some(current_id.as_str()),
        first: true,
        last_heartbeat: std::time::Instant::now(),
        shutdown: shutdown.map(|shutdown| shutdown.0 .0),
    };
    let stream = futures_util::stream::unfold(stream_state, |mut stream| async move {
        if stream.first {
            stream.first = false;
            if stream.initial_reset {
                let id = format!("{}:{}", stream.library_epoch, stream.catalog_revision);
                let payload = serde_json::json!({
                    "sequence":stream.catalog_revision,
                    "libraryEpoch":stream.library_epoch,
                    "catalogRevision":stream.catalog_revision,
                    "requestId":stream.context.request_id,
                    "reset":true
                });
                let event = axum::response::sse::Event::default()
                    .event("library.reset")
                    .id(id)
                    .json_data(payload)
                    .expect("event payload is serializable");
                return Some((Ok::<_, std::convert::Infallible>(event), stream));
            }
        }
        loop {
            if let Some(shutdown) = stream.shutdown.as_mut() {
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            return None;
                        }
                        continue;
                    }
                }
            } else {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            let engine = Arc::clone(&stream.engine);
            let request = stream.context.clone();
            let status = tokio::task::spawn_blocking(move || engine.status(&request)).await;
            let Ok(Ok(status)) = status else {
                if stream.last_heartbeat.elapsed() >= std::time::Duration::from_secs(15) {
                    stream.last_heartbeat = std::time::Instant::now();
                    return Some((
                        Ok(axum::response::sse::Event::default().comment("keepalive")),
                        stream,
                    ));
                }
                continue;
            };
            let epoch_changed = status.library_epoch != stream.library_epoch;
            let revision_changed = status.catalog_revision != stream.catalog_revision;
            if epoch_changed || revision_changed {
                stream.library_epoch = status.library_epoch;
                stream.catalog_revision = status.catalog_revision;
                stream.last_heartbeat = std::time::Instant::now();
                let id = format!("{}:{}", stream.library_epoch, stream.catalog_revision);
                let reset = epoch_changed;
                let payload = serde_json::json!({
                    "sequence":stream.catalog_revision,
                    "libraryEpoch":stream.library_epoch,
                    "catalogRevision":stream.catalog_revision,
                    "requestId":stream.context.request_id,
                    "reset":reset
                });
                let event = axum::response::sse::Event::default()
                    .event(if reset {
                        "library.reset"
                    } else {
                        "library.changed"
                    })
                    .id(id)
                    .json_data(payload)
                    .expect("event payload is serializable");
                return Some((Ok(event), stream));
            }
            if stream.last_heartbeat.elapsed() >= std::time::Duration::from_secs(15) {
                stream.last_heartbeat = std::time::Instant::now();
                return Some((
                    Ok(axum::response::sse::Event::default().comment("keepalive")),
                    stream,
                ));
            }
        }
    });
    Sse::new(stream).into_response()
}

pub fn tool_names() -> &'static [&'static str] {
    &[
        "search_models",
        "integration_get_settings",
        "integration_save_spoolman_config",
        "integration_save_ai_provider",
        "integration_delete_ai_provider",
        "discard_tag_review",
        "plan_management_action",
        "execute_management_action",
        "get_index_root_settings",
        "update_index_root_settings",
        "get_catalog_settings",
        "update_catalog_settings",
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
        "rename_metadata",
        "delete_metadata",
        "get_models_by_directory",
        "get_models_missing_thumbnails",
        "get_models_with_default_thumbnails",
        "start_thumbnail_job",
        "get_thumbnails",
        "set_thumbnail",
        "add_thumbnail",
        "set_default_thumbnail",
        "delete_thumbnail",
        "find_duplicates",
        "get_model_geometry",
        "get_hash_status",
        "calculate_missing_hashes",
        "refresh_hashes",
        "check_files_exist",
        "get_embedded_3mf_metadata",
        "pull_3mf_metadata",
        "apply_embedded_3mf_metadata",
        "generate_tags",
        "apply_tag_review",
        "scan_directory",
        "list_roots",
        "cancel_scan",
        "get_scan_status",
        "remove_model",
        "trash_file",
        "undo_trash",
        "move_files",
        "export_library",
        "backup_library",
        "backup_database",
        "restore_library",
        "list_slicers",
        "open_in_slicer",
        "prepare_bundle_preview",
        "get_bundle_preview_geometry",
        "close_bundle_preview",
        "list_capture_items",
        "retry_capture",
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
                        "limit":{"type":"integer","minimum":1,"maximum":100},
                        "thumbnailMode":{"type":"string","enum":["missing","default","custom"]},
                        "rootId":{"type":"string","maxLength":128},
                        "sourceDirectory":{"type":"string","maxLength":1024},
                        "minimumRating":{"type":"integer","minimum":0,"maximum":5},
                        "favorite":{"type":"boolean"}
                    }
                }),
                "integration_get_settings"|"sync_spoolman_filaments" => serde_json::json!({"type":"object","additionalProperties":false}),
                "integration_save_spoolman_config" => serde_json::json!({"type":"object","additionalProperties":false,"required":["config"],"properties":{"config":{"type":["object","null"]}}}),
                "integration_save_ai_provider" => serde_json::json!({"type":"object","additionalProperties":false,"required":["config","credentialUpdate"],"properties":{"config":{"type":"object"},"credentialUpdate":{"type":"object","additionalProperties":false,"required":["action"],"properties":{"action":{"type":"string","enum":["keep","clear","setSession"]},"secret":{"type":["string","null"],"maxLength":16384}}}}}),
                "rename_metadata" => serde_json::json!({"type":"object","additionalProperties":false,"required":["type","oldName","newName"],"properties":{"type":{"type":"string","enum":["designer","parentModel","license"]},"oldName":{"type":"string","minLength":1,"maxLength":200},"newName":{"type":"string","minLength":1,"maxLength":200}}}),
                "delete_metadata" => serde_json::json!({"type":"object","additionalProperties":false,"required":["type","name"],"properties":{"type":{"type":"string","enum":["designer","parentModel","license"]},"name":{"type":"string","minLength":1,"maxLength":200}}}),
                "add_model_tags"|"remove_model_tags" => serde_json::json!({"type":"object","additionalProperties":false,"required":["modelId","expectedVersion","tags"],"properties":{"modelId":{"type":"string","minLength":1,"maxLength":128},"expectedVersion":{"type":"string","pattern":"^(0|[1-9][0-9]*)$"},"tags":{"type":"array","minItems":1,"maxItems":200,"items":{"type":"string","minLength":1,"maxLength":256}}}}),
                "remove_model" => serde_json::json!({"type":"object","additionalProperties":false,"required":["modelId","expectedVersion","confirm"],"properties":{"modelId":{"type":"string","minLength":1,"maxLength":128},"expectedVersion":{"type":"string","pattern":"^(0|[1-9][0-9]*)$"},"confirm":{"const":true}}}),
                "backup_database" => serde_json::json!({"type":"object","additionalProperties":false,"required":["destinationHandleId","fileName"],"properties":{"destinationHandleId":{"type":"string","minLength":1,"maxLength":128},"fileName":{"type":"string","minLength":1,"maxLength":200}}}),
                "export_library" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"destPath":{"type":"string","maxLength":1024}}}),
                "integration_delete_ai_provider" => serde_json::json!({"type":"object","additionalProperties":false,"required":["providerId"],"properties":{"providerId":{"type":"string","minLength":1,"maxLength":128}}}),
                "generate_tags" => serde_json::json!({"type":"object","additionalProperties":false,"required":["providerId","targets"],"properties":{"providerId":{"type":"string","minLength":1,"maxLength":128},"targets":{"type":"array","minItems":1,"maxItems":50,"items":{"type":"object","required":["modelId","expectedVersion"]}}}}),
                "apply_tag_review" => serde_json::json!({"type":"object","additionalProperties":false,"required":["reviewToken","decisions","strategy"],"properties":{"reviewToken":{"type":"string","minLength":1,"maxLength":128},"decisions":{"type":"array","minItems":1,"maxItems":50},"strategy":{"type":"string","enum":["append","replace","merge"]}}}),
                "discard_tag_review" => serde_json::json!({"type":"object","additionalProperties":false,"required":["reviewToken"],"properties":{"reviewToken":{"type":"string","minLength":1,"maxLength":128}}}),
                "get_hash_status"|"calculate_missing_hashes"|"refresh_hashes" => serde_json::json!({"type":"object","additionalProperties":false}),
                "check_files_exist" => serde_json::json!({"type":"object","additionalProperties":false,"required":["modelIds"],"properties":{"modelIds":{"type":"array","minItems":1,"maxItems":10000,"items":{"type":"string","minLength":1,"maxLength":128}}}}),
                "start_thumbnail_job" => serde_json::json!({"type":"object","additionalProperties":false,"required":["modelIds"],"properties":{"modelIds":{"type":"array","minItems":1,"maxItems":10000,"items":{"type":"string","minLength":1,"maxLength":128}},"force":{"type":"boolean"}}}),
                "get_models_missing_thumbnails"|"get_models_with_default_thumbnails" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"cursor":{"type":"string","maxLength":512},"limit":{"type":"integer","minimum":1,"maximum":100}}}),
                "get_models_by_directory" => serde_json::json!({"type":"object","additionalProperties":false,"required":["rootId","directory"],"properties":{"rootId":{"type":"string","minLength":1,"maxLength":128},"directory":{"type":"string","maxLength":1024},"cursor":{"type":"string","maxLength":512},"limit":{"type":"integer","minimum":1,"maximum":100}}}),
                "plan_management_action" => serde_json::json!({
                    "type":"object","additionalProperties":false,"required":["action"],
                    "properties":{"action":{"oneOf":[
                        {"type":"object","additionalProperties":false,"required":["kind","targets"],"properties":{"kind":{"const":"recoverableRemove"},"targets":{"type":"array","maxItems":50000,"items":{"type":"object","required":["modelId","expectedVersion"]}}}},
                        {"type":"object","additionalProperties":false,"required":["kind","snapshotId"],"properties":{"kind":{"const":"restoreRemoval"},"snapshotId":{"type":"string","minLength":1,"maxLength":128}}},
                        {"type":"object","additionalProperties":false,"required":["kind","destinationHandleId","fileName"],"properties":{"kind":{"const":"createBackup"},"destinationHandleId":{"type":"string"},"fileName":{"type":"string"}}},
                        {"type":"object","additionalProperties":false,"required":["kind","sourceHandleId","fileName","rootMappings"],"properties":{"kind":{"const":"restoreBackup"},"sourceHandleId":{"type":"string"},"fileName":{"type":"string"},"rootMappings":{"type":"array","maxItems":256}}},
                        {"type":"object","additionalProperties":false,"required":["kind","targets","destinationHandleId","fileName"],"properties":{"kind":{"const":"exportSelection"},"targets":{"type":"array","maxItems":50000},"destinationHandleId":{"type":"string"},"fileName":{"type":"string"}}},
                    ]}}
                }),
                "execute_management_action" => serde_json::json!({"type":"object","additionalProperties":false,"required":["token"],"properties":{"token":{"type":"string","minLength":1,"maxLength":256}}}),
                "get_index_root_settings"|"get_catalog_settings" => serde_json::json!({"type":"object","additionalProperties":false}),
                "update_index_root_settings" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"handleId":{"type":"string","minLength":1,"maxLength":128},"settings":{"type":"object","additionalProperties":false,"required":["enabled","scanIntervalMinutes","maxFileBytes","scanZip"],"properties":{"enabled":{"type":"boolean"},"scanIntervalMinutes":{"type":"integer","minimum":1,"maximum":1440},"maxFileBytes":{"type":"string","pattern":"^(0|[1-9][0-9]*)$"},"scanZip":{"type":"boolean"}}}},"required":["handleId","settings"]}),
                "update_catalog_settings" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"settings":{"type":"object","additionalProperties":false,"required":["thumbnailSize","thumbnailBackground","thumbnailConcurrency","scanBatchSize"],"properties":{"thumbnailSize":{"type":"integer","enum":[128,256,512]},"thumbnailBackground":{"type":"string","pattern":"^#[0-9A-Fa-f]{6}$"},"thumbnailConcurrency":{"type":"integer","minimum":1,"maximum":2},"scanBatchSize":{"type":"integer","minimum":16,"maximum":256}}}},"required":["settings"]}),
                "get_model"|"get_all_metadata" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"modelId":{"type":"string","minLength":1,"maxLength":128}},"required":["modelId"]}),
                "get_library_stats"|"list_parent_models"|"list_designers"|"list_licenses"|"list_roots"|"cancel_scan"|"get_scan_status" => serde_json::json!({"type":"object","additionalProperties":false}),
                "scan_directory" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"rootId":{"type":"string","maxLength":128}}}),
                "prepare_bundle_preview" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"bundleId":{"type":"string","minLength":1,"maxLength":128}},"required":["bundleId"]}),
                "get_bundle_preview_geometry"|"download_bundle"|"close_bundle_preview" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"token":{"type":"string","minLength":1,"maxLength":256}},"required":["token"]}),
                "get_model_geometry" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"modelId":{"type":"string","minLength":1,"maxLength":128},"expectedHash":{"type":"string","pattern":"^[0-9a-f]{64}$"},"plateId":{"type":"integer","minimum":0,"maximum":4294967295_u64}},"required":["modelId"]}),
                "find_duplicates" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"modelIds":{"type":"array","minItems":1,"maxItems":50000,"items":{"type":"string","minLength":1,"maxLength":128}}},"required":["modelIds"]}),
                "get_embedded_3mf_metadata"|"pull_3mf_metadata" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"modelId":{"type":"string","minLength":1,"maxLength":128},"expectedHash":{"type":"string","pattern":"^[0-9a-f]{64}$"}},"required":["modelId"]}),
                "apply_embedded_3mf_metadata" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"modelId":{"type":"string","minLength":1,"maxLength":128},"expectedVersion":{"type":"string","pattern":"^(0|[1-9][0-9]*)$"},"expectedSourceHash":{"type":"string","pattern":"^[0-9a-f]{64}$"},"selection":{"type":"object","additionalProperties":false,"properties":{"designer":{"type":"boolean"},"sourceUrl":{"type":"boolean"},"notes":{"type":"boolean"},"modelLicense":{"type":"boolean"}}}},"required":["modelId","expectedVersion","expectedSourceHash","selection"]}),
                "list_capture_items" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"cursor":{"type":"string","maxLength":512},"limit":{"type":"integer","minimum":1,"maximum":100}}}),
                "retry_capture" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"itemId":{"type":"string","minLength":1,"maxLength":128},"envelope":{"type":"object"},"uploads":{"type":"array","maxItems":10000,"items":{"type":"object","required":["id","itemId","captureId","expiresAt","libraryEpoch"]}}},"required":["itemId","envelope","uploads"]}),
                "get_folder_tree"|"list_tags" => serde_json::json!({"type":"object","additionalProperties":false}),
                "list_filaments" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"includeUnavailable":{"type":"boolean"}}}),
                "update_model" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"target":{"type":"object","required":["modelId","expectedVersion"],"properties":{"modelId":{"type":"string"},"expectedVersion":{"type":"string","pattern":"^(0|[1-9][0-9]*)$"}}},"patch":{"type":"object","properties":{"designer":{"type":["string","null"]},"sourceUrl":{"type":["string","null"]},"notes":{"type":["string","null"]},"modelLicense":{"type":["string","null"]},"printStatus":{"type":"string"},"parentModelId":{"type":["string","null"]},"rating":{"type":"integer","minimum":0,"maximum":5},"favorite":{"type":"boolean"}}}},"required":["target","patch"]}),
                "update_models_batch" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"targets":{"type":"array","maxItems":50000,"items":{"type":"object","required":["modelId","expectedVersion"]}},"patch":{"type":"object","properties":{"designer":{"type":["string","null"]},"sourceUrl":{"type":["string","null"]},"notes":{"type":["string","null"]},"modelLicense":{"type":["string","null"]},"printStatus":{"type":"string"},"parentModelId":{"type":["string","null"]},"rating":{"type":"integer","minimum":0,"maximum":5},"favorite":{"type":"boolean"}}}},"required":["targets","patch"]}),
                "add_tag" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"name":{"type":"string","maxLength":256},"category":{"type":["string","null"],"maxLength":120}},"required":["name"]}),
                "rename_tag" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"tagId":{"type":"string"},"name":{"type":"string","maxLength":256},"category":{"type":["string","null"],"maxLength":120}},"required":["tagId","name"]}),
                "delete_tag" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"tagId":{"type":"string"}},"required":["tagId"]}),
                "save_filament" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"filamentId":{"type":"string"},"expectedVersion":{"type":"string","pattern":"^(0|[1-9][0-9]*)$"},"input":{"type":"object"}},"required":["input"]}),
                "delete_filament" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"filamentId":{"type":"string"},"expectedVersion":{"type":"string","pattern":"^(0|[1-9][0-9]*)$"}},"required":["filamentId","expectedVersion"]}),
                "set_model_filaments" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"assignments":{"type":"array","maxItems":50000}},"required":["assignments"]}),
                "get_print_events" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"modelId":{"type":"string"}},"required":["modelId"]}),
                "log_print_event" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"modelId":{"type":"string"},"input":{"type":"object"}},"required":["modelId","input"]}),
                "log_print_events_batch" => serde_json::json!({"type":"object","additionalProperties":false,"properties":{"idempotencyKey":{"type":"string","minLength":1,"maxLength":128},"items":{"type":"array","minItems":1,"maxItems":10000,"items":{"type":"object","additionalProperties":false,"required":["modelId","input"],"properties":{"modelId":{"type":"string","minLength":1,"maxLength":128},"input":{"type":"object"}}}}},"required":["idempotencyKey","items"]}),
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
                "trash_file"|"undo_trash"|"move_files"|"restore_library" => serde_json::json!({
                    "type":"object",
                    "properties":{"confirmationToken":{"type":"string","minLength":43,"maxLength":256}},
                    "required":["confirmationToken"]
                }),
                _ => serde_json::json!({"type":"object"}),
            };
            serde_json::json!({
                "name": name,
                "description": if *name == "export_library" {
                    "Export library metadata as Printventory JSON without thumbnails. Omit destPath to write a unique JSON file in the only admitted backupOnly root. Otherwise use a backup handle and filename, or an absolute path directly inside an admitted backupOnly root. Requires Maintenance and BackupWrite.".to_owned()
                } else {
                    format!("GoggleLab {} operation", name.replace('_', " "))
                },
                "inputSchema": input_schema
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
