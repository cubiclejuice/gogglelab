use serde::{Deserialize, Serialize};

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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportResult {
    pub model: Model,
    pub reused_blob: bool,
}
