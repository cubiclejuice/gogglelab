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
    PrinterNetwork,
    MediaRead,
    MediaWrite,
    Scan,
    Integration,
    Maintenance,
    PhysicalSourceWrite,
    BackupWrite,
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
    /// Path-free recovery receipts for any original source bytes retained
    /// before the operation stopped. Empty on ordinary failures.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_holds: Vec<SourceHoldReceipt>,
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

pub const MAX_MODEL_SEARCH_OFFSET: u32 = 100_000;

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
    BackupOnly,
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
pub struct IndexRootSettings {
    pub handle_id: String,
    pub catalog_root_id: String,
    pub label: String,
    pub access: RootAccess,
    pub enabled: bool,
    pub last_scanned_at: Option<String>,
    pub scan_interval_minutes: u32,
    pub max_file_bytes: u64,
    pub scan_zip: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IndexRootSettingsPatch {
    pub enabled: bool,
    pub scan_interval_minutes: u32,
    pub max_file_bytes: u64,
    pub scan_zip: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSettings {
    pub thumbnail_size: u32,
    pub thumbnail_background: String,
    pub thumbnail_concurrency: u32,
    pub scan_batch_size: u32,
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
    pub kind: ContentKind,
    pub file_name: String,
    pub mime: String,
    pub bytes: u64,
    pub sha256: String,
    pub library_epoch: u64,
    pub expires_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ContentKind {
    Thumbnail,
    Model,
    Geometry,
    BundleArchive,
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
pub struct GeometryPlate {
    pub id: u32,
    pub name: String,
    pub triangle_start: u32,
    pub triangle_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeometryPart {
    pub id: String,
    pub name: String,
    pub triangle_start: u32,
    pub triangle_count: u32,
    pub material_id: Option<String>,
    pub filament_slot: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeometryMaterial {
    pub id: String,
    pub name: Option<String>,
    pub color: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedFilament {
    pub slot: u32,
    pub name: Option<String>,
    pub material: Option<String>,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeometryLease {
    pub content: ContentLease,
    pub packet_format: String,
    pub packet_version: u32,
    pub model_format: String,
    pub triangle_count: u32,
    pub plates: Vec<GeometryPlate>,
    pub parts: Vec<GeometryPart>,
    pub materials: Vec<GeometryMaterial>,
    pub embedded_filaments: Vec<EmbeddedFilament>,
    pub part_warning: Option<String>,
    pub plate_warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub content_hash: String,
    pub models: Vec<CatalogModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogStats {
    pub total_models: u64,
    pub available_models: u64,
    pub missing_models: u64,
    pub bookmarks: u64,
    pub total_bytes: u64,
    pub bundle_count: u64,
    pub duplicate_group_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogFacetValue {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogFacets {
    pub designers: Vec<String>,
    pub licenses: Vec<String>,
    pub categories: Vec<String>,
    pub parents: Vec<CatalogFacetValue>,
    pub folder_ids: Vec<String>,
    pub root_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedMetadataFields {
    pub designer: Option<String>,
    pub source_url: Option<String>,
    pub notes: Option<String>,
    pub model_license: Option<String>,
    pub parent_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedMetadataProposal {
    pub model_id: String,
    pub source_hash: String,
    pub current: EmbeddedMetadataFields,
    pub embedded: EmbeddedMetadataFields,
    pub embedded_filaments: Vec<EmbeddedFilament>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct EmbeddedMetadataSelection {
    pub designer: bool,
    pub source_url: bool,
    pub notes: bool,
    pub model_license: bool,
    pub parent_model: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreRootMapping {
    pub backup_catalog_root_id: String,
    pub operator_handle_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MoveLooseSourceTarget {
    pub model_id: String,
    pub expected_version: u64,
    pub file_name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThumbnailCleanupMode {
    Orphans,
    ResetDefaults,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThumbnailCleanupRequest {
    pub mode: ThumbnailCleanupMode,
    pub continuation: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailBatch {
    pub mode: ThumbnailCleanupMode,
    pub reviewed_files: u32,
    pub eligible_files: u32,
    #[serde(with = "printer_decimal_u64")]
    pub eligible_bytes: u64,
    pub preserved_files: u32,
    pub protected_source_copies: u32,
    pub default_records_to_clear: u32,
    pub partial: bool,
    pub has_more: bool,
    pub remaining_files: Option<u32>,
    pub continuation: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailProgress {
    pub review: ThumbnailBatch,
    pub default_records_cleared: u32,
    pub stopped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ManagementAction {
    RecoverableRemove {
        targets: Vec<VersionedTarget>,
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
        root_mappings: Vec<RestoreRootMapping>,
    },
    ExportSelection {
        targets: Vec<VersionedTarget>,
        destination_handle_id: String,
        file_name: String,
    },
    ExportLibraryJson {
        destination_handle_id: String,
        file_name: String,
    },
    QuarantineLooseSource {
        target: VersionedTarget,
    },
    QuarantineLooseSources {
        targets: Vec<VersionedTarget>,
    },
    UndoQuarantine {
        snapshot_id: String,
    },
    MoveLooseSource {
        target: VersionedTarget,
        destination_handle_id: String,
        file_name: String,
    },
    MoveLooseSources {
        targets: Vec<MoveLooseSourceTarget>,
        destination_handle_id: String,
    },
    PurgeCatalog {
        backup_destination_handle_id: String,
        backup_file_name: String,
    },
    CleanupOrphans {
        #[serde(default)]
        continuation: Option<String>,
    },
    ResetDefaultThumbnails,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActionTargetSnapshot {
    pub model_id: String,
    pub metadata_version: u64,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActionPlan {
    pub token: String,
    pub action_kind: String,
    pub expires_at: String,
    pub library_epoch: u64,
    pub catalog_revision: u64,
    pub targets: Vec<ActionTargetSnapshot>,
    pub impact: Vec<String>,
    pub input_bytes: Option<u64>,
    pub input_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_review: Option<ThumbnailBatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ManagementResult {
    Removed {
        snapshot_id: String,
        removed_ids: Vec<String>,
    },
    RestoredRemoval {
        models: Vec<MutationVersion>,
    },
    BackupCreated {
        destination_handle_id: String,
        file_name: String,
        files: usize,
        bytes: u64,
        sha256: String,
    },
    BackupRestored {
        models: usize,
        library_epoch: u64,
    },
    SelectionExported {
        destination_handle_id: String,
        file_name: String,
        models: usize,
        bytes: u64,
    },
    LibraryExported {
        destination_handle_id: String,
        file_name: String,
        models: usize,
        bytes: u64,
    },
    Quarantined {
        snapshot_id: String,
        model_id: String,
        source_holds: Vec<SourceHoldReceipt>,
    },
    QuarantinedBatch {
        snapshot_id: String,
        models: Vec<String>,
        source_holds: Vec<SourceHoldReceipt>,
    },
    QuarantineUndone {
        model: MutationVersion,
    },
    QuarantineUndoneBatch {
        models: Vec<MutationVersion>,
    },
    Moved {
        model: MutationVersion,
        source_holds: Vec<SourceHoldReceipt>,
    },
    MovedBatch {
        models: Vec<MutationVersion>,
        source_holds: Vec<SourceHoldReceipt>,
    },
    Purged {
        backup_destination_handle_id: String,
        backup_file_name: String,
        library_epoch: u64,
    },
    CleanupCompleted {
        files: u64,
        bytes: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thumbnail_progress: Option<ThumbnailProgress>,
    },
}

/// Receipt for an original source file retained after a guarded source action.
///
/// The opaque `root_handle_id` and `relative_path` identify the original
/// location without exposing a host filesystem path. Hold and journal names
/// are sibling names in that admitted root. The payload remains recoverable;
/// no permanent deletion is implied.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SourceHoldReceipt {
    pub model_id: String,
    pub root_handle_id: String,
    pub relative_path: String,
    pub hold_name: String,
    pub journal_name: String,
    pub bytes: u64,
    pub sha256: String,
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
    pub root_id: Option<String>,
    pub bundle_id: Option<String>,
    pub archive_member: bool,
    pub parent_model_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_printed_at: Option<String>,
    pub print_count: u64,
    pub availability: ModelAvailability,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MetadataValueKind {
    Designer,
    ParentModel,
    License,
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
    pub rating: Option<u8>,
    pub favorite: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VersionedMetadataUpdate {
    pub model_id: String,
    pub expected_version: u64,
    pub patch: MetadataPatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filament_ids: Option<Vec<String>>,
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

/// Consumable hardware inventory; separate from geometric 3D parts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PartStock {
    pub id: String,
    pub metadata_version: u64,
    pub name: String,
    pub category: Option<String>,
    pub quantity: u64,
    pub unit: String,
    pub notes: String,
    pub low_stock: u32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PartStockInput {
    pub name: String,
    pub category: Option<String>,
    pub quantity: u32,
    pub unit: String,
    pub notes: String,
    pub low_stock: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PartUsageInput {
    pub id: String,
    /// Quantity consumed per copy of the model.
    pub quantity: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<PartUsageSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PartUsageSnapshot {
    pub name: String,
    pub category: Option<String>,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrintAttemptPart {
    /// `None` means the stock row was deleted; the event snapshot remains.
    pub id: Option<String>,
    pub name: String,
    pub category: Option<String>,
    pub unit: String,
    pub quantity_per_copy: u32,
    /// Total inventory consumed across all copies in this event.
    pub quantity: u32,
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
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrintAttemptInput {
    pub printed_at: String,
    pub quantity: u32,
    pub notes: String,
    pub filament_ids: Vec<String>,
    pub outcome: PrintOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<PartUsageInput>>,
}

/// Single-event input with optional local printer attribution. Keeping this
/// separate from `PrintAttemptInput` preserves the older batch wire contract.
/// On update, omitted `printerId` preserves attribution while explicit `null`
/// clears it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrintAttemptInputWithPrinter {
    #[serde(flatten)]
    pub input: PrintAttemptInput,
    #[serde(
        default,
        deserialize_with = "deserialize_present_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub printer_id: Option<Option<String>>,
}

fn deserialize_present_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
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
    #[serde(default)]
    pub parts: Vec<PrintAttemptPart>,
    pub outcome: PrintOutcome,
    #[serde(default)]
    pub stock_adjusted: Option<bool>,
    #[serde(default)]
    pub printer_id: Option<String>,
    #[serde(default)]
    pub printer_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrintAttemptBatchItem {
    pub model_id: String,
    pub input: PrintAttemptInput,
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
    pub thumbnail_mode: Option<String>,
    pub source_directory: Option<String>,
    pub minimum_rating: Option<u8>,
    pub favorite: Option<bool>,
    pub tags: Option<Vec<String>>,
    pub file_type: Option<String>,
    pub offset: Option<u32>,
    #[serde(default)]
    pub exclude_model_ids: Vec<String>,
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
            thumbnail_mode: None,
            source_directory: None,
            minimum_rating: None,
            favorite: None,
            tags: None,
            file_type: None,
            offset: None,
            exclude_model_ids: Vec::new(),
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
    pub thumbnails_total: usize,
    pub thumbnails_completed: usize,
    pub errors: Vec<String>,
}

pub trait RuntimeInfoEngine: Send + Sync {
    fn runtime_info(&self, context: &RequestContext) -> Result<RuntimeInfo, RuntimeError>;
}

pub trait SettingsEngine: Send + Sync {
    fn list_index_root_settings(
        &self,
        context: &RequestContext,
    ) -> Result<Vec<IndexRootSettings>, RuntimeError>;
    fn update_index_root_settings(
        &self,
        context: &RequestContext,
        handle_id: &str,
        patch: IndexRootSettingsPatch,
    ) -> Result<IndexRootSettings, RuntimeError>;
    fn get_catalog_settings(
        &self,
        context: &RequestContext,
    ) -> Result<CatalogSettings, RuntimeError>;
    fn update_catalog_settings(
        &self,
        context: &RequestContext,
        settings: CatalogSettings,
    ) -> Result<CatalogSettings, RuntimeError>;
}

pub trait CatalogEngine: Send + Sync {
    fn get_model(
        &self,
        context: &RequestContext,
        model_id: &str,
    ) -> Result<CatalogModel, RuntimeError>;

    fn list_models(
        &self,
        context: &RequestContext,
        query: ModelQuery,
        page: PageRequest,
    ) -> Result<Page<CatalogModel>, RuntimeError>;

    fn catalog_stats(&self, context: &RequestContext) -> Result<CatalogStats, RuntimeError>;

    fn catalog_facets(&self, context: &RequestContext) -> Result<CatalogFacets, RuntimeError>;
}

pub trait MetadataEngine: Send + Sync {
    fn rename_metadata_value(
        &self,
        context: &RequestContext,
        kind: MetadataValueKind,
        old_name: &str,
        new_name: &str,
    ) -> Result<u64, RuntimeError>;
    fn delete_metadata_value(
        &self,
        context: &RequestContext,
        kind: MetadataValueKind,
        name: &str,
    ) -> Result<u64, RuntimeError>;
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

    fn update_metadata_batch(
        &self,
        context: &RequestContext,
        updates: Vec<VersionedMetadataUpdate>,
    ) -> Result<Vec<MutationVersion>, RuntimeError>;
}

pub trait TagEngine: Send + Sync {
    fn list_tags(&self, context: &RequestContext) -> Result<Vec<Tag>, RuntimeError>;
    fn add_model_tags_by_name(
        &self,
        context: &RequestContext,
        target: VersionedTarget,
        names: Vec<String>,
    ) -> Result<MutationVersion, RuntimeError> {
        let _ = (context, target, names);
        Err(RuntimeError {
            code: RuntimeErrorCode::Unsupported,
            message: "Model tag assignment is not supported by this host".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })
    }
    fn remove_model_tags_by_name(
        &self,
        context: &RequestContext,
        target: VersionedTarget,
        names: Vec<String>,
    ) -> Result<MutationVersion, RuntimeError> {
        let _ = (context, target, names);
        Err(RuntimeError {
            code: RuntimeErrorCode::Unsupported,
            message: "Model tag assignment is not supported by this host".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })
    }
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

pub trait PartStockEngine: Send + Sync {
    fn list_part_stock(&self, context: &RequestContext) -> Result<Vec<PartStock>, RuntimeError>;
    fn create_part_stock(
        &self,
        context: &RequestContext,
        input: PartStockInput,
    ) -> Result<PartStock, RuntimeError>;
    fn update_part_stock(
        &self,
        context: &RequestContext,
        part_id: &str,
        expected_version: u64,
        input: PartStockInput,
    ) -> Result<PartStock, RuntimeError>;
    fn delete_part_stock(
        &self,
        context: &RequestContext,
        part_id: &str,
        expected_version: u64,
    ) -> Result<(), RuntimeError>;
}

pub trait PrintHistoryEngine: Send + Sync {
    fn create_print_attempts(
        &self,
        context: &RequestContext,
        idempotency_key: &str,
        items: Vec<PrintAttemptBatchItem>,
    ) -> Result<Vec<PrintAttempt>, RuntimeError>;

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
    fn create_print_attempt_with_printer(
        &self,
        context: &RequestContext,
        model_id: &str,
        input: PrintAttemptInputWithPrinter,
    ) -> Result<PrintAttempt, RuntimeError> {
        if matches!(input.printer_id, Some(Some(_))) {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Unsupported,
                message: "Printer attribution is unavailable in this runtime".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        self.create_print_attempt(context, model_id, input.input)
    }
    fn update_print_attempt(
        &self,
        context: &RequestContext,
        attempt_id: &str,
        input: PrintAttemptInput,
    ) -> Result<PrintAttempt, RuntimeError>;
    fn update_print_attempt_with_printer(
        &self,
        context: &RequestContext,
        attempt_id: &str,
        input: PrintAttemptInputWithPrinter,
    ) -> Result<PrintAttempt, RuntimeError> {
        if input.printer_id.is_some() {
            return Err(RuntimeError {
                code: RuntimeErrorCode::Unsupported,
                message: "Printer attribution is unavailable in this runtime".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            });
        }
        self.update_print_attempt(context, attempt_id, input.input)
    }
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

    fn set_thumbnail(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_version: u64,
        upload_token: &str,
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

    fn start_scan_subdirectory(
        &self,
        context: &RequestContext,
        catalog_root_id: String,
        directory: String,
    ) -> Result<ScanStatus, RuntimeError> {
        if directory.is_empty() {
            self.start_scan(context, Some(catalog_root_id))
        } else {
            Err(RuntimeError {
                code: RuntimeErrorCode::Unsupported,
                message: "Root-relative subdirectory scans are unsupported by this host".into(),
                retryable: false,
                current_version: None,
                source_holds: Vec::new(),
            })
        }
    }

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
    fn issue_bundle_download(
        &self,
        context: &RequestContext,
        preview_token: &str,
    ) -> Result<ContentLease, RuntimeError>;
}

pub trait GeometryEngine: Send + Sync {
    fn issue_model_geometry(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_hash: Option<&str>,
        plate_id: Option<u32>,
    ) -> Result<GeometryLease, RuntimeError>;
}

pub trait DuplicateEngine: Send + Sync {
    fn duplicate_groups(
        &self,
        context: &RequestContext,
        model_ids: Vec<String>,
    ) -> Result<Vec<DuplicateGroup>, RuntimeError>;
}

pub trait EmbeddedMetadataEngine: Send + Sync {
    fn read_embedded_metadata(
        &self,
        context: &RequestContext,
        model_id: &str,
        expected_hash: Option<&str>,
    ) -> Result<EmbeddedMetadataProposal, RuntimeError>;

    fn apply_embedded_metadata(
        &self,
        context: &RequestContext,
        target: VersionedTarget,
        expected_source_hash: &str,
        selection: EmbeddedMetadataSelection,
    ) -> Result<MutationVersion, RuntimeError>;
}

pub trait ManagementEngine: Send + Sync {
    fn discard_action(&self, _context: &RequestContext, _token: &str) -> Result<(), RuntimeError> {
        Err(RuntimeError {
            code: RuntimeErrorCode::Unsupported,
            message: "Management discard is unavailable".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })
    }
    fn plan_action(
        &self,
        context: &RequestContext,
        action: ManagementAction,
    ) -> Result<ActionPlan, RuntimeError>;
    fn execute_action(
        &self,
        context: &RequestContext,
        token: &str,
    ) -> Result<ManagementResult, RuntimeError>;

    fn plan_library_export(
        &self,
        context: &RequestContext,
        dest_path: Option<&str>,
    ) -> Result<ActionPlan, RuntimeError> {
        let _ = (context, dest_path);
        Err(RuntimeError {
            code: RuntimeErrorCode::Unsupported,
            message: "Library JSON export is unavailable".into(),
            retryable: false,
            current_version: None,
            source_holds: Vec::new(),
        })
    }
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

// Shared integration transport contract. Secrets are accepted only in mutation inputs and are
// never present in settings or review outputs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IntegrationEndpointScope {
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IntegrationProviderKind {
    OpenaiCompatible,
    Anthropic,
    Gemini,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IntegrationStructuredOutputMode {
    JsonSchema,
    JsonObject,
    PlainJson,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IntegrationCategorySharing {
    #[default]
    Local,
    Provider,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationSpoolmanConfig {
    pub base_url: String,
    pub endpoint_scope: IntegrationEndpointScope,
    pub page_size: u16,
    pub timeout_seconds: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiProviderConfig {
    pub id: String,
    pub label: String,
    pub kind: IntegrationProviderKind,
    pub base_url: Option<String>,
    pub endpoint_scope: IntegrationEndpointScope,
    pub model: String,
    pub credential_storage: String,
    pub has_credential: bool,
    pub max_tags: u8,
    pub detail: String,
    pub category_mode: String,
    pub concurrency: u8,
    pub timeout_seconds: u16,
    pub structured_output: IntegrationStructuredOutputMode,
    #[serde(default)]
    pub category_sharing: IntegrationCategorySharing,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntegrationCredentialUpdate {
    pub action: String,
    pub secret: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationSettings {
    pub spoolman: Option<IntegrationSpoolmanConfig>,
    pub ai_providers: Vec<AiProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationSpoolmanPullResult {
    pub filaments_received: usize,
    pub spools_received: usize,
    pub filaments_upserted: usize,
    pub spools_upserted: usize,
    pub removed_locally: usize,
    pub last_successful_pull_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationAiSuggestion {
    pub suggestion_id: String,
    pub name: String,
    pub category: Option<String>,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationAiExistingTag {
    pub name: String,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationAiSuggestionTarget {
    pub model_id: String,
    pub expected_version: u64,
    pub display_name: String,
    pub existing_tags: Vec<IntegrationAiExistingTag>,
    pub suggestions: Vec<IntegrationAiSuggestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationAiSuggestionFailure {
    pub model_id: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationAiSuggestionBatch {
    pub review_token: String,
    pub provider_id: String,
    pub model: String,
    pub category_sharing: IntegrationCategorySharing,
    pub targets: Vec<IntegrationAiSuggestionTarget>,
    pub failures: Vec<IntegrationAiSuggestionFailure>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IntegrationAiReviewAction {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum IntegrationAiReviewStrategy {
    Append,
    Replace,
    Merge,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationAiReviewDecision {
    pub model_id: String,
    pub action: IntegrationAiReviewAction,
    pub accepted_suggestion_ids: Vec<String>,
}

pub trait IntegrationEngine: Send + Sync {
    fn integration_network_enabled(&self, context: &RequestContext) -> bool;
    fn integration_settings(
        &self,
        context: &RequestContext,
    ) -> Result<IntegrationSettings, RuntimeError>;
    fn save_spoolman_config(
        &self,
        context: &RequestContext,
        config: Option<IntegrationSpoolmanConfig>,
    ) -> Result<IntegrationSettings, RuntimeError>;
    fn pull_spoolman(
        &self,
        context: &RequestContext,
    ) -> Result<IntegrationSpoolmanPullResult, RuntimeError>;
    fn save_ai_provider(
        &self,
        context: &RequestContext,
        config: AiProviderConfig,
        credential: IntegrationCredentialUpdate,
    ) -> Result<AiProviderConfig, RuntimeError>;
    fn delete_ai_provider(
        &self,
        context: &RequestContext,
        provider_id: &str,
    ) -> Result<(), RuntimeError>;
    fn suggest_ai_tags(
        &self,
        context: &RequestContext,
        provider_id: &str,
        targets: Vec<VersionedTarget>,
    ) -> Result<IntegrationAiSuggestionBatch, RuntimeError>;
    fn apply_ai_review(
        &self,
        context: &RequestContext,
        review_token: &str,
        decisions: Vec<IntegrationAiReviewDecision>,
        strategy: IntegrationAiReviewStrategy,
    ) -> Result<Vec<MutationVersion>, RuntimeError>;
    fn discard_ai_review(
        &self,
        context: &RequestContext,
        review_token: &str,
    ) -> Result<(), RuntimeError>;
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
    use serde_json::Value;

    struct LegacyPrintHistoryEngine;

    fn legacy_print_attempt() -> PrintAttempt {
        PrintAttempt {
            id: "print-1".into(),
            model_id: "model-1".into(),
            printed_at: "2026-10-14T12:00:00.000Z".into(),
            quantity: 1,
            notes: String::new(),
            filament_ids: Vec::new(),
            parts: Vec::new(),
            outcome: PrintOutcome::Printed,
            stock_adjusted: None,
            printer_id: None,
            printer_name: None,
            created_at: "2026-10-14T12:00:00.000Z".into(),
            updated_at: "2026-10-14T12:00:00.000Z".into(),
        }
    }

    impl PrintHistoryEngine for LegacyPrintHistoryEngine {
        fn create_print_attempts(
            &self,
            _: &RequestContext,
            _: &str,
            _: Vec<PrintAttemptBatchItem>,
        ) -> Result<Vec<PrintAttempt>, RuntimeError> {
            Ok(Vec::new())
        }

        fn list_print_attempts(
            &self,
            _: &RequestContext,
            _: &str,
        ) -> Result<Vec<PrintAttempt>, RuntimeError> {
            Ok(Vec::new())
        }

        fn create_print_attempt(
            &self,
            _: &RequestContext,
            _: &str,
            _: PrintAttemptInput,
        ) -> Result<PrintAttempt, RuntimeError> {
            Ok(legacy_print_attempt())
        }

        fn update_print_attempt(
            &self,
            _: &RequestContext,
            _: &str,
            _: PrintAttemptInput,
        ) -> Result<PrintAttempt, RuntimeError> {
            Ok(legacy_print_attempt())
        }

        fn delete_print_attempt(&self, _: &RequestContext, _: &str) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    #[test]
    fn legacy_print_history_engines_keep_old_calls_and_reject_unhandled_patches() {
        let engine = LegacyPrintHistoryEngine;
        let context = RequestContext {
            principal_id: "test".into(),
            capabilities: BTreeSet::new(),
            request_id: "request".into(),
        };
        let legacy_input: PrintAttemptInputWithPrinter =
            serde_json::from_value(serde_json::json!({
                "printedAt": "2026-10-14T12:00:00Z",
                "quantity": 1,
                "notes": "",
                "filamentIds": [],
                "outcome": "printed"
            }))
            .unwrap();
        assert!(engine
            .create_print_attempt_with_printer(&context, "model-1", legacy_input)
            .is_ok());

        let linked: PrintAttemptInputWithPrinter = serde_json::from_value(serde_json::json!({
            "printedAt": "2026-10-14T12:00:00Z",
            "quantity": 1,
            "notes": "",
            "filamentIds": [],
            "outcome": "printed",
            "printerId": "printer-1"
        }))
        .unwrap();
        assert_eq!(
            engine
                .create_print_attempt_with_printer(&context, "model-1", linked)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Unsupported
        );

        let clear_update: PrintAttemptInputWithPrinter =
            serde_json::from_value(serde_json::json!({
                "printedAt": "2026-10-14T12:00:00Z",
                "quantity": 1,
                "notes": "",
                "filamentIds": [],
                "outcome": "printed",
                "printerId": null
            }))
            .unwrap();
        assert_eq!(
            engine
                .update_print_attempt_with_printer(&context, "print-1", clear_update)
                .unwrap_err()
                .code,
            RuntimeErrorCode::Unsupported
        );

        let batch: Vec<PrintAttemptBatchItem> = serde_json::from_value(serde_json::json!([{
            "modelId": "model-1",
            "input": {
                "printedAt": "2026-10-14T12:00:00Z",
                "quantity": 1,
                "notes": "",
                "filamentIds": [],
                "outcome": "printed"
            }
        }]))
        .unwrap();
        assert_eq!(batch.len(), 1);
    }

    #[test]
    fn single_print_attempt_preserves_printer_patch_presence_and_batch_contract() {
        let base = serde_json::json!({
            "printedAt": "2026-10-14T12:00:00Z",
            "quantity": 1,
            "notes": "",
            "filamentIds": [],
            "outcome": "printed"
        });

        let omitted: PrintAttemptInputWithPrinter = serde_json::from_value(base.clone()).unwrap();
        assert_eq!(omitted.printer_id, None);
        assert!(serde_json::to_value(&omitted)
            .unwrap()
            .get("printerId")
            .is_none());

        let mut clear_value = base.clone();
        clear_value["printerId"] = Value::Null;
        let clear: PrintAttemptInputWithPrinter = serde_json::from_value(clear_value).unwrap();
        assert_eq!(clear.printer_id, Some(None));
        assert_eq!(
            serde_json::to_value(&clear).unwrap()["printerId"],
            Value::Null
        );

        let mut linked_value = base.clone();
        linked_value["printerId"] = Value::String("printer-1".into());
        let linked: PrintAttemptInputWithPrinter = serde_json::from_value(linked_value).unwrap();
        assert_eq!(linked.printer_id, Some(Some("printer-1".into())));

        let batch: PrintAttemptBatchItem = serde_json::from_value(serde_json::json!({
            "modelId": "model-1",
            "input": base
        }))
        .unwrap();
        assert_eq!(batch.input.quantity, 1);
        assert!(serde_json::to_value(batch).unwrap()["input"]
            .get("printerId")
            .is_none());

        let result = PrintAttempt {
            id: "print-1".into(),
            model_id: "model-1".into(),
            printed_at: "2026-10-14T12:00:00.000Z".into(),
            quantity: 1,
            notes: String::new(),
            filament_ids: Vec::new(),
            parts: Vec::new(),
            outcome: PrintOutcome::Printed,
            stock_adjusted: Some(true),
            printer_id: Some("printer-1".into()),
            printer_name: Some("Workshop".into()),
            created_at: "2026-10-14T12:00:00.000Z".into(),
            updated_at: "2026-10-14T12:00:00.000Z".into(),
        };
        let encoded = serde_json::to_value(result).unwrap();
        assert_eq!(encoded["printerId"], "printer-1");
        assert_eq!(encoded["printerName"], "Workshop");

        let mut unassigned = legacy_print_attempt();
        unassigned.printer_id = None;
        unassigned.printer_name = None;
        let encoded = serde_json::to_value(unassigned).unwrap();
        assert_eq!(encoded["printerId"], Value::Null);
        assert_eq!(encoded["printerName"], Value::Null);
    }

    #[test]
    fn cancelled_attempts_and_optional_stock_usage_keep_the_wire_contract() {
        let input: PrintAttemptInput = serde_json::from_value(serde_json::json!({
            "printedAt": "2026-10-15T12:00:00Z",
            "quantity": 2,
            "notes": "cancelled after setup",
            "filamentIds": [],
            "outcome": "cancelled",
            "parts": [{"id": "part-1", "quantity": 3}]
        }))
        .unwrap();
        assert_eq!(input.outcome, PrintOutcome::Cancelled);
        assert_eq!(input.parts.as_ref().unwrap()[0].id, "part-1");
        assert_eq!(input.parts.as_ref().unwrap()[0].quantity, 3);

        let legacy: PrintAttemptInput = serde_json::from_value(serde_json::json!({
            "printedAt": "2026-10-15T12:00:00Z",
            "quantity": 1,
            "notes": "legacy",
            "filamentIds": [],
            "outcome": "failed"
        }))
        .unwrap();
        assert_eq!(legacy.parts, None);
        assert_eq!(serde_json::to_value(legacy).unwrap().get("parts"), None);
    }

    #[test]
    fn unknown_stock_adjustment_state_is_explicit_on_the_wire() {
        let attempt = PrintAttempt {
            id: "attempt-1".into(),
            model_id: "model-1".into(),
            printed_at: "2026-10-15T12:00:00Z".into(),
            quantity: 1,
            notes: String::new(),
            filament_ids: Vec::new(),
            parts: Vec::new(),
            outcome: PrintOutcome::Failed,
            stock_adjusted: None,
            printer_id: None,
            printer_name: None,
            created_at: "2026-10-15T12:00:00Z".into(),
            updated_at: "2026-10-15T12:00:00Z".into(),
        };
        let value = serde_json::to_value(attempt).unwrap();
        assert!(value.get("stockAdjusted").is_some());
        assert_eq!(value["stockAdjusted"], Value::Null);
        assert!(value.get("stock_adjusted").is_none());
    }

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

    #[test]
    fn thumbnail_cleanup_protocol_is_additive_and_uses_exact_byte_totals() {
        let action: ManagementAction =
            serde_json::from_value(serde_json::json!({"kind":"cleanupOrphans","confirm":true}))
                .unwrap();
        assert!(matches!(
            action,
            ManagementAction::CleanupOrphans { continuation: None }
        ));
        assert!(matches!(
            serde_json::from_value::<ManagementAction>(
                serde_json::json!({"kind":"resetDefaultThumbnails","confirm":true})
            )
            .unwrap(),
            ManagementAction::ResetDefaultThumbnails
        ));
        let batch = ThumbnailBatch {
            mode: ThumbnailCleanupMode::Orphans,
            reviewed_files: 1,
            eligible_files: 1,
            eligible_bytes: u64::MAX,
            preserved_files: 0,
            protected_source_copies: 0,
            default_records_to_clear: 0,
            partial: true,
            has_more: true,
            remaining_files: None,
            continuation: Some("opaque".into()),
        };
        let value = serde_json::to_value(&batch).unwrap();
        assert_eq!(value["eligibleBytes"], u64::MAX.to_string());
        assert_eq!(value["remainingFiles"], serde_json::Value::Null);
        assert_eq!(
            serde_json::from_value::<ThumbnailBatch>(value).unwrap(),
            batch
        );
        assert!(serde_json::from_value::<ThumbnailCleanupRequest>(
            serde_json::json!({"mode":"orphans","arbitraryPath":"secret"})
        )
        .is_err());
    }

    #[test]
    fn generated_cleanup_reports_only_deleted_counts() {
        let result = ManagementResult::CleanupCompleted {
            files: 1,
            bytes: 19,
            thumbnail_progress: None,
        };
        let encoded = serde_json::to_value(&result).unwrap();
        assert_eq!(encoded["kind"], "cleanupCompleted");
        assert_eq!(encoded["files"], 1);
        assert_eq!(encoded["bytes"], 19);
        assert_eq!(
            serde_json::from_value::<ManagementResult>(encoded).unwrap(),
            result
        );
    }

    #[test]
    fn source_action_results_expose_path_free_recovery_holds() {
        let hold = SourceHoldReceipt {
            model_id: "model-1".into(),
            root_handle_id: "root-1".into(),
            relative_path: "models/part.stl".into(),
            hold_name: ".part.stl.hold.data".into(),
            journal_name: ".part.stl.hold.json".into(),
            bytes: 19,
            sha256: "b".repeat(64),
        };
        let results = [
            ManagementResult::Quarantined {
                snapshot_id: "removal-1".into(),
                model_id: "model-1".into(),
                source_holds: vec![hold.clone()],
            },
            ManagementResult::QuarantinedBatch {
                snapshot_id: "removal-1".into(),
                models: vec!["model-1".into()],
                source_holds: vec![hold.clone()],
            },
            ManagementResult::Moved {
                model: MutationVersion {
                    model_id: "model-1".into(),
                    metadata_version: 2,
                },
                source_holds: vec![hold.clone()],
            },
            ManagementResult::MovedBatch {
                models: vec![MutationVersion {
                    model_id: "model-1".into(),
                    metadata_version: 2,
                }],
                source_holds: vec![hold],
            },
        ];

        for result in results {
            let encoded = serde_json::to_value(&result).unwrap();
            assert_eq!(encoded["sourceHolds"].as_array().unwrap().len(), 1);
            assert_eq!(encoded["sourceHolds"][0]["rootHandleId"], "root-1");
            assert_eq!(encoded["sourceHolds"][0]["relativePath"], "models/part.stl");
            assert!(encoded["sourceHolds"][0].get("root_handle_id").is_none());
            assert_eq!(
                serde_json::from_value::<ManagementResult>(encoded).unwrap(),
                result
            );
        }

        let partial = RuntimeError {
            code: RuntimeErrorCode::Io,
            message: "operation stopped after retaining source bytes".into(),
            retryable: false,
            current_version: None,
            source_holds: vec![SourceHoldReceipt {
                model_id: "model-1".into(),
                root_handle_id: "root-1".into(),
                relative_path: "models/part.stl".into(),
                hold_name: ".part.stl.hold.data".into(),
                journal_name: ".part.stl.hold.json".into(),
                bytes: 19,
                sha256: "b".repeat(64),
            }],
        };
        let encoded_error = serde_json::to_value(&partial).unwrap();
        assert_eq!(encoded_error["code"], "io");
        assert_eq!(
            encoded_error["message"],
            "operation stopped after retaining source bytes"
        );
        assert_eq!(encoded_error["sourceHolds"][0]["bytes"], 19);
        assert!(encoded_error.get("source_holds").is_none());
        let empty_error = RuntimeError {
            source_holds: Vec::new(),
            ..partial
        };
        assert!(serde_json::to_value(empty_error)
            .unwrap()
            .get("sourceHolds")
            .is_none());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HashStatus {
    pub model_count: String,
    pub hashed_count: String,
    pub unhashed_count: String,
    pub bookmark_count: String,
    pub missing_count: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceState {
    Available,
    Bookmark,
    Unhashed,
    Missing,
    Changed,
    Unadmitted,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSourceStatus {
    pub model_id: String,
    pub stored_hash: Option<String>,
    pub state: SourceState,
}

pub trait CatalogJobEngine: Send + Sync {
    fn catalog_hash_status(&self, context: &RequestContext) -> Result<HashStatus, RuntimeError>;
    fn check_model_sources(
        &self,
        context: &RequestContext,
        model_ids: Vec<String>,
    ) -> Result<Vec<ModelSourceStatus>, RuntimeError>;
    fn start_thumbnail_job(
        &self,
        context: &RequestContext,
        model_ids: Vec<String>,
        force: bool,
    ) -> Result<ScanStatus, RuntimeError>;
}

// Browser/MCP printer metadata uses exact decimal strings for 64-bit counters.
mod printer_decimal_u64 {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.is_empty()
            || (value.len() > 1 && value.starts_with('0'))
            || !value.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(serde::de::Error::custom(
                "expected a canonical decimal u64 string",
            ));
        }
        value.parse().map_err(serde::de::Error::custom)
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrinterSourceKind {
    Manual,
    Discovered,
    Imported,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrinterConnectionStatus {
    #[default]
    Unknown,
    Online,
    Offline,
    AuthRequired,
    Error,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrinterJobStatus {
    #[default]
    Unknown,
    Idle,
    Printing,
    Paused,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Printer {
    pub id: String,
    #[serde(with = "printer_decimal_u64")]
    pub metadata_version: u64,
    pub nickname: String,
    pub make: Option<String>,
    pub model: Option<String>,
    pub printer_type: Option<String>,
    pub firmware: Option<String>,
    pub host: Option<String>,
    pub web_url: Option<String>,
    pub endpoint_approved: bool,
    pub bed_mm: Option<[f32; 3]>,
    pub notes: String,
    pub source_kind: PrinterSourceKind,
    pub connection_status: PrinterConnectionStatus,
    pub job_status: PrinterJobStatus,
    /// Connection and job state are a snapshot from the last explicit check.
    /// Consumers should treat old timestamps as stale until a user checks again.
    pub last_checked_at: Option<String>,
    pub connection_error_code: Option<String>,
    #[serde(with = "printer_decimal_u64")]
    pub print_count: u64,
    pub last_printed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrinterInput {
    pub nickname: String,
    pub make: Option<String>,
    pub model: Option<String>,
    pub printer_type: Option<String>,
    pub firmware: Option<String>,
    pub host: Option<String>,
    pub web_url: Option<String>,
    pub bed_mm: Option<[f32; 3]>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrinterMaintenanceInput {
    pub title: String,
    pub performed_at: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterMaintenanceLog {
    pub id: String,
    #[serde(with = "printer_decimal_u64")]
    pub metadata_version: u64,
    pub printer_id: Option<String>,
    pub printer_nickname: String,
    pub title: String,
    pub performed_at: String,
    pub notes: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrinterReminderStatus {
    #[default]
    Active,
    Paused,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrinterReminderInput {
    pub title: String,
    pub notes: String,
    pub due_at: String,
    pub interval_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterReminder {
    pub id: String,
    #[serde(with = "printer_decimal_u64")]
    pub metadata_version: u64,
    pub printer_id: Option<String>,
    pub printer_nickname: String,
    pub title: String,
    pub notes: String,
    pub due_at: String,
    pub interval_days: Option<u32>,
    pub status: PrinterReminderStatus,
    pub last_completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrinterReminderCompletionInput {
    pub completed_at: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterReminderCompletion {
    pub reminder: PrinterReminder,
    pub maintenance_log: PrinterMaintenanceLog,
}

/// Data-only fleet management. Network and credential operations are deliberately absent.
pub trait PrinterFleetEngine: Send + Sync {
    fn list_printers(&self, context: &RequestContext) -> Result<Vec<Printer>, RuntimeError>;
    fn get_printer(
        &self,
        context: &RequestContext,
        id: &str,
    ) -> Result<Option<Printer>, RuntimeError>;
    fn create_printer(
        &self,
        context: &RequestContext,
        input: PrinterInput,
    ) -> Result<Printer, RuntimeError>;
    fn update_printer(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        input: PrinterInput,
    ) -> Result<Printer, RuntimeError>;
    fn delete_printer(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
    ) -> Result<(), RuntimeError>;
    fn list_printer_maintenance_logs(
        &self,
        context: &RequestContext,
        printer_id: Option<&str>,
    ) -> Result<Vec<PrinterMaintenanceLog>, RuntimeError>;
    fn create_printer_maintenance_log(
        &self,
        context: &RequestContext,
        printer_id: &str,
        input: PrinterMaintenanceInput,
    ) -> Result<PrinterMaintenanceLog, RuntimeError>;
    fn update_printer_maintenance_log(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        input: PrinterMaintenanceInput,
    ) -> Result<PrinterMaintenanceLog, RuntimeError>;
    fn delete_printer_maintenance_log(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
    ) -> Result<(), RuntimeError>;
    fn list_printer_reminders(
        &self,
        context: &RequestContext,
        printer_id: Option<&str>,
        include_completed: bool,
    ) -> Result<Vec<PrinterReminder>, RuntimeError>;
    fn create_printer_reminder(
        &self,
        context: &RequestContext,
        printer_id: &str,
        input: PrinterReminderInput,
    ) -> Result<PrinterReminder, RuntimeError>;
    fn update_printer_reminder(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        input: PrinterReminderInput,
    ) -> Result<PrinterReminder, RuntimeError>;
    fn delete_printer_reminder(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
    ) -> Result<(), RuntimeError>;
    fn complete_printer_reminder(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        input: PrinterReminderCompletionInput,
    ) -> Result<PrinterReminderCompletion, RuntimeError>;
    fn set_printer_reminder_paused(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        paused: bool,
    ) -> Result<PrinterReminder, RuntimeError>;
    fn reschedule_printer_reminder(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        due_at: &str,
    ) -> Result<PrinterReminder, RuntimeError>;
}

/// Host-configured availability only; clients cannot expand network permission.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterNetworkPolicySummary {
    pub enabled: bool,
    pub discovery_available: bool,
    pub status_checks_available: bool,
    pub multicast_discovery_allowed: bool,
    pub carbon_broadcast_allowed: bool,
    pub max_discovery_addresses: u32,
    pub max_suggestions: u32,
    pub maximum_duration_ms: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterDiscoveryPlan {
    pub plan_token: String,
    pub normalized_range: String,
    pub address_count: u32,
    pub include_bambu_discovery: bool,
    pub multicast_disclosure: Option<String>,
    pub expires_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrinterDiscoveryJobStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterNetworkConnection {
    pub kind: String,
    pub address: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrinterSuggestionSource {
    Moonraker,
    Prusalink,
    BambuMqttTls,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterNetworkSuggestion {
    pub key: String,
    pub name: String,
    pub model: Option<String>,
    pub bed_mm: Option<[u32; 3]>,
    pub connection: PrinterNetworkConnection,
    pub source: PrinterSuggestionSource,
    pub printer_type: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterDiscoveryJob {
    pub job_id: String,
    pub status: PrinterDiscoveryJobStatus,
    pub suggestions: Vec<PrinterNetworkSuggestion>,
    pub normalized_range: String,
    pub error_code: Option<String>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PrinterConnectionResult {
    Connected,
    Unreachable,
    Unsupported,
    AuthRequired,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterConnectionResponse {
    pub result: PrinterConnectionResult,
    pub printer: Printer,
    pub status_event_observed: bool,
}
pub trait PrinterNetworkEngine: Send + Sync {
    fn get_printer_network_policy(
        &self,
        context: &RequestContext,
    ) -> Result<PrinterNetworkPolicySummary, RuntimeError>;
    fn plan_printer_discovery(
        &self,
        context: &RequestContext,
        range: &str,
        include_bambu_discovery: bool,
    ) -> Result<PrinterDiscoveryPlan, RuntimeError>;
    fn start_printer_discovery(
        &self,
        context: &RequestContext,
        plan_token: &str,
        confirm: bool,
    ) -> Result<PrinterDiscoveryJob, RuntimeError>;
    fn get_printer_discovery(
        &self,
        context: &RequestContext,
        job_id: &str,
    ) -> Result<PrinterDiscoveryJob, RuntimeError>;
    fn cancel_printer_discovery(
        &self,
        context: &RequestContext,
        job_id: &str,
    ) -> Result<PrinterDiscoveryJob, RuntimeError>;
    fn set_printer_endpoint_approval(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        approved: bool,
    ) -> Result<Printer, RuntimeError>;
    fn test_printer_connection(
        &self,
        context: &RequestContext,
        id: &str,
        expected_version: u64,
        discover_mainboard_id: bool,
        confirm: bool,
    ) -> Result<PrinterConnectionResponse, RuntimeError>;
}
