mod db;
pub mod error;
pub mod models;

pub use db::{
    LibraryDb, CURRENT_SCHEMA_VERSION, MAX_INDEXED_FILE_BYTES, MAX_SCAN_INTERVAL_MINUTES,
    MIN_SCAN_INTERVAL_MINUTES, ROOT_FOLDER_ID,
};
pub use error::LibraryError;
pub use models::{
    CatalogSettings, DuplicateGroup, Filament, FilamentInput, FilamentOrigin, FilamentResourceKind,
    FilamentSyncSummary, Folder, ImportResult, IndexRoot, IndexedSourceMetadata, MetadataPatch,
    Model, ModelFilamentAssignment, ModelMetadata, PrintAttempt, PrintAttemptInput, PrintOutcome,
    PrintStatus, RemovalResult, RemovedModel, ResolvedModelSource, ReviewedTag,
    ReviewedTagStrategy, ReviewedTagTarget, ScanReconcile, ScannedModel, SourceRef,
    SpoolmanFilamentRecord, SpoolmanSource, SpoolmanSpoolRecord, Tag, TagMode, ThumbnailCacheEntry,
    VersionedModelUpdate, VersionedTarget,
};
