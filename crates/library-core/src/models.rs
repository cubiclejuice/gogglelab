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
    pub source_kind: String,
    pub source_path: Option<String>,
    pub source_root_id: Option<String>,
    pub source_missing: bool,
    pub source_modified_at: Option<u64>,
    pub thumbnail_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexRoot {
    pub id: String,
    pub path: PathBuf,
    pub enabled: bool,
    pub last_scanned_at: Option<String>,
    pub scan_interval_minutes: u32,
    pub max_file_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScannedModel {
    pub relative_path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub format: String,
    pub modified_at: u64,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexedSourceMetadata {
    pub bytes: u64,
    pub modified_at: u64,
    pub content_hash: String,
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
