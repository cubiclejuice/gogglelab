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
    pub thumbnail_mode: Option<String>,
    pub source_directory: Option<String>,
    pub minimum_rating: Option<u8>,
    pub favorite: Option<bool>,
    #[serde(default)]
    pub tag_names: Vec<String>,
    pub file_type: Option<String>,
    #[serde(default)]
    pub exclude_model_ids: Vec<String>,
}

pub const MAX_MODEL_PAGE_OFFSET: u32 = 100_000;

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
            thumbnail_mode: None,
            source_directory: None,
            minimum_rating: None,
            favorite: None,
            tag_names: Vec::new(),
            file_type: None,
            exclude_model_ids: Vec::new(),
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
#[serde(default)]
pub struct ModelMetadata {
    pub designer: Option<String>,
    pub source_url: Option<String>,
    pub notes: Option<String>,
    pub model_license: Option<String>,
    pub print_status: PrintStatus,
    pub parent_model_id: Option<String>,
    pub last_printed_at: Option<String>,
    pub print_count: u64,
    pub rating: u8,
    pub favorite: bool,
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
    pub rating: Option<u8>,
    pub favorite: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionedModelUpdate {
    pub model_id: String,
    pub expected_version: u64,
    pub patch: MetadataPatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filament_ids: Option<Vec<String>>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionedTarget {
    pub model_id: String,
    pub expected_version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexedModelMove {
    pub model_id: String,
    pub expected_version: u64,
    pub destination_root_id: String,
    pub destination_relative_path: PathBuf,
    pub modified_at: u64,
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

/// User-managed consumable hardware inventory, distinct from 3D geometry parts.
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
#[serde(rename_all = "snake_case")]
pub enum PrintOutcome {
    Printed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrintAttemptInput {
    pub printed_at: String,
    pub quantity: u32,
    pub notes: String,
    pub filament_ids: Vec<String>,
    pub outcome: PrintOutcome,
    #[serde(default, skip_serializing_if = "part_usages_absent_or_empty")]
    pub parts: Option<Vec<PartUsageInput>>,
}

fn part_usages_absent_or_empty(value: &Option<Vec<PartUsageInput>>) -> bool {
    value.as_ref().map_or(true, Vec::is_empty)
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
pub struct PartUsageSnapshot {
    pub name: String,
    pub category: Option<String>,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrintAttemptPart {
    /// `None` means the part was deleted; its historical snapshot remains.
    pub id: Option<String>,
    pub name: String,
    pub category: Option<String>,
    pub unit: String,
    pub quantity_per_copy: u32,
    /// Total inventory consumed by this event across all copies.
    pub quantity: u32,
}

/// Compatibility input for print attempts that can optionally identify a
/// locally managed printer. The nested option preserves the wire distinction
/// needed by updates: an omitted `printer_id` keeps existing attribution and
/// an explicit JSON `null` clears it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

/// A single untrusted compatibility-history row whose printer reference has
/// already been resolved to a local profile (or retained as a display-name
/// snapshot only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrintAttemptImportItem {
    pub input: PrintAttemptInput,
    pub created_at: Option<String>,
    pub printer_id: Option<String>,
    pub printer_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrintAttemptImportResult {
    pub added_count: usize,
    pub skipped_count: usize,
    pub metadata_version: u64,
}

fn deserialize_present_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub printer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub printer_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
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

/// Catalog-owned printer record. `host` and `web_url` contain only endpoint
/// locations; this contract deliberately has no credential or secret fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Printer {
    pub id: String,
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
    pub print_count: u64,
    pub last_printed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
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

/// Legacy settings records are imported once into the catalog without treating
/// their endpoints as approved connection targets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LegacyPrinter {
    pub id: String,
    pub name: String,
    pub bed_mm: [f32; 3],
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LegacyPrinterImportSummary {
    pub inserted: usize,
    pub already_present: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterMaintenanceInput {
    pub title: String,
    pub performed_at: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterMaintenanceLog {
    pub id: String,
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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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

/// A bounded, user-reviewed import of printer profiles and their history.
/// External references identify records only within the imported document.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterDataImport {
    #[serde(default)]
    pub printers: Vec<PrinterDataImportPrinter>,
    #[serde(default)]
    pub maintenance_logs: Vec<PrinterDataImportMaintenanceLog>,
    #[serde(default)]
    pub reminders: Vec<PrinterDataImportReminder>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterDataImportPrinter {
    pub external_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_to_local_printer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub input: PrinterInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterDataImportMaintenanceLog {
    pub external_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub printer_external_ref: Option<String>,
    /// Historical name snapshot from the source document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub printer_nickname: Option<String>,
    pub input: PrinterMaintenanceInput,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterDataImportReminder {
    pub external_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub printer_external_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub printer_nickname: Option<String>,
    pub input: PrinterReminderInput,
    pub status: PrinterReminderStatus,
    #[serde(default)]
    pub last_completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterImportRefMapEntry {
    pub external_ref: String,
    pub local_printer_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrinterDataImportResult {
    pub printer_ref_map: Vec<PrinterImportRefMapEntry>,
    /// Newly created local printer profiles; mapped existing profiles are counted separately.
    pub imported_printer_count: usize,
    pub mapped_printer_count: usize,
    pub imported_maintenance_log_count: usize,
    pub imported_reminder_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrintAttemptBatchItem {
    pub model_id: String,
    pub input: PrintAttemptInput,
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

/// Canonical relative thumbnail text without platform-dependent PathBuf pushes.
/// Windows accepts either native separator; Unix never treats a literal backslash
/// as a delimiter. This is lexical read validation, not filesystem authority.
pub fn normalize_thumbnail_catalog_path(value: &str, windows_separators: bool) -> Option<String> {
    if value.is_empty()
        || value.len() > 4096
        || value.contains([':', '\0'])
        || (!windows_separators && value.contains('\\'))
    {
        return None;
    }
    let parts = value
        .split(|c| c == '/' || (windows_separators && c == '\\'))
        .collect::<Vec<_>>();
    if parts.len() < 2
        || parts[0] != "thumbnails"
        || parts.iter().any(|part| {
            part.is_empty()
                || matches!(*part, "." | "..")
                || (windows_separators
                    && (part.ends_with('.')
                        || part.ends_with(' ')
                        || windows_thumbnail_device_component(part)))
        })
    {
        return None;
    }
    Some(parts.join("/"))
}

fn windows_thumbnail_device_component(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|port| {
            matches!(
                port,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

/// Read-only catalog selection and settings from one database snapshot.
/// This descriptor conveys neither generated-file provenance nor deletion authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThumbnailReadSnapshot {
    pub relative_path: PathBuf,
    pub png_hash: String,
    pub source_content_hash: Option<String>,
    pub is_custom: bool,
    pub current_content_hash: Option<String>,
    pub settings: CatalogSettings,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogHashCounts {
    pub total: u64,
    pub hashed: u64,
    pub unhashed: u64,
    pub bookmarks: u64,
    pub missing: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
pub struct CatalogFacetValue {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogFacets {
    pub designers: Vec<String>,
    pub licenses: Vec<String>,
    pub categories: Vec<String>,
    pub parents: Vec<CatalogFacetValue>,
    pub folder_ids: Vec<String>,
    pub root_ids: Vec<String>,
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

#[cfg(test)]
mod thumbnail_path_tests {
    use super::normalize_thumbnail_catalog_path;
    #[test]
    fn catalog_thumbnail_path_modes_normalize_native_windows_delimiters_safely() {
        for path in [
            r"thumbnails\nested\preview.png",
            r"thumbnails\nested/preview.png",
            "thumbnails/nested/preview.png",
        ] {
            assert_eq!(
                normalize_thumbnail_catalog_path(path, true).as_deref(),
                Some("thumbnails/nested/preview.png")
            );
        }
        assert!(
            normalize_thumbnail_catalog_path(r"thumbnails\nested\preview.png", false).is_none()
        );
        assert_eq!(
            normalize_thumbnail_catalog_path("thumbnails/nested/preview.png", false).as_deref(),
            Some("thumbnails/nested/preview.png")
        );
        for path in [
            r"\thumbnails\a.png",
            r"\\server\thumbnails\a.png",
            r"C:\thumbnails\a.png",
            r"thumbnails\..\a.png",
            r"thumbnails/..\a.png",
            r"thumbnails\.. \a.png",
            r"thumbnails\nested.\a.png",
            r"thumbnails\a.png:stream",
            r"thumbnails\\a.png",
            r"thumbnails\.\a.png",
            r"thumbnails\a.png\",
            "/thumbnails/a.png",
            "thumbnails/a\0.png",
        ] {
            assert!(
                normalize_thumbnail_catalog_path(path, true).is_none(),
                "{path}"
            );
        }
    }
    #[test]
    fn catalog_thumbnail_windows_mode_rejects_device_stems_without_restricting_unicode() {
        for name in [
            "CON",
            "con.png",
            "NUL",
            "aux.PNG",
            "PRN.zip",
            "COM1",
            "COM9.png",
            "LPT1",
            "lpt9.PNG",
            "CONIN$.png",
            "CONOUT$",
            "CLOCK$",
            "COM¹.png",
            "LPT².png",
            "NUL .png",
        ] {
            let path = format!("thumbnails/{name}");
            assert!(
                normalize_thumbnail_catalog_path(&path, true).is_none(),
                "{name}"
            );
            assert!(
                normalize_thumbnail_catalog_path(&path, false).is_some(),
                "Unix literal {name}"
            );
            assert!(
                normalize_thumbnail_catalog_path(&format!("thumbnails/{name}/preview.png"), true)
                    .is_none(),
                "nested {name}"
            );
        }
        for path in [
            "thumbnails/模型/café.png",
            "thumbnails/COM10.png",
            "thumbnails/conifer.png",
        ] {
            assert_eq!(
                normalize_thumbnail_catalog_path(path, true).as_deref(),
                Some(path)
            );
        }
    }
}
