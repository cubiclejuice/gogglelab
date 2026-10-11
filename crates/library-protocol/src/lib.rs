use std::collections::BTreeSet;
use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

pub const CAPTURE_ENVELOPE_VERSION: u32 = 1;
pub const DEFAULT_PAGE_LIMIT: u16 = 100;
pub const MAX_PAGE_LIMIT: u16 = 500;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Capability {
    CatalogRead,
    CatalogWrite,
    MediaRead,
    MediaWrite,
    Scan,
    Integration,
    Maintenance,
    PhysicalSourceWrite,
    HostPaths,
    HostSlicer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RequestContext {
    pub principal_id: String,
    pub capabilities: BTreeSet<Capability>,
    pub request_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeErrorCode {
    InvalidInput,
    NotFound,
    Conflict,
    Stale,
    Busy,
    Maintenance,
    Forbidden,
    Io,
    Corrupt,
    Unsupported,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeError {
    pub code: RuntimeErrorCode,
    pub message: String,
    pub retryable: bool,
    pub current_version: Option<u64>,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RuntimeError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PageRequest {
    pub cursor: Option<String>,
    pub limit: u16,
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            cursor: None,
            limit: DEFAULT_PAGE_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub snapshot_revision: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RootAccess {
    ReadOnly,
    Writable,
    Inbox,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RootHandle {
    pub id: String,
    pub catalog_root_id: Option<String>,
    pub label: String,
    pub access: RootAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeOwnership {
    pub lock_identity: String,
    pub exclusive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    pub library_epoch: u64,
    pub catalog_revision: u64,
    pub ownership: RuntimeOwnership,
    pub capabilities: BTreeSet<Capability>,
    pub roots: Vec<RootHandle>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContentLease {
    pub id: String,
    pub file_name: String,
    pub mime: String,
    pub bytes: u64,
    pub sha256: String,
    pub library_epoch: u64,
    pub expires_at: String,
}

pub type SourceLease = ContentLease;
pub type MediaLease = ContentLease;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CapturePage {
    pub url: String,
    pub title: String,
    pub designer: Option<String>,
    pub license: Option<String>,
    pub notes: Option<String>,
    pub parent_external_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureDownload {
    pub capture_id: String,
    pub file_name: String,
    pub relative_inbox_path: Option<String>,
    pub sha256: Option<String>,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureEnvelope {
    pub version: u32,
    pub item_id: String,
    pub created_at: String,
    pub page: CapturePage,
    pub downloads: Vec<CaptureDownload>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadIntent {
    pub item_id: String,
    pub capture_id: String,
    pub file_name: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadToken {
    pub id: String,
    pub item_id: String,
    pub capture_id: String,
    pub expires_at: String,
    pub library_epoch: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CaptureItemStatus {
    Imported,
    Partial,
    Failed,
    AlreadyImported,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CaptureDownloadStatus {
    Imported,
    Failed,
    AlreadyImported,
    AlreadyImportedRemoved,
    NeedsFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureDownloadResult {
    pub capture_id: String,
    pub status: CaptureDownloadStatus,
    pub model_id: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureImportResult {
    pub item_id: String,
    pub status: CaptureItemStatus,
    pub page_model_id: Option<String>,
    pub downloads: Vec<CaptureDownloadResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSummary {
    pub item_id: String,
    pub created_at: String,
    pub status: CaptureItemStatus,
    pub page_model_id: Option<String>,
    pub title: String,
    pub url: String,
    pub download_count: u64,
    pub failed_count: u64,
    pub needs_file_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BundlePreviewMember {
    pub model_id: String,
    pub display_name: String,
    pub triangle_count: u32,
    pub offset: [f32; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BundlePreviewSummary {
    pub token: String,
    pub bundle_id: String,
    pub member_count: usize,
    pub previewed_count: usize,
    pub omitted_count: usize,
    pub triangle_count: u32,
    pub layout: String,
    pub members: Vec<BundlePreviewMember>,
    pub notice: Option<String>,
    pub geometry_bytes: u64,
    pub geometry_sha256: String,
    pub library_epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailDescriptor {
    pub thumbnail_id: String,
    pub model_id: String,
    pub ordinal: u16,
    pub active: bool,
    pub custom: bool,
    pub mime: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ModelAvailability {
    Available,
    Missing,
    Bookmark,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: String,
    pub display_name: String,
    pub folder_id: String,
    pub format: Option<String>,
    pub bytes: u64,
    pub content_hash: Option<String>,
    pub metadata_version: u64,
    pub tags: Vec<String>,
    pub filament_ids: Vec<String>,
    pub source_kind: String,
    pub availability: ModelAvailability,
    pub catalog_kind: String,
    pub thumbnail_id: Option<String>,
    pub designer: Option<String>,
    pub source_url: Option<String>,
    pub notes: Option<String>,
    pub model_license: Option<String>,
    pub print_status: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ModelSort {
    Name,
    Size,
    Date,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ModelGroupFilter {
    Standalone,
    Bundle,
    Archive,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PrintStatus {
    Unprinted,
    Want,
    Queued,
    Printing,
    Printed,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TagMode {
    Replace,
    Merge,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VersionedTarget {
    pub model_id: String,
    pub expected_version: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MetadataPatch {
    pub designer: Option<String>,
    pub source_url: Option<String>,
    pub notes: Option<String>,
    pub model_license: Option<String>,
    pub print_status: Option<PrintStatus>,
    pub tag_ids: Option<Vec<String>>,
    pub tag_mode: Option<TagMode>,
    pub folder_id: Option<String>,
    pub parent_model_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MutationVersion {
    pub model_id: String,
    pub metadata_version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub model_count: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FilamentInput {
    pub brand: String,
    pub material: String,
    pub color: String,
    pub name: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Filament {
    pub id: String,
    pub version: u64,
    pub brand: String,
    pub material: String,
    pub color: String,
    pub name: String,
    pub notes: String,
    pub origin: String,
    pub resource_kind: String,
    pub source_id: Option<String>,
    pub external_id: Option<String>,
    pub parent_filament_id: Option<String>,
    pub archived: bool,
    pub source_missing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelFilamentAssignment {
    pub target: VersionedTarget,
    pub filament_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PrintOutcome {
    Printed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrintAttemptInput {
    pub printed_at: String,
    pub quantity: u32,
    pub notes: String,
    pub filament_ids: Vec<String>,
    pub outcome: PrintOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrintAttempt {
    pub id: String,
    pub model_id: String,
    pub printed_at: String,
    pub quantity: u32,
    pub notes: String,
    pub filament_ids: Vec<String>,
    pub outcome: PrintOutcome,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelQuery {
    pub folder_id: Option<String>,
    pub search: Option<String>,
    pub sort: ModelSort,
    pub direction: SortDirection,
    pub tag_id: Option<String>,
    pub category: Option<String>,
    pub designer: Option<String>,
    pub model_license: Option<String>,
    pub print_status: Option<PrintStatus>,
    pub ever_printed: Option<bool>,
    pub bundle_id: Option<String>,
    pub root_id: Option<String>,
    pub filament_id: Option<String>,
    pub source_missing: Option<bool>,
    pub duplicates_only: bool,
    pub source_kind: Option<String>,
    pub group: Option<ModelGroupFilter>,
}

impl Default for ModelQuery {
    fn default() -> Self {
        Self {
            folder_id: None,
            search: None,
            sort: ModelSort::Date,
            direction: SortDirection::Asc,
            tag_id: None,
            category: None,
            designer: None,
            model_license: None,
            print_status: None,
            ever_printed: None,
            bundle_id: None,
            root_id: None,
            filament_id: None,
            source_missing: None,
            duplicates_only: false,
            source_kind: None,
            group: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub library_epoch: u64,
    pub catalog_revision: u64,
    pub maintenance: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEvent {
    pub revision: u64,
    pub library_epoch: u64,
    pub kind: String,
    pub entity_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    pub running: bool,
    pub cancelling: bool,
    pub job_kind: Option<String>,
    pub current_root_id: Option<String>,
    pub roots_total: usize,
    pub roots_completed: usize,
    pub files_visited: usize,
    pub files_indexed: usize,
    pub errors: Vec<String>,
}

pub trait RuntimeInfoEngine: Send + Sync {
    fn runtime_info(&self, context: &RequestContext) -> Result<RuntimeInfo, RuntimeError>;
}

pub trait CatalogEngine: Send + Sync {
    fn list_models(
        &self,
        context: &RequestContext,
        query: ModelQuery,
        page: PageRequest,
    ) -> Result<Page<CatalogModel>, RuntimeError>;
}

pub trait MetadataEngine: Send + Sync {
    fn update_metadata(
        &self,
        context: &RequestContext,
        target: VersionedTarget,
        patch: MetadataPatch,
    ) -> Result<MutationVersion, RuntimeError>;

    fn bulk_update_metadata(
        &self,
        context: &RequestContext,
        targets: Vec<VersionedTarget>,
        patch: MetadataPatch,
    ) -> Result<Vec<MutationVersion>, RuntimeError>;
}

pub trait TagEngine: Send + Sync {
    fn list_tags(&self, context: &RequestContext) -> Result<Vec<Tag>, RuntimeError>;
    fn create_tag(
        &self,
        context: &RequestContext,
        name: String,
        category: Option<String>,
    ) -> Result<Tag, RuntimeError>;
    fn update_tag(
        &self,
        context: &RequestContext,
        tag_id: &str,
        name: String,
        category: Option<String>,
    ) -> Result<Tag, RuntimeError>;
    fn delete_tag(&self, context: &RequestContext, tag_id: &str) -> Result<(), RuntimeError>;
}

pub trait FolderEngine: Send + Sync {
    fn list_folders(&self, context: &RequestContext) -> Result<Vec<Folder>, RuntimeError>;
    fn create_folder(
        &self,
        context: &RequestContext,
        name: String,
        parent_id: Option<String>,
    ) -> Result<Folder, RuntimeError>;
    fn rename_folder(
        &self,
        context: &RequestContext,
        folder_id: &str,
        name: String,
    ) -> Result<Folder, RuntimeError>;
    fn move_folder(
        &self,
        context: &RequestContext,
        folder_id: &str,
        parent_id: &str,
    ) -> Result<Folder, RuntimeError>;
    fn delete_folder(&self, context: &RequestContext, folder_id: &str) -> Result<(), RuntimeError>;
}

pub trait FilamentEngine: Send + Sync {
    fn list_filaments(
        &self,
        context: &RequestContext,
        include_unavailable: bool,
    ) -> Result<Vec<Filament>, RuntimeError>;
    fn create_filament(
        &self,
        context: &RequestContext,
        input: FilamentInput,
    ) -> Result<Filament, RuntimeError>;
    fn update_filament(
        &self,
        context: &RequestContext,
        filament_id: &str,
        expected_version: u64,
        input: FilamentInput,
    ) -> Result<Filament, RuntimeError>;
    fn delete_filament(
        &self,
        context: &RequestContext,
        filament_id: &str,
        expected_version: u64,
    ) -> Result<Option<Filament>, RuntimeError>;
    fn bulk_assign_model_filaments(
        &self,
        context: &RequestContext,
        assignments: Vec<ModelFilamentAssignment>,
    ) -> Result<Vec<MutationVersion>, RuntimeError>;
}

pub trait PrintHistoryEngine: Send + Sync {
    fn list_print_attempts(
        &self,
        context: &RequestContext,
        model_id: &str,
    ) -> Result<Vec<PrintAttempt>, RuntimeError>;
    fn create_print_attempt(
        &self,
        context: &RequestContext,
        model_id: &str,
        input: PrintAttemptInput,
    ) -> Result<PrintAttempt, RuntimeError>;
    fn update_print_attempt(
        &self,
        context: &RequestContext,
        attempt_id: &str,
        input: PrintAttemptInput,
    ) -> Result<PrintAttempt, RuntimeError>;
    fn delete_print_attempt(
        &self,
        context: &RequestContext,
        attempt_id: &str,
    ) -> Result<(), RuntimeError>;
}

pub trait RuntimeStatusEngine: Send + Sync {
    fn runtime_status(&self, context: &RequestContext) -> Result<RuntimeStatus, RuntimeError>;

    fn events_since(
        &self,
        context: &RequestContext,
        after_revision: u64,
        page: PageRequest,
    ) -> Result<Page<RuntimeEvent>, RuntimeError>;
}

pub trait GalleryEngine: Send + Sync {
    fn list_thumbnails(
        &self,
        context: &RequestContext,
        model_id: &str,
    ) -> Result<Vec<ThumbnailDescriptor>, RuntimeError>;

    fn set_active_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        thumbnail_id: &str,
    ) -> Result<u64, RuntimeError>;

    fn add_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        upload_token: &str,
    ) -> Result<u64, RuntimeError>;

    fn delete_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        thumbnail_id: &str,
    ) -> Result<u64, RuntimeError>;

    fn issue_thumbnail_media(
        &self,
        context: &RequestContext,
        model_id: &str,
        thumbnail_id: &str,
    ) -> Result<MediaLease, RuntimeError>;
}

pub trait ScanEngine: Send + Sync {
    fn start_scan(
        &self,
        context: &RequestContext,
        catalog_root_id: Option<String>,
    ) -> Result<ScanStatus, RuntimeError>;

    fn cancel_scan(&self, context: &RequestContext) -> Result<ScanStatus, RuntimeError>;

    fn scan_status(&self, context: &RequestContext) -> Result<ScanStatus, RuntimeError>;
}

pub trait BundleEngine: Send + Sync {
    fn prepare_bundle_preview(
        &self,
        context: &RequestContext,
        bundle_id: &str,
    ) -> Result<BundlePreviewSummary, RuntimeError>;
    fn describe_bundle_geometry(
        &self,
        context: &RequestContext,
        token: &str,
    ) -> Result<ContentLease, RuntimeError>;
    fn stream_bundle_geometry(
        &self,
        context: &RequestContext,
        token: &str,
        sink: &mut dyn Write,
    ) -> Result<(), RuntimeError>;
    fn close_bundle_preview(
        &self,
        context: &RequestContext,
        token: &str,
    ) -> Result<(), RuntimeError>;
}

pub trait CaptureEngine: Send + Sync {
    fn list_capture_items(
        &self,
        context: &RequestContext,
        page: PageRequest,
    ) -> Result<Page<CaptureSummary>, RuntimeError>;

    fn import_capture(
        &self,
        context: &RequestContext,
        envelope: CaptureEnvelope,
        uploads: Vec<UploadToken>,
    ) -> Result<CaptureImportResult, RuntimeError>;

    fn get_capture_result(
        &self,
        context: &RequestContext,
        item_id: &str,
    ) -> Result<CaptureImportResult, RuntimeError>;
}

pub trait ContentEngine: Send + Sync {
    fn describe_content(
        &self,
        context: &RequestContext,
        lease_id: &str,
    ) -> Result<ContentLease, RuntimeError>;

    fn stream_content(
        &self,
        context: &RequestContext,
        lease_id: &str,
        sink: &mut dyn Write,
    ) -> Result<(), RuntimeError>;
}

pub trait UploadEngine: Send + Sync {
    fn stage_upload(
        &self,
        context: &RequestContext,
        intent: UploadIntent,
        source: &mut dyn Read,
    ) -> Result<UploadToken, RuntimeError>;

    fn discard_upload(
        &self,
        context: &RequestContext,
        upload_token: &str,
    ) -> Result<(), RuntimeError>;
}

pub trait DownloadEngine: Send + Sync {
    fn issue_model_download(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_hash: Option<&str>,
    ) -> Result<SourceLease, RuntimeError>;
}

pub trait LibraryEngine:
    RuntimeInfoEngine
    + CatalogEngine
    + RuntimeStatusEngine
    + GalleryEngine
    + DownloadEngine
    + ContentEngine
    + UploadEngine
    + ScanEngine
{
}

impl<T> LibraryEngine for T where
    T: RuntimeInfoEngine
        + CatalogEngine
        + RuntimeStatusEngine
        + GalleryEngine
        + DownloadEngine
        + ContentEngine
        + UploadEngine
        + ScanEngine
{
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_dtos_use_camel_case_and_keep_secrets_out_of_context() {
        let envelope = CaptureEnvelope {
            version: CAPTURE_ENVELOPE_VERSION,
            item_id: "item-1".into(),
            created_at: "2026-10-11T00:00:00Z".into(),
            page: CapturePage {
                url: "https://example.test/model".into(),
                title: "Model".into(),
                designer: None,
                license: None,
                notes: None,
                parent_external_id: None,
            },
            downloads: vec![],
        };
        let value = serde_json::to_value(envelope).unwrap();
        assert_eq!(value["itemId"], "item-1");
        assert!(value.get("item_id").is_none());
        let context_fields = serde_json::to_value(RequestContext {
            principal_id: "principal".into(),
            capabilities: BTreeSet::new(),
            request_id: "request".into(),
        })
        .unwrap();
        assert!(context_fields.get("token").is_none());
        assert!(context_fields.get("secret").is_none());
    }
}
