use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub model_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub folder_id: String,
    pub original_name: Option<String>,
    pub format: Option<String>,
    pub bytes: u64,
    pub content_hash: Option<String>,
    pub storage_path: Option<String>,
    pub metadata_version: u64,
    pub created_at: String,
    pub updated_at: String,
    pub tags: Vec<String>,
    pub filament_ids: Vec<String>,
    pub source_kind: String,
    pub source_path: Option<String>,
    pub source_root_id: Option<String>,
    pub source_missing: bool,
    pub source_modified_at: Option<u64>,
    pub thumbnail_path: Option<String>,
    pub metadata: ModelMetadata,
    pub bundle_id: Option<String>,
    pub source_archive_member: Option<String>,
    pub catalog_kind: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelSort {
    Name,
    Size,
    Date,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelGroupFilter {
    Standalone,
    Bundle,
    Archive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelPageQuery {
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

impl Default for ModelPageQuery {
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
#[serde(rename_all = "snake_case")]
pub enum FilamentOrigin {
    Manual,
    Spoolman,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FilamentResourceKind {
    Manual,
    Filament,
    Spool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Filament {
    pub id: String,
    pub version: u64,
    pub brand: String,
    pub material: String,
    pub color: String,
    pub name: String,
    pub notes: String,
    pub origin: FilamentOrigin,
    pub resource_kind: FilamentResourceKind,
    pub source_id: Option<String>,
    pub external_id: Option<String>,
    pub parent_filament_id: Option<String>,
    pub archived: bool,
    pub source_missing: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilamentInput {
    pub brand: String,
    pub material: String,
    pub color: String,
    pub name: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpoolmanSource {
    pub id: String,
    pub base_url: String,
    pub last_synced_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpoolmanFilamentRecord {
    pub external_id: String,
    pub brand: String,
    pub material: String,
    pub color: String,
    pub name: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpoolmanSpoolRecord {
    pub external_id: String,
    pub filament_external_id: String,
    pub brand: String,
    pub material: String,
    pub color: String,
    pub name: String,
    pub notes: String,
    pub archived: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilamentSyncSummary {
    pub source_id: String,
    pub seen: usize,
    pub added: usize,
    pub updated: usize,
    pub missing: usize,
    pub completed_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrintStatus {
    #[default]
    Unprinted,
    Want,
    Queued,
    Printing,
    Printed,
    Failed,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelMetadata {
    pub designer: Option<String>,
    pub source_url: Option<String>,
    pub notes: Option<String>,
    pub model_license: Option<String>,
    pub print_status: PrintStatus,
    pub parent_model_id: Option<String>,
    pub last_printed_at: Option<String>,
    pub print_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TagMode {
    Replace,
    Merge,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetadataPatch {
    pub designer: Option<String>,
    pub source_url: Option<String>,
    pub notes: Option<String>,
    pub model_license: Option<String>,
    pub print_status: Option<PrintStatus>,
    pub tags: Option<Vec<String>>,
    pub tag_mode: Option<TagMode>,
    pub folder_id: Option<String>,
    pub parent_model_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionedModelUpdate {
    pub model_id: String,
    pub expected_version: u64,
    pub patch: MetadataPatch,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionedTarget {
    pub model_id: String,
    pub expected_version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelFilamentAssignment {
    pub target: VersionedTarget,
    pub filament_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewedTag {
    pub name: String,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewedTagTarget {
    pub model_id: String,
    pub expected_version: u64,
    pub tags: Vec<ReviewedTag>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewedTagStrategy {
    Append,
    Replace,
    Merge,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tag {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrintOutcome {
    Printed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrintAttemptInput {
    pub printed_at: String,
    pub quantity: u32,
    pub notes: String,
    pub filament_ids: Vec<String>,
    pub outcome: PrintOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
pub struct IndexRoot {
    pub id: String,
    pub path: PathBuf,
    pub enabled: bool,
    pub last_scanned_at: Option<String>,
    pub scan_interval_minutes: u32,
    pub max_file_bytes: u64,
    pub scan_zip: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScannedModel {
    pub relative_path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub format: String,
    pub modified_at: u64,
    pub display_name: Option<String>,
    pub archive_member: Option<String>,
    pub outer_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SourceRef {
    pub relative_path: PathBuf,
    pub archive_member: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedModelSource {
    pub outer_path: PathBuf,
    pub archive_member: Option<String>,
    pub outer_fingerprint: Option<String>,
    pub content_hash: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexedSourceMetadata {
    pub bytes: u64,
    pub modified_at: u64,
    pub content_hash: String,
    pub archive_member: Option<String>,
    pub outer_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogSettings {
    pub thumbnail_size: u32,
    pub thumbnail_background: String,
    pub thumbnail_concurrency: u32,
    pub scan_batch_size: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThumbnailCacheEntry {
    pub path: PathBuf,
    pub png_hash: String,
    pub is_custom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThumbnailGalleryItem {
    pub id: String,
    pub model_id: String,
    pub relative_path: String,
    pub content_hash: String,
    pub source_content_hash: Option<String>,
    pub is_custom: bool,
    pub ordinal: u16,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapturePageInput {
    pub url: String,
    pub title: String,
    pub designer: Option<String>,
    pub model_license: Option<String>,
    pub notes: Option<String>,
    pub parent_external_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureDownloadInput {
    pub capture_id: String,
    pub file_name: String,
    pub sha256: Option<String>,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureReceiptInput {
    pub item_id: String,
    pub payload_hash: String,
    pub created_at: String,
    pub page: CapturePageInput,
    pub downloads: Vec<CaptureDownloadInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureDownloadRecord {
    pub capture_id: String,
    pub status: String,
    pub model_id: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureReceipt {
    pub item_id: String,
    pub payload_hash: String,
    pub status: String,
    pub page_model_id: Option<String>,
    pub downloads: Vec<CaptureDownloadRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureSummary {
    pub item_id: String,
    pub created_at: String,
    pub status: String,
    pub page_model_id: Option<String>,
    pub title: String,
    pub url: String,
    pub download_count: u64,
    pub failed_count: u64,
    pub needs_file_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub content_hash: String,
    pub models: Vec<Model>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemovalResult {
    pub snapshot_id: String,
    pub removed_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemovedModel {
    pub model: Model,
    pub removed_at: String,
    pub snapshot_id: String,
}

impl Default for CatalogSettings {
    fn default() -> Self {
        Self {
            thumbnail_size: 256,
            thumbnail_background: "#12171F".into(),
            thumbnail_concurrency: 2,
            scan_batch_size: 128,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScanReconcile {
    pub seen: usize,
    pub added: usize,
    pub updated: usize,
    pub renamed: usize,
    pub missing: usize,
    pub added_model_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportResult {
    pub model: Model,
    pub reused_blob: bool,
}
