use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LibraryError {
    #[error("library I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("library database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("library schema version {found} is newer than supported version {supported}")]
    SchemaTooNew { found: i64, supported: i64 },
    #[error("folder not found: {0}")]
    FolderNotFound(String),
    #[error("folder cannot contain itself or one of its descendants")]
    FolderCycle,
    #[error("folder is not empty")]
    FolderNotEmpty,
    #[error("the root folder cannot be renamed or deleted")]
    RootFolderMutation,
    #[error("model not found: {0}")]
    ModelNotFound(String),
    #[error("index root not found: {0}")]
    IndexRootNotFound(String),
    #[error("source file changed while it was being imported")]
    SourceChanged,
    #[error("model {model_id} metadata version conflict: expected {expected}, found {actual}")]
    MetadataConflict {
        model_id: String,
        expected: u64,
        actual: u64,
    },
    #[error("filament {filament_id} version conflict: expected {expected}, found {actual}")]
    FilamentVersionConflict {
        filament_id: String,
        expected: u64,
        actual: u64,
    },
    #[error("part {part_id} version conflict: expected {expected}, found {actual}")]
    PartStockVersionConflict {
        part_id: String,
        expected: u64,
        actual: u64,
    },
    #[error("idempotency key conflicts with another payload: {0}")]
    IdempotencyConflict(String),
    #[error("unsupported library format: {0}")]
    UnsupportedFormat(String),
    #[error("invalid library input: {0}")]
    InvalidInput(String),
    #[error("STL validation failed: {0}")]
    Stl(#[from] stl_core::StlError),
}

pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> LibraryError {
    LibraryError::Io {
        path: path.into(),
        source,
    }
}
