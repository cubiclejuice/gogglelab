use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{io, LibraryError};
use crate::models::{
    CaptureDownloadRecord, CaptureReceipt, CaptureReceiptInput, CaptureSummary, CatalogFacetValue,
    CatalogFacets, CatalogSettings, CatalogStats, DuplicateGroup, Filament, FilamentInput,
    FilamentOrigin, FilamentResourceKind, FilamentSyncSummary, Folder, ImportResult, IndexRoot,
    IndexedModelMove, IndexedSourceMetadata, MetadataPatch, Model, ModelFilamentAssignment,
    ModelGroupFilter, ModelMetadata, ModelPageQuery, ModelSort, PrintAttempt,
    PrintAttemptBatchItem, PrintAttemptInput, PrintOutcome, PrintStatus, RemovalResult,
    RemovedModel, ResolvedModelSource, ReviewedTagStrategy, ReviewedTagTarget, ScanReconcile,
    ScannedModel, SortDirection, SourceRef, SpoolmanFilamentRecord, SpoolmanSource,
    SpoolmanSpoolRecord, Tag, TagMode, ThumbnailCacheEntry, ThumbnailGalleryItem,
    VersionedModelUpdate, VersionedTarget, MAX_MODEL_PAGE_OFFSET,
};

pub const CURRENT_SCHEMA_VERSION: i64 = 6;
const MAX_GLOBAL_METADATA_TARGETS: i64 = 50_000;
pub const ROOT_FOLDER_ID: &str = "root";
pub const MIN_SCAN_INTERVAL_MINUTES: u32 = 1;
pub const MAX_SCAN_INTERVAL_MINUTES: u32 = 1_440;
pub const MAX_INDEXED_FILE_BYTES: u64 = stl_core::limits::MAX_FILE_BYTES;

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);
const MAX_CACHED_THUMBNAIL_BYTES: u64 = 4 * 1024 * 1024;

// Internal SQLite projection shared by managed and indexed source resolution.
type SourceResolutionRow = (
    String,
    Option<String>,
    Option<String>,
    bool,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
);

pub struct LibraryDb {
    conn: Connection,
    root: PathBuf,
}

impl LibraryDb {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, LibraryError> {
        let root = root.as_ref().to_path_buf();
        match std::fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(LibraryError::InvalidInput(
                    "library root must be a real directory".into(),
                ))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(&root).map_err(|e| io(&root, e))?
            }
            Err(error) => return Err(io(&root, error)),
        }
        let root = root.canonicalize().map_err(|error| io(&root, error))?;
        for dir in ["originals", "manuals", "thumbnails", "staging", "trash"] {
            ensure_owned_directory(&root, dir)?;
        }
        let db_path = root.join("library.sqlite3");
        match std::fs::symlink_metadata(&db_path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(LibraryError::InvalidInput(
                    "library database must be a regular owned file".into(),
                ))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(&db_path, error)),
        }
        let conn = Connection::open(&db_path)?;
        let opened_db = std::fs::symlink_metadata(&db_path).map_err(|error| io(&db_path, error))?;
        if opened_db.file_type().is_symlink()
            || !opened_db.is_file()
            || db_path
                .canonicalize()
                .map_err(|error| io(&db_path, error))?
                .parent()
                != Some(root.as_path())
        {
            return Err(LibraryError::InvalidInput(
                "library database identity changed".into(),
            ));
        }
        Self::initialize(conn, root)
    }

    pub fn open_in_memory() -> Result<Self, LibraryError> {
        Self::initialize(Connection::open_in_memory()?, PathBuf::from("."))
    }

    fn initialize(conn: Connection, root: PathBuf) -> Result<Self, LibraryError> {
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > CURRENT_SCHEMA_VERSION {
            return Err(LibraryError::SchemaTooNew {
                found: version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        if version == 0 {
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
        } else if version == 1 {
            create_migration_backup(&conn, &root, version)?;
            conn.execute_batch(MIGRATE_V1_TO_V2)?;
            conn.execute_batch(MIGRATE_V2_TO_V3)?;
            conn.execute_batch(MIGRATE_V3_TO_V4)?;
            conn.execute_batch(MIGRATE_V4_TO_V5)?;
            conn.execute_batch(MIGRATE_V5_TO_V6)?;
        } else if version == 2 {
            create_migration_backup(&conn, &root, version)?;
            conn.execute_batch(MIGRATE_V2_TO_V3)?;
            conn.execute_batch(MIGRATE_V3_TO_V4)?;
            conn.execute_batch(MIGRATE_V4_TO_V5)?;
            conn.execute_batch(MIGRATE_V5_TO_V6)?;
        } else if version == 3 {
            create_migration_backup(&conn, &root, version)?;
            conn.execute_batch(MIGRATE_V3_TO_V4)?;
            conn.execute_batch(MIGRATE_V4_TO_V5)?;
            conn.execute_batch(MIGRATE_V5_TO_V6)?;
        } else if version == 4 {
            create_migration_backup(&conn, &root, version)?;
            conn.execute_batch(MIGRATE_V4_TO_V5)?;
            conn.execute_batch(MIGRATE_V5_TO_V6)?;
        } else if version == 5 {
            create_migration_backup(&conn, &root, version)?;
            conn.execute_batch(MIGRATE_V5_TO_V6)?;
        }
        conn.execute_batch(ENSURE_CATALOG_SETTINGS)?;
        conn.execute_batch(ENSURE_PRINT_BATCH_RECEIPTS)?;
        Ok(Self { conn, root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn total_changes(&self) -> u64 {
        self.conn.total_changes()
    }

    pub fn create_model(
        &self,
        display_name: &str,
        folder_id: Option<&str>,
    ) -> Result<Model, LibraryError> {
        let folder_id = folder_id.unwrap_or(ROOT_FOLDER_ID);
        self.ensure_folder(folder_id)?;
        let display_name = clean_name(display_name, "Model")?;
        let id = new_id("model");
        let now = timestamp();
        self.conn.execute(
            "INSERT INTO models (id, display_name, description, folder_id, metadata_version, created_at, updated_at)
             VALUES (?1, ?2, '', ?3, 1, ?4, ?4)",
            params![id, display_name, folder_id, now],
        )?;
        self.get_model(&id)?.ok_or(LibraryError::ModelNotFound(id))
    }

    pub fn get_model(&self, id: &str) -> Result<Option<Model>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare(&format!("{MODEL_SELECT} WHERE m.id = ?1"))?;
        let model = stmt.query_row(params![id], model_from_row).optional()?;
        if let Some(mut model) = model {
            model.tags = self.tags_for(&model.id)?;
            model.filament_ids = self.filament_ids_for_model(&model.id)?;
            Ok(Some(model))
        } else {
            Ok(None)
        }
    }

    pub fn resolve_parent_model_id(&self, value: &str) -> Result<String, LibraryError> {
        let value = clean_name(value, "Parent model")?;
        let id_match: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM models WHERE id=?1 AND deleted_at IS NULL",
                params![value],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = id_match {
            return Ok(id);
        }
        let ids = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM models WHERE deleted_at IS NULL AND lower(display_name)=lower(?1)
                 ORDER BY id LIMIT 2",
            )?;
            let rows = stmt.query_map(params![value], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        match ids.as_slice() {
            [id] => Ok(id.clone()),
            [] => Err(LibraryError::InvalidInput(
                "embedded parent model does not match an active catalog model".into(),
            )),
            _ => Err(LibraryError::InvalidInput(
                "embedded parent model name is ambiguous".into(),
            )),
        }
    }

    pub fn list_models(
        &self,
        folder_id: Option<&str>,
        query: Option<&str>,
    ) -> Result<Vec<Model>, LibraryError> {
        let query = query.map(str::trim).filter(|q| !q.is_empty());
        let mut stmt = self.conn.prepare(
            &format!(
                "{MODEL_SELECT} WHERE m.deleted_at IS NULL
                 AND (?1 IS NULL OR m.folder_id = ?1)
                 AND (?2 IS NULL OR lower(m.display_name) LIKE '%' || lower(?2) || '%'
                      OR EXISTS (SELECT 1 FROM model_tags mt JOIN tags t ON t.id = mt.tag_id
                                 WHERE mt.model_id = m.id AND lower(t.name) LIKE '%' || lower(?2) || '%'))
                 ORDER BY m.created_at, m.id"
            ),
        )?;
        let rows = stmt.query_map(params![folder_id, query], model_from_row)?;
        let mut models = Vec::new();
        for row in rows {
            models.push(row?);
        }
        if models.is_empty() {
            return Ok(models);
        }
        let mut tags = self.tags_by_model()?;
        let mut filaments = self.filaments_by_model()?;
        for model in &mut models {
            model.tags = tags.remove(&model.id).unwrap_or_default();
            model.filament_ids = filaments.remove(&model.id).unwrap_or_default();
        }
        Ok(models)
    }

    pub fn list_models_page(
        &self,
        query: &ModelPageQuery,
        after_sort_value: Option<&str>,
        after_id: Option<&str>,
        limit: u16,
    ) -> Result<(Vec<Model>, bool), LibraryError> {
        self.list_models_page_with_offset(query, after_sort_value, after_id, limit, 0)
    }

    pub fn list_models_page_with_offset(
        &self,
        query: &ModelPageQuery,
        after_sort_value: Option<&str>,
        after_id: Option<&str>,
        limit: u16,
        offset: u32,
    ) -> Result<(Vec<Model>, bool), LibraryError> {
        if limit == 0
            || limit > 500
            || offset > MAX_MODEL_PAGE_OFFSET
            || after_sort_value.is_some() != after_id.is_some()
            || (offset > 0 && after_sort_value.is_some())
        {
            return Err(LibraryError::InvalidInput(
                "model page request is invalid".into(),
            ));
        }
        if query
            .thumbnail_mode
            .as_deref()
            .is_some_and(|value| !matches!(value, "missing" | "default" | "custom"))
        {
            return Err(LibraryError::InvalidInput(
                "Thumbnail filter is invalid".into(),
            ));
        }
        if let Some(directory) = query.source_directory.as_deref() {
            if query.root_id.is_none()
                || directory.len() > 1024
                || directory.contains(['\\', '\0'])
                || (!directory.is_empty()
                    && directory
                        .split('/')
                        .any(|part| part.is_empty() || matches!(part, "." | "..")))
            {
                return Err(LibraryError::InvalidInput(
                    "Source directory must be relative to a selected root".into(),
                ));
            }
        }
        if query.minimum_rating.is_some_and(|rating| rating > 5) {
            return Err(LibraryError::InvalidInput(
                "Minimum rating must be between 0 and 5".into(),
            ));
        }
        if query.exclude_model_ids.len() > 2000 {
            return Err(LibraryError::InvalidInput(
                "Thumbnail exclusion list exceeds 2000 model IDs".into(),
            ));
        }
        if query.tag_names.len() > 100 {
            return Err(LibraryError::InvalidInput(
                "Tag filter exceeds 100 names".into(),
            ));
        }
        let tag_names = query
            .tag_names
            .iter()
            .map(|name| name.trim().to_owned())
            .collect::<Vec<_>>();
        let mut unique_tag_names = HashSet::new();
        if tag_names.iter().any(|name| {
            name.is_empty() || name.len() > 256 || !unique_tag_names.insert(name.to_lowercase())
        }) {
            return Err(LibraryError::InvalidInput(
                "Tag filter contains an invalid or duplicate name".into(),
            ));
        }
        let file_type = query.file_type.as_deref().map(|value| {
            value
                .trim()
                .strip_prefix('.')
                .unwrap_or(value.trim())
                .to_ascii_lowercase()
        });
        if query.file_type.as_deref().is_some_and(|value| {
            let extension = value.trim().strip_prefix('.').unwrap_or(value.trim());
            extension.is_empty()
                || extension.len() > 16
                || !extension
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-'))
        }) {
            return Err(LibraryError::InvalidInput(
                "File type must be a short extension".into(),
            ));
        }
        let mut excluded = HashSet::new();
        if query
            .exclude_model_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 128 || !excluded.insert(id.as_str()))
        {
            return Err(LibraryError::InvalidInput(
                "Thumbnail exclusion list contains an invalid or duplicate model ID".into(),
            ));
        }
        let exclude_model_ids = serde_json::to_string(&query.exclude_model_ids)
            .map_err(|_| LibraryError::InvalidInput("Thumbnail exclusions are invalid".into()))?;
        let tag_names = serde_json::to_string(&tag_names)
            .map_err(|_| LibraryError::InvalidInput("Tag filter is invalid".into()))?;
        let search = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let print_status = query.print_status.as_ref().map(|status| match status {
            PrintStatus::Unprinted => "unprinted",
            PrintStatus::Want => "want",
            PrintStatus::Queued => "queued",
            PrintStatus::Printing => "printing",
            PrintStatus::Printed => "printed",
            PrintStatus::Failed => "failed",
        });
        let group = query.group.map(|group| match group {
            ModelGroupFilter::Standalone => 1_i64,
            ModelGroupFilter::Bundle => 2,
            ModelGroupFilter::Archive => 3,
        });
        let (sort_expression, cursor_expression) = match query.sort {
            ModelSort::Name => ("lower(m.display_name)", "?16"),
            ModelSort::Size => ("m.bytes", "CAST(?16 AS INTEGER)"),
            ModelSort::Date => ("m.created_at", "?16"),
        };
        let comparison = match query.direction {
            SortDirection::Asc => ">",
            SortDirection::Desc => "<",
        };
        let direction = match query.direction {
            SortDirection::Asc => "ASC",
            SortDirection::Desc => "DESC",
        };
        let mut stmt = self.conn.prepare(&format!(
            "{MODEL_SELECT} WHERE m.deleted_at IS NULL
             AND (?1 IS NULL OR m.folder_id=?1)
             AND (?2 IS NULL OR lower(m.display_name) LIKE '%' || lower(?2) || '%'
                  OR EXISTS (SELECT 1 FROM model_tags mt JOIN tags t ON t.id=mt.tag_id
                             WHERE mt.model_id=m.id AND lower(t.name) LIKE '%' || lower(?2) || '%'))
             AND (?3 IS NULL OR EXISTS (SELECT 1 FROM model_tags mt
                                         WHERE mt.model_id=m.id AND mt.tag_id=?3))
             AND (?4 IS NULL OR EXISTS (SELECT 1 FROM model_tags mt JOIN tags t ON t.id=mt.tag_id
                                         WHERE mt.model_id=m.id AND t.category=?4))
             AND (?5 IS NULL OR m.designer=?5)
             AND (?6 IS NULL OR m.model_license=?6)
             AND (?7 IS NULL OR m.print_status=?7)
             AND (?8 IS NULL OR (?8=1 AND m.print_count>0) OR (?8=0 AND m.print_count=0))
             AND (?9 IS NULL OR m.bundle_id=?9)
             AND (?10 IS NULL OR m.source_root_id=?10)
             AND (?11 IS NULL OR EXISTS (SELECT 1 FROM model_filaments mf
                                          WHERE mf.model_id=m.id AND mf.filament_id=?11))
             AND (?12 IS NULL OR m.source_missing=?12)
             AND (?13=0 OR (COALESCE(r.blob_hash,d.content_hash) IS NOT NULL AND EXISTS (
                  SELECT 1 FROM models duplicate
                  LEFT JOIN revisions duplicate_revision ON duplicate_revision.id=duplicate.current_revision_id
                  LEFT JOIN indexed_model_data duplicate_data ON duplicate_data.model_id=duplicate.id
                  WHERE duplicate.deleted_at IS NULL AND duplicate.id!=m.id
                  AND COALESCE(duplicate_revision.blob_hash,duplicate_data.content_hash)
                      =COALESCE(r.blob_hash,d.content_hash))))
             AND (?14 IS NULL OR m.source_kind=?14)
             AND (?15 IS NULL OR (?15=1 AND m.bundle_id IS NULL AND m.source_archive_member IS NULL)
                  OR (?15=2 AND m.bundle_id IS NOT NULL)
                  OR (?15=3 AND m.source_archive_member IS NOT NULL))
             AND (?19 IS NULL
                  OR (?19='missing' AND NOT EXISTS (SELECT 1 FROM thumbnails tf
                       WHERE tf.model_id=m.id AND (tf.is_custom=1 OR tf.source_content_hash IS NULL
                       OR tf.source_content_hash=COALESCE(r.blob_hash,d.content_hash))))
                  OR (?19 IN ('default','custom') AND
                       (SELECT tf.is_custom FROM thumbnails tf WHERE tf.model_id=m.id
                        AND (tf.is_custom=1 OR tf.source_content_hash IS NULL
                        OR tf.source_content_hash=COALESCE(r.blob_hash,d.content_hash))
                        ORDER BY tf.is_active DESC,tf.is_custom DESC,tf.ordinal,tf.id LIMIT 1)
                       =CASE WHEN ?19='custom' THEN 1 ELSE 0 END))
             AND (?20 IS NULL OR (m.source_kind='indexed'
                  AND (?20='' OR substr(m.source_path,1,length(?20)+1)=?20||'/')))
             AND (?21 IS NULL OR m.rating>=?21)
             AND (?22 IS NULL OR m.favorite=?22)
             AND NOT EXISTS (
                 SELECT 1 FROM json_each(?23) excluded WHERE excluded.value=m.id
             )
             AND NOT EXISTS (
                 SELECT 1 FROM json_each(?24) requested_tag
                 WHERE NOT EXISTS (
                     SELECT 1 FROM model_tags mt JOIN tags t ON t.id=mt.tag_id
                     WHERE mt.model_id=m.id AND t.name=requested_tag.value
                 )
             )
             AND (?25 IS NULL OR (?25='stl' AND COALESCE(r.format,d.format) IN ('stl-binary','stl-ascii'))
                  OR lower(COALESCE(r.format,d.format))=?25
                  OR lower(COALESCE(r.original_name,d.original_name)) LIKE '%.'||?25
                  OR (m.source_kind='indexed' AND lower(m.source_path) LIKE '%.'||?25))
             AND (?16 IS NULL OR {sort_expression}{comparison}{cursor_expression}
                  OR ({sort_expression}={cursor_expression} AND m.id{comparison}?17))
             ORDER BY {sort_expression} {direction},m.id {direction} LIMIT ?18 OFFSET ?26"
        ))?;
        let rows = stmt.query_map(
            params![
                query.folder_id.as_deref(),
                search,
                query.tag_id.as_deref(),
                query.category.as_deref(),
                query.designer.as_deref(),
                query.model_license.as_deref(),
                print_status,
                query.ever_printed.map(i64::from),
                query.bundle_id.as_deref(),
                query.root_id.as_deref(),
                query.filament_id.as_deref(),
                query.source_missing.map(i64::from),
                i64::from(query.duplicates_only),
                query.source_kind.as_deref(),
                group,
                after_sort_value,
                after_id,
                i64::from(limit) + 1,
                query.thumbnail_mode.as_deref(),
                query.source_directory.as_deref(),
                query.minimum_rating.map(i64::from),
                query.favorite.map(i64::from),
                exclude_model_ids,
                tag_names,
                file_type,
                i64::from(offset)
            ],
            model_from_row,
        )?;
        let mut models = rows.collect::<Result<Vec<_>, _>>()?;
        let has_more = models.len() > usize::from(limit);
        models.truncate(usize::from(limit));
        for model in &mut models {
            model.tags = self.tags_for(&model.id)?;
            model.filament_ids = self.filament_ids_for_model(&model.id)?;
        }
        Ok((models, has_more))
    }

    pub fn list_bundle_models(&self, bundle_id: &str) -> Result<Vec<Model>, LibraryError> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM bundles WHERE id=?1)",
            params![bundle_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(LibraryError::InvalidInput("bundle not found".into()));
        }
        let mut stmt=self.conn.prepare(&format!("{MODEL_SELECT} WHERE m.bundle_id=?1 AND m.deleted_at IS NULL ORDER BY lower(m.display_name),m.id"))?;
        let rows = stmt.query_map(params![bundle_id], model_from_row)?;
        let mut models = rows.collect::<Result<Vec<_>, _>>()?;
        for model in &mut models {
            model.tags = self.tags_for(&model.id)?;
            model.filament_ids = self.filament_ids_for_model(&model.id)?;
        }
        Ok(models)
    }

    pub fn duplicate_groups(
        &self,
        model_ids: &[String],
    ) -> Result<Vec<DuplicateGroup>, LibraryError> {
        if model_ids.len() > 1_000 {
            return Err(LibraryError::InvalidInput(
                "duplicate scope exceeds 1000 models".into(),
            ));
        }
        let mut unique = HashSet::new();
        let mut groups: BTreeMap<String, Vec<Model>> = BTreeMap::new();
        for id in model_ids {
            if !unique.insert(id) {
                return Err(LibraryError::InvalidInput(
                    "duplicate model ID in scope".into(),
                ));
            }
            let active: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM models WHERE id=?1 AND deleted_at IS NULL)",
                params![id],
                |row| row.get(0),
            )?;
            if !active {
                return Err(LibraryError::ModelNotFound(id.clone()));
            }
            let model = self
                .get_model(id)?
                .ok_or_else(|| LibraryError::ModelNotFound(id.clone()))?;
            if let Some(hash) = model.content_hash.clone() {
                groups.entry(hash).or_default().push(model);
            }
        }
        Ok(groups
            .into_iter()
            .filter_map(|(content_hash, models)| {
                (models.len() > 1).then_some(DuplicateGroup {
                    content_hash,
                    models,
                })
            })
            .collect())
    }

    pub fn catalog_hash_counts(&self) -> Result<crate::models::CatalogHashCounts, LibraryError> {
        self.conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(COALESCE(r.blob_hash,d.content_hash) IS NOT NULL),0),
                    COALESCE(SUM(m.catalog_kind!='bookmark' AND COALESCE(r.blob_hash,d.content_hash) IS NULL),0),
                    COALESCE(SUM(m.catalog_kind='bookmark'),0),
                    COALESCE(SUM(m.source_missing=1),0)
             FROM models m LEFT JOIN revisions r ON r.id=m.current_revision_id
             LEFT JOIN indexed_model_data d ON d.model_id=m.id WHERE m.deleted_at IS NULL",
            [], |row| {
                let count = |index: usize| -> rusqlite::Result<u64> {
                    let value: i64 = row.get(index)?;
                    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
                };
                Ok(crate::models::CatalogHashCounts { total: count(0)?, hashed: count(1)?,
                    unhashed: count(2)?, bookmarks: count(3)?, missing: count(4)? })
            },
        ).map_err(LibraryError::from)
    }

    pub fn catalog_stats(&self) -> Result<CatalogStats, LibraryError> {
        let (total, available, missing, bookmarks, bytes): (i64, i64, i64, i64, i64) =
            self.conn.query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(catalog_kind='file' AND source_missing=0),0),
                        COALESCE(SUM(catalog_kind='file' AND source_missing=1),0),
                        COALESCE(SUM(catalog_kind='bookmark'),0),
                        COALESCE(SUM(CASE WHEN catalog_kind='file' THEN COALESCE(r.bytes,d.bytes,0) ELSE 0 END),0)
                 FROM models m
                 LEFT JOIN revisions r ON r.id=m.current_revision_id
                 LEFT JOIN indexed_model_data d ON d.model_id=m.id
                 WHERE m.deleted_at IS NULL",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )?;
        let bundle_count: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT bundle_id) FROM models
             WHERE deleted_at IS NULL AND bundle_id IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        let duplicate_group_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM (
                 SELECT COALESCE(r.blob_hash,d.content_hash) hash
                 FROM models m
                 LEFT JOIN revisions r ON r.id=m.current_revision_id
                 LEFT JOIN indexed_model_data d ON d.model_id=m.id
                 WHERE m.deleted_at IS NULL AND COALESCE(r.blob_hash,d.content_hash) IS NOT NULL
                 GROUP BY hash HAVING COUNT(*)>1)",
            [],
            |row| row.get(0),
        )?;
        Ok(CatalogStats {
            total_models: total as u64,
            available_models: available as u64,
            missing_models: missing as u64,
            bookmarks: bookmarks as u64,
            total_bytes: bytes as u64,
            bundle_count: bundle_count as u64,
            duplicate_group_count: duplicate_group_count as u64,
        })
    }

    pub fn catalog_facets(&self) -> Result<CatalogFacets, LibraryError> {
        fn strings(conn: &Connection, sql: &str) -> Result<Vec<String>, rusqlite::Error> {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |row| row.get(0))?;
            rows.collect()
        }
        let designers = strings(
            &self.conn,
            "SELECT DISTINCT designer FROM models WHERE deleted_at IS NULL AND designer IS NOT NULL
             ORDER BY lower(designer) LIMIT 5000",
        )?;
        let licenses = strings(
            &self.conn,
            "SELECT DISTINCT model_license FROM models WHERE deleted_at IS NULL AND model_license IS NOT NULL
             ORDER BY lower(model_license) LIMIT 5000",
        )?;
        let categories = strings(
            &self.conn,
            "SELECT DISTINCT category FROM tags WHERE category IS NOT NULL
             ORDER BY lower(category) LIMIT 5000",
        )?;
        let folder_ids = strings(
            &self.conn,
            "SELECT id FROM folders ORDER BY lower(name),id LIMIT 5000",
        )?;
        let root_ids = strings(
            &self.conn,
            "SELECT id FROM index_roots ORDER BY id LIMIT 5000",
        )?;
        let mut stmt = self.conn.prepare(
            "SELECT id,display_name FROM models WHERE deleted_at IS NULL
             ORDER BY lower(display_name),id LIMIT 5000",
        )?;
        let parents = stmt
            .query_map([], |row| {
                Ok(CatalogFacetValue {
                    id: row.get(0)?,
                    label: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CatalogFacets {
            designers,
            licenses,
            categories,
            parents,
            folder_ids,
            root_ids,
        })
    }

    pub fn remove_models_recoverably(
        &self,
        targets: &[VersionedTarget],
    ) -> Result<RemovalResult, LibraryError> {
        self.remove_models_recoverably_with_snapshot_id(targets, &new_id("removal"))
    }

    pub fn remove_models_recoverably_with_snapshot_id(
        &self,
        targets: &[VersionedTarget],
        snapshot_id: &str,
    ) -> Result<RemovalResult, LibraryError> {
        if targets.is_empty() || targets.len() > 50_000 {
            return Err(LibraryError::InvalidInput(
                "removal target count is invalid".into(),
            ));
        }
        if snapshot_id.is_empty()
            || snapshot_id.len() > 128
            || !snapshot_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(LibraryError::InvalidInput(
                "removal snapshot id is invalid".into(),
            ));
        }
        let mut unique = HashSet::new();
        if targets
            .iter()
            .any(|target| !unique.insert(&target.model_id))
        {
            return Err(LibraryError::InvalidInput(
                "duplicate removal target".into(),
            ));
        }
        let now = timestamp();
        let tx = self.conn.unchecked_transaction()?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM jobs WHERE kind='removal' AND status=?1 LIMIT 1",
                params![snapshot_id],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_some() {
            return Err(LibraryError::InvalidInput(
                "removal snapshot id is already in use".into(),
            ));
        }
        for target in targets {
            let row: Option<(i64, Option<String>)> = tx
                .query_row(
                    "SELECT metadata_version,deleted_at FROM models WHERE id=?1",
                    params![target.model_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let (actual, deleted) =
                row.ok_or_else(|| LibraryError::ModelNotFound(target.model_id.clone()))?;
            if actual as u64 != target.expected_version {
                return Err(LibraryError::MetadataConflict {
                    model_id: target.model_id.clone(),
                    expected: target.expected_version,
                    actual: actual as u64,
                });
            }
            if deleted.is_some() {
                return Err(LibraryError::InvalidInput(
                    "model is already removed".into(),
                ));
            }
            tx.execute("UPDATE models SET deleted_at=?1,metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",params![now,target.model_id])?;
            tx.execute(
                "DELETE FROM jobs WHERE model_id=?1 AND kind='removal'",
                params![target.model_id],
            )?;
            tx.execute("INSERT INTO jobs(id,model_id,kind,status,retry_count,created_at,updated_at) VALUES(?1,?2,'removal',?3,0,?4,?4)",params![new_id("removal-item"),target.model_id,snapshot_id,now])?;
        }
        tx.commit()?;
        Ok(RemovalResult {
            snapshot_id: snapshot_id.to_owned(),
            removed_ids: targets
                .iter()
                .map(|target| target.model_id.clone())
                .collect(),
        })
    }

    pub fn list_removed_models(&self) -> Result<Vec<RemovedModel>, LibraryError> {
        let rows = {
            let mut stmt=self.conn.prepare("SELECT m.id,m.deleted_at,j.status FROM models m JOIN jobs j ON j.model_id=m.id AND j.kind='removal' WHERE m.deleted_at IS NOT NULL ORDER BY m.deleted_at DESC,m.id")?;
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        rows.into_iter()
            .map(|(id, removed_at, snapshot_id)| {
                Ok(RemovedModel {
                    model: self
                        .get_model(&id)?
                        .ok_or_else(|| LibraryError::ModelNotFound(id.clone()))?,
                    removed_at,
                    snapshot_id,
                })
            })
            .collect()
    }

    pub fn restore_removal(&self, snapshot_id: &str) -> Result<Vec<Model>, LibraryError> {
        let ids = {
            let mut stmt = self.conn.prepare(
                "SELECT model_id FROM jobs WHERE kind='removal' AND status=?1 ORDER BY model_id",
            )?;
            let rows = stmt.query_map(params![snapshot_id], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if ids.is_empty() {
            return Err(LibraryError::InvalidInput(
                "removal snapshot not found".into(),
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        for id in &ids {
            let deleted: Option<String> = tx
                .query_row(
                    "SELECT deleted_at FROM models WHERE id=?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            if deleted.is_none() {
                return Err(LibraryError::InvalidInput(
                    "removal snapshot conflicts with active model".into(),
                ));
            }
            tx.execute("UPDATE models SET deleted_at=NULL,metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",params![timestamp(),id])?;
        }
        tx.execute(
            "DELETE FROM jobs WHERE kind='removal' AND status=?1",
            params![snapshot_id],
        )?;
        tx.commit()?;
        ids.iter()
            .map(|id| {
                self.get_model(id)?
                    .ok_or_else(|| LibraryError::ModelNotFound(id.clone()))
            })
            .collect()
    }

    pub fn restore_removal_with_owned_source(
        &self,
        snapshot_id: &str,
        model_id: &str,
        content_hash: &str,
        bytes: u64,
        modified_at: u64,
    ) -> Result<Model, LibraryError> {
        validate_hash(content_hash)?;
        let stored_bytes = i64::try_from(bytes)
            .map_err(|_| LibraryError::InvalidInput("source byte count is too large".into()))?;
        let stored_modified_at = i64::try_from(modified_at)
            .map_err(|_| LibraryError::InvalidInput("source timestamp is too large".into()))?;
        let ids = {
            let mut stmt = self.conn.prepare(
                "SELECT model_id FROM jobs WHERE kind='removal' AND status=?1 ORDER BY model_id",
            )?;
            let rows = stmt.query_map(params![snapshot_id], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if ids.len() != 1 || ids[0] != model_id {
            return Err(LibraryError::InvalidInput(
                "owned source recovery does not match removal snapshot".into(),
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        type RecoverySourceRow = (Option<String>, String, Option<String>, String, i64);
        let source: Option<RecoverySourceRow> = tx
            .query_row(
                "SELECT m.deleted_at,m.source_kind,m.source_archive_member,d.content_hash,d.bytes
                 FROM models m JOIN indexed_model_data d ON d.model_id=m.id WHERE m.id=?1",
                params![model_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((deleted_at, source_kind, archive_member, stored_hash, catalog_bytes)) = source
        else {
            return Err(LibraryError::ModelNotFound(model_id.to_owned()));
        };
        if deleted_at.is_none()
            || source_kind != "indexed"
            || archive_member.is_some()
            || stored_hash != content_hash
            || catalog_bytes != stored_bytes
        {
            return Err(LibraryError::InvalidInput(
                "owned source recovery conflicts with catalog".into(),
            ));
        }
        tx.execute(
            "UPDATE models SET deleted_at=NULL,source_missing=0,source_modified_at=?1,
                    metadata_version=metadata_version+1,updated_at=?2 WHERE id=?3",
            params![stored_modified_at, timestamp(), model_id],
        )?;
        tx.execute(
            "DELETE FROM jobs WHERE kind='removal' AND status=?1",
            params![snapshot_id],
        )?;
        tx.commit()?;
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.to_owned()))
    }

    pub fn create_folder(
        &self,
        name: &str,
        parent_id: Option<&str>,
    ) -> Result<Folder, LibraryError> {
        let parent_id = parent_id.unwrap_or(ROOT_FOLDER_ID);
        self.ensure_folder(parent_id)?;
        let name = clean_name(name, "Folder")?;
        let id = new_id("folder");
        self.conn.execute(
            "INSERT INTO folders (id, name, parent_id) VALUES (?1, ?2, ?3)",
            params![id, name, parent_id],
        )?;
        self.get_folder(&id)?
            .ok_or(LibraryError::FolderNotFound(id))
    }

    pub fn rename_folder(&self, id: &str, name: &str) -> Result<Folder, LibraryError> {
        if id == ROOT_FOLDER_ID {
            return Err(LibraryError::RootFolderMutation);
        }
        self.ensure_folder(id)?;
        let name = clean_name(name, "Folder")?;
        self.conn.execute(
            "UPDATE folders SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
        self.get_folder(id)?
            .ok_or_else(|| LibraryError::FolderNotFound(id.to_string()))
    }

    pub fn move_folder(&self, id: &str, new_parent_id: &str) -> Result<Folder, LibraryError> {
        if id == ROOT_FOLDER_ID {
            return Err(LibraryError::RootFolderMutation);
        }
        self.ensure_folder(id)?;
        self.ensure_folder(new_parent_id)?;
        let mut ancestor = Some(new_parent_id.to_owned());
        while let Some(current) = ancestor {
            if current == id {
                return Err(LibraryError::FolderCycle);
            }
            ancestor = self.conn.query_row(
                "SELECT parent_id FROM folders WHERE id = ?1",
                params![current],
                |row| row.get::<_, Option<String>>(0),
            )?;
        }
        self.conn.execute(
            "UPDATE folders SET parent_id = ?1 WHERE id = ?2",
            params![new_parent_id, id],
        )?;
        self.get_folder(id)?
            .ok_or_else(|| LibraryError::FolderNotFound(id.to_string()))
    }

    pub fn delete_folder(&self, id: &str) -> Result<(), LibraryError> {
        if id == ROOT_FOLDER_ID {
            return Err(LibraryError::RootFolderMutation);
        }
        self.ensure_folder(id)?;
        let children: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM folders WHERE parent_id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        let models: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM models WHERE folder_id = ?1 AND deleted_at IS NULL",
            params![id],
            |row| row.get(0),
        )?;
        if children > 0 || models > 0 {
            return Err(LibraryError::FolderNotEmpty);
        }
        self.conn
            .execute("DELETE FROM folders WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn list_folders(&self) -> Result<Vec<Folder>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.name, f.parent_id,
                    (SELECT COUNT(*) FROM models m WHERE m.folder_id = f.id AND m.deleted_at IS NULL)
             FROM folders f ORDER BY f.parent_id IS NOT NULL, lower(f.name), f.id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Folder {
                id: row.get(0)?,
                name: row.get(1)?,
                parent_id: row.get(2)?,
                model_count: row.get::<_, i64>(3)? as u64,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn import_file(
        &self,
        source: impl AsRef<Path>,
        folder_id: Option<&str>,
        display_name: Option<&str>,
    ) -> Result<ImportResult, LibraryError> {
        let source = source.as_ref();
        let folder_id = folder_id.unwrap_or(ROOT_FOLDER_ID);
        self.ensure_folder(folder_id)?;
        let display_name = display_name
            .map(str::to_owned)
            .or_else(|| {
                source
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "Model".into());
        let display_name = clean_name(&display_name, "Model")?;
        let metadata = std::fs::symlink_metadata(source).map_err(|e| io(source, e))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(LibraryError::InvalidInput("source is not a file".into()));
        }
        if metadata.len() > MAX_INDEXED_FILE_BYTES {
            return Err(LibraryError::InvalidInput(
                "source exceeds the native file size limit".into(),
            ));
        }
        let mut file = std::fs::File::open(source).map_err(|error| io(source, error))?;
        let opened = file.metadata().map_err(|error| io(source, error))?;
        if !opened.is_file()
            || opened.len() != metadata.len()
            || opened.len() > MAX_INDEXED_FILE_BYTES
        {
            return Err(LibraryError::SourceChanged);
        }
        let mut bytes = Vec::with_capacity(opened.len() as usize);
        std::io::Read::by_ref(&mut file)
            .take(MAX_INDEXED_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| io(source, error))?;
        let after = file.metadata().map_err(|error| io(source, error))?;
        if metadata.len() != bytes.len() as u64
            || bytes.len() as u64 > MAX_INDEXED_FILE_BYTES
            || after.len() != opened.len()
        {
            return Err(LibraryError::SourceChanged);
        }
        let parsed = stl_core::parse_model(&bytes)?;
        let (format, extension) = match parsed.format {
            stl_core::StlFormat::Binary => ("stl-binary", "stl"),
            stl_core::StlFormat::Ascii => ("stl-ascii", "stl"),
            stl_core::StlFormat::ThreeMf => ("3mf", "3mf"),
        };
        let hash = hex_digest(&bytes);
        let relative_path = format!("originals/{hash}.{extension}");
        let originals = ensure_owned_directory(&self.root, "originals")?;
        let staging_root = ensure_owned_directory(&self.root, "staging")?;
        let destination = originals.join(format!("{hash}.{extension}"));
        let staging = staging_root.join(format!("{}.part", Uuid::new_v4()));
        let mut staged = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .map_err(|error| io(&staging, error))?;
        staged
            .write_all(&bytes)
            .map_err(|error| io(&staging, error))?;
        drop(staged);
        let blob = self
            .conn
            .query_row(
                "SELECT relative_path,bytes FROM blobs WHERE hash = ?1",
                params![hash],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let mut reused_blob = blob.is_some();
        if let Some((stored, stored_bytes)) = blob {
            if stored != relative_path || stored_bytes != bytes.len() as i64 {
                let _ = std::fs::remove_file(&staging);
                return Err(LibraryError::InvalidInput(
                    "blob catalog metadata is inconsistent".into(),
                ));
            }
            if let Err(error) = validate_managed_blob(&destination, &hash, bytes.len() as u64) {
                let _ = std::fs::remove_file(&staging);
                return Err(error);
            }
            let _ = std::fs::remove_file(&staging);
        } else {
            match std::fs::hard_link(&staging, &destination) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&staging);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Err(error) =
                        validate_managed_blob(&destination, &hash, bytes.len() as u64)
                    {
                        let _ = std::fs::remove_file(&staging);
                        return Err(error);
                    }
                    let _ = std::fs::remove_file(&staging);
                    reused_blob = true;
                }
                Err(error) => {
                    let _ = std::fs::remove_file(&staging);
                    return Err(io(&destination, error));
                }
            }
        }

        let original_name = source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("model.stl");
        let model_id = new_id("model");
        let revision_id = new_id("revision");
        let now = timestamp();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO blobs (hash, relative_path, bytes) VALUES (?1, ?2, ?3)",
            params![hash, relative_path, bytes.len() as i64],
        )?;
        tx.execute(
            "INSERT INTO models (id, display_name, description, folder_id, current_revision_id, metadata_version, created_at, updated_at)
             VALUES (?1, ?2, '', ?3, ?4, 1, ?5, ?5)",
            params![model_id, display_name, folder_id, revision_id, now],
        )?;
        tx.execute(
            "INSERT INTO revisions (id, model_id, blob_hash, original_name, format, bytes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![revision_id, model_id, hash, original_name, format, bytes.len() as i64, now],
        )?;
        tx.commit()?;
        let model = self
            .get_model(&model_id)?
            .ok_or(LibraryError::ModelNotFound(model_id))?;
        Ok(ImportResult { model, reused_blob })
    }

    pub fn add_index_root(
        &self,
        path: impl AsRef<Path>,
        scan_interval_minutes: u32,
        max_file_bytes: u64,
    ) -> Result<IndexRoot, LibraryError> {
        validate_root_limits(scan_interval_minutes, max_file_bytes)?;
        let path = canonical_directory(path.as_ref())?;
        self.ensure_root_does_not_overlap(&path, None)?;
        let root = IndexRoot {
            id: new_id("index-root"),
            path,
            enabled: true,
            last_scanned_at: None,
            scan_interval_minutes,
            max_file_bytes,
            scan_zip: false,
        };
        self.conn.execute(
            "INSERT INTO index_roots
             (id, path, enabled, last_scanned_at, scan_interval_minutes, max_file_bytes, scan_zip)
             VALUES (?1, ?2, 1, NULL, ?3, ?4, 0)",
            params![
                root.id,
                root.path.to_string_lossy(),
                i64::from(root.scan_interval_minutes),
                root.max_file_bytes as i64
            ],
        )?;
        Ok(root)
    }

    pub fn list_index_roots(&self) -> Result<Vec<IndexRoot>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, enabled, last_scanned_at, scan_interval_minutes, max_file_bytes, scan_zip
             FROM index_roots ORDER BY lower(path), id",
        )?;
        let rows = stmt.query_map([], index_root_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_catalog_settings(&self) -> Result<CatalogSettings, LibraryError> {
        let (thumbnail_size, thumbnail_background, thumbnail_concurrency, scan_batch_size): (
            i64,
            String,
            i64,
            i64,
        ) = self.conn.query_row(
            "SELECT thumbnail_size, thumbnail_background, thumbnail_concurrency,
                        scan_batch_size FROM catalog_settings WHERE id = 'local'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        let corrupt = || LibraryError::InvalidInput("catalog settings row is corrupt".into());
        let settings = CatalogSettings {
            thumbnail_size: u32::try_from(thumbnail_size).map_err(|_| corrupt())?,
            thumbnail_background,
            thumbnail_concurrency: u32::try_from(thumbnail_concurrency).map_err(|_| corrupt())?,
            scan_batch_size: u32::try_from(scan_batch_size).map_err(|_| corrupt())?,
        };
        validate_catalog_settings(&settings)?;
        Ok(settings)
    }

    pub fn update_catalog_settings(
        &self,
        settings: &CatalogSettings,
    ) -> Result<CatalogSettings, LibraryError> {
        validate_catalog_settings(settings)?;
        let current = self.get_catalog_settings()?;
        let appearance_changed = current.thumbnail_size != settings.thumbnail_size
            || current.thumbnail_background != settings.thumbnail_background;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE catalog_settings SET thumbnail_size = ?1, thumbnail_background = ?2,
                    thumbnail_concurrency = ?3, scan_batch_size = ?4
             WHERE id = 'local'",
            params![
                i64::from(settings.thumbnail_size),
                settings.thumbnail_background,
                i64::from(settings.thumbnail_concurrency),
                i64::from(settings.scan_batch_size)
            ],
        )?;
        if appearance_changed {
            let mut affected = Vec::new();
            {
                let mut stmt =
                    tx.prepare("SELECT DISTINCT model_id FROM thumbnails WHERE is_custom=0")?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                affected.extend(rows.collect::<Result<Vec<_>, _>>()?);
            }
            tx.execute("DELETE FROM thumbnails WHERE is_custom = 0", [])?;
            for model_id in affected {
                compact_thumbnail_ordinals(&tx, &model_id)?;
                select_thumbnail_fallback(&tx, &model_id)?;
            }
        }
        tx.commit()?;
        self.get_catalog_settings()
    }

    pub fn update_index_root(&self, root: &IndexRoot) -> Result<IndexRoot, LibraryError> {
        validate_root_limits(root.scan_interval_minutes, root.max_file_bytes)?;
        let existing = self
            .get_index_root(&root.id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(root.id.clone()))?;
        if root.path != existing.path {
            return Err(LibraryError::InvalidInput(
                "index root paths are immutable; remove and add the root again".into(),
            ));
        }
        let changed = self.conn.execute(
            "UPDATE index_roots SET path = ?1, enabled = ?2, scan_interval_minutes = ?3,
                    max_file_bytes = ?4, scan_zip = ?5
             WHERE id = ?6",
            params![
                existing.path.to_string_lossy(),
                root.enabled,
                i64::from(root.scan_interval_minutes),
                root.max_file_bytes as i64,
                root.scan_zip,
                root.id
            ],
        )?;
        if changed == 0 {
            return Err(LibraryError::IndexRootNotFound(root.id.clone()));
        }
        self.get_index_root(&root.id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(root.id.clone()))
    }

    pub fn remove_index_root(&self, id: &str) -> Result<(), LibraryError> {
        self.get_index_root(id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(id.to_owned()))?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM models WHERE source_kind = 'indexed' AND source_root_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM index_roots WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn remap_restored_index_roots(
        &self,
        mappings: &[(String, String, PathBuf, bool)],
    ) -> Result<(), LibraryError> {
        let mut backup_ids = HashSet::new();
        let mut target_ids = HashSet::new();
        let mut paths = Vec::new();
        for (backup_id, target_id, path, _) in mappings {
            if !backup_ids.insert(backup_id.clone()) || !target_ids.insert(target_id.clone()) {
                return Err(LibraryError::InvalidInput(
                    "restore root mapping is duplicated".into(),
                ));
            }
            let path = canonical_directory(path)?;
            if paths
                .iter()
                .any(|existing: &PathBuf| path.starts_with(existing) || existing.starts_with(&path))
            {
                return Err(LibraryError::InvalidInput(
                    "restore root mappings overlap".into(),
                ));
            }
            paths.push(path);
        }
        self.conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
        let result = (|| {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute("UPDATE index_roots SET enabled=0", [])?;
            for ((backup_id, target_id, _, enabled), path) in mappings.iter().zip(&paths) {
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM index_roots WHERE id=?1)",
                    params![backup_id],
                    |row| row.get(0),
                )?;
                if !exists {
                    return Err(LibraryError::IndexRootNotFound(backup_id.clone()));
                }
                if backup_id != target_id {
                    let target_exists: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM index_roots WHERE id=?1)",
                        params![target_id],
                        |row| row.get(0),
                    )?;
                    if target_exists {
                        let displaced = new_id("unmapped-root");
                        tx.execute(
                            "UPDATE models SET source_root_id=?1 WHERE source_root_id=?2",
                            params![displaced, target_id],
                        )?;
                        tx.execute(
                            "UPDATE bundles SET source_root_id=?1 WHERE source_root_id=?2",
                            params![displaced, target_id],
                        )?;
                        tx.execute(
                            "UPDATE index_roots SET id=?1,enabled=0 WHERE id=?2",
                            params![displaced, target_id],
                        )?;
                    }
                    tx.execute(
                        "UPDATE models SET source_root_id=?1 WHERE source_root_id=?2",
                        params![target_id, backup_id],
                    )?;
                    tx.execute(
                        "UPDATE bundles SET source_root_id=?1 WHERE source_root_id=?2",
                        params![target_id, backup_id],
                    )?;
                }
                tx.execute(
                    "UPDATE index_roots SET id=?1,path=?2,enabled=?3 WHERE id=?4",
                    params![target_id, path.to_string_lossy(), enabled, backup_id],
                )?;
            }
            tx.commit()?;
            Ok(())
        })();
        self.conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        result?;
        let violation: Option<String> = self
            .conn
            .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
            .optional()?;
        if violation.is_some() {
            return Err(LibraryError::InvalidInput(
                "restore root mapping violates catalog links".into(),
            ));
        }
        Ok(())
    }

    pub fn move_loose_indexed_model(
        &self,
        model_id: &str,
        expected_version: u64,
        destination_root_id: &str,
        destination_relative_path: &Path,
        modified_at: u64,
    ) -> Result<Model, LibraryError> {
        self.move_loose_indexed_models(&[IndexedModelMove {
            model_id: model_id.into(),
            expected_version,
            destination_root_id: destination_root_id.into(),
            destination_relative_path: destination_relative_path.to_path_buf(),
            modified_at,
        }])?
        .into_iter()
        .next()
        .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    pub fn move_loose_indexed_models(
        &self,
        moves: &[IndexedModelMove],
    ) -> Result<Vec<Model>, LibraryError> {
        const MAX_MOVE_TARGETS: usize = 100;
        if moves.is_empty() || moves.len() > MAX_MOVE_TARGETS {
            return Err(LibraryError::InvalidInput(
                "move target count is invalid".into(),
            ));
        }
        let mut model_ids = HashSet::new();
        let mut destinations = HashSet::new();
        let mut normalized = Vec::with_capacity(moves.len());
        for item in moves {
            if !model_ids.insert(item.model_id.as_str()) {
                return Err(LibraryError::InvalidInput("duplicate move target".into()));
            }
            let relative = validated_relative_path(&item.destination_relative_path)?;
            if !destinations.insert((item.destination_root_id.clone(), relative.clone())) {
                return Err(LibraryError::InvalidInput(
                    "duplicate move destination".into(),
                ));
            }
            let modified_at = i64::try_from(item.modified_at)
                .map_err(|_| LibraryError::InvalidInput("modified time is out of range".into()))?;
            normalized.push((item, relative, modified_at));
        }
        let tx = self.conn.unchecked_transaction()?;
        let move_ids = model_ids;
        for (item, _, _) in &normalized {
            tx.query_row(
                "SELECT id FROM index_roots WHERE id=?1",
                params![item.destination_root_id],
                |_| Ok(()),
            )
            .optional()?
            .ok_or_else(|| LibraryError::IndexRootNotFound(item.destination_root_id.clone()))?;
            let row: Option<(i64, String, Option<String>, Option<String>)> = tx
                .query_row(
                    "SELECT metadata_version,source_kind,source_archive_member,deleted_at
                     FROM models WHERE id=?1",
                    params![item.model_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            let Some((actual, source_kind, archive_member, deleted_at)) = row else {
                return Err(LibraryError::ModelNotFound(item.model_id.clone()));
            };
            if actual as u64 != item.expected_version {
                return Err(LibraryError::MetadataConflict {
                    model_id: item.model_id.clone(),
                    expected: item.expected_version,
                    actual: actual as u64,
                });
            }
            if deleted_at.is_some() || source_kind != "indexed" || archive_member.is_some() {
                return Err(LibraryError::InvalidInput(
                    "only active loose indexed models can be moved".into(),
                ));
            }
        }
        for (item, relative, _) in &normalized {
            let collision: Option<String> = tx
                .query_row(
                    "SELECT id FROM models WHERE source_kind='indexed' AND source_root_id=?1
                     AND source_path=?2 AND COALESCE(source_archive_member,'')=''
                     AND id<>?3 LIMIT 1",
                    params![item.destination_root_id, relative, item.model_id],
                    |row| row.get(0),
                )
                .optional()?;
            if collision
                .as_ref()
                .is_some_and(|id| !move_ids.contains(id.as_str()))
            {
                return Err(LibraryError::InvalidInput(
                    "move destination is already cataloged".into(),
                ));
            }
        }
        let now = timestamp();
        for (item, _, _) in &normalized {
            let temporary_path = format!(".gogglelab-move-{}", Uuid::new_v4().simple());
            tx.execute(
                "UPDATE models SET source_path=?1 WHERE id=?2",
                params![temporary_path, item.model_id],
            )?;
        }
        for (item, relative, modified_at) in &normalized {
            tx.execute(
                "UPDATE models SET source_root_id=?1,source_path=?2,source_missing=0,
                        source_modified_at=?3,metadata_version=metadata_version+1,updated_at=?4
                 WHERE id=?5",
                params![
                    item.destination_root_id,
                    relative,
                    modified_at,
                    now,
                    item.model_id
                ],
            )?;
        }
        tx.commit()?;
        moves
            .iter()
            .map(|item| {
                self.get_model(&item.model_id)?
                    .ok_or_else(|| LibraryError::ModelNotFound(item.model_id.clone()))
            })
            .collect()
    }

    pub fn indexed_source_metadata(
        &self,
        root_id: &str,
        relative_path: &Path,
        archive_member: Option<&str>,
    ) -> Result<Option<IndexedSourceMetadata>, LibraryError> {
        let relative_path = validated_relative_path(relative_path)?;
        let archive_member = archive_member.map(validated_archive_member).transpose()?;
        let root = self
            .get_index_root(root_id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(root_id.to_owned()))?;
        let absolute_path = root
            .path
            .join(&relative_path)
            .to_str()
            .map(ToOwned::to_owned);
        self.conn
            .query_row(
                "SELECT d.bytes, m.source_modified_at, d.content_hash,
                        m.source_archive_member, d.outer_fingerprint
                 FROM models m JOIN indexed_model_data d ON d.model_id = m.id
                 WHERE m.source_kind = 'indexed' AND m.source_root_id = ?1
                   AND (m.source_path = ?2 OR (?4 IS NOT NULL AND m.source_path = ?4))
                   AND COALESCE(m.source_archive_member,'') = COALESCE(?3,'')
                 ORDER BY (m.source_path = ?2) DESC LIMIT 1",
                params![root_id, relative_path, archive_member, absolute_path],
                |row| {
                    Ok(IndexedSourceMetadata {
                        bytes: row.get::<_, i64>(0)? as u64,
                        modified_at: row.get::<_, i64>(1)? as u64,
                        content_hash: row.get(2)?,
                        archive_member: row.get(3)?,
                        outer_fingerprint: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn indexed_archive_metadata(
        &self,
        root_id: &str,
        relative_path: &Path,
    ) -> Result<Vec<IndexedSourceMetadata>, LibraryError> {
        let relative = validated_relative_path(relative_path)?;
        let root = self
            .get_index_root(root_id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(root_id.to_owned()))?;
        let absolute = root.path.join(&relative).to_str().map(ToOwned::to_owned);
        let mut stmt = self.conn.prepare("SELECT d.bytes,m.source_modified_at,d.content_hash,m.source_archive_member,d.outer_fingerprint FROM models m JOIN indexed_model_data d ON d.model_id=m.id WHERE m.source_kind='indexed' AND m.source_root_id=?1 AND (m.source_path=?2 OR (?3 IS NOT NULL AND m.source_path=?3)) AND m.source_archive_member IS NOT NULL ORDER BY m.source_archive_member")?;
        let rows = stmt.query_map(params![root_id, relative, absolute], |row| {
            Ok(IndexedSourceMetadata {
                bytes: row.get::<_, i64>(0)? as u64,
                modified_at: row.get::<_, i64>(1)? as u64,
                content_hash: row.get(2)?,
                archive_member: row.get(3)?,
                outer_fingerprint: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn record_scan_entries(
        &self,
        root_id: &str,
        entries: &[ScannedModel],
    ) -> Result<ScanReconcile, LibraryError> {
        let root = self
            .get_index_root(root_id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(root_id.to_owned()))?;
        if !root.enabled {
            return Err(LibraryError::InvalidInput("index root is disabled".into()));
        }
        let mut validated = Vec::with_capacity(entries.len());
        let mut paths = HashSet::with_capacity(entries.len());
        for entry in entries {
            let relative = validated_relative_path(&entry.relative_path)?;
            let archive_member = entry
                .archive_member
                .as_deref()
                .map(validated_archive_member)
                .transpose()?;
            if archive_member.is_some()
                && entry.outer_fingerprint.as_deref().is_none_or(str::is_empty)
            {
                return Err(LibraryError::InvalidInput(
                    "archive entries require an outer fingerprint".into(),
                ));
            }
            if !paths.insert((relative.clone(), archive_member.clone())) {
                return Err(LibraryError::InvalidInput(
                    "scan batch contains duplicate relative paths".into(),
                ));
            }
            validate_hash(&entry.content_hash)?;
            if entry.format.trim().is_empty() || entry.bytes > root.max_file_bytes {
                return Err(LibraryError::InvalidInput(
                    "invalid scanned model metadata".into(),
                ));
            }
            resolve_under_root(&root.path, Path::new(&relative))?;
            let absolute = root.path.join(&relative).to_str().map(ToOwned::to_owned);
            validated.push((relative, absolute, archive_member, entry));
        }

        let tx = self.conn.unchecked_transaction()?;
        let mut result = ScanReconcile {
            seen: validated.len(),
            ..ScanReconcile::default()
        };
        for (relative, absolute, archive_member, entry) in validated {
            let existing: Option<(String, String)> = tx
                .query_row(
                    "SELECT m.id, d.content_hash
                     FROM models m JOIN indexed_model_data d ON d.model_id = m.id
                     WHERE m.source_kind = 'indexed' AND m.source_root_id = ?1
                       AND (m.source_path = ?2 OR (?4 IS NOT NULL AND m.source_path = ?4))
                       AND COALESCE(m.source_archive_member,'')=COALESCE(?3,'')
                     ORDER BY (m.source_path = ?2) DESC LIMIT 1",
                    params![root_id, relative, archive_member, absolute],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let original_name = archive_member.as_deref().unwrap_or(&relative);
            let original_name = Path::new(original_name)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("model")
                .to_owned();
            if let Some((model_id, existing_hash)) = existing {
                let content_changed = existing_hash != entry.content_hash;
                tx.execute(
                    "UPDATE indexed_model_data SET content_hash = ?1, original_name = ?2,
                            format = ?3, bytes = ?4, outer_fingerprint=?5 WHERE model_id = ?6",
                    params![
                        entry.content_hash,
                        original_name,
                        entry.format,
                        entry.bytes as i64,
                        entry.outer_fingerprint,
                        model_id
                    ],
                )?;
                tx.execute(
                    "UPDATE models SET source_path = ?1, source_missing = 0, source_modified_at = ?2,
                            metadata_version = metadata_version + ?3, updated_at = ?4 WHERE id = ?5",
                    params![
                        relative,
                        entry.modified_at as i64,
                        if content_changed { 1_i64 } else { 0_i64 },
                        timestamp(),
                        model_id
                    ],
                )?;
                if archive_member.is_some() {
                    tx.execute(
                        "UPDATE bundles SET source_path=?1
                         WHERE id=(SELECT bundle_id FROM models WHERE id=?2)
                           AND source_root_id=?3",
                        params![relative, model_id, root_id],
                    )?;
                }
                result.updated += 1;
            } else {
                let model_id = new_id("model");
                let bundle_id = if archive_member.is_some() {
                    Some(ensure_archive_bundle(&tx, root_id, &relative)?)
                } else {
                    None
                };
                let display_name = entry
                    .display_name
                    .as_deref()
                    .map(|value| clean_name(value, "Model"))
                    .transpose()?
                    .or_else(|| {
                        Path::new(&relative)
                            .file_stem()
                            .and_then(|value| value.to_str())
                            .map(ToOwned::to_owned)
                    })
                    .unwrap_or_else(|| "Model".into());
                let now = timestamp();
                tx.execute(
                    "INSERT INTO models
                     (id, display_name, description, folder_id, metadata_version, created_at,
                      updated_at, source_kind, source_path, source_root_id, source_missing,
                      source_modified_at, source_archive_member, bundle_id)
                     VALUES (?1, ?2, '', ?3, 1, ?4, ?4, 'indexed', ?5, ?6, 0, ?7, ?8, ?9)",
                    params![
                        model_id,
                        display_name,
                        ROOT_FOLDER_ID,
                        now,
                        relative,
                        root_id,
                        entry.modified_at as i64,
                        archive_member,
                        bundle_id
                    ],
                )?;
                tx.execute(
                    "INSERT INTO indexed_model_data
                     (model_id, content_hash, original_name, format, bytes, outer_fingerprint)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        model_id,
                        entry.content_hash,
                        original_name,
                        entry.format,
                        entry.bytes as i64,
                        entry.outer_fingerprint
                    ],
                )?;
                result.added_model_ids.push(model_id);
                result.added += 1;
            }
        }
        tx.commit()?;
        Ok(result)
    }

    pub fn complete_scan(
        &self,
        root_id: &str,
        seen_relative_paths: &[PathBuf],
        completed_at: &str,
    ) -> Result<ScanReconcile, LibraryError> {
        self.complete_scan_with_new_models(root_id, seen_relative_paths, &[], completed_at)
    }

    pub fn complete_scan_with_new_models(
        &self,
        root_id: &str,
        seen_relative_paths: &[PathBuf],
        newly_added_model_ids: &[String],
        completed_at: &str,
    ) -> Result<ScanReconcile, LibraryError> {
        let seen = seen_relative_paths
            .iter()
            .cloned()
            .map(|relative_path| SourceRef {
                relative_path,
                archive_member: None,
            })
            .collect::<Vec<_>>();
        self.complete_scan_sources_with_new_models(
            root_id,
            &seen,
            newly_added_model_ids,
            completed_at,
        )
    }

    pub fn complete_scan_sources_with_new_models(
        &self,
        root_id: &str,
        seen_sources: &[SourceRef],
        newly_added_model_ids: &[String],
        completed_at: &str,
    ) -> Result<ScanReconcile, LibraryError> {
        self.complete_scan_sources_in_scope(
            root_id,
            None,
            seen_sources,
            newly_added_model_ids,
            Some(completed_at),
        )
    }

    pub fn indexed_archive_source_refs(
        &self,
        root_id: &str,
        scope: Option<&Path>,
    ) -> Result<Vec<SourceRef>, LibraryError> {
        let root = self
            .get_index_root(root_id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(root_id.to_owned()))?;
        let mut stmt = self.conn.prepare(
            "SELECT source_path, source_archive_member FROM models
             WHERE source_kind='indexed' AND source_root_id=?1 AND deleted_at IS NULL
               AND source_archive_member IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![root_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        let mut sources = Vec::new();
        for row in rows {
            let (path, archive_member) = row?;
            let relative_path = normalize_indexed_source_path(&root.path, Path::new(&path))?;
            if scope.is_some_and(|scope| !relative_path.starts_with(scope)) {
                continue;
            }
            sources.push(SourceRef {
                relative_path,
                archive_member: Some(validated_archive_member(
                    archive_member.as_deref().ok_or_else(|| {
                        LibraryError::InvalidInput("archive member path is missing".into())
                    })?,
                )?),
            });
        }
        Ok(sources)
    }

    pub fn complete_subdirectory_scan_sources_with_new_models(
        &self,
        root_id: &str,
        relative_directory: &Path,
        seen_sources: &[SourceRef],
        newly_added_model_ids: &[String],
    ) -> Result<ScanReconcile, LibraryError> {
        let relative_directory = PathBuf::from(validated_relative_path(relative_directory)?);
        self.complete_scan_sources_in_scope(
            root_id,
            Some(&relative_directory),
            seen_sources,
            newly_added_model_ids,
            None,
        )
    }

    fn complete_scan_sources_in_scope(
        &self,
        root_id: &str,
        scope: Option<&Path>,
        seen_sources: &[SourceRef],
        newly_added_model_ids: &[String],
        completed_at: Option<&str>,
    ) -> Result<ScanReconcile, LibraryError> {
        let root = self
            .get_index_root(root_id)?
            .ok_or_else(|| LibraryError::IndexRootNotFound(root_id.to_owned()))?;
        if !root.enabled {
            return Err(LibraryError::InvalidInput("index root is disabled".into()));
        }
        if canonical_directory(&root.path)? != root.path {
            return Err(LibraryError::InvalidInput(
                "configured root identity changed".into(),
            ));
        }
        let scope_relative = scope.and_then(|scope| scope.to_str().map(ToOwned::to_owned));
        let scope_absolute =
            scope.and_then(|scope| root.path.join(scope).to_str().map(ToOwned::to_owned));
        if scope.is_some() && scope_relative.is_none() {
            return Err(LibraryError::InvalidInput(
                "scan directory is not valid UTF-8".into(),
            ));
        }
        let separator = std::path::MAIN_SEPARATOR.to_string();
        let seen = seen_sources
            .iter()
            .map(|source| -> Result<SourceRef, LibraryError> {
                let source = SourceRef {
                    relative_path: PathBuf::from(validated_relative_path(&source.relative_path)?),
                    archive_member: source
                        .archive_member
                        .as_deref()
                        .map(validated_archive_member)
                        .transpose()?,
                };
                if scope.is_some_and(|scope| !source.relative_path.starts_with(scope)) {
                    return Err(LibraryError::InvalidInput(
                        "scanned source is outside the selected directory".into(),
                    ));
                }
                Ok(source)
            })
            .collect::<Result<HashSet<_>, _>>()?;
        let tx = self.conn.unchecked_transaction()?;
        let mut indexed = Vec::new();
        {
            let mut stmt = tx.prepare(
                "SELECT m.id, m.source_path, m.source_archive_member, d.content_hash
                 FROM models m JOIN indexed_model_data d ON d.model_id = m.id
                 WHERE m.source_kind = 'indexed' AND m.source_root_id = ?1
                   AND (?2 IS NULL OR m.source_path = ?2
                        OR substr(m.source_path, 1, length(?2) + 1) = ?2 || ?3
                        OR (?4 IS NOT NULL AND (m.source_path = ?4
                            OR substr(m.source_path, 1, length(?4) + 1) = ?4 || ?3)))",
            )?;
            let rows = stmt.query_map(
                params![root_id, scope_relative, separator, scope_absolute],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )?;
            for row in rows {
                let (id, path, member, hash) = row?;
                let path = normalize_indexed_source_path(&root.path, Path::new(&path))?;
                if scope.is_some_and(|scope| !path.starts_with(scope)) {
                    continue;
                }
                indexed.push((id, validated_relative_path(&path)?, member, hash));
            }
        }

        let newly_added = newly_added_model_ids.iter().collect::<HashSet<_>>();
        let mut absent_by_hash: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut present_by_hash: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (id, path, member, hash) in &indexed {
            let source = SourceRef {
                relative_path: PathBuf::from(path),
                archive_member: member.clone(),
            };
            let target = if seen.contains(&source) {
                &mut present_by_hash
            } else {
                &mut absent_by_hash
            };
            target.entry(hash.clone()).or_default().push((
                id.clone(),
                format!("{}\0{}", path, member.as_deref().unwrap_or("")),
            ));
        }

        let mut result = ScanReconcile {
            seen: seen.len(),
            ..ScanReconcile::default()
        };
        let mut rename_targets = HashMap::<String, (String, String, String)>::new();
        for (hash, absent) in &absent_by_hash {
            let Some(present) = present_by_hash.get(hash) else {
                continue;
            };
            if absent.len() != 1 || present.len() != 1 {
                continue;
            }
            let (old_id, _) = &absent[0];
            let (new_id, new_key) = &present[0];
            if old_id == new_id || !newly_added.contains(new_id) {
                continue;
            }
            let (new_path, new_member) = new_key.split_once('\0').unwrap();
            rename_targets.insert(
                old_id.clone(),
                (new_id.clone(), new_path.to_owned(), new_member.to_owned()),
            );
        }
        for (hash, absent) in absent_by_hash.iter() {
            let Some(present) = present_by_hash.get(hash) else {
                continue;
            };
            if absent.len() != 1 || present.len() != 1 {
                continue;
            }
            let (old_id, _) = &absent[0];
            let (new_id, new_key) = &present[0];
            if old_id == new_id || !newly_added.contains(new_id) {
                continue;
            }
            let (new_path, new_member) = new_key.split_once('\0').unwrap();
            let (modified_at, original_name, format, bytes, outer_fingerprint): (i64, String, String, i64, Option<String>) = tx
                .query_row(
                    "SELECT m.source_modified_at, d.original_name, d.format, d.bytes,d.outer_fingerprint
                     FROM models m JOIN indexed_model_data d ON d.model_id = m.id WHERE m.id = ?1",
                    params![new_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,row.get(4)?)),
                )?;
            let old_archive_member: Option<String> = tx.query_row(
                "SELECT source_archive_member FROM models WHERE id=?1",
                params![old_id],
                |row| row.get(0),
            )?;
            let new_is_archive_member = !new_member.is_empty();
            let archive_kind_changed = old_archive_member.is_some() != new_is_archive_member;
            if archive_kind_changed {
                let old_bundle:Option<(Option<String>,Option<String>)>=tx.query_row("SELECT m.bundle_id,b.source_root_id FROM models m LEFT JOIN bundles b ON b.id=m.bundle_id WHERE m.id=?1",params![old_id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
                if let Some((Some(_), Some(_))) = old_bundle {
                    if new_member.is_empty() {
                        tx.execute(
                            "UPDATE models SET bundle_id=NULL WHERE id=?1",
                            params![old_id],
                        )?;
                    } else {
                        let new_bundle: Option<String> = tx
                            .query_row(
                                "SELECT bundle_id FROM models WHERE id=?1",
                                params![new_id],
                                |row| row.get(0),
                            )
                            .optional()?
                            .flatten();
                        tx.execute(
                            "UPDATE models SET bundle_id=?1 WHERE id=?2",
                            params![new_bundle, old_id],
                        )?;
                    }
                }
            }
            if !archive_kind_changed && !new_member.is_empty() {
                let old_bundle:Option<(Option<String>,Option<String>)>=tx.query_row("SELECT m.bundle_id,b.source_root_id FROM models m LEFT JOIN bundles b ON b.id=m.bundle_id WHERE m.id=?1",params![old_id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
                let new_bundle: Option<String> = tx
                    .query_row(
                        "SELECT bundle_id FROM models WHERE id=?1",
                        params![new_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();
                if let Some((Some(old_bundle_id), Some(_))) = old_bundle {
                    if let Some(new_bundle_id) = new_bundle {
                        if new_bundle_id != old_bundle_id {
                            let mut refs = Vec::new();
                            {
                                let mut stmt=tx.prepare("SELECT id,source_path,source_archive_member FROM models WHERE bundle_id=?1")?;
                                let rows = stmt.query_map(params![old_bundle_id], |row| {
                                    Ok((
                                        row.get::<_, String>(0)?,
                                        row.get::<_, String>(1)?,
                                        row.get::<_, Option<String>>(2)?,
                                    ))
                                })?;
                                refs.extend(rows.collect::<Result<Vec<_>, _>>()?);
                            }
                            let all_moving = refs.iter().all(|(id, _, _)| {
                                rename_targets
                                    .get(id)
                                    .is_some_and(|(_, target_path, _)| target_path == new_path)
                            });
                            if all_moving {
                                tx.execute(
                                    "UPDATE models SET bundle_id=?1 WHERE bundle_id=?2",
                                    params![old_bundle_id, new_bundle_id],
                                )?;
                                tx.execute(
                                    "DELETE FROM bundles WHERE id=?1",
                                    params![new_bundle_id],
                                )?;
                                tx.execute("UPDATE bundles SET source_root_id=?1,source_path=?2 WHERE id=?3",params![root_id,new_path,old_bundle_id])?;
                            } else {
                                tx.execute(
                                    "UPDATE models SET bundle_id=?1 WHERE id=?2",
                                    params![new_bundle_id, old_id],
                                )?;
                            }
                        }
                    }
                }
            }
            tx.execute("DELETE FROM models WHERE id = ?1", params![new_id])?;
            tx.execute(
                "UPDATE models SET source_path = ?1, source_archive_member=?2, source_missing = 0,
                        source_modified_at = ?3, metadata_version=metadata_version+1,
                        updated_at = ?4 WHERE id = ?5",
                params![
                    new_path,
                    if new_member.is_empty() {
                        None
                    } else {
                        Some(new_member)
                    },
                    modified_at,
                    timestamp(),
                    old_id
                ],
            )?;
            tx.execute(
                "UPDATE indexed_model_data SET original_name = ?1, format = ?2, bytes = ?3,outer_fingerprint=?4
                 WHERE model_id = ?5",
                params![original_name, format, bytes, outer_fingerprint, old_id],
            )?;
            result.renamed += 1;
        }

        let mut ids_and_paths = Vec::new();
        {
            let mut stmt = tx.prepare(
                "SELECT id, source_path,source_archive_member FROM models
                 WHERE source_kind = 'indexed' AND source_root_id = ?1
                   AND (?2 IS NULL OR source_path = ?2
                        OR substr(source_path, 1, length(?2) + 1) = ?2 || ?3
                        OR (?4 IS NOT NULL AND (source_path = ?4
                            OR substr(source_path, 1, length(?4) + 1) = ?4 || ?3)))",
            )?;
            let rows = stmt.query_map(
                params![root_id, scope_relative, separator, scope_absolute],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )?;
            for row in rows {
                let (id, path, member) = row?;
                let path = normalize_indexed_source_path(&root.path, Path::new(&path))?;
                if scope.is_some_and(|scope| !path.starts_with(scope)) {
                    continue;
                }
                ids_and_paths.push((id, validated_relative_path(&path)?, member));
            }
        }
        for (id, path, member) in ids_and_paths {
            let missing = !seen.contains(&SourceRef {
                relative_path: PathBuf::from(path),
                archive_member: member,
            });
            tx.execute(
                "UPDATE models SET source_missing = ?1 WHERE id = ?2",
                params![missing, id],
            )?;
            result.missing += usize::from(missing);
        }
        if scope.is_none() {
            reconcile_folder_bundles(&tx, root_id)?;
            let changed = tx.execute(
                "UPDATE index_roots SET last_scanned_at = ?1 WHERE id = ?2",
                params![
                    completed_at.ok_or_else(|| {
                        LibraryError::InvalidInput(
                            "full scan is missing its completion time".into(),
                        )
                    })?,
                    root_id
                ],
            )?;
            if changed == 0 {
                return Err(LibraryError::IndexRootNotFound(root_id.to_owned()));
            }
        }
        tx.commit()?;
        Ok(result)
    }

    pub fn resolve_model_source(&self, model_id: &str) -> Result<PathBuf, LibraryError> {
        Ok(self.resolve_model_source_ref(model_id)?.outer_path)
    }

    pub fn resolve_model_source_ref(
        &self,
        model_id: &str,
    ) -> Result<ResolvedModelSource, LibraryError> {
        let source: Option<SourceResolutionRow> = self
            .conn
            .query_row(
                "SELECT m.source_kind, m.source_path, m.source_root_id, m.source_missing,
                        b.relative_path,m.source_archive_member,d.outer_fingerprint,
                        COALESCE(r.blob_hash,d.content_hash),COALESCE(r.bytes,d.bytes,0)
                 FROM models m
                 LEFT JOIN revisions r ON r.id = m.current_revision_id
                 LEFT JOIN blobs b ON b.hash = r.blob_hash
                 LEFT JOIN indexed_model_data d ON d.model_id=m.id
                 WHERE m.id = ?1",
                params![model_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                    ))
                },
            )
            .optional()?;
        let (
            source_kind,
            source_path,
            source_root_id,
            source_missing,
            managed_path,
            archive_member,
            outer_fingerprint,
            content_hash,
            bytes,
        ) = source.ok_or_else(|| LibraryError::ModelNotFound(model_id.to_owned()))?;
        let content_hash = content_hash
            .ok_or_else(|| LibraryError::InvalidInput("model source has no content hash".into()))?;
        if source_missing {
            return Err(LibraryError::InvalidInput("model source is missing".into()));
        }
        match source_kind.as_str() {
            "managed" => {
                let relative = managed_path.ok_or_else(|| {
                    LibraryError::InvalidInput("managed model has no storage path".into())
                })?;
                if Path::new(&relative).components().next()
                    != Some(Component::Normal("originals".as_ref()))
                {
                    return Err(LibraryError::InvalidInput(
                        "managed model source must be stored under originals".into(),
                    ));
                }
                Ok(ResolvedModelSource {
                    outer_path: resolve_under_root(&self.root, Path::new(&relative))?,
                    archive_member: None,
                    outer_fingerprint: None,
                    content_hash,
                    bytes: bytes as u64,
                })
            }
            "indexed" => {
                let root_id = source_root_id.ok_or_else(|| {
                    LibraryError::InvalidInput("indexed model has no source root".into())
                })?;
                let root = self
                    .get_index_root(&root_id)?
                    .ok_or(LibraryError::IndexRootNotFound(root_id))?;
                let relative = source_path.ok_or_else(|| {
                    LibraryError::InvalidInput("indexed model has no source path".into())
                })?;
                let relative = normalize_indexed_source_path(&root.path, Path::new(&relative))?;
                Ok(ResolvedModelSource {
                    outer_path: resolve_under_root(&root.path, &relative)?,
                    archive_member,
                    outer_fingerprint,
                    content_hash,
                    bytes: bytes as u64,
                })
            }
            _ => Err(LibraryError::InvalidInput(
                "unknown model source kind".into(),
            )),
        }
    }

    pub fn set_thumbnail(
        &self,
        model_id: &str,
        content_hash: &str,
        thumbnail_relative_path: &Path,
    ) -> Result<(), LibraryError> {
        let model = self
            .get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.to_owned()))?;
        let source_content_hash = model
            .content_hash
            .ok_or_else(|| LibraryError::InvalidInput("model has no source content hash".into()))?;
        self.set_thumbnail_for_source(
            model_id,
            &source_content_hash,
            content_hash,
            thumbnail_relative_path,
        )
    }

    pub fn set_thumbnail_for_source(
        &self,
        model_id: &str,
        source_content_hash: &str,
        content_hash: &str,
        thumbnail_relative_path: &Path,
    ) -> Result<(), LibraryError> {
        self.set_thumbnail_for_source_with_expected_settings(
            model_id,
            source_content_hash,
            None,
            content_hash,
            thumbnail_relative_path,
        )
    }

    pub fn set_thumbnail_for_source_and_settings(
        &self,
        model_id: &str,
        source_content_hash: &str,
        expected_settings: &CatalogSettings,
        content_hash: &str,
        thumbnail_relative_path: &Path,
    ) -> Result<(), LibraryError> {
        validate_catalog_settings(expected_settings)?;
        self.set_thumbnail_for_source_with_expected_settings(
            model_id,
            source_content_hash,
            Some(expected_settings),
            content_hash,
            thumbnail_relative_path,
        )
    }

    pub fn set_custom_thumbnail(
        &self,
        model_id: &str,
        expected_version: u64,
        png_hash: &str,
        thumbnail_relative_path: &Path,
    ) -> Result<Model, LibraryError> {
        let relative = validated_relative_path(thumbnail_relative_path)?;
        if Path::new(&relative).components().next()
            != Some(Component::Normal("thumbnails".as_ref()))
        {
            return Err(LibraryError::InvalidInput(
                "custom thumbnail must be stored under thumbnails".into(),
            ));
        }
        validated_thumbnail_cache_path(&self.root, Path::new(&relative), png_hash)?;
        let tx = self.conn.unchecked_transaction()?;
        let row: Option<(i64, Option<String>)> = tx
            .query_row(
                "SELECT metadata_version,deleted_at FROM models WHERE id=?1",
                params![model_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (actual, deleted) = row.ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))?;
        if actual as u64 != expected_version {
            return Err(LibraryError::MetadataConflict {
                model_id: model_id.into(),
                expected: expected_version,
                actual: actual as u64,
            });
        }
        if deleted.is_some() {
            return Err(LibraryError::InvalidInput(
                "removed model cannot change preview".into(),
            ));
        }
        tx.execute(
            "DELETE FROM thumbnails WHERE model_id=?1 AND is_custom=1",
            params![model_id],
        )?;
        tx.execute(
            "UPDATE thumbnails SET is_active=0 WHERE model_id=?1",
            params![model_id],
        )?;
        let ordinal: i64 = tx.query_row(
            "SELECT COALESCE(MAX(ordinal)+1,0) FROM thumbnails WHERE model_id=?1",
            params![model_id],
            |row| row.get(0),
        )?;
        tx.execute("INSERT INTO thumbnails(id,model_id,relative_path,content_hash,is_custom,source_content_hash,ordinal,is_active) VALUES(?1,?2,?3,?4,1,NULL,?5,1)",params![new_id("custom-thumbnail"),model_id,relative,png_hash,ordinal])?;
        tx.execute(
            "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
            params![timestamp(), model_id],
        )?;
        tx.commit()?;
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    pub fn clear_custom_thumbnail(
        &self,
        model_id: &str,
        expected_version: u64,
    ) -> Result<Model, LibraryError> {
        let tx = self.conn.unchecked_transaction()?;
        let row: Option<(i64, Option<String>)> = tx
            .query_row(
                "SELECT metadata_version,deleted_at FROM models WHERE id=?1",
                params![model_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (actual, deleted) = row.ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))?;
        if actual as u64 != expected_version {
            return Err(LibraryError::MetadataConflict {
                model_id: model_id.into(),
                expected: expected_version,
                actual: actual as u64,
            });
        }
        if deleted.is_some() {
            return Err(LibraryError::InvalidInput(
                "removed model cannot change preview".into(),
            ));
        }
        tx.execute(
            "DELETE FROM thumbnails WHERE model_id=?1 AND is_custom=1",
            params![model_id],
        )?;
        select_thumbnail_fallback(&tx, model_id)?;
        tx.execute(
            "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
            params![timestamp(), model_id],
        )?;
        tx.commit()?;
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    pub fn list_thumbnail_gallery(
        &self,
        model_id: &str,
    ) -> Result<Vec<ThumbnailGalleryItem>, LibraryError> {
        self.ensure_model(model_id)?;
        let mut stmt = self.conn.prepare(
            "SELECT t.id,t.model_id,t.relative_path,t.content_hash,t.source_content_hash,
                    t.is_custom,t.ordinal,t.is_active
             FROM thumbnails t JOIN models m ON m.id=t.model_id
             LEFT JOIN revisions r ON r.id=m.current_revision_id
             LEFT JOIN indexed_model_data d ON d.model_id=m.id
             WHERE t.model_id=?1
               AND (t.is_custom=1 OR t.source_content_hash IS NULL
                    OR t.source_content_hash=COALESCE(r.blob_hash,d.content_hash))
             ORDER BY t.ordinal,t.id",
        )?;
        let rows = stmt.query_map(params![model_id], thumbnail_gallery_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn add_custom_thumbnail(
        &self,
        model_id: &str,
        expected_version: u64,
        png_hash: &str,
        thumbnail_relative_path: &Path,
    ) -> Result<(Model, ThumbnailGalleryItem), LibraryError> {
        let relative =
            validate_thumbnail_record_path(&self.root, thumbnail_relative_path, png_hash)?;
        let tx = self.conn.unchecked_transaction()?;
        validate_active_model_version(&tx, model_id, expected_version)?;
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM thumbnails WHERE model_id=?1",
            params![model_id],
            |row| row.get(0),
        )?;
        if count >= 12 {
            return Err(LibraryError::InvalidInput(
                "thumbnail gallery is limited to 12 images".into(),
            ));
        }
        let ordinal: i64 = tx.query_row(
            "SELECT COALESCE(MAX(ordinal)+1,0) FROM thumbnails WHERE model_id=?1",
            params![model_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "UPDATE thumbnails SET is_active=0 WHERE model_id=?1",
            params![model_id],
        )?;
        let id = new_id("custom-thumbnail");
        tx.execute(
            "INSERT INTO thumbnails(id,model_id,relative_path,content_hash,is_custom,
             source_content_hash,ordinal,is_active) VALUES(?1,?2,?3,?4,1,NULL,?5,1)",
            params![id, model_id, relative, png_hash, ordinal],
        )?;
        tx.execute(
            "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
            params![timestamp(), model_id],
        )?;
        tx.commit()?;
        let model = self
            .get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))?;
        let item = self
            .list_thumbnail_gallery(model_id)?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| LibraryError::InvalidInput("thumbnail gallery item missing".into()))?;
        Ok((model, item))
    }

    pub fn set_active_thumbnail(
        &self,
        model_id: &str,
        expected_version: u64,
        thumbnail_id: &str,
    ) -> Result<Model, LibraryError> {
        let tx = self.conn.unchecked_transaction()?;
        validate_active_model_version(&tx, model_id, expected_version)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM thumbnails t JOIN models m ON m.id=t.model_id
             LEFT JOIN revisions r ON r.id=m.current_revision_id
             LEFT JOIN indexed_model_data d ON d.model_id=m.id
             WHERE t.id=?1 AND t.model_id=?2
               AND (t.is_custom=1 OR t.source_content_hash IS NULL
                    OR t.source_content_hash=COALESCE(r.blob_hash,d.content_hash)))",
            params![thumbnail_id, model_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(LibraryError::InvalidInput(
                "thumbnail gallery item not found".into(),
            ));
        }
        tx.execute(
            "UPDATE thumbnails SET is_active=0 WHERE model_id=?1",
            params![model_id],
        )?;
        tx.execute(
            "UPDATE thumbnails SET is_active=1 WHERE id=?1 AND model_id=?2",
            params![thumbnail_id, model_id],
        )?;
        tx.execute(
            "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
            params![timestamp(), model_id],
        )?;
        tx.commit()?;
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    pub fn delete_thumbnail_gallery_item(
        &self,
        model_id: &str,
        expected_version: u64,
        thumbnail_id: &str,
    ) -> Result<(Model, PathBuf), LibraryError> {
        let tx = self.conn.unchecked_transaction()?;
        validate_active_model_version(&tx, model_id, expected_version)?;
        let item: Option<(String, String, bool)> = tx
            .query_row(
                "SELECT relative_path,content_hash,is_active FROM thumbnails
                 WHERE id=?1 AND model_id=?2",
                params![thumbnail_id, model_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((relative_path, content_hash, active)) = item else {
            return Err(LibraryError::InvalidInput(
                "thumbnail gallery item not found".into(),
            ));
        };
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM thumbnails WHERE model_id=?1",
            params![model_id],
            |row| row.get(0),
        )?;
        if active || count <= 1 {
            return Err(LibraryError::InvalidInput(
                "active or last thumbnail cannot be deleted".into(),
            ));
        }
        let absolute =
            validated_thumbnail_cache_path(&self.root, Path::new(&relative_path), &content_hash)?;
        tx.execute("DELETE FROM thumbnails WHERE id=?1", params![thumbnail_id])?;
        compact_thumbnail_ordinals(&tx, model_id)?;
        tx.execute(
            "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
            params![timestamp(), model_id],
        )?;
        tx.commit()?;
        let model = self
            .get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))?;
        Ok((model, absolute))
    }

    fn set_thumbnail_for_source_with_expected_settings(
        &self,
        model_id: &str,
        source_content_hash: &str,
        expected_settings: Option<&CatalogSettings>,
        content_hash: &str,
        thumbnail_relative_path: &Path,
    ) -> Result<(), LibraryError> {
        let relative = validated_relative_path(thumbnail_relative_path)?;
        if Path::new(&relative).components().next()
            != Some(Component::Normal("thumbnails".as_ref()))
        {
            return Err(LibraryError::InvalidInput(
                "thumbnail must be stored in the managed thumbnails directory".into(),
            ));
        }
        resolve_under_root(&self.root, Path::new(&relative))?;
        validate_hash(content_hash)?;
        validate_hash(source_content_hash)?;
        let tx = self.conn.unchecked_transaction()?;
        let current_source_hash: Option<String> = tx
            .query_row(
                "SELECT COALESCE(r.blob_hash, d.content_hash)
                 FROM models m
                 LEFT JOIN revisions r ON r.id = m.current_revision_id
                 LEFT JOIN indexed_model_data d ON d.model_id = m.id
                 WHERE m.id = ?1",
                params![model_id],
                |row| row.get(0),
            )
            .optional()?;
        let current_source_hash =
            current_source_hash.ok_or_else(|| LibraryError::ModelNotFound(model_id.to_owned()))?;
        if current_source_hash != source_content_hash {
            return Err(LibraryError::SourceChanged);
        }
        if let Some(expected) = expected_settings {
            let current: (i64, String) = tx.query_row(
                "SELECT thumbnail_size, thumbnail_background
                 FROM catalog_settings WHERE id = 'local'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if current.0 != i64::from(expected.thumbnail_size)
                || current.1 != expected.thumbnail_background
            {
                return Err(LibraryError::InvalidInput(
                    "catalog thumbnail settings changed while rendering".into(),
                ));
            }
        }
        let unchanged: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM thumbnails
             WHERE model_id=?1 AND is_custom=0 AND relative_path=?2
               AND content_hash=?3 AND source_content_hash=?4)",
            params![model_id, relative, content_hash, source_content_hash],
            |row| row.get(0),
        )?;
        if unchanged {
            tx.commit()?;
            return Ok(());
        }
        tx.execute(
            "DELETE FROM thumbnails WHERE model_id = ?1 AND is_custom = 0",
            params![model_id],
        )?;
        let ordinal: i64 = tx.query_row(
            "SELECT COALESCE(MAX(ordinal)+1,0) FROM thumbnails WHERE model_id=?1",
            params![model_id],
            |row| row.get(0),
        )?;
        let has_active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM thumbnails WHERE model_id=?1 AND is_active=1)",
            params![model_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO thumbnails
             (id, model_id, relative_path, content_hash, is_custom, source_content_hash,ordinal,is_active)
             VALUES (?1, ?2, ?3, ?4, 0, ?5,?6,?7)",
            params![
                new_id("thumbnail"),
                model_id,
                relative,
                content_hash,
                source_content_hash,
                ordinal,
                !has_active
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn thumbnail_for_model(&self, model_id: &str) -> Result<Option<PathBuf>, LibraryError> {
        Ok(self
            .thumbnail_for_model_with_hash(model_id)?
            .map(|(path, _)| path))
    }

    pub fn thumbnail_for_model_with_hash(
        &self,
        model_id: &str,
    ) -> Result<Option<(PathBuf, String)>, LibraryError> {
        Ok(self
            .thumbnail_cache_entry_for_model(model_id)?
            .map(|entry| (entry.path, entry.png_hash)))
    }

    pub fn thumbnail_cache_entry_for_model(
        &self,
        model_id: &str,
    ) -> Result<Option<ThumbnailCacheEntry>, LibraryError> {
        let thumbnail: Option<(String, String, bool)> = self
            .conn
            .query_row(
                "SELECT t.relative_path, t.content_hash, t.is_custom FROM thumbnails t
                 LEFT JOIN revisions r ON r.id = (SELECT current_revision_id FROM models WHERE id = ?1)
                 LEFT JOIN indexed_model_data d ON d.model_id = ?1
                 WHERE t.model_id = ?1
                   AND (t.is_custom = 1 OR t.source_content_hash IS NULL
                        OR t.source_content_hash = COALESCE(r.blob_hash, d.content_hash))
                 ORDER BY t.is_custom DESC, t.id LIMIT 1",
                params![model_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        thumbnail
            .map(|(path, png_hash, is_custom)| {
                resolve_under_root(&self.root, Path::new(&path)).map(|absolute| {
                    ThumbnailCacheEntry {
                        path: absolute,
                        png_hash,
                        is_custom,
                    }
                })
            })
            .transpose()
    }

    pub fn thumbnail_relative_paths(&self) -> Result<HashSet<String>, LibraryError> {
        let mut stmt = self.conn.prepare("SELECT relative_path FROM thumbnails")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<Result<HashSet<_>, _>>()?)
    }
    pub fn catalog_content_hashes(&self) -> Result<HashSet<String>, LibraryError> {
        let mut hashes = HashSet::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT blob_hash FROM revisions UNION SELECT content_hash FROM indexed_model_data",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                hashes.insert(row?);
            }
        }
        Ok(hashes)
    }
    pub fn max_metadata_version(&self) -> Result<u64, LibraryError> {
        let value: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(metadata_version),0) FROM models",
            [],
            |row| row.get(0),
        )?;
        u64::try_from(value)
            .map_err(|_| LibraryError::InvalidInput("catalog metadata version is invalid".into()))
    }
    pub fn bump_all_metadata_versions_above(&self, floor: u64) -> Result<(), LibraryError> {
        let next = floor
            .checked_add(1)
            .ok_or_else(|| LibraryError::InvalidInput("metadata version exhausted".into()))?;
        let next = i64::try_from(next)
            .map_err(|_| LibraryError::InvalidInput("metadata version exhausted".into()))?;
        self.conn.execute(
            "UPDATE models SET metadata_version=?1,updated_at=?2",
            params![next, timestamp()],
        )?;
        Ok(())
    }

    pub fn purge_stale_thumbnails(&self) -> Result<Vec<PathBuf>, LibraryError> {
        self.purge_thumbnail_records(true)
    }

    pub fn purge_thumbnails(&self) -> Result<Vec<PathBuf>, LibraryError> {
        self.purge_thumbnail_records(false)
    }

    fn purge_thumbnail_records(&self, stale_only: bool) -> Result<Vec<PathBuf>, LibraryError> {
        let predicate = if stale_only {
            "t.is_custom = 0 AND t.source_content_hash IS NOT NULL
             AND t.source_content_hash != COALESCE(r.blob_hash, d.content_hash, '')"
        } else {
            "t.is_custom = 0"
        };
        let sql = format!(
            "SELECT t.id, t.model_id, t.relative_path, t.content_hash FROM thumbnails t
             JOIN models m ON m.id = t.model_id
             LEFT JOIN revisions r ON r.id = m.current_revision_id
             LEFT JOIN indexed_model_data d ON d.model_id = m.id
             WHERE {predicate}"
        );
        let mut records = Vec::new();
        {
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            records.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }
        let preserved_paths = {
            let mut stmt = self
                .conn
                .prepare("SELECT relative_path FROM thumbnails WHERE is_custom = 1")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<HashSet<_>, _>>()?
        };
        let validated_paths = records
            .iter()
            .map(|(_, _, path, hash)| {
                validated_thumbnail_cache_path(&self.root, Path::new(path), hash)
                    .map(|absolute| (path, absolute))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let tx = self.conn.unchecked_transaction()?;
        let affected = records
            .iter()
            .map(|(_, model_id, _, _)| model_id.clone())
            .collect::<BTreeSet<_>>();
        for (id, _, _, _) in &records {
            tx.execute("DELETE FROM thumbnails WHERE id = ?1", params![id])?;
        }
        for model_id in affected {
            select_thumbnail_fallback(&tx, &model_id)?;
        }
        tx.commit()?;
        Ok(validated_paths
            .into_iter()
            .filter_map(|(relative, absolute)| {
                (!preserved_paths.contains(relative)).then_some(absolute)
            })
            .collect())
    }

    fn get_index_root(&self, id: &str) -> Result<Option<IndexRoot>, LibraryError> {
        self.conn
            .query_row(
                "SELECT id, path, enabled, last_scanned_at, scan_interval_minutes, max_file_bytes, scan_zip
                 FROM index_roots WHERE id = ?1",
                params![id],
                index_root_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn ensure_root_does_not_overlap(
        &self,
        path: &Path,
        excluding_id: Option<&str>,
    ) -> Result<(), LibraryError> {
        for existing in self.list_index_roots()? {
            if excluding_id == Some(existing.id.as_str()) {
                continue;
            }
            if path.starts_with(&existing.path) || existing.path.starts_with(path) {
                return Err(LibraryError::InvalidInput(
                    "index roots cannot duplicate or overlap".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn list_tags(&self) -> Result<Vec<Tag>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT id,name,category FROM tags ORDER BY COALESCE(category,''),lower(name),id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Tag {
                id: r.get(0)?,
                name: r.get(1)?,
                category: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn create_tag(&self, name: &str, category: Option<&str>) -> Result<Tag, LibraryError> {
        let name = clean_tag_name(name)?;
        let category = clean_optional(category, 120)?;
        let id = new_id("tag");
        self.conn.execute(
            "INSERT INTO tags(id,name,category) VALUES(?1,?2,?3)",
            params![id, name, category],
        )?;
        Ok(Tag { id, name, category })
    }

    pub fn rename_tag(
        &self,
        id: &str,
        name: &str,
        category: Option<&str>,
    ) -> Result<Tag, LibraryError> {
        let name = clean_tag_name(name)?;
        let category = clean_optional(category, 120)?;
        let tx = self.conn.unchecked_transaction()?;
        let affected = tag_model_ids(&tx, id)?;
        if tx.execute(
            "UPDATE tags SET name=?1,category=?2 WHERE id=?3",
            params![name, category, id],
        )? == 0
        {
            return Err(LibraryError::InvalidInput("tag not found".into()));
        }
        bump_models(&tx, &affected)?;
        tx.commit()?;
        Ok(Tag {
            id: id.into(),
            name,
            category,
        })
    }

    pub fn delete_tag(&self, id: &str) -> Result<(), LibraryError> {
        let tx = self.conn.unchecked_transaction()?;
        let affected = tag_model_ids(&tx, id)?;
        if tx.execute("DELETE FROM tags WHERE id=?1", params![id])? == 0 {
            return Err(LibraryError::InvalidInput("tag not found".into()));
        }
        bump_models(&tx, &affected)?;
        tx.commit()?;
        Ok(())
    }

    pub fn add_model_tags_by_name(
        &self,
        model_id: &str,
        expected_version: u64,
        names: &[String],
    ) -> Result<Model, LibraryError> {
        let names = clean_model_tag_names(names)?;
        let tx = self.conn.unchecked_transaction()?;
        validate_active_model_version(&tx, model_id, expected_version)?;
        let mut changed = false;
        for name in names {
            let tag_id = tx
                .query_row("SELECT id FROM tags WHERE name=?1", params![name], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?;
            let tag_id = if let Some(id) = tag_id {
                id
            } else {
                let id = new_id("tag");
                tx.execute(
                    "INSERT INTO tags(id,name,category) VALUES(?1,?2,NULL)",
                    params![id, name],
                )?;
                id
            };
            changed |= tx.execute(
                "INSERT OR IGNORE INTO model_tags(model_id,tag_id) VALUES(?1,?2)",
                params![model_id, tag_id],
            )? > 0;
        }
        if changed {
            tx.execute(
                "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
                params![timestamp(), model_id],
            )?;
        }
        tx.commit()?;
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    pub fn remove_model_tags_by_name(
        &self,
        model_id: &str,
        expected_version: u64,
        names: &[String],
    ) -> Result<Model, LibraryError> {
        let names = clean_model_tag_names(names)?;
        let tx = self.conn.unchecked_transaction()?;
        validate_active_model_version(&tx, model_id, expected_version)?;
        let mut changed = false;
        for name in names {
            let tag_id = tx
                .query_row("SELECT id FROM tags WHERE name=?1", params![name], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?;
            if let Some(tag_id) = tag_id {
                changed |= tx.execute(
                    "DELETE FROM model_tags WHERE model_id=?1 AND tag_id=?2",
                    params![model_id, tag_id],
                )? > 0;
            }
        }
        if changed {
            tx.execute(
                "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
                params![timestamp(), model_id],
            )?;
        }
        tx.commit()?;
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    pub fn apply_reviewed_tags(
        &self,
        targets: &[ReviewedTagTarget],
        strategy: ReviewedTagStrategy,
    ) -> Result<Vec<Model>, LibraryError> {
        if targets.is_empty() || targets.len() > 50_000 {
            return Err(LibraryError::InvalidInput(
                "reviewed tag target count is invalid".into(),
            ));
        }
        let mut model_ids = HashSet::new();
        let mut accepted_count = 0_usize;
        let mut validated = Vec::with_capacity(targets.len());
        for target in targets {
            accepted_count = accepted_count
                .checked_add(target.tags.len())
                .ok_or_else(|| LibraryError::InvalidInput("reviewed tag count overflow".into()))?;
            if !model_ids.insert(target.model_id.clone())
                || target.tags.len() > 200
                || accepted_count > 100_000
            {
                return Err(LibraryError::InvalidInput(
                    "reviewed tag target is duplicated or too large".into(),
                ));
            }
            let mut names = HashSet::new();
            let mut tags = Vec::with_capacity(target.tags.len());
            for tag in &target.tags {
                let name = clean_tag_name(&tag.name)?;
                let category = clean_optional(tag.category.as_deref(), 120)?;
                if !names.insert(name.to_lowercase()) {
                    return Err(LibraryError::InvalidInput(
                        "reviewed tags contain duplicate names".into(),
                    ));
                }
                tags.push((name, category));
            }
            validated.push((target, tags));
        }

        let tx = self.conn.unchecked_transaction()?;
        for (target, _) in &validated {
            validate_active_model_version(&tx, &target.model_id, target.expected_version)?;
        }
        for (target, tags) in &validated {
            let mut accepted = BTreeMap::<String, Option<String>>::new();
            let mut represented_categories = BTreeSet::<String>::new();
            for (name, requested_category) in tags {
                let existing: Option<(String, Option<String>)> = tx
                    .query_row(
                        "SELECT id,category FROM tags WHERE lower(name)=lower(?1)",
                        params![name],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                let (id, actual_category) = if let Some((id, actual_category)) = existing {
                    if requested_category.is_some() && requested_category != &actual_category {
                        return Err(LibraryError::InvalidInput(format!(
                            "reviewed tag category conflicts with existing tag: {name}"
                        )));
                    }
                    (id, actual_category)
                } else {
                    let id = new_id("tag");
                    tx.execute(
                        "INSERT INTO tags(id,name,category) VALUES(?1,?2,?3)",
                        params![id, name, requested_category],
                    )?;
                    (id, requested_category.clone())
                };
                if let Some(category) = requested_category {
                    represented_categories.insert(category.clone());
                }
                accepted.insert(id, actual_category);
            }

            let mut current = BTreeMap::<String, Option<String>>::new();
            {
                let mut stmt = tx.prepare(
                    "SELECT t.id,t.category FROM tags t JOIN model_tags mt ON mt.tag_id=t.id
                     WHERE mt.model_id=?1",
                )?;
                let rows = stmt.query_map(params![target.model_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })?;
                current.extend(rows.collect::<Result<Vec<_>, _>>()?);
            }
            let final_tags = match strategy {
                ReviewedTagStrategy::Append => {
                    let mut result = current.clone();
                    result.extend(accepted);
                    result
                }
                ReviewedTagStrategy::Replace => accepted,
                ReviewedTagStrategy::Merge => {
                    let mut result = current
                        .iter()
                        .filter(|(_, category)| {
                            category
                                .as_ref()
                                .is_none_or(|category| !represented_categories.contains(category))
                        })
                        .map(|(id, category)| (id.clone(), category.clone()))
                        .collect::<BTreeMap<_, _>>();
                    result.extend(accepted);
                    result
                }
            };
            if final_tags != current {
                tx.execute(
                    "DELETE FROM model_tags WHERE model_id=?1",
                    params![target.model_id],
                )?;
                for id in final_tags.keys() {
                    tx.execute(
                        "INSERT INTO model_tags(model_id,tag_id) VALUES(?1,?2)",
                        params![target.model_id, id],
                    )?;
                }
                tx.execute(
                    "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
                    params![timestamp(),target.model_id],
                )?;
            }
        }
        tx.commit()?;
        targets
            .iter()
            .map(|target| {
                self.get_model(&target.model_id)?
                    .ok_or_else(|| LibraryError::ModelNotFound(target.model_id.clone()))
            })
            .collect()
    }

    pub fn update_model_metadata(
        &self,
        model_id: &str,
        expected_version: u64,
        patch: &MetadataPatch,
    ) -> Result<Model, LibraryError> {
        self.update_models_metadata(&[VersionedModelUpdate {
            model_id: model_id.into(),
            expected_version,
            patch: patch.clone(),
        }])?
        .into_iter()
        .next()
        .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    /// Renames one designer, license, or parent-model name atomically. Parent links
    /// keep their stable model IDs when their referenced display name changes.
    pub fn rename_metadata_value(
        &self,
        kind: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<u64, LibraryError> {
        let (old_name, new_name, count_sql, update_sql) = match kind {
            "designer" | "license" => {
                let old_name = clean_optional(Some(old_name), 200)?
                    .ok_or_else(|| LibraryError::InvalidInput("oldName is required".into()))?;
                let new_name = clean_optional(Some(new_name), 200)?
                    .ok_or_else(|| LibraryError::InvalidInput("newName is required".into()))?;
                let column = if kind == "designer" {
                    "designer"
                } else {
                    "model_license"
                };
                (
                    old_name,
                    new_name,
                    format!("SELECT COUNT(*) FROM models WHERE deleted_at IS NULL AND {column}=?1"),
                    format!("UPDATE models SET {column}=?1,metadata_version=metadata_version+1,updated_at=?2 WHERE deleted_at IS NULL AND {column}=?3"),
                )
            }
            "parentModel" => {
                let old_name = clean_name(old_name, "Parent model")?;
                let new_name = clean_name(new_name, "Parent model")?;
                if old_name.len() > 200 || new_name.len() > 200 {
                    return Err(LibraryError::InvalidInput(
                        "Parent model name exceeds 200 characters".into(),
                    ));
                }
                (
                    old_name,
                    new_name,
                    "SELECT COUNT(*) FROM models WHERE deleted_at IS NULL AND display_name=?1".into(),
                    "UPDATE models SET display_name=?1,metadata_version=metadata_version+1,updated_at=?2 WHERE deleted_at IS NULL AND display_name=?3".into(),
                )
            }
            _ => {
                return Err(LibraryError::InvalidInput(
                    "Metadata kind must be designer, parentModel, or license".into(),
                ))
            }
        };
        if old_name == new_name {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        let count: i64 = tx.query_row(&count_sql, params![old_name], |row| row.get(0))?;
        if count > MAX_GLOBAL_METADATA_TARGETS {
            return Err(LibraryError::InvalidInput(
                "Global metadata rename exceeds 50000 models".into(),
            ));
        }
        let changed = tx.execute(&update_sql, params![new_name, timestamp(), old_name])?;
        tx.commit()?;
        Ok(changed as u64)
    }

    /// Clears one designer, license, or parent-model reference atomically.
    pub fn delete_metadata_value(&self, kind: &str, name: &str) -> Result<u64, LibraryError> {
        let name = match kind {
            "designer" | "license" => clean_optional(Some(name), 200)?
                .ok_or_else(|| LibraryError::InvalidInput("name is required".into()))?,
            "parentModel" => clean_name(name, "Parent model")?,
            _ => {
                return Err(LibraryError::InvalidInput(
                    "Metadata kind must be designer, parentModel, or license".into(),
                ))
            }
        };
        let (count_sql, update_sql) = match kind {
            "designer" => (
                "SELECT COUNT(*) FROM models WHERE deleted_at IS NULL AND designer=?1",
                "UPDATE models SET designer=NULL,metadata_version=metadata_version+1,updated_at=?1 WHERE deleted_at IS NULL AND designer=?2",
            ),
            "license" => (
                "SELECT COUNT(*) FROM models WHERE deleted_at IS NULL AND model_license=?1",
                "UPDATE models SET model_license=NULL,metadata_version=metadata_version+1,updated_at=?1 WHERE deleted_at IS NULL AND model_license=?2",
            ),
            "parentModel" => (
                "SELECT COUNT(*) FROM models WHERE deleted_at IS NULL AND parent_model_id IN
                   (SELECT id FROM models WHERE deleted_at IS NULL AND display_name=?1)",
                "UPDATE models SET parent_model_id=NULL,metadata_version=metadata_version+1,updated_at=?1
                 WHERE deleted_at IS NULL AND parent_model_id IN
                   (SELECT id FROM models WHERE deleted_at IS NULL AND display_name=?2)",
            ),
            _ => unreachable!("Metadata kind was validated above"),
        };
        let tx = self.conn.unchecked_transaction()?;
        let count: i64 = tx.query_row(count_sql, params![name], |row| row.get(0))?;
        if count > MAX_GLOBAL_METADATA_TARGETS {
            return Err(LibraryError::InvalidInput(
                "Global metadata delete exceeds 50000 models".into(),
            ));
        }
        let changed = tx.execute(update_sql, params![timestamp(), name])?;
        tx.commit()?;
        Ok(changed as u64)
    }

    pub fn update_models_metadata(
        &self,
        updates: &[VersionedModelUpdate],
    ) -> Result<Vec<Model>, LibraryError> {
        let mut unique = HashSet::new();
        for update in updates {
            if !unique.insert(update.model_id.clone()) {
                return Err(LibraryError::InvalidInput(
                    "duplicate model in metadata batch".into(),
                ));
            }
            validate_metadata_patch(&update.patch)?;
        }
        let tx = self.conn.unchecked_transaction()?;
        for update in updates {
            apply_metadata_patch(
                &tx,
                &update.model_id,
                update.expected_version,
                &update.patch,
            )?;
        }
        tx.commit()?;
        updates
            .iter()
            .map(|u| {
                self.get_model(&u.model_id)?
                    .ok_or_else(|| LibraryError::ModelNotFound(u.model_id.clone()))
            })
            .collect()
    }
    pub fn bulk_update_metadata(
        &self,
        targets: &[VersionedTarget],
        patch: &MetadataPatch,
    ) -> Result<Vec<Model>, LibraryError> {
        if targets.len() > 50_000 {
            return Err(LibraryError::InvalidInput(
                "metadata batch exceeds 50000 models".into(),
            ));
        }
        validate_metadata_patch(patch)?;
        let mut unique = HashSet::new();
        if targets.iter().any(|t| !unique.insert(t.model_id.clone())) {
            return Err(LibraryError::InvalidInput(
                "duplicate model in metadata batch".into(),
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        for target in targets {
            apply_metadata_patch(&tx, &target.model_id, target.expected_version, patch)?;
        }
        tx.commit()?;
        targets
            .iter()
            .map(|t| {
                self.get_model(&t.model_id)?
                    .ok_or_else(|| LibraryError::ModelNotFound(t.model_id.clone()))
            })
            .collect()
    }

    pub fn create_print_attempt(
        &self,
        model_id: &str,
        input: &PrintAttemptInput,
    ) -> Result<PrintAttempt, LibraryError> {
        validate_print_attempt(input)?;
        self.ensure_model(model_id)?;
        let id = new_id("print");
        let now = timestamp();
        let tx = self.conn.unchecked_transaction()?;
        write_print_attempt(&tx, &id, model_id, input, &now, true)?;
        recompute_print_summary(&tx, model_id)?;
        tx.commit()?;
        self.get_print_attempt(&id)?
            .ok_or_else(|| LibraryError::InvalidInput("print attempt missing".into()))
    }

    pub fn create_print_attempts_idempotent(
        &self,
        idempotency_key: &str,
        items: &[PrintAttemptBatchItem],
    ) -> Result<Vec<PrintAttempt>, LibraryError> {
        validate_plain_capture_id(idempotency_key, "print batch idempotency key")?;
        if items.is_empty() || items.len() > 10_000 {
            return Err(LibraryError::InvalidInput(
                "print attempt batch count is invalid".into(),
            ));
        }
        for item in items {
            validate_print_attempt(&item.input)?;
        }
        let payload = serde_json::to_vec(items)
            .map_err(|_| LibraryError::InvalidInput("print batch payload is invalid".into()))?;
        let payload_hash = format!("{:x}", Sha256::digest(&payload));
        if let Some((stored_hash, attempt_ids_json)) = self
            .conn
            .query_row(
                "SELECT payload_hash,attempt_ids_json FROM print_batch_receipts
                 WHERE idempotency_key=?1",
                params![idempotency_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
        {
            if stored_hash != payload_hash {
                return Err(LibraryError::IdempotencyConflict(idempotency_key.into()));
            }
            let ids: Vec<String> = serde_json::from_str(&attempt_ids_json)
                .map_err(|_| LibraryError::InvalidInput("print batch receipt is corrupt".into()))?;
            return ids
                .into_iter()
                .map(|id| {
                    self.get_print_attempt(&id)?.ok_or_else(|| {
                        LibraryError::InvalidInput("print batch receipt target is missing".into())
                    })
                })
                .collect();
        }

        let tx = self.conn.unchecked_transaction()?;
        let mut affected = BTreeSet::new();
        for item in items {
            let active: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM models WHERE id=?1 AND deleted_at IS NULL)",
                params![item.model_id],
                |row| row.get(0),
            )?;
            if !active {
                return Err(LibraryError::ModelNotFound(item.model_id.clone()));
            }
            affected.insert(item.model_id.clone());
        }
        let now = timestamp();
        let mut ids = Vec::with_capacity(items.len());
        for item in items {
            let id = new_id("print");
            write_print_attempt(&tx, &id, &item.model_id, &item.input, &now, true)?;
            ids.push(id);
        }
        for model_id in affected {
            recompute_print_summary(&tx, &model_id)?;
        }
        tx.execute(
            "INSERT INTO print_batch_receipts
             (idempotency_key,payload_hash,attempt_ids_json,created_at) VALUES(?1,?2,?3,?4)",
            params![
                idempotency_key,
                payload_hash,
                serde_json::to_string(&ids).map_err(|_| LibraryError::InvalidInput(
                    "print batch receipt could not be encoded".into()
                ))?,
                now
            ],
        )?;
        tx.commit()?;
        ids.into_iter()
            .map(|id| {
                self.get_print_attempt(&id)?.ok_or_else(|| {
                    LibraryError::InvalidInput("created print attempt is missing".into())
                })
            })
            .collect()
    }
    pub fn list_print_attempts(&self, model_id: &str) -> Result<Vec<PrintAttempt>, LibraryError> {
        let mut stmt=self.conn.prepare("SELECT id,model_id,printed_at,quantity,notes,outcome,created_at,updated_at FROM print_attempts WHERE model_id=?1 ORDER BY printed_at DESC,id DESC")?;
        let rows = stmt.query_map(params![model_id], attempt_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            let mut a = row?;
            a.filament_ids = attempt_filaments(&self.conn, &a.id)?;
            out.push(a);
        }
        Ok(out)
    }
    pub fn get_print_attempt(&self, id: &str) -> Result<Option<PrintAttempt>, LibraryError> {
        let mut a=self.conn.query_row("SELECT id,model_id,printed_at,quantity,notes,outcome,created_at,updated_at FROM print_attempts WHERE id=?1",params![id],attempt_from_row).optional()?;
        if let Some(ref mut value) = a {
            value.filament_ids = attempt_filaments(&self.conn, id)?;
        }
        Ok(a)
    }
    pub fn update_print_attempt(
        &self,
        id: &str,
        input: &PrintAttemptInput,
    ) -> Result<PrintAttempt, LibraryError> {
        validate_print_attempt(input)?;
        let tx = self.conn.unchecked_transaction()?;
        let model_id: String = tx
            .query_row(
                "SELECT model_id FROM print_attempts WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("print attempt not found".into()))?;
        write_print_attempt(&tx, id, &model_id, input, &timestamp(), false)?;
        recompute_print_summary(&tx, &model_id)?;
        tx.commit()?;
        self.get_print_attempt(id)?
            .ok_or_else(|| LibraryError::InvalidInput("print attempt not found".into()))
    }
    pub fn delete_print_attempt(&self, id: &str) -> Result<(), LibraryError> {
        let tx = self.conn.unchecked_transaction()?;
        let model_id: String = tx
            .query_row(
                "SELECT model_id FROM print_attempts WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| LibraryError::InvalidInput("print attempt not found".into()))?;
        tx.execute("DELETE FROM print_attempts WHERE id=?1", params![id])?;
        recompute_print_summary(&tx, &model_id)?;
        tx.commit()?;
        Ok(())
    }

    pub fn create_or_get_capture_receipt(
        &self,
        input: &CaptureReceiptInput,
    ) -> Result<CaptureReceipt, LibraryError> {
        validate_capture_receipt(input)?;
        if let Some(existing_hash) = self
            .conn
            .query_row(
                "SELECT payload_hash FROM capture_items WHERE item_id=?1",
                params![input.item_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            if existing_hash != input.payload_hash {
                return Err(LibraryError::InvalidInput(
                    "capture item ID conflicts with a different payload".into(),
                ));
            }
            return self
                .get_capture_receipt(&input.item_id)?
                .ok_or_else(|| LibraryError::InvalidInput("capture receipt is missing".into()));
        }
        let tx = self.conn.unchecked_transaction()?;
        for download in &input.downloads {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM capture_downloads WHERE capture_id=?1)",
                params![download.capture_id],
                |row| row.get(0),
            )?;
            if exists {
                return Err(LibraryError::InvalidInput(
                    "capture download ID is already owned by another item".into(),
                ));
            }
        }
        let parent_model_id = if let Some(parent) = &input.page.parent_external_id {
            tx.query_row(
                "SELECT page_model_id FROM capture_items WHERE item_id=?1",
                params![parent],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .ok_or_else(|| LibraryError::InvalidInput("capture parent item is unknown".into()))?
            .into()
        } else {
            None
        };
        let model_id = new_id("bookmark");
        let now = timestamp();
        tx.execute(
            "INSERT INTO models(id,display_name,description,folder_id,metadata_version,
             created_at,updated_at,source_kind,source_missing,designer,source_url,notes,
             model_license,parent_model_id,catalog_kind)
             VALUES(?1,?2,'','root',1,?3,?3,'managed',0,?4,?5,?6,?7,?8,'bookmark')",
            params![
                model_id,
                input.page.title.trim(),
                now,
                clean_optional(input.page.designer.as_deref(), 200)?,
                input.page.url.trim(),
                clean_optional(input.page.notes.as_deref(), 20_000)?,
                clean_optional(input.page.model_license.as_deref(), 200)?,
                parent_model_id
            ],
        )?;
        let status = if input.downloads.is_empty() {
            "imported"
        } else {
            "partial"
        };
        tx.execute(
            "INSERT INTO capture_items(item_id,payload_hash,status,page_model_id,created_at,
             received_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?6)",
            params![
                input.item_id,
                input.payload_hash,
                status,
                model_id,
                input.created_at,
                now
            ],
        )?;
        for (ordinal, download) in input.downloads.iter().enumerate() {
            tx.execute(
                "INSERT INTO capture_downloads(capture_id,item_id,ordinal,file_name,expected_hash,
                 expected_bytes,status) VALUES(?1,?2,?3,?4,?5,?6,'needs_file')",
                params![
                    download.capture_id,
                    input.item_id,
                    ordinal as i64,
                    download.file_name,
                    download.sha256,
                    download.bytes.map(|value| value as i64)
                ],
            )?;
        }
        tx.commit()?;
        self.get_capture_receipt(&input.item_id)?
            .ok_or_else(|| LibraryError::InvalidInput("capture receipt is missing".into()))
    }

    pub fn get_capture_receipt(
        &self,
        item_id: &str,
    ) -> Result<Option<CaptureReceipt>, LibraryError> {
        let item: Option<(String, String, String, Option<String>)> = self
            .conn
            .query_row(
                "SELECT item_id,payload_hash,status,page_model_id FROM capture_items WHERE item_id=?1",
                params![item_id],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            )
            .optional()?;
        let Some((item_id, payload_hash, status, page_model_id)) = item else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare(
            "SELECT d.capture_id,d.status,d.model_id,d.error_code,m.deleted_at
             FROM capture_downloads d LEFT JOIN models m ON m.id=d.model_id
             WHERE d.item_id=?1 ORDER BY d.ordinal,d.capture_id",
        )?;
        let rows = stmt.query_map(params![item_id], |row| {
            let status: String = row.get(1)?;
            let deleted_at: Option<String> = row.get(4)?;
            Ok(CaptureDownloadRecord {
                capture_id: row.get(0)?,
                status: if status == "imported" && deleted_at.is_some() {
                    "imported_removed".into()
                } else {
                    status
                },
                model_id: row.get(2)?,
                error_code: row.get(3)?,
            })
        })?;
        Ok(Some(CaptureReceipt {
            item_id,
            payload_hash,
            status,
            page_model_id,
            downloads: rows.collect::<Result<Vec<_>, _>>()?,
        }))
    }

    pub fn list_capture_summaries_page(
        &self,
        after_created_at: Option<&str>,
        after_item_id: Option<&str>,
        limit: u16,
    ) -> Result<(Vec<CaptureSummary>, bool), LibraryError> {
        if limit == 0 || limit > 500 || after_created_at.is_some() != after_item_id.is_some() {
            return Err(LibraryError::InvalidInput(
                "capture page cursor or limit is invalid".into(),
            ));
        }
        let mut stmt = self.conn.prepare(
            "SELECT ci.item_id,ci.created_at,ci.status,ci.page_model_id,
                    COALESCE(m.display_name,''),COALESCE(m.source_url,''),
                    COUNT(cd.capture_id),
                    COALESCE(SUM(cd.status='failed'),0),
                    COALESCE(SUM(cd.status='needs_file'),0)
             FROM capture_items ci
             LEFT JOIN models m ON m.id=ci.page_model_id
             LEFT JOIN capture_downloads cd ON cd.item_id=ci.item_id
             WHERE ?1 IS NULL OR ci.created_at < ?1
                OR (ci.created_at = ?1 AND ci.item_id < ?2)
             GROUP BY ci.item_id,ci.created_at,ci.status,ci.page_model_id,
                      m.display_name,m.source_url
             ORDER BY ci.created_at DESC,ci.item_id DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![after_created_at, after_item_id, i64::from(limit) + 1],
            |row| {
                Ok(CaptureSummary {
                    item_id: row.get(0)?,
                    created_at: row.get(1)?,
                    status: row.get(2)?,
                    page_model_id: row.get(3)?,
                    title: row.get(4)?,
                    url: row.get(5)?,
                    download_count: row.get::<_, i64>(6)? as u64,
                    failed_count: row.get::<_, i64>(7)? as u64,
                    needs_file_count: row.get::<_, i64>(8)? as u64,
                })
            },
        )?;
        let mut summaries = rows.collect::<Result<Vec<_>, _>>()?;
        let has_more = summaries.len() > usize::from(limit);
        summaries.truncate(usize::from(limit));
        Ok((summaries, has_more))
    }

    pub fn update_capture_download_result(
        &self,
        item_id: &str,
        capture_id: &str,
        model_id: Option<&str>,
        error_code: Option<&str>,
    ) -> Result<CaptureReceipt, LibraryError> {
        if model_id.is_some() == error_code.is_some() {
            return Err(LibraryError::InvalidInput(
                "capture result must be either imported or failed".into(),
            ));
        }
        if let Some(code) = error_code {
            validate_plain_capture_id(code, "capture error code")?;
        }
        let tx = self.conn.unchecked_transaction()?;
        if let Some(model_id) = model_id {
            let active: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM models WHERE id=?1 AND deleted_at IS NULL)",
                params![model_id],
                |row| row.get(0),
            )?;
            if !active {
                return Err(LibraryError::ModelNotFound(model_id.into()));
            }
        }
        let changed = tx.execute(
            "UPDATE capture_downloads SET status=?1,model_id=?2,error_code=?3
             WHERE item_id=?4 AND capture_id=?5 AND status!='imported'",
            params![
                if model_id.is_some() {
                    "imported"
                } else {
                    "failed"
                },
                model_id,
                error_code,
                item_id,
                capture_id
            ],
        )?;
        if changed == 0 {
            let existing: Option<Option<String>> = tx
                .query_row(
                    "SELECT model_id FROM capture_downloads WHERE item_id=?1 AND capture_id=?2",
                    params![item_id, capture_id],
                    |row| row.get(0),
                )
                .optional()?;
            if existing.flatten().as_deref() != model_id {
                return Err(LibraryError::InvalidInput(
                    "capture result conflicts with an existing import".into(),
                ));
            }
        }
        let (remaining, failed): (i64, i64) = tx.query_row(
            "SELECT SUM(status!='imported'),SUM(status='failed') FROM capture_downloads WHERE item_id=?1",
            params![item_id],
            |row| Ok((row.get::<_,Option<i64>>(0)?.unwrap_or(0),row.get::<_,Option<i64>>(1)?.unwrap_or(0))),
        )?;
        let item_status = if remaining == 0 {
            "imported"
        } else if failed > 0 {
            "failed"
        } else {
            "partial"
        };
        tx.execute(
            "UPDATE capture_items SET status=?1,updated_at=?2 WHERE item_id=?3",
            params![item_status, timestamp(), item_id],
        )?;
        tx.commit()?;
        self.get_capture_receipt(item_id)?
            .ok_or_else(|| LibraryError::InvalidInput("capture receipt is missing".into()))
    }

    pub fn create_filament(&self, input: &FilamentInput) -> Result<Filament, LibraryError> {
        let input = validate_filament_input(input)?;
        let id = new_id("filament");
        let now = timestamp();
        self.conn.execute(
            "INSERT INTO filaments(id,version,brand,material,color,name,notes,origin,resource_kind,
             archived,source_missing,created_at,updated_at)
             VALUES(?1,1,?2,?3,?4,?5,?6,'manual','manual',0,0,?7,?7)",
            params![
                id,
                input.brand,
                input.material,
                input.color,
                input.name,
                input.notes,
                now
            ],
        )?;
        self.get_filament(&id)?
            .ok_or_else(|| LibraryError::InvalidInput("created filament is missing".into()))
    }

    pub fn get_filament(&self, id: &str) -> Result<Option<Filament>, LibraryError> {
        self.conn
            .query_row(
                &format!("{FILAMENT_SELECT} WHERE f.id=?1"),
                params![id],
                filament_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_filaments(&self, include_unavailable: bool) -> Result<Vec<Filament>, LibraryError> {
        let mut stmt = self.conn.prepare(&format!(
            "{FILAMENT_SELECT} WHERE ?1 OR (f.archived=0 AND f.source_missing=0)
             ORDER BY lower(f.brand),lower(f.material),lower(f.name),f.resource_kind,f.external_id,f.id"
        ))?;
        let rows = stmt.query_map(params![include_unavailable], filament_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn update_filament(
        &self,
        id: &str,
        expected_version: u64,
        input: &FilamentInput,
    ) -> Result<Filament, LibraryError> {
        let input = validate_filament_input(input)?;
        let actual: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT version,origin FROM filaments WHERE id=?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((actual, origin)) = actual else {
            return Err(LibraryError::InvalidInput("filament not found".into()));
        };
        if actual as u64 != expected_version {
            return Err(LibraryError::FilamentVersionConflict {
                filament_id: id.into(),
                expected: expected_version,
                actual: actual as u64,
            });
        }
        if origin != "manual" {
            return Err(LibraryError::InvalidInput(
                "Spoolman filaments are updated by synchronization".into(),
            ));
        }
        self.conn.execute(
            "UPDATE filaments SET brand=?1,material=?2,color=?3,name=?4,notes=?5,
             version=version+1,updated_at=?6 WHERE id=?7",
            params![
                input.brand,
                input.material,
                input.color,
                input.name,
                input.notes,
                timestamp(),
                id
            ],
        )?;
        self.get_filament(id)?
            .ok_or_else(|| LibraryError::InvalidInput("filament not found".into()))
    }

    pub fn delete_filament(
        &self,
        id: &str,
        expected_version: u64,
    ) -> Result<Option<Filament>, LibraryError> {
        let tx = self.conn.unchecked_transaction()?;
        let actual: Option<(i64, String)> = tx
            .query_row(
                "SELECT version,origin FROM filaments WHERE id=?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((actual, origin)) = actual else {
            return Err(LibraryError::InvalidInput("filament not found".into()));
        };
        if actual as u64 != expected_version {
            return Err(LibraryError::FilamentVersionConflict {
                filament_id: id.into(),
                expected: expected_version,
                actual: actual as u64,
            });
        }
        if origin != "manual" {
            return Err(LibraryError::InvalidInput(
                "Spoolman filaments cannot be deleted locally".into(),
            ));
        }
        let referenced: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM model_filaments WHERE filament_id=?1)
                 OR EXISTS(SELECT 1 FROM print_attempt_filaments WHERE filament_id=?1)",
            params![id],
            |row| row.get(0),
        )?;
        if referenced {
            tx.execute(
                "UPDATE filaments SET archived=1,version=version+1,updated_at=?1 WHERE id=?2",
                params![timestamp(), id],
            )?;
        } else {
            tx.execute("DELETE FROM filaments WHERE id=?1", params![id])?;
        }
        tx.commit()?;
        self.get_filament(id)
    }

    pub fn set_model_filaments(
        &self,
        model_id: &str,
        expected_version: u64,
        filament_ids: &[String],
    ) -> Result<Model, LibraryError> {
        let tx = self.conn.unchecked_transaction()?;
        set_model_filaments_tx(&tx, model_id, expected_version, filament_ids)?;
        tx.commit()?;
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    pub fn bulk_set_model_filaments(
        &self,
        targets: &[VersionedTarget],
        filament_ids: &[String],
    ) -> Result<Vec<Model>, LibraryError> {
        if targets.is_empty() || targets.len() > 50_000 {
            return Err(LibraryError::InvalidInput(
                "filament assignment target count is invalid".into(),
            ));
        }
        validate_filament_ids(filament_ids)?;
        let mut unique = HashSet::new();
        if targets
            .iter()
            .any(|target| !unique.insert(&target.model_id))
        {
            return Err(LibraryError::InvalidInput(
                "duplicate filament assignment target".into(),
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        validate_catalog_filament_ids(&tx, filament_ids)?;
        for target in targets {
            validate_active_model_version(&tx, &target.model_id, target.expected_version)?;
        }
        for target in targets {
            write_model_filaments_tx(&tx, &target.model_id, filament_ids)?;
        }
        tx.commit()?;
        targets
            .iter()
            .map(|target| {
                self.get_model(&target.model_id)?
                    .ok_or_else(|| LibraryError::ModelNotFound(target.model_id.clone()))
            })
            .collect()
    }

    pub fn bulk_assign_model_filaments(
        &self,
        assignments: &[ModelFilamentAssignment],
    ) -> Result<Vec<Model>, LibraryError> {
        if assignments.is_empty() || assignments.len() > 50_000 {
            return Err(LibraryError::InvalidInput(
                "filament assignment count is invalid".into(),
            ));
        }
        let mut unique_models = HashSet::new();
        for assignment in assignments {
            if !unique_models.insert(&assignment.target.model_id) {
                return Err(LibraryError::InvalidInput(
                    "duplicate filament assignment target".into(),
                ));
            }
            validate_filament_ids(&assignment.filament_ids)?;
        }
        let tx = self.conn.unchecked_transaction()?;
        let all_filament_ids = assignments
            .iter()
            .flat_map(|assignment| assignment.filament_ids.iter())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        validate_catalog_filament_ids(&tx, &all_filament_ids)?;
        for assignment in assignments {
            validate_active_model_version(
                &tx,
                &assignment.target.model_id,
                assignment.target.expected_version,
            )?;
        }
        for assignment in assignments {
            write_model_filaments_tx(&tx, &assignment.target.model_id, &assignment.filament_ids)?;
        }
        tx.commit()?;
        assignments
            .iter()
            .map(|assignment| {
                self.get_model(&assignment.target.model_id)?
                    .ok_or_else(|| LibraryError::ModelNotFound(assignment.target.model_id.clone()))
            })
            .collect()
    }

    pub fn upsert_spoolman_source(&self, base_url: &str) -> Result<SpoolmanSource, LibraryError> {
        let base_url = validate_spoolman_base_url(base_url)?;
        if let Some(source) = self
            .conn
            .query_row(
                "SELECT id,base_url,last_synced_at FROM filament_sources WHERE base_url=?1",
                params![base_url],
                spoolman_source_from_row,
            )
            .optional()?
        {
            return Ok(source);
        }
        let id = new_id("spoolman");
        let now = timestamp();
        self.conn.execute(
            "INSERT INTO filament_sources(id,kind,base_url,created_at,updated_at)
             VALUES(?1,'spoolman',?2,?3,?3)",
            params![id, base_url, now],
        )?;
        self.get_spoolman_source(&id)?
            .ok_or_else(|| LibraryError::InvalidInput("Spoolman source is missing".into()))
    }

    pub fn get_spoolman_source(&self, id: &str) -> Result<Option<SpoolmanSource>, LibraryError> {
        self.conn
            .query_row(
                "SELECT id,base_url,last_synced_at FROM filament_sources WHERE id=?1 AND kind='spoolman'",
                params![id],
                spoolman_source_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_spoolman_sources(&self) -> Result<Vec<SpoolmanSource>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT id,base_url,last_synced_at FROM filament_sources WHERE kind='spoolman' ORDER BY lower(base_url),id",
        )?;
        let rows = stmt.query_map([], spoolman_source_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn sync_spoolman_snapshot(
        &self,
        source_id: &str,
        filaments: &[SpoolmanFilamentRecord],
        spools: &[SpoolmanSpoolRecord],
        completed_at: &str,
    ) -> Result<FilamentSyncSummary, LibraryError> {
        if filaments.len().saturating_add(spools.len()) > 100_000 {
            return Err(LibraryError::InvalidInput(
                "Spoolman snapshot exceeds 100000 records".into(),
            ));
        }
        let completed_at = normalize_rfc3339(completed_at)?;
        let mut filament_records = Vec::with_capacity(filaments.len());
        let mut filament_external_ids = HashSet::new();
        for record in filaments {
            let external_id = validate_external_id(&record.external_id)?;
            if !filament_external_ids.insert(external_id.clone()) {
                return Err(LibraryError::InvalidInput(
                    "duplicate Spoolman filament ID".into(),
                ));
            }
            filament_records.push((external_id, validate_spoolman_filament(record)?));
        }
        let mut spool_records = Vec::with_capacity(spools.len());
        let mut spool_external_ids = HashSet::new();
        for record in spools {
            let external_id = validate_external_id(&record.external_id)?;
            let parent_external_id = validate_external_id(&record.filament_external_id)?;
            if !spool_external_ids.insert(external_id.clone()) {
                return Err(LibraryError::InvalidInput(
                    "duplicate Spoolman spool ID".into(),
                ));
            }
            if !filament_external_ids.contains(&parent_external_id) {
                return Err(LibraryError::InvalidInput(
                    "Spoolman spool references an unknown filament".into(),
                ));
            }
            spool_records.push((
                external_id,
                parent_external_id,
                validate_spoolman_spool(record)?,
                record.archived,
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        let source_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM filament_sources WHERE id=?1 AND kind='spoolman')",
            params![source_id],
            |row| row.get(0),
        )?;
        if !source_exists {
            return Err(LibraryError::InvalidInput(
                "Spoolman source not found".into(),
            ));
        }
        let mut summary = FilamentSyncSummary {
            source_id: source_id.into(),
            seen: filament_records.len() + spool_records.len(),
            completed_at: completed_at.clone(),
            ..FilamentSyncSummary::default()
        };
        let mut local_filament_ids = HashMap::new();
        let mut seen = HashSet::new();
        for (external_id, input) in filament_records {
            let (id, added, updated) = upsert_remote_filament(
                &tx,
                source_id,
                "filament",
                &external_id,
                None,
                &input,
                false,
            )?;
            summary.added += usize::from(added);
            summary.updated += usize::from(updated);
            local_filament_ids.insert(external_id.clone(), id);
            seen.insert(("filament".to_owned(), external_id));
        }
        for (external_id, parent_external_id, input, archived) in spool_records {
            let parent_id = local_filament_ids.get(&parent_external_id).ok_or_else(|| {
                LibraryError::InvalidInput("Spoolman filament identity is missing".into())
            })?;
            let (_id, added, updated) = upsert_remote_filament(
                &tx,
                source_id,
                "spool",
                &external_id,
                Some(parent_id),
                &input,
                archived,
            )?;
            summary.added += usize::from(added);
            summary.updated += usize::from(updated);
            seen.insert(("spool".to_owned(), external_id));
        }
        let mut existing = Vec::new();
        {
            let mut stmt = tx.prepare(
                "SELECT id,resource_kind,external_id,source_missing FROM filaments WHERE source_id=?1",
            )?;
            let rows = stmt.query_map(params![source_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            })?;
            existing.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }
        for (id, kind, external_id, was_missing) in existing {
            if !seen.contains(&(kind, external_id)) && !was_missing {
                tx.execute(
                    "UPDATE filaments SET source_missing=1,version=version+1,updated_at=?1 WHERE id=?2",
                    params![timestamp(),id],
                )?;
                summary.missing += 1;
            }
        }
        tx.execute(
            "UPDATE filament_sources SET last_synced_at=?1,updated_at=?2 WHERE id=?3",
            params![completed_at, timestamp(), source_id],
        )?;
        tx.commit()?;
        Ok(summary)
    }

    pub fn create_bundle(&self, model_ids: &[String]) -> Result<String, LibraryError> {
        if model_ids.is_empty() {
            return Err(LibraryError::InvalidInput("bundle requires models".into()));
        }
        let id = new_id("bundle");
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO bundles(id,created_at) VALUES(?1,?2)",
            params![id, timestamp()],
        )?;
        for model in model_ids {
            if tx.execute("UPDATE models SET bundle_id=?1,metadata_version=metadata_version+1,updated_at=?2 WHERE id=?3",params![id,timestamp(),model])?==0{return Err(LibraryError::ModelNotFound(model.clone()));}
        }
        tx.commit()?;
        Ok(id)
    }
    pub fn set_model_bundle(
        &self,
        model_id: &str,
        bundle_id: Option<&str>,
    ) -> Result<Model, LibraryError> {
        if let Some(id) = bundle_id {
            let source_root: Option<Option<String>> = self
                .conn
                .query_row(
                    "SELECT source_root_id FROM bundles WHERE id=?1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()?;
            match source_root {
                None => return Err(LibraryError::InvalidInput("bundle not found".into())),
                Some(Some(_)) => {
                    return Err(LibraryError::InvalidInput(
                        "automatic source bundles are managed by scanning".into(),
                    ))
                }
                Some(None) => {}
            }
        }
        if self.conn.execute("UPDATE models SET bundle_id=?1,metadata_version=metadata_version+1,updated_at=?2 WHERE id=?3",params![bundle_id,timestamp(),model_id])?==0{return Err(LibraryError::ModelNotFound(model_id.into()));}
        self.get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.into()))
    }

    fn ensure_model(&self, id: &str) -> Result<(), LibraryError> {
        if self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM models WHERE id=?1)",
            params![id],
            |r| r.get(0),
        )? {
            Ok(())
        } else {
            Err(LibraryError::ModelNotFound(id.into()))
        }
    }

    fn ensure_folder(&self, id: &str) -> Result<(), LibraryError> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM folders WHERE id = ?1)",
            params![id],
            |row| row.get(0),
        )?;
        if exists {
            Ok(())
        } else {
            Err(LibraryError::FolderNotFound(id.to_string()))
        }
    }

    fn get_folder(&self, id: &str) -> Result<Option<Folder>, LibraryError> {
        self.conn.query_row(
            "SELECT f.id, f.name, f.parent_id,
                    (SELECT COUNT(*) FROM models m WHERE m.folder_id = f.id AND m.deleted_at IS NULL)
             FROM folders f WHERE f.id = ?1",
            params![id],
            |row| Ok(Folder {
                id: row.get(0)?, name: row.get(1)?, parent_id: row.get(2)?, model_count: row.get::<_, i64>(3)? as u64,
            }),
        ).optional().map_err(Into::into)
    }

    fn tags_for(&self, model_id: &str) -> Result<Vec<String>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name FROM tags t JOIN model_tags mt ON mt.tag_id = t.id WHERE mt.model_id = ?1 ORDER BY lower(t.name), t.name",
        )?;
        let rows = stmt.query_map(params![model_id], |row| row.get(0))?;
        Ok(rows.collect::<Result<Vec<String>, _>>()?)
    }

    fn tags_by_model(&self) -> Result<HashMap<String, Vec<String>>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT mt.model_id, t.name FROM model_tags mt
             JOIN tags t ON t.id = mt.tag_id
             ORDER BY mt.model_id, lower(t.name), t.name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut tags = HashMap::<String, Vec<String>>::new();
        for row in rows {
            let (model_id, tag) = row?;
            tags.entry(model_id).or_default().push(tag);
        }
        Ok(tags)
    }

    fn filament_ids_for_model(&self, model_id: &str) -> Result<Vec<String>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT filament_id FROM model_filaments WHERE model_id=?1 ORDER BY ordinal",
        )?;
        let rows = stmt.query_map(params![model_id], |row| row.get(0))?;
        Ok(rows.collect::<Result<Vec<String>, _>>()?)
    }

    fn filaments_by_model(&self) -> Result<HashMap<String, Vec<String>>, LibraryError> {
        let mut stmt = self.conn.prepare(
            "SELECT model_id,filament_id FROM model_filaments ORDER BY model_id,ordinal",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut filaments = HashMap::<String, Vec<String>>::new();
        for row in rows {
            let (model_id, filament_id) = row?;
            filaments.entry(model_id).or_default().push(filament_id);
        }
        Ok(filaments)
    }
}

const MODEL_SELECT: &str = "SELECT m.id, m.display_name, m.description, m.folder_id,
            COALESCE(r.original_name, d.original_name), COALESCE(r.format, d.format),
            COALESCE(r.bytes, d.bytes, 0), COALESCE(r.blob_hash, d.content_hash),
            b.relative_path, m.metadata_version, m.created_at, m.updated_at,
            m.source_kind,
            CASE WHEN m.source_kind = 'indexed'
                 THEN CASE WHEN (ir.path = '/' AND substr(m.source_path, 1, 1) = '/')
                                OR (ir.path <> '/' AND (
                                    m.source_path = ir.path
                                    OR substr(m.source_path, 1, length(ir.path) + 1) = ir.path || '/'
                                    OR substr(m.source_path, 1, length(ir.path) + 1) = ir.path || char(92)
                                ))
                           THEN m.source_path
                           ELSE rtrim(ir.path, '/') || '/' || m.source_path
                      END
                 ELSE b.relative_path END,
            m.source_root_id, m.source_missing, m.source_modified_at,
            (SELECT t.relative_path FROM thumbnails t
             WHERE t.model_id = m.id
               AND (t.is_custom = 1 OR t.source_content_hash IS NULL
                    OR t.source_content_hash = COALESCE(r.blob_hash, d.content_hash))
             ORDER BY t.is_active DESC, t.is_custom DESC, t.ordinal, t.id LIMIT 1),
            m.designer, m.source_url, m.notes, m.model_license, m.print_status,
            m.parent_model_id, m.last_printed_at, m.print_count, m.bundle_id,
            m.source_archive_member,m.catalog_kind,m.rating,m.favorite
     FROM models m
     LEFT JOIN revisions r ON r.id = m.current_revision_id
     LEFT JOIN blobs b ON b.hash = r.blob_hash
     LEFT JOIN indexed_model_data d ON d.model_id = m.id
     LEFT JOIN index_roots ir ON ir.id = m.source_root_id";

const FILAMENT_SELECT: &str = "SELECT f.id,f.version,f.brand,f.material,f.color,f.name,f.notes,
       f.origin,f.resource_kind,f.source_id,f.external_id,f.parent_filament_id,
       f.archived,f.source_missing,f.created_at,f.updated_at FROM filaments f";

const SCHEMA: &str = r#"
CREATE TABLE libraries (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL);
INSERT INTO libraries (id, name, created_at) VALUES ('local', 'Local Library', '1970-01-01T00:00:00Z');
CREATE TABLE catalog_settings (
    id TEXT PRIMARY KEY CHECK(id = 'local'),
    thumbnail_size INTEGER NOT NULL CHECK(thumbnail_size IN (128, 256, 512)),
    thumbnail_background TEXT NOT NULL CHECK(
        length(thumbnail_background) = 7
        AND substr(thumbnail_background, 1, 1) = '#'
        AND substr(thumbnail_background, 2) NOT GLOB '*[^0-9A-Fa-f]*'
    ),
    thumbnail_concurrency INTEGER NOT NULL CHECK(thumbnail_concurrency BETWEEN 1 AND 2),
    scan_batch_size INTEGER NOT NULL CHECK(scan_batch_size BETWEEN 16 AND 256)
);
INSERT INTO catalog_settings VALUES ('local', 256, '#12171F', 2, 128);
CREATE TABLE folders (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    parent_id TEXT REFERENCES folders(id) ON DELETE RESTRICT,
    UNIQUE(parent_id, name)
);
INSERT INTO folders (id, name, parent_id) VALUES ('root', 'All Models', NULL);
CREATE TABLE blobs (
    hash TEXT PRIMARY KEY,
    relative_path TEXT NOT NULL UNIQUE,
    bytes INTEGER NOT NULL CHECK(bytes >= 0)
);
CREATE TABLE models (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    folder_id TEXT NOT NULL REFERENCES folders(id) ON DELETE RESTRICT,
    current_revision_id TEXT,
    metadata_version INTEGER NOT NULL DEFAULT 1 CHECK(metadata_version > 0),
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    source_kind TEXT NOT NULL DEFAULT 'managed' CHECK(source_kind IN ('managed', 'indexed')),
    source_path TEXT,
    source_root_id TEXT,
    source_missing INTEGER NOT NULL DEFAULT 0 CHECK(source_missing IN (0, 1)),
    source_modified_at INTEGER,
    source_archive_member TEXT,
    designer TEXT, source_url TEXT, notes TEXT, model_license TEXT,
    print_status TEXT NOT NULL DEFAULT 'unprinted',
    parent_model_id TEXT,
    last_printed_at TEXT, print_count INTEGER NOT NULL DEFAULT 0,
    bundle_id TEXT,
    catalog_kind TEXT NOT NULL DEFAULT 'file' CHECK(catalog_kind IN ('file','bookmark')),
    rating INTEGER NOT NULL DEFAULT 0 CHECK(rating BETWEEN 0 AND 5),
    favorite INTEGER NOT NULL DEFAULT 0 CHECK(favorite IN (0,1))
);
CREATE INDEX models_folder_idx ON models(folder_id, deleted_at, created_at, id);
CREATE TABLE revisions (
    id TEXT PRIMARY KEY,
    model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    blob_hash TEXT NOT NULL REFERENCES blobs(hash) ON DELETE RESTRICT,
    original_name TEXT NOT NULL,
    format TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK(bytes >= 0),
    created_at TEXT NOT NULL
);
CREATE INDEX revisions_model_idx ON revisions(model_id, created_at);
CREATE TABLE tags (id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE, category TEXT);
CREATE TABLE model_tags (
    model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY(model_id, tag_id)
);
CREATE TABLE manuals (model_id TEXT PRIMARY KEY REFERENCES models(id) ON DELETE CASCADE, relative_path TEXT NOT NULL, content_hash TEXT NOT NULL);
CREATE TABLE thumbnails (id TEXT PRIMARY KEY, model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE, relative_path TEXT NOT NULL, content_hash TEXT NOT NULL, is_custom INTEGER NOT NULL DEFAULT 0, source_content_hash TEXT, ordinal INTEGER NOT NULL DEFAULT 0 CHECK(ordinal >= 0), is_active INTEGER NOT NULL DEFAULT 0 CHECK(is_active IN (0,1)));
CREATE INDEX thumbnails_model_idx ON thumbnails(model_id, is_active DESC, ordinal, id);
CREATE UNIQUE INDEX thumbnails_active_model_idx ON thumbnails(model_id) WHERE is_active=1;
CREATE TABLE jobs (id TEXT PRIMARY KEY, model_id TEXT REFERENCES models(id) ON DELETE CASCADE, kind TEXT NOT NULL, status TEXT NOT NULL, retry_count INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE index_roots (
    id TEXT PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0, 1)),
    last_scanned_at TEXT,
    scan_interval_minutes INTEGER NOT NULL CHECK(scan_interval_minutes BETWEEN 1 AND 1440),
    max_file_bytes INTEGER NOT NULL CHECK(max_file_bytes BETWEEN 1 AND 104857600),
    scan_zip INTEGER NOT NULL DEFAULT 0 CHECK(scan_zip IN (0,1))
);
CREATE TABLE indexed_model_data (
    model_id TEXT PRIMARY KEY REFERENCES models(id) ON DELETE CASCADE,
    content_hash TEXT NOT NULL,
    original_name TEXT NOT NULL,
    format TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK(bytes >= 0),
    outer_fingerprint TEXT
);
CREATE INDEX indexed_model_hash_idx ON indexed_model_data(content_hash, model_id);
CREATE UNIQUE INDEX indexed_model_source_idx ON models(source_root_id, source_path, COALESCE(source_archive_member,''))
    WHERE source_kind = 'indexed';
CREATE TABLE bundles (id TEXT PRIMARY KEY, source_root_id TEXT, source_path TEXT, created_at TEXT NOT NULL, UNIQUE(source_root_id,source_path));
CREATE TABLE print_attempts (id TEXT PRIMARY KEY, model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE, printed_at TEXT NOT NULL, quantity INTEGER NOT NULL, notes TEXT NOT NULL, outcome TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE print_attempt_filaments (attempt_id TEXT NOT NULL REFERENCES print_attempts(id) ON DELETE CASCADE, filament_id TEXT NOT NULL, ordinal INTEGER NOT NULL, PRIMARY KEY(attempt_id,filament_id));
CREATE TABLE filament_sources (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind='spoolman'),
    base_url TEXT NOT NULL UNIQUE,
    last_synced_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE filaments (
    id TEXT PRIMARY KEY,
    version INTEGER NOT NULL DEFAULT 1 CHECK(version > 0),
    brand TEXT NOT NULL DEFAULT '', material TEXT NOT NULL DEFAULT '',
    color TEXT NOT NULL DEFAULT '', name TEXT NOT NULL, notes TEXT NOT NULL DEFAULT '',
    origin TEXT NOT NULL CHECK(origin IN ('manual','spoolman')),
    resource_kind TEXT NOT NULL CHECK(resource_kind IN ('manual','filament','spool')),
    source_id TEXT REFERENCES filament_sources(id) ON DELETE RESTRICT,
    external_id TEXT,
    parent_filament_id TEXT REFERENCES filaments(id) ON DELETE RESTRICT,
    archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1)),
    source_missing INTEGER NOT NULL DEFAULT 0 CHECK(source_missing IN (0,1)),
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
    CHECK((origin='manual' AND resource_kind='manual' AND source_id IS NULL AND external_id IS NULL AND parent_filament_id IS NULL AND source_missing=0)
       OR (origin='spoolman' AND resource_kind IN ('filament','spool') AND source_id IS NOT NULL AND external_id IS NOT NULL))
);
CREATE UNIQUE INDEX filaments_remote_identity_idx ON filaments(source_id,resource_kind,external_id)
    WHERE source_id IS NOT NULL;
CREATE TABLE model_filaments (
    model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    filament_id TEXT NOT NULL REFERENCES filaments(id) ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 4),
    PRIMARY KEY(model_id,filament_id), UNIQUE(model_id,ordinal)
);
CREATE INDEX model_filaments_filament_idx ON model_filaments(filament_id,model_id);
CREATE TABLE capture_items (
    item_id TEXT PRIMARY KEY,
    payload_hash TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('imported','partial','failed')),
    page_model_id TEXT REFERENCES models(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    received_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    error_code TEXT
);
CREATE TABLE capture_downloads (
    capture_id TEXT PRIMARY KEY,
    item_id TEXT NOT NULL REFERENCES capture_items(item_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    file_name TEXT NOT NULL,
    expected_hash TEXT,
    expected_bytes INTEGER CHECK(expected_bytes IS NULL OR expected_bytes >= 0),
    status TEXT NOT NULL CHECK(status IN ('needs_file','imported','failed')),
    model_id TEXT REFERENCES models(id) ON DELETE RESTRICT,
    error_code TEXT,
    UNIQUE(item_id,ordinal)
);
CREATE INDEX capture_downloads_item_idx ON capture_downloads(item_id,ordinal);
"#;

const MIGRATE_V1_TO_V2: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE models ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'managed'
    CHECK(source_kind IN ('managed', 'indexed'));
ALTER TABLE models ADD COLUMN source_path TEXT;
ALTER TABLE models ADD COLUMN source_root_id TEXT;
ALTER TABLE models ADD COLUMN source_missing INTEGER NOT NULL DEFAULT 0
    CHECK(source_missing IN (0, 1));
ALTER TABLE models ADD COLUMN source_modified_at INTEGER;
ALTER TABLE thumbnails ADD COLUMN source_content_hash TEXT;
CREATE INDEX thumbnails_model_idx ON thumbnails(model_id, is_custom);
CREATE TABLE catalog_settings (
    id TEXT PRIMARY KEY CHECK(id = 'local'),
    thumbnail_size INTEGER NOT NULL CHECK(thumbnail_size IN (128, 256, 512)),
    thumbnail_background TEXT NOT NULL CHECK(
        length(thumbnail_background) = 7
        AND substr(thumbnail_background, 1, 1) = '#'
        AND substr(thumbnail_background, 2) NOT GLOB '*[^0-9A-Fa-f]*'
    ),
    thumbnail_concurrency INTEGER NOT NULL CHECK(thumbnail_concurrency BETWEEN 1 AND 2),
    scan_batch_size INTEGER NOT NULL CHECK(scan_batch_size BETWEEN 16 AND 256)
);
INSERT INTO catalog_settings VALUES ('local', 256, '#12171F', 2, 128);
CREATE TABLE index_roots (
    id TEXT PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0, 1)),
    last_scanned_at TEXT,
    scan_interval_minutes INTEGER NOT NULL CHECK(scan_interval_minutes BETWEEN 1 AND 1440),
    max_file_bytes INTEGER NOT NULL CHECK(max_file_bytes BETWEEN 1 AND 104857600)
);
CREATE TABLE indexed_model_data (
    model_id TEXT PRIMARY KEY REFERENCES models(id) ON DELETE CASCADE,
    content_hash TEXT NOT NULL,
    original_name TEXT NOT NULL,
    format TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK(bytes >= 0)
);
CREATE INDEX indexed_model_hash_idx ON indexed_model_data(content_hash, model_id);
CREATE UNIQUE INDEX indexed_model_source_idx ON models(source_root_id, source_path)
    WHERE source_kind = 'indexed';
PRAGMA user_version = 2;
COMMIT;
"#;

const MIGRATE_V2_TO_V3: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE index_roots ADD COLUMN scan_zip INTEGER NOT NULL DEFAULT 0 CHECK(scan_zip IN (0,1));
ALTER TABLE models ADD COLUMN source_archive_member TEXT;
ALTER TABLE models ADD COLUMN designer TEXT;
ALTER TABLE models ADD COLUMN source_url TEXT;
ALTER TABLE models ADD COLUMN notes TEXT;
UPDATE models SET notes = description WHERE description != '';
ALTER TABLE models ADD COLUMN model_license TEXT;
ALTER TABLE models ADD COLUMN print_status TEXT NOT NULL DEFAULT 'unprinted';
ALTER TABLE models ADD COLUMN parent_model_id TEXT;
ALTER TABLE models ADD COLUMN last_printed_at TEXT;
ALTER TABLE models ADD COLUMN print_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE models ADD COLUMN bundle_id TEXT;
ALTER TABLE tags ADD COLUMN category TEXT;
ALTER TABLE indexed_model_data ADD COLUMN outer_fingerprint TEXT;
DROP INDEX indexed_model_source_idx;
CREATE UNIQUE INDEX indexed_model_source_idx ON models(source_root_id, source_path, COALESCE(source_archive_member,'')) WHERE source_kind='indexed';
CREATE TABLE bundles (id TEXT PRIMARY KEY, source_root_id TEXT, source_path TEXT, created_at TEXT NOT NULL, UNIQUE(source_root_id,source_path));
CREATE TABLE print_attempts (id TEXT PRIMARY KEY, model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE, printed_at TEXT NOT NULL, quantity INTEGER NOT NULL, notes TEXT NOT NULL, outcome TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE print_attempt_filaments (attempt_id TEXT NOT NULL REFERENCES print_attempts(id) ON DELETE CASCADE, filament_id TEXT NOT NULL, ordinal INTEGER NOT NULL, PRIMARY KEY(attempt_id,filament_id));
PRAGMA user_version=3;
COMMIT;
"#;

const MIGRATE_V3_TO_V4: &str = r#"
BEGIN IMMEDIATE;
CREATE TABLE filament_sources (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind='spoolman'),
    base_url TEXT NOT NULL UNIQUE,
    last_synced_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE filaments (
    id TEXT PRIMARY KEY,
    version INTEGER NOT NULL DEFAULT 1 CHECK(version > 0),
    brand TEXT NOT NULL DEFAULT '', material TEXT NOT NULL DEFAULT '',
    color TEXT NOT NULL DEFAULT '', name TEXT NOT NULL, notes TEXT NOT NULL DEFAULT '',
    origin TEXT NOT NULL CHECK(origin IN ('manual','spoolman')),
    resource_kind TEXT NOT NULL CHECK(resource_kind IN ('manual','filament','spool')),
    source_id TEXT REFERENCES filament_sources(id) ON DELETE RESTRICT,
    external_id TEXT,
    parent_filament_id TEXT REFERENCES filaments(id) ON DELETE RESTRICT,
    archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1)),
    source_missing INTEGER NOT NULL DEFAULT 0 CHECK(source_missing IN (0,1)),
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
    CHECK((origin='manual' AND resource_kind='manual' AND source_id IS NULL AND external_id IS NULL AND parent_filament_id IS NULL AND source_missing=0)
       OR (origin='spoolman' AND resource_kind IN ('filament','spool') AND source_id IS NOT NULL AND external_id IS NOT NULL))
);
CREATE UNIQUE INDEX filaments_remote_identity_idx ON filaments(source_id,resource_kind,external_id)
    WHERE source_id IS NOT NULL;
CREATE TABLE model_filaments (
    model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    filament_id TEXT NOT NULL REFERENCES filaments(id) ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 4),
    PRIMARY KEY(model_id,filament_id), UNIQUE(model_id,ordinal)
);
CREATE INDEX model_filaments_filament_idx ON model_filaments(filament_id,model_id);
PRAGMA user_version=4;
COMMIT;
"#;

const MIGRATE_V4_TO_V5: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE models ADD COLUMN catalog_kind TEXT NOT NULL DEFAULT 'file'
    CHECK(catalog_kind IN ('file','bookmark'));
ALTER TABLE thumbnails ADD COLUMN ordinal INTEGER NOT NULL DEFAULT 0 CHECK(ordinal >= 0);
ALTER TABLE thumbnails ADD COLUMN is_active INTEGER NOT NULL DEFAULT 0 CHECK(is_active IN (0,1));
UPDATE thumbnails AS target
SET ordinal=(SELECT COUNT(*) FROM thumbnails prior
             WHERE prior.model_id=target.model_id AND prior.id < target.id);
UPDATE thumbnails SET is_active=1
WHERE id=(SELECT candidate.id FROM thumbnails candidate
          WHERE candidate.model_id=thumbnails.model_id
          ORDER BY candidate.is_custom DESC,candidate.id LIMIT 1);
DROP INDEX IF EXISTS thumbnails_model_idx;
CREATE INDEX thumbnails_model_idx ON thumbnails(model_id,is_active DESC,ordinal,id);
CREATE UNIQUE INDEX thumbnails_active_model_idx ON thumbnails(model_id) WHERE is_active=1;
CREATE TABLE capture_items (
    item_id TEXT PRIMARY KEY,
    payload_hash TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('imported','partial','failed')),
    page_model_id TEXT REFERENCES models(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    received_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    error_code TEXT
);
CREATE TABLE capture_downloads (
    capture_id TEXT PRIMARY KEY,
    item_id TEXT NOT NULL REFERENCES capture_items(item_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    file_name TEXT NOT NULL,
    expected_hash TEXT,
    expected_bytes INTEGER CHECK(expected_bytes IS NULL OR expected_bytes >= 0),
    status TEXT NOT NULL CHECK(status IN ('needs_file','imported','failed')),
    model_id TEXT REFERENCES models(id) ON DELETE RESTRICT,
    error_code TEXT,
    UNIQUE(item_id,ordinal)
);
CREATE INDEX capture_downloads_item_idx ON capture_downloads(item_id,ordinal);
PRAGMA user_version=5;
COMMIT;
"#;

const MIGRATE_V5_TO_V6: &str = r#"
BEGIN IMMEDIATE;
ALTER TABLE models ADD COLUMN rating INTEGER NOT NULL DEFAULT 0 CHECK(rating BETWEEN 0 AND 5);
ALTER TABLE models ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0 CHECK(favorite IN (0,1));
PRAGMA user_version=6;
COMMIT;
"#;

const ENSURE_CATALOG_SETTINGS: &str = r#"
CREATE TABLE IF NOT EXISTS catalog_settings (
    id TEXT PRIMARY KEY CHECK(id = 'local'),
    thumbnail_size INTEGER NOT NULL CHECK(thumbnail_size IN (128, 256, 512)),
    thumbnail_background TEXT NOT NULL CHECK(
        length(thumbnail_background) = 7
        AND substr(thumbnail_background, 1, 1) = '#'
        AND substr(thumbnail_background, 2) NOT GLOB '*[^0-9A-Fa-f]*'
    ),
    thumbnail_concurrency INTEGER NOT NULL CHECK(thumbnail_concurrency BETWEEN 1 AND 2),
    scan_batch_size INTEGER NOT NULL CHECK(scan_batch_size BETWEEN 16 AND 256)
);
INSERT OR IGNORE INTO catalog_settings
    (id, thumbnail_size, thumbnail_background, thumbnail_concurrency, scan_batch_size)
VALUES ('local', 256, '#12171F', 2, 128);
"#;

const ENSURE_PRINT_BATCH_RECEIPTS: &str = r#"
CREATE TABLE IF NOT EXISTS print_batch_receipts (
    idempotency_key TEXT PRIMARY KEY,
    payload_hash TEXT NOT NULL,
    attempt_ids_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
"#;

fn model_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Model> {
    Ok(Model {
        id: row.get(0)?,
        display_name: row.get(1)?,
        description: row.get(2)?,
        folder_id: row.get(3)?,
        original_name: row.get(4)?,
        format: row.get(5)?,
        bytes: row.get::<_, i64>(6)? as u64,
        content_hash: row.get(7)?,
        storage_path: row.get(8)?,
        metadata_version: row.get::<_, i64>(9)? as u64,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        tags: Vec::new(),
        filament_ids: Vec::new(),
        source_kind: row.get(12)?,
        source_path: row.get(13)?,
        source_root_id: row.get(14)?,
        source_missing: row.get(15)?,
        source_modified_at: row.get::<_, Option<i64>>(16)?.map(|value| value as u64),
        thumbnail_path: row.get(17)?,
        metadata: ModelMetadata {
            designer: row.get(18)?,
            source_url: row.get(19)?,
            notes: row.get(20)?,
            model_license: row.get(21)?,
            print_status: parse_print_status(row.get::<_, String>(22)?)?,
            parent_model_id: row.get(23)?,
            last_printed_at: row.get(24)?,
            print_count: row.get::<_, i64>(25)? as u64,
            rating: u8::try_from(row.get::<_, i64>(29)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            favorite: row.get(30)?,
        },
        bundle_id: row.get(26)?,
        source_archive_member: row.get(27)?,
        catalog_kind: row.get(28)?,
    })
}

fn filament_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Filament> {
    let origin = match row.get::<_, String>(7)?.as_str() {
        "manual" => FilamentOrigin::Manual,
        "spoolman" => FilamentOrigin::Spoolman,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let resource_kind = match row.get::<_, String>(8)?.as_str() {
        "manual" => FilamentResourceKind::Manual,
        "filament" => FilamentResourceKind::Filament,
        "spool" => FilamentResourceKind::Spool,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let version = row.get::<_, i64>(1)?;
    Ok(Filament {
        id: row.get(0)?,
        version: u64::try_from(version)
            .map_err(|_| invalid_persisted_integer(1, "filament version is negative"))?,
        brand: row.get(2)?,
        material: row.get(3)?,
        color: row.get(4)?,
        name: row.get(5)?,
        notes: row.get(6)?,
        origin,
        resource_kind,
        source_id: row.get(9)?,
        external_id: row.get(10)?,
        parent_filament_id: row.get(11)?,
        archived: row.get(12)?,
        source_missing: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

fn spoolman_source_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SpoolmanSource> {
    Ok(SpoolmanSource {
        id: row.get(0)?,
        base_url: row.get(1)?,
        last_synced_at: row.get(2)?,
    })
}

fn thumbnail_gallery_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ThumbnailGalleryItem> {
    let ordinal = row.get::<_, i64>(6)?;
    let is_custom = row.get::<_, i64>(5)?;
    let is_active = row.get::<_, i64>(7)?;
    if ordinal < 0 || !matches!(is_custom, 0 | 1) || !matches!(is_active, 0 | 1) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(ThumbnailGalleryItem {
        id: row.get(0)?,
        model_id: row.get(1)?,
        relative_path: row.get(2)?,
        content_hash: row.get(3)?,
        source_content_hash: row.get(4)?,
        is_custom: is_custom == 1,
        ordinal: u16::try_from(ordinal)
            .map_err(|_| invalid_persisted_integer(6, "thumbnail ordinal exceeds u16"))?,
        is_active: is_active == 1,
    })
}

fn parse_print_status(value: String) -> rusqlite::Result<PrintStatus> {
    match value.as_str() {
        "unprinted" => Ok(PrintStatus::Unprinted),
        "want" => Ok(PrintStatus::Want),
        "queued" => Ok(PrintStatus::Queued),
        "printing" => Ok(PrintStatus::Printing),
        "printed" => Ok(PrintStatus::Printed),
        "failed" => Ok(PrintStatus::Failed),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn index_root_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IndexRoot> {
    let enabled: i64 = row.get(2)?;
    let scan_interval: i64 = row.get(4)?;
    let max_file_bytes: i64 = row.get(5)?;
    let scan_zip: i64 = row.get(6)?;
    let scan_interval_minutes = u32::try_from(scan_interval)
        .map_err(|_| invalid_persisted_integer(4, "scan interval is outside u32"))?;
    let max_file_bytes = u64::try_from(max_file_bytes)
        .map_err(|_| invalid_persisted_integer(5, "maximum file size is negative"))?;
    if !(0..=1).contains(&enabled) {
        return Err(invalid_persisted_integer(2, "enabled must be zero or one"));
    }
    if !(0..=1).contains(&scan_zip) {
        return Err(invalid_persisted_integer(6, "scan_zip must be zero or one"));
    }
    if !(MIN_SCAN_INTERVAL_MINUTES..=MAX_SCAN_INTERVAL_MINUTES).contains(&scan_interval_minutes) {
        return Err(invalid_persisted_integer(
            4,
            "scan interval is outside the supported range",
        ));
    }
    if max_file_bytes == 0 || max_file_bytes > MAX_INDEXED_FILE_BYTES {
        return Err(invalid_persisted_integer(
            5,
            "maximum file size is outside the supported range",
        ));
    }
    Ok(IndexRoot {
        id: row.get(0)?,
        path: PathBuf::from(row.get::<_, String>(1)?),
        enabled: enabled == 1,
        last_scanned_at: row.get(3)?,
        scan_interval_minutes,
        max_file_bytes,
        scan_zip: scan_zip == 1,
    })
}

fn invalid_persisted_integer(index: usize, message: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Integer,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message.to_owned(),
        )),
    )
}

fn clean_name(value: &str, kind: &str) -> Result<String, LibraryError> {
    let name = value.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(LibraryError::InvalidInput(format!(
            "{kind} name is invalid"
        )));
    }
    Ok(name.to_owned())
}

fn clean_tag_name(value: &str) -> Result<String, LibraryError> {
    if value.len() > 256 || value.chars().any(char::is_control) {
        return Err(LibraryError::InvalidInput(
            "Tag name exceeds its limit or contains control characters".into(),
        ));
    }
    clean_name(value, "Tag")
}

fn clean_model_tag_names(values: &[String]) -> Result<Vec<String>, LibraryError> {
    if values.is_empty() || values.len() > 200 {
        return Err(LibraryError::InvalidInput(
            "Model tag count is outside its limit".into(),
        ));
    }
    let mut names = Vec::with_capacity(values.len());
    let mut seen = HashSet::with_capacity(values.len());
    for value in values {
        let name = clean_tag_name(value)?;
        if !seen.insert(name.to_lowercase()) {
            return Err(LibraryError::InvalidInput(
                "Model tag names contain duplicates".into(),
            ));
        }
        names.push(name);
    }
    Ok(names)
}

fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{seconds}")
}

fn new_id(prefix: &str) -> String {
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{counter}", Uuid::new_v4().simple())
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn create_migration_backup(
    conn: &Connection,
    root: &Path,
    version: i64,
) -> Result<(), LibraryError> {
    let mut backup = root.join(format!("library.sqlite3.schema-v{version}.backup"));
    if backup.exists() {
        backup = root.join(format!(
            "library.sqlite3.schema-v{version}.{}.backup",
            Uuid::new_v4().simple()
        ));
    }
    conn.execute("VACUUM INTO ?1", params![backup.to_string_lossy()])?;
    Ok(())
}

fn canonical_directory(path: &Path) -> Result<PathBuf, LibraryError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| io(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(LibraryError::InvalidInput(
            "index root must be an existing non-symlink directory".into(),
        ));
    }
    path.canonicalize().map_err(|error| io(path, error))
}

fn ensure_owned_directory(root: &Path, name: &str) -> Result<PathBuf, LibraryError> {
    let path = root.join(name);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(LibraryError::InvalidInput(format!(
                "managed {name} namespace is unsafe"
            )))
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&path).map_err(|error| io(&path, error))?
        }
        Err(error) => return Err(io(&path, error)),
    }
    let canonical = path.canonicalize().map_err(|error| io(&path, error))?;
    if canonical.parent() != Some(root) || canonical != path {
        return Err(LibraryError::InvalidInput(format!(
            "managed {name} namespace identity changed"
        )));
    }
    Ok(path)
}
fn validate_managed_blob(
    path: &Path,
    expected_hash: &str,
    expected_bytes: u64,
) -> Result<(), LibraryError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| io(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != expected_bytes
    {
        return Err(LibraryError::InvalidInput(
            "managed blob target is unsafe or inconsistent".into(),
        ));
    }
    let mut file = std::fs::File::open(path).map_err(|error| io(path, error))?;
    let mut digest = Sha256::new();
    let copied = std::io::copy(
        &mut (&mut file).take(expected_bytes.saturating_add(1)),
        &mut digest,
    )
    .map_err(|error| io(path, error))?;
    if copied != expected_bytes || format!("{:x}", digest.finalize()) != expected_hash {
        return Err(LibraryError::InvalidInput(
            "managed blob target hash mismatch".into(),
        ));
    }
    Ok(())
}

fn validated_relative_path(path: &Path) -> Result<String, LibraryError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(LibraryError::InvalidInput(
            "source path must be a safe relative path".into(),
        ));
    }
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| LibraryError::InvalidInput("source path is not valid UTF-8".into()))
}

fn normalize_indexed_source_path(root: &Path, stored_path: &Path) -> Result<PathBuf, LibraryError> {
    let relative_path = if stored_path.is_absolute() {
        stored_path.strip_prefix(root).map_err(|_| {
            LibraryError::InvalidInput("indexed source path is outside its configured root".into())
        })?
    } else {
        stored_path
    };
    Ok(PathBuf::from(validated_relative_path(relative_path)?))
}

fn resolve_under_root(root: &Path, relative: &Path) -> Result<PathBuf, LibraryError> {
    let relative = validated_relative_path(relative)?;
    let canonical_root = root.canonicalize().map_err(|error| io(root, error))?;
    if canonical_root != root {
        return Err(LibraryError::InvalidInput(
            "configured root identity changed".into(),
        ));
    }
    let candidate = canonical_root.join(relative);
    let canonical_candidate = candidate
        .canonicalize()
        .map_err(|error| io(&candidate, error))?;
    if !canonical_candidate.starts_with(&canonical_root) {
        return Err(LibraryError::InvalidInput(
            "source path escapes its configured root".into(),
        ));
    }
    let metadata =
        std::fs::metadata(&canonical_candidate).map_err(|error| io(&canonical_candidate, error))?;
    if !metadata.is_file() {
        return Err(LibraryError::InvalidInput("source is not a file".into()));
    }
    Ok(canonical_candidate)
}

fn validated_thumbnail_cache_path(
    root: &Path,
    relative: &Path,
    expected_hash: &str,
) -> Result<PathBuf, LibraryError> {
    let relative = validated_relative_path(relative)?;
    validate_hash(expected_hash)?;
    let components = Path::new(&relative).components().collect::<Vec<_>>();
    if components.len() < 2 || components[0] != Component::Normal("thumbnails".as_ref()) {
        return Err(LibraryError::InvalidInput(
            "generated thumbnail path is outside the thumbnails directory".into(),
        ));
    }
    let canonical_root = root.canonicalize().map_err(|error| io(root, error))?;
    if canonical_root != root {
        return Err(LibraryError::InvalidInput(
            "library root identity changed".into(),
        ));
    }
    let mut current = canonical_root.clone();
    let mut missing = false;
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            return Err(LibraryError::InvalidInput(
                "generated thumbnail path is unsafe".into(),
            ));
        };
        current.push(component);
        if missing {
            continue;
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(LibraryError::InvalidInput(
                        "generated thumbnail path contains a symlink".into(),
                    ));
                }
                let is_leaf = index + 1 == components.len();
                if (is_leaf && !metadata.is_file()) || (!is_leaf && !metadata.is_dir()) {
                    return Err(LibraryError::InvalidInput(
                        "generated thumbnail path has an unexpected file type".into(),
                    ));
                }
                if is_leaf {
                    if metadata.len() > MAX_CACHED_THUMBNAIL_BYTES {
                        return Err(LibraryError::InvalidInput(
                            "generated thumbnail exceeds the cache size limit".into(),
                        ));
                    }
                    let mut file =
                        std::fs::File::open(&current).map_err(|error| io(&current, error))?;
                    let mut bytes = Vec::with_capacity(metadata.len() as usize);
                    std::io::Read::by_ref(&mut file)
                        .take(MAX_CACHED_THUMBNAIL_BYTES + 1)
                        .read_to_end(&mut bytes)
                        .map_err(|error| io(&current, error))?;
                    if bytes.len() as u64 > MAX_CACHED_THUMBNAIL_BYTES
                        || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                        || hex_digest(&bytes) != expected_hash
                    {
                        return Err(LibraryError::InvalidInput(
                            "generated thumbnail does not match its catalog hash".into(),
                        ));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => missing = true,
            Err(error) => return Err(io(&current, error)),
        }
    }
    if !current.starts_with(&canonical_root) {
        return Err(LibraryError::InvalidInput(
            "generated thumbnail path escapes the library root".into(),
        ));
    }
    Ok(current)
}

fn validate_hash(hash: &str) -> Result<(), LibraryError> {
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(LibraryError::InvalidInput(
            "content hash must be a 64-character hexadecimal SHA-256 digest".into(),
        ));
    }
    Ok(())
}

fn validate_capture_receipt(input: &CaptureReceiptInput) -> Result<(), LibraryError> {
    validate_plain_capture_id(&input.item_id, "capture item ID")?;
    validate_hash(&input.payload_hash)?;
    normalize_rfc3339(&input.created_at)?;
    let url = input.page.url.trim();
    if url.len() > 2048
        || !(url.starts_with("http://") || url.starts_with("https://"))
        || url.chars().any(char::is_control)
    {
        return Err(LibraryError::InvalidInput(
            "capture page URL is invalid".into(),
        ));
    }
    let title = input.page.title.trim();
    if title.is_empty() || title.len() > 512 || title.chars().any(char::is_control) {
        return Err(LibraryError::InvalidInput(
            "capture page title is invalid".into(),
        ));
    }
    clean_optional(input.page.designer.as_deref(), 200)?;
    clean_optional(input.page.model_license.as_deref(), 200)?;
    clean_optional(input.page.notes.as_deref(), 20_000)?;
    if let Some(parent) = &input.page.parent_external_id {
        validate_plain_capture_id(parent, "capture parent ID")?;
    }
    if input.downloads.len() > 128 {
        return Err(LibraryError::InvalidInput(
            "capture item has too many downloads".into(),
        ));
    }
    let mut ids = HashSet::new();
    for download in &input.downloads {
        validate_plain_capture_id(&download.capture_id, "capture download ID")?;
        if !ids.insert(&download.capture_id) {
            return Err(LibraryError::InvalidInput(
                "capture item repeats a download ID".into(),
            ));
        }
        if download.file_name.is_empty()
            || download.file_name.len() > 255
            || download.file_name.contains(['/', '\\', '\0'])
            || Path::new(&download.file_name).file_name() != Some(download.file_name.as_ref())
        {
            return Err(LibraryError::InvalidInput(
                "capture download filename is invalid".into(),
            ));
        }
        if let Some(hash) = &download.sha256 {
            validate_hash(hash)?;
        }
        if download
            .bytes
            .is_some_and(|bytes| bytes == 0 || bytes > MAX_INDEXED_FILE_BYTES)
        {
            return Err(LibraryError::InvalidInput(
                "capture download byte count is invalid".into(),
            ));
        }
    }
    Ok(())
}

fn validate_plain_capture_id(value: &str, label: &str) -> Result<(), LibraryError> {
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        })
    {
        return Err(LibraryError::InvalidInput(format!("{label} is invalid")));
    }
    Ok(())
}

fn select_thumbnail_fallback(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
) -> Result<(), LibraryError> {
    let has_active: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM thumbnails WHERE model_id=?1 AND is_active=1)",
        params![model_id],
        |row| row.get(0),
    )?;
    if !has_active {
        let fallback: Option<String> = tx
            .query_row(
                "SELECT id FROM thumbnails WHERE model_id=?1
                 ORDER BY is_custom DESC,ordinal,id LIMIT 1",
                params![model_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = fallback {
            tx.execute("UPDATE thumbnails SET is_active=1 WHERE id=?1", params![id])?;
        }
    }
    Ok(())
}

fn compact_thumbnail_ordinals(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
) -> Result<(), LibraryError> {
    let mut ids = Vec::new();
    {
        let mut stmt =
            tx.prepare("SELECT id FROM thumbnails WHERE model_id=?1 ORDER BY ordinal,id")?;
        let rows = stmt.query_map(params![model_id], |row| row.get::<_, String>(0))?;
        ids.extend(rows.collect::<Result<Vec<_>, _>>()?);
    }
    for (ordinal, id) in ids.into_iter().enumerate() {
        tx.execute(
            "UPDATE thumbnails SET ordinal=?1 WHERE id=?2",
            params![ordinal as i64, id],
        )?;
    }
    Ok(())
}

fn validate_thumbnail_record_path(
    root: &Path,
    thumbnail_relative_path: &Path,
    png_hash: &str,
) -> Result<String, LibraryError> {
    let relative = validated_relative_path(thumbnail_relative_path)?;
    if Path::new(&relative).components().next() != Some(Component::Normal("thumbnails".as_ref())) {
        return Err(LibraryError::InvalidInput(
            "custom thumbnail must be stored under thumbnails".into(),
        ));
    }
    validated_thumbnail_cache_path(root, Path::new(&relative), png_hash)?;
    Ok(relative)
}

fn validate_root_limits(
    scan_interval_minutes: u32,
    max_file_bytes: u64,
) -> Result<(), LibraryError> {
    if !(MIN_SCAN_INTERVAL_MINUTES..=MAX_SCAN_INTERVAL_MINUTES).contains(&scan_interval_minutes) {
        return Err(LibraryError::InvalidInput(format!(
            "scan interval must be between {MIN_SCAN_INTERVAL_MINUTES} and {MAX_SCAN_INTERVAL_MINUTES} minutes"
        )));
    }
    if max_file_bytes == 0 || max_file_bytes > MAX_INDEXED_FILE_BYTES {
        return Err(LibraryError::InvalidInput(format!(
            "maximum file size must be between 1 and {MAX_INDEXED_FILE_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_catalog_settings(settings: &CatalogSettings) -> Result<(), LibraryError> {
    if !matches!(settings.thumbnail_size, 128 | 256 | 512) {
        return Err(LibraryError::InvalidInput(
            "thumbnail size must be 128, 256, or 512 pixels".into(),
        ));
    }
    let background = settings.thumbnail_background.as_bytes();
    if background.len() != 7
        || background[0] != b'#'
        || !background[1..].iter().all(u8::is_ascii_hexdigit)
    {
        return Err(LibraryError::InvalidInput(
            "thumbnail background must be a #RRGGBB color".into(),
        ));
    }
    if !(1..=2).contains(&settings.thumbnail_concurrency) {
        return Err(LibraryError::InvalidInput(
            "thumbnail concurrency must be between 1 and 2".into(),
        ));
    }
    if !(16..=256).contains(&settings.scan_batch_size) {
        return Err(LibraryError::InvalidInput(
            "scan batch size must be between 16 and 256".into(),
        ));
    }
    Ok(())
}

fn clean_optional(value: Option<&str>, max: usize) -> Result<Option<String>, LibraryError> {
    match value {
        Some(v) => {
            let v = v.trim();
            if v.is_empty() {
                Ok(None)
            } else if v.len() > max {
                Err(LibraryError::InvalidInput(
                    "metadata value is too long".into(),
                ))
            } else {
                Ok(Some(v.into()))
            }
        }
        None => Ok(None),
    }
}

fn validate_filament_input(input: &FilamentInput) -> Result<FilamentInput, LibraryError> {
    Ok(FilamentInput {
        brand: clean_filament_text(&input.brand, 200, false, false)?,
        material: clean_filament_text(&input.material, 200, false, false)?,
        color: clean_filament_text(&input.color, 256, false, false)?,
        name: clean_filament_text(&input.name, 200, true, false)?,
        notes: clean_filament_text(&input.notes, 20_000, false, true)?,
    })
}

fn validate_spoolman_filament(
    record: &SpoolmanFilamentRecord,
) -> Result<FilamentInput, LibraryError> {
    validate_filament_input(&FilamentInput {
        brand: record.brand.clone(),
        material: record.material.clone(),
        color: record.color.clone(),
        name: record.name.clone(),
        notes: record.notes.clone(),
    })
}

fn validate_spoolman_spool(record: &SpoolmanSpoolRecord) -> Result<FilamentInput, LibraryError> {
    validate_filament_input(&FilamentInput {
        brand: record.brand.clone(),
        material: record.material.clone(),
        color: record.color.clone(),
        name: record.name.clone(),
        notes: record.notes.clone(),
    })
}

fn clean_filament_text(
    value: &str,
    max: usize,
    required: bool,
    multiline: bool,
) -> Result<String, LibraryError> {
    let cleaned = value.trim();
    let invalid_control = cleaned.chars().any(|character| {
        character == '\0'
            || (character.is_control() && (!multiline || !matches!(character, '\n' | '\r' | '\t')))
    });
    if cleaned.len() > max || invalid_control || (required && cleaned.is_empty()) {
        return Err(LibraryError::InvalidInput(
            "filament text field is invalid".into(),
        ));
    }
    Ok(cleaned.into())
}

fn validate_external_id(value: &str) -> Result<String, LibraryError> {
    let cleaned = value.trim();
    if cleaned.is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
        || value != cleaned
    {
        return Err(LibraryError::InvalidInput(
            "Spoolman external ID is invalid".into(),
        ));
    }
    Ok(cleaned.into())
}

fn validate_spoolman_base_url(value: &str) -> Result<String, LibraryError> {
    let value = value.trim().trim_end_matches('/');
    if value.len() > 2048
        || !(value.starts_with("http://") || value.starts_with("https://"))
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || value.contains(['@', '?', '#'])
        || value
            .split_once("://")
            .is_none_or(|(_, remainder)| remainder.is_empty() || remainder.starts_with('/'))
    {
        return Err(LibraryError::InvalidInput(
            "Spoolman base URL must be a bounded HTTP or HTTPS URL".into(),
        ));
    }
    Ok(value.into())
}

fn validate_filament_ids(ids: &[String]) -> Result<(), LibraryError> {
    let mut unique = HashSet::new();
    if ids.len() > 5
        || ids.iter().any(|id| {
            id.is_empty()
                || id.len() > 256
                || id.trim() != id
                || id.chars().any(char::is_control)
                || !unique.insert(id)
        })
    {
        return Err(LibraryError::InvalidInput(
            "model filament assignments are invalid".into(),
        ));
    }
    Ok(())
}

fn validate_catalog_filament_ids(
    tx: &rusqlite::Transaction<'_>,
    ids: &[String],
) -> Result<(), LibraryError> {
    for id in ids {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM filaments WHERE id=?1)",
            params![id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(LibraryError::InvalidInput(format!(
                "filament not found: {id}"
            )));
        }
    }
    Ok(())
}

fn set_model_filaments_tx(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
    expected_version: u64,
    filament_ids: &[String],
) -> Result<(), LibraryError> {
    validate_filament_ids(filament_ids)?;
    validate_catalog_filament_ids(tx, filament_ids)?;
    set_model_filaments_tx_validated(tx, model_id, expected_version, filament_ids)
}

fn set_model_filaments_tx_validated(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
    expected_version: u64,
    filament_ids: &[String],
) -> Result<(), LibraryError> {
    validate_active_model_version(tx, model_id, expected_version)?;
    write_model_filaments_tx(tx, model_id, filament_ids)
}

fn write_model_filaments_tx(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
    filament_ids: &[String],
) -> Result<(), LibraryError> {
    tx.execute(
        "DELETE FROM model_filaments WHERE model_id=?1",
        params![model_id],
    )?;
    for (ordinal, filament_id) in filament_ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO model_filaments(model_id,filament_id,ordinal) VALUES(?1,?2,?3)",
            params![model_id, filament_id, ordinal as i64],
        )?;
    }
    tx.execute(
        "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
        params![timestamp(), model_id],
    )?;
    Ok(())
}

fn validate_active_model_version(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
    expected_version: u64,
) -> Result<(), LibraryError> {
    let current: Option<(i64, Option<String>)> = tx
        .query_row(
            "SELECT metadata_version,deleted_at FROM models WHERE id=?1",
            params![model_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((actual, deleted_at)) = current else {
        return Err(LibraryError::ModelNotFound(model_id.into()));
    };
    if deleted_at.is_some() {
        return Err(LibraryError::ModelNotFound(model_id.into()));
    }
    if actual as u64 != expected_version {
        return Err(LibraryError::MetadataConflict {
            model_id: model_id.into(),
            expected: expected_version,
            actual: actual as u64,
        });
    }
    Ok(())
}

fn upsert_remote_filament(
    tx: &rusqlite::Transaction<'_>,
    source_id: &str,
    resource_kind: &str,
    external_id: &str,
    parent_filament_id: Option<&String>,
    input: &FilamentInput,
    archived: bool,
) -> Result<(String, bool, bool), LibraryError> {
    type ExistingRemoteFilament = (
        String,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        bool,
        bool,
    );
    let existing: Option<ExistingRemoteFilament> = tx
        .query_row(
            "SELECT id,brand,material,color,name,notes,parent_filament_id,archived,source_missing
             FROM filaments WHERE source_id=?1 AND resource_kind=?2 AND external_id=?3",
            params![source_id, resource_kind, external_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()?;
    if let Some((id, brand, material, color, name, notes, parent, old_archived, missing)) = existing
    {
        let changed = brand != input.brand
            || material != input.material
            || color != input.color
            || name != input.name
            || notes != input.notes
            || parent.as_deref() != parent_filament_id.map(String::as_str)
            || old_archived != archived
            || missing;
        if changed {
            tx.execute(
                "UPDATE filaments SET brand=?1,material=?2,color=?3,name=?4,notes=?5,
                 parent_filament_id=?6,archived=?7,source_missing=0,version=version+1,updated_at=?8
                 WHERE id=?9",
                params![
                    input.brand,
                    input.material,
                    input.color,
                    input.name,
                    input.notes,
                    parent_filament_id,
                    archived,
                    timestamp(),
                    id
                ],
            )?;
        }
        Ok((id, false, changed))
    } else {
        let id = new_id("filament");
        let now = timestamp();
        tx.execute(
            "INSERT INTO filaments(id,version,brand,material,color,name,notes,origin,resource_kind,
             source_id,external_id,parent_filament_id,archived,source_missing,created_at,updated_at)
             VALUES(?1,1,?2,?3,?4,?5,?6,'spoolman',?7,?8,?9,?10,?11,0,?12,?12)",
            params![
                id,
                input.brand,
                input.material,
                input.color,
                input.name,
                input.notes,
                resource_kind,
                source_id,
                external_id,
                parent_filament_id,
                archived,
                now
            ],
        )?;
        Ok((id, true, false))
    }
}
fn validated_archive_member(value: &str) -> Result<String, LibraryError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        Err(LibraryError::InvalidInput("unsafe archive member".into()))
    } else {
        Ok(value.into())
    }
}
fn ensure_archive_bundle(
    tx: &rusqlite::Transaction<'_>,
    root: &str,
    path: &str,
) -> Result<String, LibraryError> {
    if let Some(id) = tx
        .query_row(
            "SELECT id FROM bundles WHERE source_root_id=?1 AND source_path=?2",
            params![root, path],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    let id = new_id("bundle");
    tx.execute(
        "INSERT INTO bundles(id,source_root_id,source_path,created_at) VALUES(?1,?2,?3,?4)",
        params![id, root, path, timestamp()],
    )?;
    Ok(id)
}
fn reconcile_folder_bundles(
    tx: &rusqlite::Transaction<'_>,
    root_id: &str,
) -> Result<(), LibraryError> {
    #[derive(Clone)]
    struct Item {
        id: String,
        bundle_id: Option<String>,
        auto_bundle: Option<String>,
        manual: bool,
    }
    let mut groups: BTreeMap<Option<String>, Vec<Item>> = BTreeMap::new();
    {
        let mut stmt = tx.prepare(
            "SELECT m.id,m.source_path,m.bundle_id,b.id,b.source_root_id
             FROM models m LEFT JOIN bundles b ON b.id=m.bundle_id
             WHERE m.source_kind='indexed' AND m.source_root_id=?1
               AND m.source_archive_member IS NULL AND m.source_missing=0",
        )?;
        let rows = stmt.query_map(params![root_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        for row in rows {
            let (id, path, bundle_id, bundle_exists, bundle_root) = row?;
            let parent = Path::new(&path)
                .parent()
                .and_then(Path::to_str)
                .filter(|parent| !parent.is_empty())
                .map(str::to_owned);
            let manual = bundle_id.is_some() && bundle_exists.is_some() && bundle_root.is_none();
            let auto_bundle = bundle_root.is_some().then_some(bundle_id.clone()).flatten();
            groups.entry(parent).or_default().push(Item {
                id,
                bundle_id,
                auto_bundle,
                manual,
            });
        }
    }
    let mut bundle_folders: BTreeMap<String, BTreeSet<Option<String>>> = BTreeMap::new();
    for (folder, items) in &groups {
        for item in items {
            if !item.manual {
                if let Some(bundle) = &item.auto_bundle {
                    bundle_folders
                        .entry(bundle.clone())
                        .or_default()
                        .insert(folder.clone());
                }
            }
        }
    }
    for (folder, items) in groups {
        let eligible = items
            .into_iter()
            .filter(|item| !item.manual)
            .collect::<Vec<_>>();
        if folder.is_none() || eligible.len() < 2 {
            for item in eligible {
                if item.auto_bundle.is_some() {
                    tx.execute("UPDATE models SET bundle_id=NULL,metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",params![timestamp(),item.id])?;
                }
            }
            continue;
        }
        let marker = format!("{}/", folder.unwrap().trim_end_matches('/'));
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM bundles WHERE source_root_id=?1 AND source_path=?2",
                params![root_id, marker],
                |row| row.get(0),
            )
            .optional()?;
        let bundle_id = if let Some(id) = existing {
            id
        } else {
            let candidates = eligible
                .iter()
                .filter_map(|item| item.auto_bundle.clone())
                .collect::<BTreeSet<_>>();
            if candidates.len() == 1
                && bundle_folders
                    .get(candidates.iter().next().unwrap())
                    .is_some_and(|folders| folders.len() == 1)
            {
                let id = candidates.into_iter().next().unwrap();
                tx.execute(
                    "UPDATE bundles SET source_root_id=?1,source_path=?2 WHERE id=?3",
                    params![root_id, marker, id],
                )?;
                id
            } else {
                let id = new_id("bundle");
                tx.execute("INSERT INTO bundles(id,source_root_id,source_path,created_at) VALUES(?1,?2,?3,?4)",params![id,root_id,marker,timestamp()])?;
                id
            }
        };
        for item in eligible {
            if item.bundle_id.as_deref() != Some(&bundle_id) {
                tx.execute("UPDATE models SET bundle_id=?1,metadata_version=metadata_version+1,updated_at=?2 WHERE id=?3",params![bundle_id,timestamp(),item.id])?;
            }
        }
    }
    Ok(())
}
fn print_status_value(value: &PrintStatus) -> &'static str {
    match value {
        PrintStatus::Unprinted => "unprinted",
        PrintStatus::Want => "want",
        PrintStatus::Queued => "queued",
        PrintStatus::Printing => "printing",
        PrintStatus::Printed => "printed",
        PrintStatus::Failed => "failed",
    }
}
fn print_outcome_value(value: &PrintOutcome) -> &'static str {
    match value {
        PrintOutcome::Printed => "printed",
        PrintOutcome::Failed => "failed",
    }
}
fn validate_metadata_patch(p: &MetadataPatch) -> Result<(), LibraryError> {
    if p.rating.is_some_and(|rating| rating > 5) {
        return Err(LibraryError::InvalidInput(
            "rating must be between 0 and 5".into(),
        ));
    }
    clean_optional(p.designer.as_deref(), 200)?;
    clean_optional(p.source_url.as_deref(), 2048)?;
    clean_optional(p.notes.as_deref(), 20_000)?;
    clean_optional(p.model_license.as_deref(), 200)?;
    if let Some(url) = clean_optional(p.source_url.as_deref(), 2048)? {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(LibraryError::InvalidInput(
                "source URL must use http or https".into(),
            ));
        }
    }
    if let Some(tags) = &p.tags {
        if tags.len() > 200 {
            return Err(LibraryError::InvalidInput("too many tags".into()));
        }
    }
    Ok(())
}
fn apply_metadata_patch(
    tx: &rusqlite::Transaction<'_>,
    model_id: &str,
    expected_version: u64,
    patch: &MetadataPatch,
) -> Result<(), LibraryError> {
    let actual: i64 = tx
        .query_row(
            "SELECT metadata_version FROM models WHERE id=?1",
            params![model_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| LibraryError::ModelNotFound(model_id.to_owned()))?;
    if actual as u64 != expected_version {
        return Err(LibraryError::MetadataConflict {
            model_id: model_id.to_owned(),
            expected: expected_version,
            actual: actual as u64,
        });
    }
    if let Some(folder) = &patch.folder_id {
        let ok: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM folders WHERE id=?1)",
            params![folder],
            |r| r.get(0),
        )?;
        if !ok {
            return Err(LibraryError::FolderNotFound(folder.clone()));
        }
    }
    if let Some(parent) = &patch.parent_model_id {
        let parent = parent.trim();
        if !parent.is_empty() {
            ensure_no_parent_cycle(tx, model_id, parent)?;
        }
    }
    let designer = patch
        .designer
        .as_deref()
        .map(|v| clean_optional(Some(v), 200))
        .transpose()?
        .flatten();
    let source_url = patch
        .source_url
        .as_deref()
        .map(|v| clean_optional(Some(v), 2048))
        .transpose()?
        .flatten();
    let notes = patch
        .notes
        .as_deref()
        .map(|v| clean_optional(Some(v), 20_000))
        .transpose()?
        .flatten();
    let license = patch
        .model_license
        .as_deref()
        .map(|v| clean_optional(Some(v), 200))
        .transpose()?
        .flatten();
    let parent = patch.parent_model_id.as_deref().map(str::trim).map(|v| {
        if v.is_empty() {
            None
        } else {
            Some(v.to_owned())
        }
    });
    tx.execute("UPDATE models SET designer=CASE WHEN ?1 THEN ?2 ELSE designer END,source_url=CASE WHEN ?3 THEN ?4 ELSE source_url END,notes=CASE WHEN ?5 THEN ?6 ELSE notes END,description=CASE WHEN ?5 THEN COALESCE(?6,'') ELSE description END,model_license=CASE WHEN ?7 THEN ?8 ELSE model_license END,print_status=COALESCE(?9,print_status),folder_id=COALESCE(?10,folder_id),parent_model_id=CASE WHEN ?11 THEN ?12 ELSE parent_model_id END,rating=COALESCE(?13,rating),favorite=COALESCE(?14,favorite),metadata_version=metadata_version+1,updated_at=?15 WHERE id=?16",params![patch.designer.is_some(),designer,patch.source_url.is_some(),source_url,patch.notes.is_some(),notes,patch.model_license.is_some(),license,patch.print_status.as_ref().map(print_status_value),patch.folder_id,patch.parent_model_id.is_some(),parent.flatten(),patch.rating.map(i64::from),patch.favorite.map(i64::from),timestamp(),model_id])?;
    if let Some(tags) = &patch.tags {
        for id in tags {
            let ok: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM tags WHERE id=?1)",
                params![id],
                |r| r.get(0),
            )?;
            if !ok {
                return Err(LibraryError::InvalidInput(format!("tag not found: {id}")));
            }
        }
        if !matches!(patch.tag_mode, Some(TagMode::Merge)) {
            tx.execute(
                "DELETE FROM model_tags WHERE model_id=?1",
                params![model_id],
            )?;
        }
        for id in tags {
            tx.execute(
                "INSERT OR IGNORE INTO model_tags(model_id,tag_id) VALUES(?1,?2)",
                params![model_id, id],
            )?;
        }
    }
    Ok(())
}
fn ensure_no_parent_cycle(
    tx: &rusqlite::Transaction<'_>,
    model: &str,
    parent: &str,
) -> Result<(), LibraryError> {
    let mut cur = Some(parent.to_owned());
    while let Some(id) = cur {
        if id == model {
            return Err(LibraryError::InvalidInput("model parent cycle".into()));
        }
        cur = tx
            .query_row(
                "SELECT parent_model_id FROM models WHERE id=?1",
                params![id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .ok_or_else(|| LibraryError::ModelNotFound(id.clone()))?;
    }
    Ok(())
}
fn tag_model_ids(tx: &rusqlite::Transaction<'_>, tag: &str) -> Result<Vec<String>, LibraryError> {
    let mut s = tx.prepare("SELECT model_id FROM model_tags WHERE tag_id=?1")?;
    let r = s.query_map(params![tag], |row| row.get(0))?;
    Ok(r.collect::<Result<Vec<_>, _>>()?)
}
fn bump_models(tx: &rusqlite::Transaction<'_>, ids: &[String]) -> Result<(), LibraryError> {
    for id in ids {
        tx.execute(
            "UPDATE models SET metadata_version=metadata_version+1,updated_at=?1 WHERE id=?2",
            params![timestamp(), id],
        )?;
    }
    Ok(())
}
fn validate_print_attempt(i: &PrintAttemptInput) -> Result<(), LibraryError> {
    if i.quantity == 0
        || i.quantity > 10_000
        || i.notes.len() > 20_000
        || i.filament_ids.len() > 5
        || i.filament_ids.iter().any(|id| {
            id.is_empty() || id.len() > 256 || id.trim() != id || id.chars().any(char::is_control)
        })
        || normalize_rfc3339(&i.printed_at).is_err()
    {
        return Err(LibraryError::InvalidInput("invalid print attempt".into()));
    }
    Ok(())
}
fn write_print_attempt(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    model: &str,
    i: &PrintAttemptInput,
    now: &str,
    create: bool,
) -> Result<(), LibraryError> {
    let printed_at = normalize_rfc3339(&i.printed_at)?;
    if create {
        tx.execute(
            "INSERT INTO print_attempts VALUES(?1,?2,?3,?4,?5,?6,?7,?7)",
            params![
                id,
                model,
                printed_at,
                i.quantity,
                i.notes,
                print_outcome_value(&i.outcome),
                now
            ],
        )?;
    } else {
        tx.execute("UPDATE print_attempts SET printed_at=?1,quantity=?2,notes=?3,outcome=?4,updated_at=?5 WHERE id=?6",params![printed_at,i.quantity,i.notes,print_outcome_value(&i.outcome),now,id])?;
        tx.execute(
            "DELETE FROM print_attempt_filaments WHERE attempt_id=?1",
            params![id],
        )?;
    }
    tx.execute(
        "UPDATE models SET print_status=?1 WHERE id=?2",
        params![print_outcome_value(&i.outcome), model],
    )?;
    let mut seen = HashSet::new();
    for (n, f) in i.filament_ids.iter().enumerate() {
        if !seen.insert(f) {
            return Err(LibraryError::InvalidInput("duplicate filament".into()));
        }
        tx.execute(
            "INSERT INTO print_attempt_filaments VALUES(?1,?2,?3)",
            params![id, f, n as i64],
        )?;
    }
    Ok(())
}
fn recompute_print_summary(
    tx: &rusqlite::Transaction<'_>,
    model: &str,
) -> Result<(), LibraryError> {
    let (count,last):(i64,Option<String>)=tx.query_row("SELECT COALESCE(SUM(CASE WHEN outcome='printed' THEN quantity ELSE 0 END),0),MAX(CASE WHEN outcome='printed' THEN printed_at END) FROM print_attempts WHERE model_id=?1",params![model],|r|Ok((r.get(0)?,r.get(1)?)))?;
    tx.execute("UPDATE models SET print_count=?1,last_printed_at=?2,metadata_version=metadata_version+1,updated_at=?3 WHERE id=?4",params![count,last,timestamp(),model])?;
    Ok(())
}
fn attempt_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PrintAttempt> {
    let o: String = r.get(5)?;
    Ok(PrintAttempt {
        id: r.get(0)?,
        model_id: r.get(1)?,
        printed_at: r.get(2)?,
        quantity: r.get::<_, i64>(3)? as u32,
        notes: r.get(4)?,
        filament_ids: Vec::new(),
        outcome: if o == "printed" {
            PrintOutcome::Printed
        } else {
            PrintOutcome::Failed
        },
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}
fn attempt_filaments(conn: &Connection, id: &str) -> Result<Vec<String>, LibraryError> {
    let mut s = conn.prepare(
        "SELECT filament_id FROM print_attempt_filaments WHERE attempt_id=?1 ORDER BY ordinal",
    )?;
    let r = s.query_map(params![id], |row| row.get(0))?;
    Ok(r.collect::<Result<Vec<_>, _>>()?)
}
fn normalize_rfc3339(value: &str) -> Result<String, LibraryError> {
    use time::format_description::well_known::Rfc3339;
    use time::{OffsetDateTime, UtcOffset};
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| LibraryError::InvalidInput("printed_at must be RFC3339".into()))?
        .to_offset(UtcOffset::UTC);
    let millis = parsed.unix_timestamp_nanos().div_euclid(1_000_000);
    let normalized = OffsetDateTime::from_unix_timestamp_nanos(millis * 1_000_000)
        .map_err(|_| LibraryError::InvalidInput("printed_at is outside supported range".into()))?;
    let format = time::format_description::parse_borrowed::<2>(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z",
    )
    .map_err(|_| LibraryError::InvalidInput("timestamp format is unavailable".into()))?;
    normalized
        .format(&format)
        .map_err(|_| LibraryError::InvalidInput("printed_at is outside supported range".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const TEST_SCHEMA_V1: &str = r#"
    CREATE TABLE libraries (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL);
    INSERT INTO libraries VALUES ('local', 'Local Library', '0');
    CREATE TABLE folders (id TEXT PRIMARY KEY, name TEXT NOT NULL, parent_id TEXT REFERENCES folders(id) ON DELETE RESTRICT, UNIQUE(parent_id, name));
    INSERT INTO folders VALUES ('root', 'All Models', NULL);
    CREATE TABLE blobs (hash TEXT PRIMARY KEY, relative_path TEXT NOT NULL UNIQUE, bytes INTEGER NOT NULL CHECK(bytes >= 0));
    CREATE TABLE models (id TEXT PRIMARY KEY, display_name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', folder_id TEXT NOT NULL REFERENCES folders(id) ON DELETE RESTRICT, current_revision_id TEXT, metadata_version INTEGER NOT NULL DEFAULT 1 CHECK(metadata_version > 0), deleted_at TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
    CREATE TABLE revisions (id TEXT PRIMARY KEY, model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE, blob_hash TEXT NOT NULL REFERENCES blobs(hash) ON DELETE RESTRICT, original_name TEXT NOT NULL, format TEXT NOT NULL, bytes INTEGER NOT NULL CHECK(bytes >= 0), created_at TEXT NOT NULL);
    CREATE TABLE tags (id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE);
    CREATE TABLE model_tags (model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE, tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE, PRIMARY KEY(model_id, tag_id));
    CREATE TABLE manuals (model_id TEXT PRIMARY KEY REFERENCES models(id) ON DELETE CASCADE, relative_path TEXT NOT NULL, content_hash TEXT NOT NULL);
    CREATE TABLE thumbnails (id TEXT PRIMARY KEY, model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE, relative_path TEXT NOT NULL, content_hash TEXT NOT NULL, is_custom INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE jobs (id TEXT PRIMARY KEY, model_id TEXT REFERENCES models(id) ON DELETE CASCADE, kind TEXT NOT NULL, status TEXT NOT NULL, retry_count INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
    "#;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "gogglelab-library-{label}-{}-{}",
            std::process::id(),
            Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    fn hash(byte: u8) -> String {
        format!("{byte:02x}").repeat(32)
    }

    fn png_fixture() -> Vec<u8> {
        b"\x89PNG\r\n\x1a\nthumbnail fixture".to_vec()
    }

    fn stl_fixture() -> Vec<u8> {
        let mut bytes = vec![0; 80];
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(
            &[0.0f32; 3]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        for value in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
        bytes
    }

    #[test]
    fn fresh_database_has_root_and_round_trips_model_after_restart() {
        let root = temp_root("restart");
        let db = LibraryDb::open(&root).unwrap();
        let folder = db.create_folder("Projects", None).unwrap();
        let model = db.create_model("Bracket", Some(&folder.id)).unwrap();
        drop(db);
        let reopened = LibraryDb::open(&root).unwrap();
        assert_eq!(
            reopened.get_model(&model.id).unwrap().unwrap().display_name,
            "Bracket"
        );
        assert_eq!(reopened.list_folders().unwrap().len(), 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn catalog_settings_persist_and_existing_schema_v2_gets_defaults() {
        let root = temp_root("catalog-settings");
        let db = LibraryDb::open(&root).unwrap();
        assert_eq!(
            db.get_catalog_settings().unwrap(),
            CatalogSettings::default()
        );
        let updated = CatalogSettings {
            thumbnail_size: 512,
            thumbnail_background: "#A1B2C3".into(),
            thumbnail_concurrency: 1,
            scan_batch_size: 256,
        };
        assert_eq!(db.update_catalog_settings(&updated).unwrap(), updated);
        drop(db);
        assert_eq!(
            LibraryDb::open(&root)
                .unwrap()
                .get_catalog_settings()
                .unwrap(),
            updated
        );

        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute("DROP TABLE catalog_settings", []).unwrap();
        drop(conn);
        assert_eq!(
            LibraryDb::open(&root)
                .unwrap()
                .get_catalog_settings()
                .unwrap(),
            CatalogSettings::default()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn catalog_settings_reject_out_of_range_values_without_persisting() {
        let db = LibraryDb::open_in_memory().unwrap();
        let original = db.get_catalog_settings().unwrap();
        for invalid in [
            CatalogSettings {
                thumbnail_size: 300,
                ..original.clone()
            },
            CatalogSettings {
                thumbnail_background: "12171F".into(),
                ..original.clone()
            },
            CatalogSettings {
                thumbnail_concurrency: 3,
                ..original.clone()
            },
            CatalogSettings {
                scan_batch_size: 15,
                ..original.clone()
            },
        ] {
            assert!(matches!(
                db.update_catalog_settings(&invalid),
                Err(LibraryError::InvalidInput(_))
            ));
            assert_eq!(db.get_catalog_settings().unwrap(), original);
        }

        db.conn
            .execute_batch(
                "DROP TABLE catalog_settings;
                 CREATE TABLE catalog_settings (
                   id TEXT PRIMARY KEY,
                   thumbnail_size INTEGER NOT NULL,
                   thumbnail_background TEXT NOT NULL,
                   thumbnail_concurrency INTEGER NOT NULL,
                   scan_batch_size INTEGER NOT NULL
                 );
                 INSERT INTO catalog_settings VALUES
                   ('local', 4294967552, '#12171F', 4294967297, 128);",
            )
            .unwrap();
        assert!(matches!(
            db.get_catalog_settings(),
            Err(LibraryError::InvalidInput(_))
        ));
    }

    #[test]
    fn metadata_tags_history_and_parent_updates_are_versioned_and_atomic() {
        let db = LibraryDb::open_in_memory().unwrap();
        let a = db.create_model("A", None).unwrap();
        let b = db.create_model("B", None).unwrap();
        let tag = db.create_tag("Mechanical", Some("Type")).unwrap();
        let patch = MetadataPatch {
            designer: Some("Ada".into()),
            notes: Some("notes".into()),
            tags: Some(vec![tag.id.clone()]),
            tag_mode: Some(TagMode::Replace),
            parent_model_id: Some(b.id.clone()),
            ..MetadataPatch::default()
        };
        let a = db.update_model_metadata(&a.id, 1, &patch).unwrap();
        assert_eq!(a.metadata.designer.as_deref(), Some("Ada"));
        assert_eq!(a.tags, vec!["Mechanical"]);
        assert_eq!(a.metadata.parent_model_id.as_deref(), Some(b.id.as_str()));
        assert!(matches!(
            db.update_model_metadata(&a.id, 1, &MetadataPatch::default()),
            Err(LibraryError::MetadataConflict { .. })
        ));
        assert!(db
            .update_model_metadata(
                &b.id,
                1,
                &MetadataPatch {
                    parent_model_id: Some(a.id.clone()),
                    ..MetadataPatch::default()
                }
            )
            .is_err());
        let before = a.metadata_version;
        assert!(db
            .bulk_update_metadata(
                &[
                    VersionedTarget {
                        model_id: a.id.clone(),
                        expected_version: before
                    },
                    VersionedTarget {
                        model_id: b.id.clone(),
                        expected_version: 999
                    }
                ],
                &MetadataPatch {
                    notes: Some("bulk".into()),
                    ..MetadataPatch::default()
                }
            )
            .is_err());
        assert_eq!(
            db.get_model(&a.id).unwrap().unwrap().metadata_version,
            before
        );
        let renamed = db.rename_tag(&tag.id, "Hardware", Some("Type")).unwrap();
        assert_eq!(renamed.name, "Hardware");
        let bumped = db.get_model(&a.id).unwrap().unwrap();
        assert!(bumped.metadata_version > before);
        assert_eq!(bumped.tags, vec!["Hardware"]);
        db.delete_tag(&tag.id).unwrap();
        assert!(db.get_model(&a.id).unwrap().unwrap().tags.is_empty());
        let attempt = db
            .create_print_attempt(
                &a.id,
                &PrintAttemptInput {
                    printed_at: "2026-10-10T12:00:00-05:00".into(),
                    quantity: 2,
                    notes: "good".into(),
                    filament_ids: vec!["pla-red".into()],
                    outcome: PrintOutcome::Printed,
                },
            )
            .unwrap();
        assert_eq!(attempt.filament_ids, vec!["pla-red"]);
        assert_eq!(attempt.printed_at, "2026-10-10T17:00:00.000Z");
        let printed = db.get_model(&a.id).unwrap().unwrap();
        assert_eq!(printed.metadata.print_count, 2);
        assert_eq!(
            printed.metadata.last_printed_at.as_deref(),
            Some("2026-10-10T17:00:00.000Z")
        );
        let failed_attempt = db
            .create_print_attempt(
                &a.id,
                &PrintAttemptInput {
                    printed_at: "2026-10-11T00:00:00Z".into(),
                    quantity: 1,
                    notes: "failed".into(),
                    filament_ids: vec![],
                    outcome: PrintOutcome::Failed,
                },
            )
            .unwrap();
        db.update_print_attempt(
            &failed_attempt.id,
            &PrintAttemptInput {
                printed_at: "2026-10-11T01:00:00+01:00".into(),
                quantity: 1,
                notes: "failed edited".into(),
                filament_ids: vec![],
                outcome: PrintOutcome::Failed,
            },
        )
        .unwrap();
        let after_failure = db.get_model(&a.id).unwrap().unwrap();
        assert_eq!(after_failure.metadata.print_status, PrintStatus::Failed);
        assert_eq!(after_failure.metadata.print_count, 2);
        assert_eq!(
            after_failure.metadata.last_printed_at.as_deref(),
            Some("2026-10-10T17:00:00.000Z")
        );
        let current = db.get_model(&a.id).unwrap().unwrap();
        db.update_model_metadata(
            &a.id,
            current.metadata_version,
            &MetadataPatch {
                print_status: Some(PrintStatus::Queued),
                ..MetadataPatch::default()
            },
        )
        .unwrap();
        db.delete_print_attempt(&attempt.id).unwrap();
        assert_eq!(
            db.get_model(&a.id).unwrap().unwrap().metadata.print_status,
            PrintStatus::Queued
        );
        let current = db.get_model(&a.id).unwrap().unwrap();
        let cleared = db
            .update_model_metadata(
                &a.id,
                current.metadata_version,
                &MetadataPatch {
                    notes: Some(String::new()),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        assert_eq!(cleared.metadata.notes, None);
        assert_eq!(cleared.description, "");
    }

    #[test]
    fn global_metadata_rename_and_delete_are_atomic_and_preserve_parent_identity() {
        let db = LibraryDb::open_in_memory().unwrap();
        let parent_a = db.create_model("Old parent", None).unwrap();
        let parent_b = db.create_model("Old parent", None).unwrap();
        let child = db.create_model("Child", None).unwrap();
        let child = db
            .update_model_metadata(
                &child.id,
                child.metadata_version,
                &MetadataPatch {
                    designer: Some("Old designer".into()),
                    model_license: Some("Old license".into()),
                    parent_model_id: Some(parent_a.id.clone()),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();

        assert_eq!(
            db.rename_metadata_value("designer", "Old designer", "New designer")
                .unwrap(),
            1
        );
        assert_eq!(
            db.rename_metadata_value("license", "Old license", "New license")
                .unwrap(),
            1
        );
        assert_eq!(
            db.rename_metadata_value("parentModel", "Old parent", "New parent")
                .unwrap(),
            2
        );
        let renamed_parent = db.get_model(&parent_a.id).unwrap().unwrap();
        assert_eq!(renamed_parent.display_name, "New parent");
        assert_eq!(
            db.get_model(&parent_b.id).unwrap().unwrap().display_name,
            "New parent"
        );
        let child = db.get_model(&child.id).unwrap().unwrap();
        assert_eq!(
            child.metadata.parent_model_id.as_deref(),
            Some(parent_a.id.as_str())
        );
        assert_eq!(child.metadata.designer.as_deref(), Some("New designer"));
        assert_eq!(child.metadata.model_license.as_deref(), Some("New license"));
        let prior_version = child.metadata_version;

        assert_eq!(
            db.delete_metadata_value("parentModel", "New parent")
                .unwrap(),
            1
        );
        assert_eq!(
            db.delete_metadata_value("designer", "New designer")
                .unwrap(),
            1
        );
        assert_eq!(
            db.delete_metadata_value("license", "New license").unwrap(),
            1
        );
        let child = db.get_model(&child.id).unwrap().unwrap();
        assert!(child.metadata.parent_model_id.is_none());
        assert!(child.metadata.designer.is_none());
        assert!(child.metadata.model_license.is_none());
        assert!(child.metadata_version > prior_version);
        assert!(db.rename_metadata_value("unknown", "x", "y").is_err());
    }

    #[test]
    fn thumbnail_filters_use_active_preview_and_filter_before_cursor_limit() {
        let db = LibraryDb::open_in_memory().unwrap();
        let default = db.create_model("Alpha default", None).unwrap();
        let custom = db.create_model("Beta custom", None).unwrap();
        let missing = db.create_model("Gamma missing", None).unwrap();
        let stale = db.create_model("Delta stale", None).unwrap();
        for (id, model, is_custom, active, source_hash) in [
            ("default", &default, 0, 1, None),
            ("custom-fallback", &custom, 0, 0, None),
            ("custom", &custom, 1, 1, None),
            ("stale", &stale, 0, 1, Some("no-longer-current")),
        ] {
            db.conn.execute(
                "INSERT INTO thumbnails(id,model_id,relative_path,content_hash,is_custom,source_content_hash,ordinal,is_active) VALUES(?1,?2,'thumbnails/fixture.png',?3,?4,?5,0,?6)",
                params![id, model.id, "0".repeat(64), is_custom, source_hash, active],
            ).unwrap();
        }
        for (mode, expected) in [("default", &default.id), ("custom", &custom.id)] {
            let query = ModelPageQuery {
                thumbnail_mode: Some(mode.into()),
                sort: ModelSort::Name,
                ..ModelPageQuery::default()
            };
            let (models, more) = db.list_models_page(&query, None, None, 1).unwrap();
            assert_eq!(models[0].id, *expected);
            assert!(!more);
        }
        let query = ModelPageQuery {
            thumbnail_mode: Some("missing".into()),
            sort: ModelSort::Name,
            ..ModelPageQuery::default()
        };
        let (first, more) = db.list_models_page(&query, None, None, 1).unwrap();
        assert_eq!(first[0].id, stale.id);
        assert!(more);
        let (second, more) = db
            .list_models_page(&query, Some("delta stale"), Some(&stale.id), 1)
            .unwrap();
        assert_eq!(second[0].id, missing.id);
        assert!(!more);
        assert!(db
            .list_models_page(
                &ModelPageQuery {
                    thumbnail_mode: Some("invalid".into()),
                    ..ModelPageQuery::default()
                },
                None,
                None,
                1
            )
            .is_err());
    }

    #[test]
    fn model_tag_name_mutations_are_atomic_versioned_and_keep_removed_tags() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Tagged model", None).unwrap();
        let keep = db.create_tag("Keep", Some("Type")).unwrap();
        let remove = db.create_tag("Remove", Some("Type")).unwrap();
        let model = db
            .update_model_metadata(
                &model.id,
                model.metadata_version,
                &MetadataPatch {
                    tags: Some(vec![keep.id.clone(), remove.id.clone()]),
                    tag_mode: Some(TagMode::Replace),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();

        let added = db
            .add_model_tags_by_name(
                &model.id,
                model.metadata_version,
                &["Keep".into(), "New tag".into()],
            )
            .unwrap();
        assert_eq!(added.metadata_version, model.metadata_version + 1);
        assert_eq!(added.tags, vec!["Keep", "New tag", "Remove"]);
        assert!(db
            .list_tags()
            .unwrap()
            .iter()
            .any(|tag| tag.name == "New tag"));

        let removed = db
            .remove_model_tags_by_name(&model.id, added.metadata_version, &["remove".into()])
            .unwrap();
        assert_eq!(removed.metadata_version, added.metadata_version + 1);
        assert_eq!(removed.tags, vec!["Keep", "New tag"]);
        assert!(db
            .list_tags()
            .unwrap()
            .iter()
            .any(|tag| tag.id == remove.id));

        let unchanged = db
            .remove_model_tags_by_name(&model.id, removed.metadata_version, &["Remove".into()])
            .unwrap();
        assert_eq!(unchanged.metadata_version, removed.metadata_version);

        assert!(db
            .add_model_tags_by_name(&model.id, model.metadata_version, &["Stale tag".into()])
            .is_err());
        assert!(!db
            .list_tags()
            .unwrap()
            .iter()
            .any(|tag| tag.name == "Stale tag"));
        assert!(db
            .add_model_tags_by_name(
                &model.id,
                removed.metadata_version,
                &["Dup".into(), "dup".into()]
            )
            .is_err());
        assert!(!db.list_tags().unwrap().iter().any(|tag| tag.name == "Dup"));
    }

    #[test]
    fn source_directory_filter_is_root_relative_and_applied_before_keyset_limits() {
        let db = LibraryDb::open_in_memory().unwrap();
        let first_root_path = temp_root("source-directory-first-root");
        fs::create_dir_all(&first_root_path).unwrap();
        let root = db
            .add_index_root(&first_root_path, 60, 1024 * 1024)
            .unwrap();
        let entries = [
            ("A in parts", "Parts/a.stl"),
            ("B nested in parts", "Parts/Nested/b.stl"),
            ("C outside parts", "Parts-old/c.stl"),
        ];
        let mut models = Vec::new();
        for (name, relative) in entries {
            let model = db.create_model(name, None).unwrap();
            db.conn.execute(
                "UPDATE models SET source_kind='indexed',source_root_id=?1,source_path=?2,source_missing=0 WHERE id=?3",
                params![root.id, relative, model.id],
            ).unwrap();
            models.push(db.get_model(&model.id).unwrap().unwrap());
        }
        let second_root_path = temp_root("source-directory-second-root");
        fs::create_dir_all(&second_root_path).unwrap();
        let second_root = db
            .add_index_root(&second_root_path, 60, 1024 * 1024)
            .unwrap();
        let second_root_model = db.create_model("Parts in second root", None).unwrap();
        db.conn.execute(
            "UPDATE models SET source_kind='indexed',source_root_id=?1,source_path='Parts/d.stl',source_missing=0 WHERE id=?2",
            params![second_root.id, second_root_model.id],
        ).unwrap();
        let query = ModelPageQuery {
            root_id: Some(root.id),
            source_directory: Some("Parts".into()),
            sort: ModelSort::Name,
            ..ModelPageQuery::default()
        };
        let (first, has_more) = db.list_models_page(&query, None, None, 1).unwrap();
        assert_eq!(first[0].id, models[0].id);
        assert!(has_more);
        let (second, has_more) = db
            .list_models_page(&query, Some("a in parts"), Some(&first[0].id), 1)
            .unwrap();
        assert_eq!(second[0].id, models[1].id);
        assert!(!has_more);
        let (other_root_parts, has_more) = db
            .list_models_page(
                &ModelPageQuery {
                    root_id: Some(second_root.id),
                    source_directory: Some("Parts".into()),
                    ..ModelPageQuery::default()
                },
                None,
                None,
                10,
            )
            .unwrap();
        assert_eq!(other_root_parts.len(), 1);
        assert_eq!(other_root_parts[0].id, second_root_model.id);
        assert!(!has_more);

        for (root_id, source_directory) in [
            (None, Some("Parts".to_owned())),
            (
                Some(models[0].source_root_id.clone().unwrap()),
                Some("../outside".to_owned()),
            ),
        ] {
            assert!(db
                .list_models_page(
                    &ModelPageQuery {
                        root_id,
                        source_directory,
                        ..ModelPageQuery::default()
                    },
                    None,
                    None,
                    10
                )
                .is_err());
        }
        let _ = fs::remove_dir_all(first_root_path);
        let _ = fs::remove_dir_all(second_root_path);
    }

    #[test]
    fn file_type_filter_matches_archive_source_extension_and_member_format() {
        let db = LibraryDb::open_in_memory().unwrap();
        let source_root = temp_root("file-type-archive-source");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(source_root.join("parts.zip"), b"synthetic archive").unwrap();
        let mut root = db.add_index_root(&source_root, 60, 1024 * 1024).unwrap();
        root.scan_zip = true;
        db.update_index_root(&root).unwrap();
        db.record_scan_entries(
            &root.id,
            &[ScannedModel {
                relative_path: "parts.zip".into(),
                content_hash: hash(7),
                bytes: 17,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("Archive member".into()),
                archive_member: Some("member.stl".into()),
                outer_fingerprint: Some("synthetic-archive-fingerprint".into()),
            }],
        )
        .unwrap();

        let (archive_matches, _) = db
            .list_models_page(
                &ModelPageQuery {
                    file_type: Some("zip".into()),
                    ..ModelPageQuery::default()
                },
                None,
                None,
                10,
            )
            .unwrap();
        assert_eq!(archive_matches.len(), 1);
        assert_eq!(archive_matches[0].display_name, "Archive member");

        let (mesh_matches, _) = db
            .list_models_page(
                &ModelPageQuery {
                    file_type: Some(".STL".into()),
                    ..ModelPageQuery::default()
                },
                None,
                None,
                10,
            )
            .unwrap();
        assert_eq!(mesh_matches.len(), 1);
        let _ = fs::remove_dir_all(source_root);
    }

    #[test]
    fn indexed_source_move_batch_is_atomic_and_rejects_duplicate_destinations() {
        let db = LibraryDb::open_in_memory().unwrap();
        let source_root_path = temp_root("move-batch-source");
        let destination_root_path = temp_root("move-batch-destination");
        fs::create_dir_all(&source_root_path).unwrap();
        fs::create_dir_all(&destination_root_path).unwrap();
        fs::write(source_root_path.join("a.stl"), b"synthetic a").unwrap();
        fs::write(source_root_path.join("b.stl"), b"synthetic b").unwrap();
        let source_root = db
            .add_index_root(&source_root_path, 60, 1024 * 1024)
            .unwrap();
        let destination_root = db
            .add_index_root(&destination_root_path, 60, 1024 * 1024)
            .unwrap();
        db.record_scan_entries(
            &source_root.id,
            &[
                ScannedModel {
                    relative_path: "a.stl".into(),
                    content_hash: hash(1),
                    bytes: 11,
                    format: "stl-binary".into(),
                    modified_at: 1,
                    display_name: Some("A".into()),
                    archive_member: None,
                    outer_fingerprint: None,
                },
                ScannedModel {
                    relative_path: "b.stl".into(),
                    content_hash: hash(2),
                    bytes: 11,
                    format: "stl-binary".into(),
                    modified_at: 1,
                    display_name: Some("B".into()),
                    archive_member: None,
                    outer_fingerprint: None,
                },
            ],
        )
        .unwrap();
        let before = db.list_models(None, None).unwrap();
        let moves = vec![
            IndexedModelMove {
                model_id: before[0].id.clone(),
                expected_version: before[0].metadata_version,
                destination_root_id: destination_root.id.clone(),
                destination_relative_path: "moved-a.stl".into(),
                modified_at: 2,
            },
            IndexedModelMove {
                model_id: before[1].id.clone(),
                expected_version: before[1].metadata_version,
                destination_root_id: destination_root.id.clone(),
                destination_relative_path: "moved-b.stl".into(),
                modified_at: 2,
            },
        ];

        let moved = db.move_loose_indexed_models(&moves).unwrap();
        assert_eq!(moved.len(), 2);
        for (index, model) in moved.iter().enumerate() {
            assert_eq!(model.id, before[index].id);
            assert_eq!(model.metadata_version, before[index].metadata_version + 1);
            assert_eq!(
                model.source_root_id.as_deref(),
                Some(destination_root.id.as_str())
            );
            assert!(model
                .source_path
                .as_deref()
                .unwrap()
                .ends_with(if index == 0 {
                    "moved-a.stl"
                } else {
                    "moved-b.stl"
                }));
        }

        let mut stale = moves.clone();
        stale[0].destination_relative_path = "should-not-move-a.stl".into();
        stale[0].expected_version = moved[0].metadata_version;
        stale[1].destination_relative_path = "should-not-move-b.stl".into();
        stale[1].expected_version = moved[1].metadata_version + 1;
        assert!(db.move_loose_indexed_models(&stale).is_err());

        let mut duplicate_destinations = moves.clone();
        duplicate_destinations[0].expected_version = moved[0].metadata_version;
        duplicate_destinations[1].expected_version = moved[1].metadata_version;
        duplicate_destinations[0].destination_relative_path = "same.stl".into();
        duplicate_destinations[1].destination_relative_path = "same.stl".into();
        assert!(db
            .move_loose_indexed_models(&duplicate_destinations)
            .is_err());
        for (index, original) in before.iter().enumerate() {
            let unchanged = db.get_model(&original.id).unwrap().unwrap();
            assert_eq!(unchanged.metadata_version, moved[index].metadata_version);
            assert!(unchanged
                .source_path
                .as_deref()
                .unwrap()
                .ends_with(if index == 0 {
                    "moved-a.stl"
                } else {
                    "moved-b.stl"
                }));
        }
        let _ = fs::remove_dir_all(source_root_path);
        let _ = fs::remove_dir_all(destination_root_path);
    }

    #[test]
    fn paged_catalog_filters_are_applied_before_keyset_limits() {
        let db = LibraryDb::open_in_memory().unwrap();
        let a = db.create_model("Alpha", None).unwrap();
        let b = db.create_model("Beta", None).unwrap();
        let c = db.create_model("Gamma", None).unwrap();
        let tag = db.create_tag("Mechanical", Some("Type")).unwrap();
        let a = db
            .update_model_metadata(
                &a.id,
                a.metadata_version,
                &MetadataPatch {
                    designer: Some("Designer".into()),
                    model_license: Some("CC0".into()),
                    print_status: Some(PrintStatus::Printed),
                    tags: Some(vec![tag.id.clone()]),
                    tag_mode: Some(TagMode::Replace),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        let filament = db
            .create_filament(&FilamentInput {
                brand: "Example".into(),
                material: "PLA".into(),
                color: "#123456".into(),
                name: "Catalog filter".into(),
                notes: String::new(),
            })
            .unwrap();
        db.set_model_filaments(
            &a.id,
            a.metadata_version,
            std::slice::from_ref(&filament.id),
        )
        .unwrap();
        db.create_print_attempt(
            &a.id,
            &PrintAttemptInput {
                printed_at: "2026-10-11T12:00:00Z".into(),
                quantity: 1,
                notes: String::new(),
                filament_ids: vec![filament.id.clone()],
                outcome: PrintOutcome::Printed,
            },
        )
        .unwrap();
        let bundle_id = db.create_bundle(&[b.id.clone(), c.id.clone()]).unwrap();

        for query in [
            ModelPageQuery {
                category: Some("Type".into()),
                ..ModelPageQuery::default()
            },
            ModelPageQuery {
                designer: Some("Designer".into()),
                ..ModelPageQuery::default()
            },
            ModelPageQuery {
                model_license: Some("CC0".into()),
                ..ModelPageQuery::default()
            },
            ModelPageQuery {
                ever_printed: Some(true),
                ..ModelPageQuery::default()
            },
            ModelPageQuery {
                filament_id: Some(filament.id.clone()),
                ..ModelPageQuery::default()
            },
            ModelPageQuery {
                group: Some(ModelGroupFilter::Standalone),
                ..ModelPageQuery::default()
            },
        ] {
            let (models, more) = db.list_models_page(&query, None, None, 1).unwrap();
            assert!(!more);
            assert_eq!(models[0].id, a.id);
        }
        let (bundled, more) = db
            .list_models_page(
                &ModelPageQuery {
                    bundle_id: Some(bundle_id),
                    sort: ModelSort::Name,
                    direction: SortDirection::Desc,
                    ..ModelPageQuery::default()
                },
                None,
                None,
                1,
            )
            .unwrap();
        assert!(more);
        assert_eq!(bundled[0].display_name, "Gamma");
        let stats = db.catalog_stats().unwrap();
        assert_eq!(stats.total_models, 3);
        assert_eq!(stats.available_models, 3);
        assert_eq!(stats.bundle_count, 1);
        let facets = db.catalog_facets().unwrap();
        assert_eq!(facets.designers, ["Designer"]);
        assert_eq!(facets.licenses, ["CC0"]);
        assert_eq!(facets.categories, ["Type"]);
        assert_eq!(facets.parents.len(), 3);
        assert!(facets.folder_ids.iter().any(|id| id == ROOT_FOLDER_ID));
    }

    #[test]
    fn reviewed_ai_tags_apply_atomically_with_category_aware_merge() {
        use crate::models::ReviewedTag;

        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("AI review", None).unwrap();
        let color = db.create_tag("Red", Some("Color")).unwrap();
        let style = db.create_tag("Cute", Some("Style")).unwrap();
        let model = db
            .update_model_metadata(
                &model.id,
                model.metadata_version,
                &MetadataPatch {
                    tags: Some(vec![color.id, style.id]),
                    tag_mode: Some(TagMode::Replace),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        let merged = db
            .apply_reviewed_tags(
                &[ReviewedTagTarget {
                    model_id: model.id.clone(),
                    expected_version: model.metadata_version,
                    tags: vec![
                        ReviewedTag {
                            name: "Blue".into(),
                            category: Some("Color".into()),
                        },
                        ReviewedTag {
                            name: "Desk toy".into(),
                            category: None,
                        },
                    ],
                }],
                ReviewedTagStrategy::Merge,
            )
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(merged.tags, ["Blue", "Cute", "Desk toy"]);
        assert!(!merged.tags.contains(&"Red".into()));

        let other = db.create_model("Stale", None).unwrap();
        let changed = db
            .update_model_metadata(
                &other.id,
                other.metadata_version,
                &MetadataPatch {
                    notes: Some("changed".into()),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        assert!(matches!(
            db.apply_reviewed_tags(
                &[
                    ReviewedTagTarget {
                        model_id: merged.id.clone(),
                        expected_version: merged.metadata_version,
                        tags: vec![ReviewedTag {
                            name: "Must roll back".into(),
                            category: None,
                        }],
                    },
                    ReviewedTagTarget {
                        model_id: other.id.clone(),
                        expected_version: other.metadata_version,
                        tags: vec![],
                    },
                ],
                ReviewedTagStrategy::Append,
            ),
            Err(LibraryError::MetadataConflict { .. })
        ));
        assert!(!db
            .list_tags()
            .unwrap()
            .iter()
            .any(|tag| tag.name == "Must roll back"));
        assert_eq!(db.get_model(&merged.id).unwrap().unwrap().tags, merged.tags);
        assert!(changed.metadata_version > other.metadata_version);

        assert!(db
            .apply_reviewed_tags(
                &[ReviewedTagTarget {
                    model_id: merged.id.clone(),
                    expected_version: merged.metadata_version,
                    tags: vec![
                        ReviewedTag {
                            name: "Category rollback".into(),
                            category: None,
                        },
                        ReviewedTag {
                            name: "Cute".into(),
                            category: Some("Color".into()),
                        },
                    ],
                }],
                ReviewedTagStrategy::Append,
            )
            .is_err());
        assert!(!db
            .list_tags()
            .unwrap()
            .iter()
            .any(|tag| tag.name == "Category rollback"));
        assert_eq!(db.get_model(&merged.id).unwrap().unwrap().tags, merged.tags);
        let cleared = db
            .apply_reviewed_tags(
                &[ReviewedTagTarget {
                    model_id: merged.id,
                    expected_version: merged.metadata_version,
                    tags: vec![],
                }],
                ReviewedTagStrategy::Replace,
            )
            .unwrap();
        assert!(cleared[0].tags.is_empty());
    }

    #[test]
    fn print_timestamps_use_strict_rfc3339_and_fixed_utc_milliseconds() {
        assert_eq!(
            normalize_rfc3339("2024-02-29T23:59:59.123987+02:30").unwrap(),
            "2024-02-29T21:29:59.123Z"
        );
        for invalid in [
            "2023-02-29T00:00:00Z",
            "2026-01-01T-1:00:00Z",
            "2026-01-01T00:-1:00Z",
            "💥2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00+25:00",
        ] {
            assert!(normalize_rfc3339(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn archive_members_are_distinct_and_auto_grouped_in_a_stable_bundle() {
        let library = temp_root("archive-library");
        let sources = temp_root("archive-source");
        fs::create_dir_all(&sources).unwrap();
        fs::write(sources.join("parts.zip"), b"archive").unwrap();
        let db = LibraryDb::open(&library).unwrap();
        let mut root = db.add_index_root(&sources, 60, 1024).unwrap();
        root.scan_zip = true;
        db.update_index_root(&root).unwrap();
        let entries = [
            ScannedModel {
                relative_path: "parts.zip".into(),
                content_hash: hash(1),
                bytes: 10,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("A".into()),
                archive_member: Some("a.stl".into()),
                outer_fingerprint: Some("fp1".into()),
            },
            ScannedModel {
                relative_path: "parts.zip".into(),
                content_hash: hash(1),
                bytes: 10,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("B".into()),
                archive_member: Some("b.stl".into()),
                outer_fingerprint: Some("fp1".into()),
            },
        ];
        let added = db.record_scan_entries(&root.id, &entries).unwrap();
        assert_eq!(added.added, 2);
        let seen = vec![
            SourceRef {
                relative_path: "parts.zip".into(),
                archive_member: Some("a.stl".into()),
            },
            SourceRef {
                relative_path: "parts.zip".into(),
                archive_member: Some("b.stl".into()),
            },
        ];
        db.complete_scan_sources_with_new_models(&root.id, &seen, &added.added_model_ids, "1")
            .unwrap();
        let models = db.list_models(None, None).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].bundle_id, models[1].bundle_id);
        assert_ne!(models[0].id, models[1].id);
        let resolved = db.resolve_model_source_ref(&models[0].id).unwrap();
        assert_eq!(
            resolved.outer_path,
            sources.join("parts.zip").canonicalize().unwrap()
        );
        assert_eq!(resolved.outer_fingerprint.as_deref(), Some("fp1"));
        assert!(resolved.archive_member.is_some());
        let again = db.record_scan_entries(&root.id, &entries).unwrap();
        assert_eq!(again.added, 0);
        assert_eq!(
            db.list_models(None, None).unwrap()[0].bundle_id,
            models[0].bundle_id
        );
        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn archive_outer_rename_updates_automatic_bundle_path_without_changing_ids() {
        let library = temp_root("archive-rename-library");
        let sources = temp_root("archive-rename-source");
        fs::create_dir_all(&sources).unwrap();
        fs::write(sources.join("pack.zip"), b"first").unwrap();
        let db = LibraryDb::open(&library).unwrap();
        let root = db.add_index_root(&sources, 60, 1024).unwrap();
        let first = ScannedModel {
            relative_path: "pack.zip".into(),
            content_hash: hash(1),
            bytes: 10,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: Some("Part".into()),
            archive_member: Some("part.stl".into()),
            outer_fingerprint: Some("fp1".into()),
        };
        let added = db
            .record_scan_entries(&root.id, std::slice::from_ref(&first))
            .unwrap();
        db.complete_scan_sources_with_new_models(
            &root.id,
            &[SourceRef {
                relative_path: "pack.zip".into(),
                archive_member: Some("part.stl".into()),
            }],
            &added.added_model_ids,
            "1",
        )
        .unwrap();
        let original = db.list_models(None, None).unwrap().pop().unwrap();
        let bundle = original.bundle_id.clone().unwrap();
        fs::rename(sources.join("pack.zip"), sources.join("renamed.zip")).unwrap();
        let renamed = ScannedModel {
            relative_path: "renamed.zip".into(),
            outer_fingerprint: Some("fp2".into()),
            modified_at: 2,
            ..first
        };
        let batch = db
            .record_scan_entries(&root.id, std::slice::from_ref(&renamed))
            .unwrap();
        db.complete_scan_sources_with_new_models(
            &root.id,
            &[SourceRef {
                relative_path: "renamed.zip".into(),
                archive_member: Some("part.stl".into()),
            }],
            &batch.added_model_ids,
            "2",
        )
        .unwrap();
        let current = db.get_model(&original.id).unwrap().unwrap();
        assert_eq!(current.id, original.id);
        assert_eq!(current.bundle_id.as_deref(), Some(bundle.as_str()));
        let bundle_path: String = db
            .conn
            .query_row(
                "SELECT source_path FROM bundles WHERE id=?1",
                params![bundle],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bundle_path, "renamed.zip");
        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn scoped_scan_matches_and_updates_absolute_legacy_source_paths() {
        let library = temp_root("absolute-legacy-scan-library");
        let sources = temp_root("absolute-legacy-scan-source");
        fs::create_dir_all(sources.join("parts")).unwrap();
        fs::write(sources.join("parts/loose.stl"), b"old loose").unwrap();
        fs::write(sources.join("parts/pack.zip"), b"old archive").unwrap();
        let db = LibraryDb::open(&library).unwrap();
        let root = db.add_index_root(&sources, 60, 1024).unwrap();
        let initial = [
            ScannedModel {
                relative_path: "parts/loose.stl".into(),
                content_hash: hash(1),
                bytes: 9,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("Loose".into()),
                archive_member: None,
                outer_fingerprint: None,
            },
            ScannedModel {
                relative_path: "parts/pack.zip".into(),
                content_hash: hash(2),
                bytes: 11,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("Archive member".into()),
                archive_member: Some("member.stl".into()),
                outer_fingerprint: Some("archive-v1".into()),
            },
        ];
        db.record_scan_entries(&root.id, &initial).unwrap();
        let models = db.list_models(None, None).unwrap();
        let loose_id = models
            .iter()
            .find(|model| model.source_archive_member.is_none())
            .unwrap()
            .id
            .clone();
        let archive_id = models
            .iter()
            .find(|model| model.source_archive_member.as_deref() == Some("member.stl"))
            .unwrap()
            .id
            .clone();
        let absolute_loose = root.path.join("parts/loose.stl");
        let absolute_archive = root.path.join("parts/pack.zip");
        db.conn
            .execute(
                "UPDATE models SET source_path=?1 WHERE id=?2",
                params![absolute_loose.to_string_lossy(), loose_id],
            )
            .unwrap();
        db.conn
            .execute(
                "UPDATE models SET source_path=?1 WHERE id=?2",
                params![absolute_archive.to_string_lossy(), archive_id],
            )
            .unwrap();

        assert_eq!(
            db.resolve_model_source_ref(&loose_id).unwrap().outer_path,
            absolute_loose.canonicalize().unwrap()
        );
        let archive_source = db.resolve_model_source_ref(&archive_id).unwrap();
        assert_eq!(
            archive_source.outer_path,
            absolute_archive.canonicalize().unwrap()
        );
        assert_eq!(archive_source.archive_member.as_deref(), Some("member.stl"));
        assert_eq!(
            db.indexed_source_metadata(&root.id, Path::new("parts/loose.stl"), None)
                .unwrap()
                .unwrap()
                .content_hash,
            hash(1)
        );
        assert_eq!(
            db.indexed_archive_metadata(&root.id, Path::new("parts/pack.zip"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            db.indexed_archive_source_refs(&root.id, Some(Path::new("parts")))
                .unwrap(),
            vec![SourceRef {
                relative_path: "parts/pack.zip".into(),
                archive_member: Some("member.stl".into()),
            }]
        );

        let seen = [
            SourceRef {
                relative_path: "parts/loose.stl".into(),
                archive_member: None,
            },
            SourceRef {
                relative_path: "parts/pack.zip".into(),
                archive_member: Some("member.stl".into()),
            },
        ];
        db.complete_subdirectory_scan_sources_with_new_models(
            &root.id,
            Path::new("parts"),
            &seen,
            &[],
        )
        .unwrap();
        for id in [&loose_id, &archive_id] {
            assert!(!db.get_model(id).unwrap().unwrap().source_missing);
        }
        assert_eq!(
            db.get_model(&loose_id)
                .unwrap()
                .unwrap()
                .source_path
                .as_deref(),
            absolute_loose.to_str()
        );

        fs::write(sources.join("parts/loose.stl"), b"new loose bytes").unwrap();
        fs::write(sources.join("parts/pack.zip"), b"new archive bytes").unwrap();
        let changed = [
            ScannedModel {
                content_hash: hash(3),
                bytes: 15,
                modified_at: 2,
                ..initial[0].clone()
            },
            ScannedModel {
                content_hash: hash(4),
                bytes: 16,
                modified_at: 2,
                outer_fingerprint: Some("archive-v2".into()),
                ..initial[1].clone()
            },
        ];
        let updated = db.record_scan_entries(&root.id, &changed).unwrap();
        assert_eq!(updated.added, 0);
        assert_eq!(updated.updated, 2);
        assert!(updated.added_model_ids.is_empty());
        let stored_paths = db
            .conn
            .prepare("SELECT source_path FROM models ORDER BY source_archive_member IS NOT NULL")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(stored_paths, ["parts/loose.stl", "parts/pack.zip"]);
        let current = db.list_models(None, None).unwrap();
        assert_eq!(current.len(), 2);
        assert!(current.iter().any(|model| {
            model.id == loose_id
                && model.source_path.as_deref() == absolute_loose.to_str()
                && model.content_hash.as_deref() == Some(hash(3).as_str())
                && !model.source_missing
        }));
        assert!(current.iter().any(|model| {
            model.id == archive_id
                && model.source_path.as_deref() == absolute_archive.to_str()
                && model.content_hash.as_deref() == Some(hash(4).as_str())
                && !model.source_missing
        }));

        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn moving_one_archive_member_splits_automatic_bundle_without_rewriting_remaining_source() {
        let library = temp_root("archive-split-library");
        let sources = temp_root("archive-split-source");
        fs::create_dir_all(&sources).unwrap();
        fs::write(sources.join("old.zip"), b"old").unwrap();
        fs::write(sources.join("new.zip"), b"new").unwrap();
        let db = LibraryDb::open(&library).unwrap();
        let root = db.add_index_root(&sources, 60, 1024).unwrap();
        let old = [
            ScannedModel {
                relative_path: "old.zip".into(),
                content_hash: hash(1),
                bytes: 84,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("A".into()),
                archive_member: Some("a.stl".into()),
                outer_fingerprint: Some("oldfp".into()),
            },
            ScannedModel {
                relative_path: "old.zip".into(),
                content_hash: hash(2),
                bytes: 84,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("B".into()),
                archive_member: Some("b.stl".into()),
                outer_fingerprint: Some("oldfp".into()),
            },
        ];
        let added = db.record_scan_entries(&root.id, &old).unwrap();
        let initial_seen = old
            .iter()
            .map(|entry| SourceRef {
                relative_path: entry.relative_path.clone(),
                archive_member: entry.archive_member.clone(),
            })
            .collect::<Vec<_>>();
        db.complete_scan_sources_with_new_models(
            &root.id,
            &initial_seen,
            &added.added_model_ids,
            "1",
        )
        .unwrap();
        let before = db.list_models(None, None).unwrap();
        let a = before
            .iter()
            .find(|m| m.source_archive_member.as_deref() == Some("a.stl"))
            .unwrap();
        let b = before
            .iter()
            .find(|m| m.source_archive_member.as_deref() == Some("b.stl"))
            .unwrap();
        let a_id = a.id.clone();
        let b_id = b.id.clone();
        let old_bundle = b.bundle_id.clone().unwrap();
        let moved = ScannedModel {
            relative_path: "new.zip".into(),
            content_hash: hash(1),
            bytes: 84,
            format: "stl-binary".into(),
            modified_at: 2,
            display_name: Some("A".into()),
            archive_member: Some("a.stl".into()),
            outer_fingerprint: Some("newfp".into()),
        };
        let batch = db
            .record_scan_entries(&root.id, std::slice::from_ref(&moved))
            .unwrap();
        let seen = [
            SourceRef {
                relative_path: "new.zip".into(),
                archive_member: Some("a.stl".into()),
            },
            SourceRef {
                relative_path: "old.zip".into(),
                archive_member: Some("b.stl".into()),
            },
        ];
        db.complete_scan_sources_with_new_models(&root.id, &seen, &batch.added_model_ids, "2")
            .unwrap();
        let a = db.get_model(&a_id).unwrap().unwrap();
        let b = db.get_model(&b_id).unwrap().unwrap();
        assert_ne!(a.bundle_id, b.bundle_id);
        assert_eq!(b.bundle_id.as_deref(), Some(old_bundle.as_str()));
        let old_path: String = db
            .conn
            .query_row(
                "SELECT source_path FROM bundles WHERE id=?1",
                params![old_bundle],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_path, "old.zip");
        let new_path: String = db
            .conn
            .query_row(
                "SELECT source_path FROM bundles WHERE id=?1",
                params![a.bundle_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(new_path, "new.zip");
        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn archive_member_to_loose_transition_does_not_reuse_archive_bundle() {
        let library = temp_root("archive-loose-library");
        let sources = temp_root("archive-loose-source");
        fs::create_dir_all(sources.join("folder")).unwrap();
        fs::write(sources.join("old.zip"), b"old").unwrap();
        fs::write(sources.join("folder/a.stl"), b"a").unwrap();
        let db = LibraryDb::open(&library).unwrap();
        let root = db.add_index_root(&sources, 60, 1024).unwrap();
        let old = [
            ScannedModel {
                relative_path: "old.zip".into(),
                content_hash: hash(1),
                bytes: 1,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: None,
                archive_member: Some("a.stl".into()),
                outer_fingerprint: Some("fp".into()),
            },
            ScannedModel {
                relative_path: "old.zip".into(),
                content_hash: hash(2),
                bytes: 1,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: None,
                archive_member: Some("b.stl".into()),
                outer_fingerprint: Some("fp".into()),
            },
        ];
        let added = db.record_scan_entries(&root.id, &old).unwrap();
        let seen = old
            .iter()
            .map(|e| SourceRef {
                relative_path: e.relative_path.clone(),
                archive_member: e.archive_member.clone(),
            })
            .collect::<Vec<_>>();
        db.complete_scan_sources_with_new_models(&root.id, &seen, &added.added_model_ids, "1")
            .unwrap();
        let models = db.list_models(None, None).unwrap();
        let a = models
            .iter()
            .find(|m| m.source_archive_member.as_deref() == Some("a.stl"))
            .unwrap();
        let b = models
            .iter()
            .find(|m| m.source_archive_member.as_deref() == Some("b.stl"))
            .unwrap();
        let a_id = a.id.clone();
        let b_id = b.id.clone();
        let archive_bundle = b.bundle_id.clone().unwrap();
        let loose = ScannedModel {
            relative_path: "folder/a.stl".into(),
            content_hash: hash(1),
            bytes: 1,
            format: "stl-binary".into(),
            modified_at: 2,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        let batch = db
            .record_scan_entries(&root.id, std::slice::from_ref(&loose))
            .unwrap();
        let seen = [
            SourceRef {
                relative_path: "folder/a.stl".into(),
                archive_member: None,
            },
            SourceRef {
                relative_path: "old.zip".into(),
                archive_member: Some("b.stl".into()),
            },
        ];
        db.complete_scan_sources_with_new_models(&root.id, &seen, &batch.added_model_ids, "2")
            .unwrap();
        let a = db.get_model(&a_id).unwrap().unwrap();
        let b = db.get_model(&b_id).unwrap().unwrap();
        assert!(a.source_archive_member.is_none());
        assert!(a.bundle_id.is_none());
        assert_eq!(b.bundle_id.as_deref(), Some(archive_bundle.as_str()));
        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn completed_scan_auto_groups_subfolders_with_stable_bundles_and_preserves_manual_assignments()
    {
        let library = temp_root("folder-bundle-library");
        let sources = temp_root("folder-bundle-source");
        fs::create_dir_all(sources.join("nested")).unwrap();
        fs::create_dir_all(sources.join("solo")).unwrap();
        for path in ["nested/a.stl", "nested/b.stl", "solo/c.stl", "root.stl"] {
            fs::write(sources.join(path), path.as_bytes()).unwrap();
        }
        let db = LibraryDb::open(&library).unwrap();
        let root = db.add_index_root(&sources, 60, 1024).unwrap();
        let make = |path: &str, n: u8| ScannedModel {
            relative_path: path.into(),
            content_hash: hash(n),
            bytes: path.len() as u64,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        let initial = [
            make("nested/a.stl", 1),
            make("nested/b.stl", 2),
            make("solo/c.stl", 3),
            make("root.stl", 4),
        ];
        let added = db.record_scan_entries(&root.id, &initial).unwrap();
        let seen = initial
            .iter()
            .map(|entry| SourceRef {
                relative_path: entry.relative_path.clone(),
                archive_member: None,
            })
            .collect::<Vec<_>>();
        db.complete_scan_sources_with_new_models(&root.id, &seen, &added.added_model_ids, "1")
            .unwrap();
        let models = db.list_models(None, None).unwrap();
        let a = models
            .iter()
            .find(|m| m.source_path.as_deref().unwrap().ends_with("nested/a.stl"))
            .unwrap();
        let b = models
            .iter()
            .find(|m| m.source_path.as_deref().unwrap().ends_with("nested/b.stl"))
            .unwrap();
        let stable_bundle = a.bundle_id.clone().unwrap();
        let a_id = a.id.clone();
        assert_eq!(b.bundle_id.as_deref(), Some(stable_bundle.as_str()));
        assert!(models
            .iter()
            .find(|m| m.source_path.as_deref().unwrap().ends_with("solo/c.stl"))
            .unwrap()
            .bundle_id
            .is_none());
        assert!(models
            .iter()
            .find(|m| m.source_path.as_deref().unwrap().ends_with("root.stl"))
            .unwrap()
            .bundle_id
            .is_none());
        fs::rename(sources.join("nested"), sources.join("renamed")).unwrap();
        let renamed = [
            make("renamed/a.stl", 1),
            make("renamed/b.stl", 2),
            make("solo/c.stl", 3),
            make("root.stl", 4),
        ];
        let batch = db.record_scan_entries(&root.id, &renamed).unwrap();
        let seen = renamed
            .iter()
            .map(|entry| SourceRef {
                relative_path: entry.relative_path.clone(),
                archive_member: None,
            })
            .collect::<Vec<_>>();
        db.complete_scan_sources_with_new_models(&root.id, &seen, &batch.added_model_ids, "2")
            .unwrap();
        let models = db.list_models(None, None).unwrap();
        let renamed_a = models
            .iter()
            .find(|m| m.source_path.as_deref().unwrap().ends_with("renamed/a.stl"))
            .unwrap();
        assert_eq!(renamed_a.id, a_id);
        assert_eq!(renamed_a.bundle_id.as_deref(), Some(stable_bundle.as_str()));
        assert!(db
            .set_model_bundle(&renamed_a.id, Some(&stable_bundle))
            .is_err());
        let manual = db
            .create_bundle(std::slice::from_ref(&renamed_a.id))
            .unwrap();
        db.complete_scan_sources_with_new_models(&root.id, &seen, &[], "3")
            .unwrap();
        let models = db.list_models(None, None).unwrap();
        assert_eq!(
            models
                .iter()
                .find(|m| m.id == renamed_a.id)
                .unwrap()
                .bundle_id
                .as_deref(),
            Some(manual.as_str())
        );
        assert!(models
            .iter()
            .find(|m| m.source_path.as_deref().unwrap().ends_with("renamed/b.stl"))
            .unwrap()
            .bundle_id
            .is_none());
        drop(db);
        let reopened = LibraryDb::open(&library).unwrap();
        assert_eq!(
            reopened
                .get_model(&a_id)
                .unwrap()
                .unwrap()
                .bundle_id
                .as_deref(),
            Some(manual.as_str())
        );
        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn tag_and_filament_inputs_are_bounded_at_the_storage_boundary() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Bounded", None).unwrap();
        for name in [
            "x".repeat(257),
            "line\\nfeed".replace("\\n", "\n"),
            "null\0".to_owned(),
        ] {
            assert!(db.create_tag(&name, None).is_err());
        }
        let tag = db.create_tag("Valid", None).unwrap();
        assert!(db.rename_tag(&tag.id, &"x".repeat(257), None).is_err());
        assert_eq!(db.list_tags().unwrap()[0].name, "Valid");
        for ids in [
            (0..6).map(|n| format!("filament-{n}")).collect::<Vec<_>>(),
            vec!["x".repeat(257)],
            vec!["".into()],
            vec!["bad\nref".into()],
            vec![" padded ".into()],
            vec!["duplicate".into(), "duplicate".into()],
        ] {
            let input = PrintAttemptInput {
                printed_at: "2026-10-10T00:00:00.000Z".into(),
                quantity: 1,
                notes: String::new(),
                filament_ids: ids,
                outcome: PrintOutcome::Printed,
            };
            assert!(db.create_print_attempt(&model.id, &input).is_err());
            assert!(db.list_print_attempts(&model.id).unwrap().is_empty());
            assert_eq!(
                db.get_model(&model.id)
                    .unwrap()
                    .unwrap()
                    .metadata
                    .print_count,
                0
            );
        }
    }

    #[test]
    fn print_history_persists_across_restart() {
        let root = temp_root("history-restart");
        let db = LibraryDb::open(&root).unwrap();
        let model = db.create_model("History", None).unwrap();
        let attempt = db
            .create_print_attempt(
                &model.id,
                &PrintAttemptInput {
                    printed_at: "2026-10-10T12:34:56.789Z".into(),
                    quantity: 3,
                    notes: "persist".into(),
                    filament_ids: vec!["f1".into(), "f2".into()],
                    outcome: PrintOutcome::Printed,
                },
            )
            .unwrap();
        drop(db);
        let reopened = LibraryDb::open(&root).unwrap();
        let loaded = reopened.get_print_attempt(&attempt.id).unwrap().unwrap();
        assert_eq!(loaded.quantity, 3);
        assert_eq!(loaded.filament_ids, vec!["f1", "f2"]);
        assert_eq!(
            reopened
                .get_model(&model.id)
                .unwrap()
                .unwrap()
                .metadata
                .print_count,
            3
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn print_attempt_batches_are_atomic_and_idempotent_without_storing_payloads() {
        let db = LibraryDb::open_in_memory().unwrap();
        let first = db.create_model("First", None).unwrap();
        let second = db.create_model("Second", None).unwrap();
        let input = PrintAttemptInput {
            printed_at: "2026-10-11T12:00:00Z".into(),
            quantity: 2,
            notes: "private batch note".into(),
            filament_ids: vec![],
            outcome: PrintOutcome::Printed,
        };
        let invalid = vec![
            PrintAttemptBatchItem {
                model_id: first.id.clone(),
                input: input.clone(),
            },
            PrintAttemptBatchItem {
                model_id: "missing".into(),
                input: input.clone(),
            },
        ];
        assert!(db
            .create_print_attempts_idempotent("batch-invalid", &invalid)
            .is_err());
        assert!(db.list_print_attempts(&first.id).unwrap().is_empty());

        let items = vec![
            PrintAttemptBatchItem {
                model_id: first.id.clone(),
                input: input.clone(),
            },
            PrintAttemptBatchItem {
                model_id: second.id.clone(),
                input,
            },
        ];
        let created = db
            .create_print_attempts_idempotent("batch-1", &items)
            .unwrap();
        let replayed = db
            .create_print_attempts_idempotent("batch-1", &items)
            .unwrap();
        assert_eq!(created, replayed);
        assert_eq!(db.list_print_attempts(&first.id).unwrap().len(), 1);
        assert_eq!(db.list_print_attempts(&second.id).unwrap().len(), 1);
        let mut changed = items;
        changed[0].input.quantity = 3;
        assert!(db
            .create_print_attempts_idempotent("batch-1", &changed)
            .is_err());
        let receipt: String = db
            .conn
            .query_row(
                "SELECT attempt_ids_json FROM print_batch_receipts WHERE idempotency_key='batch-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!receipt.contains("private batch note"));
    }

    #[test]
    fn management_duplicates_removal_restore_and_custom_previews_are_scoped_and_versioned() {
        let root = temp_root("management-core");
        let sources = temp_root("management-core-source");
        fs::create_dir_all(&sources).unwrap();
        let source = sources.join("part.stl");
        fs::write(&source, stl_fixture()).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let first = db.import_file(&source, None, Some("First")).unwrap().model;
        let second = db.import_file(&source, None, Some("Second")).unwrap().model;
        assert!(db
            .duplicate_groups(std::slice::from_ref(&first.id))
            .unwrap()
            .is_empty());
        let groups = db
            .duplicate_groups(&[first.id.clone(), second.id.clone()])
            .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].models.len(), 2);
        let managed = db.resolve_model_source(&first.id).unwrap();
        let before = fs::read(&managed).unwrap();
        let removal = db
            .remove_models_recoverably(&[VersionedTarget {
                model_id: first.id.clone(),
                expected_version: first.metadata_version,
            }])
            .unwrap();
        assert_eq!(db.list_models(None, None).unwrap().len(), 1);
        assert_eq!(fs::read(&managed).unwrap(), before);
        assert_eq!(db.list_removed_models().unwrap().len(), 1);
        let restored = db.restore_removal(&removal.snapshot_id).unwrap();
        assert_eq!(restored[0].id, first.id);
        assert_eq!(db.list_models(None, None).unwrap().len(), 2);
        let png = png_fixture();
        let hash = hex_digest(&png);
        let relative = Path::new("thumbnails/custom-management.png");
        fs::write(db.root().join(relative), png).unwrap();
        let current = db.get_model(&first.id).unwrap().unwrap();
        let custom = db
            .set_custom_thumbnail(&first.id, current.metadata_version, &hash, relative)
            .unwrap();
        assert!(
            db.thumbnail_cache_entry_for_model(&first.id)
                .unwrap()
                .unwrap()
                .is_custom
        );
        assert!(matches!(
            db.clear_custom_thumbnail(&first.id, current.metadata_version),
            Err(LibraryError::MetadataConflict { .. })
        ));
        let cleared = db
            .clear_custom_thumbnail(&first.id, custom.metadata_version)
            .unwrap();
        assert!(!db
            .thumbnail_cache_entry_for_model(&first.id)
            .unwrap()
            .is_some_and(|entry| entry.is_custom));
        assert!(cleared.metadata_version > custom.metadata_version);
        let floor = db.max_metadata_version().unwrap();
        db.bump_all_metadata_versions_above(floor).unwrap();
        assert!(db.get_model(&first.id).unwrap().unwrap().metadata_version > floor);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn duplicate_scope_limit_is_checked_before_model_lookups() {
        let root = temp_root("duplicate-scope-limit");
        let db = LibraryDb::open(&root).unwrap();
        let ids = (0..1_001)
            .map(|index| format!("model-{index}"))
            .collect::<Vec<_>>();

        assert!(matches!(
            db.duplicate_groups(&ids),
            Err(LibraryError::InvalidInput(_))
        ));
    }

    #[test]
    fn indexed_tombstones_survive_rescan_and_zip_member_removal_never_changes_archive() {
        let library = temp_root("management-indexed");
        let sources = temp_root("management-indexed-source");
        fs::create_dir_all(&sources).unwrap();
        let archive = sources.join("pack.zip");
        let archive_bytes = b"outer archive unchanged";
        fs::write(&archive, archive_bytes).unwrap();
        let db = LibraryDb::open(&library).unwrap();
        let root = db.add_index_root(&sources, 60, 1024).unwrap();
        let entries = [
            ScannedModel {
                relative_path: "pack.zip".into(),
                content_hash: hash(1),
                bytes: 10,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("A".into()),
                archive_member: Some("a.stl".into()),
                outer_fingerprint: Some("fp".into()),
            },
            ScannedModel {
                relative_path: "pack.zip".into(),
                content_hash: hash(2),
                bytes: 10,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("B".into()),
                archive_member: Some("b.stl".into()),
                outer_fingerprint: Some("fp".into()),
            },
        ];
        let added = db.record_scan_entries(&root.id, &entries).unwrap();
        let seen = entries
            .iter()
            .map(|entry| SourceRef {
                relative_path: entry.relative_path.clone(),
                archive_member: entry.archive_member.clone(),
            })
            .collect::<Vec<_>>();
        db.complete_scan_sources_with_new_models(&root.id, &seen, &added.added_model_ids, "1")
            .unwrap();
        let models = db.list_models(None, None).unwrap();
        let removed = models
            .iter()
            .find(|model| model.source_archive_member.as_deref() == Some("a.stl"))
            .unwrap();
        let removal = db
            .remove_models_recoverably(&[VersionedTarget {
                model_id: removed.id.clone(),
                expected_version: removed.metadata_version,
            }])
            .unwrap();
        db.record_scan_entries(&root.id, &entries).unwrap();
        db.complete_scan_sources_with_new_models(&root.id, &seen, &[], "2")
            .unwrap();
        assert_eq!(db.list_models(None, None).unwrap().len(), 1);
        assert_eq!(fs::read(&archive).unwrap(), archive_bytes);
        let restored = db.restore_removal(&removal.snapshot_id).unwrap();
        assert_eq!(restored[0].id, removed.id);
        assert_eq!(db.list_models(None, None).unwrap().len(), 2);
        let _ = fs::remove_dir_all(library);
        let _ = fs::remove_dir_all(sources);
    }

    #[test]
    fn thumbnail_appearance_settings_invalidate_generated_rows_and_preserve_custom() {
        let root = temp_root("catalog-settings-thumbnails");
        let source_dir = temp_root("catalog-settings-thumbnails-source");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("part.stl");
        fs::write(&source, stl_fixture()).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        fs::write(root.join("thumbnails/generated.png"), b"generated").unwrap();
        fs::write(root.join("thumbnails/custom.png"), b"custom").unwrap();
        db.set_thumbnail(&model.id, &hash(9), Path::new("thumbnails/generated.png"))
            .unwrap();
        db.conn.execute("INSERT INTO thumbnails (id, model_id, relative_path, content_hash, is_custom) VALUES ('settings-custom', ?1, 'thumbnails/custom.png', 'custom-hash', 1)", params![model.id]).unwrap();

        let mut settings = db.get_catalog_settings().unwrap();
        settings.thumbnail_concurrency = 1;
        settings.scan_batch_size = 64;
        db.update_catalog_settings(&settings).unwrap();
        let generated_before_appearance_change: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE is_custom = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generated_before_appearance_change, 1);
        settings.thumbnail_size = 512;
        db.update_catalog_settings(&settings).unwrap();
        let generated: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE is_custom = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let custom: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE is_custom = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generated, 0);
        assert_eq!(custom, 1);
        assert!(db.thumbnail_for_model(&model.id).unwrap().is_some());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_dir);
    }

    #[test]
    fn thumbnail_commit_rejects_stale_appearance_settings_atomically() {
        let root = temp_root("thumbnail-settings-race");
        let source_dir = temp_root("thumbnail-settings-race-source");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("part.stl");
        fs::write(&source, stl_fixture()).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        let png = png_fixture();
        let png_hash = hex_digest(&png);
        let relative = Path::new("thumbnails/settings-race.png");
        fs::write(db.root().join(relative), png).unwrap();
        let stale = db.get_catalog_settings().unwrap();
        let mut current = stale.clone();
        current.thumbnail_background = "#223344".into();
        db.update_catalog_settings(&current).unwrap();

        assert!(matches!(
            db.set_thumbnail_for_source_and_settings(
                &model.id,
                model.content_hash.as_deref().unwrap(),
                &stale,
                &png_hash,
                relative,
            ),
            Err(LibraryError::InvalidInput(_))
        ));
        let generated: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE is_custom = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generated, 0);
        db.set_thumbnail_for_source_and_settings(
            &model.id,
            model.content_hash.as_deref().unwrap(),
            &current,
            &png_hash,
            relative,
        )
        .unwrap();
        let first_id: String = db
            .conn
            .query_row(
                "SELECT id FROM thumbnails WHERE model_id=?1 AND is_custom=0",
                [&model.id],
                |row| row.get(0),
            )
            .unwrap();
        db.set_thumbnail_for_source_and_settings(
            &model.id,
            model.content_hash.as_deref().unwrap(),
            &current,
            &png_hash,
            relative,
        )
        .unwrap();
        let repeated_id: String = db
            .conn
            .query_row(
                "SELECT id FROM thumbnails WHERE model_id=?1 AND is_custom=0",
                [&model.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(repeated_id, first_id);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_dir);
    }

    #[test]
    fn schema_v1_migration_creates_consistent_backup_and_preserves_managed_data() {
        let root = temp_root("existing-v1");
        fs::create_dir_all(root.join("originals")).unwrap();
        fs::create_dir_all(root.join("thumbnails")).unwrap();
        let source_bytes = stl_fixture();
        let content_hash = hex_digest(&source_bytes);
        let storage_path = format!("originals/{content_hash}.stl");
        fs::write(root.join(&storage_path), &source_bytes).unwrap();
        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute_batch(TEST_SCHEMA_V1).unwrap();
        conn.execute(
            "INSERT INTO blobs VALUES (?1, ?2, ?3)",
            params![content_hash, storage_path, source_bytes.len() as i64],
        )
        .unwrap();
        conn.execute("INSERT INTO models (id, display_name, description, folder_id, current_revision_id, metadata_version, created_at, updated_at) VALUES ('model-v1', 'Tagged Bracket', 'legacy notes', 'root', 'revision-v1', 1, '1', '1')", []).unwrap();
        conn.execute("INSERT INTO revisions VALUES ('revision-v1', 'model-v1', ?1, 'Tagged Bracket.stl', 'stl-binary', ?2, '1')", params![content_hash, source_bytes.len() as i64]).unwrap();
        conn.execute("INSERT INTO tags VALUES ('tag-existing', 'Favorite')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO model_tags VALUES ('model-v1', 'tag-existing')",
            [],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        drop(conn);

        let db = LibraryDb::open(&root).unwrap();
        let existing = db.get_model("model-v1").unwrap().unwrap();
        assert_eq!(existing.id, "model-v1");
        assert_eq!(existing.display_name, "Tagged Bracket");
        assert_eq!(existing.folder_id, ROOT_FOLDER_ID);
        assert_eq!(
            existing.original_name.as_deref(),
            Some("Tagged Bracket.stl")
        );
        assert_eq!(
            existing.content_hash.as_deref(),
            Some(content_hash.as_str())
        );
        assert_eq!(
            existing.storage_path.as_deref(),
            Some(storage_path.as_str())
        );
        assert_eq!(existing.tags, vec!["Favorite"]);
        assert_eq!(existing.source_kind, "managed");
        assert_eq!(existing.metadata.notes.as_deref(), Some("legacy notes"));
        assert_eq!(fs::read(root.join(storage_path)).unwrap(), source_bytes);
        drop(db);

        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        drop(conn);

        let backup = root.join("library.sqlite3.schema-v1.backup");
        assert!(backup.is_file());
        let backup_conn = Connection::open(backup).unwrap();
        let backup_version: i64 = backup_conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(backup_version, 1);
        let backup_name: String = backup_conn
            .query_row(
                "SELECT display_name FROM models WHERE id = 'model-v1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(backup_name, "Tagged Bracket");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn schema_v2_migration_creates_backup_and_preserves_legacy_notes() {
        let root = temp_root("existing-v2");
        fs::create_dir_all(&root).unwrap();
        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute_batch(TEST_SCHEMA_V1).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute("INSERT INTO models (id,display_name,description,folder_id,metadata_version,created_at,updated_at) VALUES ('legacy','Legacy','keep me','root',1,'1','1')",[]).unwrap();
        conn.execute_batch(MIGRATE_V1_TO_V2).unwrap();
        drop(conn);
        let db = LibraryDb::open(&root).unwrap();
        let model = db.get_model("legacy").unwrap().unwrap();
        assert_eq!(model.metadata.notes.as_deref(), Some("keep me"));
        drop(db);
        let backup = root.join("library.sqlite3.schema-v2.backup");
        assert!(backup.is_file());
        let backup_conn = Connection::open(backup).unwrap();
        let version: i64 = backup_conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn newer_schema_is_rejected_without_migration() {
        let root = temp_root("schema");
        let db = LibraryDb::open(&root).unwrap();
        drop(db);
        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION + 1)
            .unwrap();
        drop(conn);
        let result = LibraryDb::open(&root);
        assert!(matches!(result, Err(LibraryError::SchemaTooNew { .. })));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn import_copies_validated_bytes_and_never_overwrites_same_name() {
        let root = temp_root("import");
        let source_dir = temp_root("source");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("Bracket.stl");
        let bytes = stl_fixture();
        fs::write(&source, &bytes).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let first = db.import_file(&source, None, None).unwrap();
        let second = db.import_file(&source, None, None).unwrap();
        assert_ne!(first.model.id, second.model.id);
        assert_eq!(first.model.display_name, "Bracket");
        assert_eq!(first.model.original_name.as_deref(), Some("Bracket.stl"));
        assert_eq!(first.model.content_hash, second.model.content_hash);
        assert_eq!(first.model.bytes, bytes.len() as u64);
        assert!(first
            .model
            .storage_path
            .as_ref()
            .unwrap()
            .contains("originals/"));
        assert_eq!(fs::read(&source).unwrap(), bytes);
        assert!(!first.reused_blob);
        assert!(second.reused_blob);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_dir);
    }

    #[test]
    fn import_rejects_oversize_before_read_and_never_overwrites_or_follows_owned_namespace_links() {
        let root = temp_root("import-hardening");
        let sources = temp_root("import-hardening-source");
        fs::create_dir_all(&sources).unwrap();
        let oversized = sources.join("oversized.stl");
        let file = fs::File::create(&oversized).unwrap();
        file.set_len(MAX_INDEXED_FILE_BYTES + 1).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        assert!(db.import_file(&oversized, None, None).is_err());
        assert!(db.list_models(None, None).unwrap().is_empty());
        let source = sources.join("part.stl");
        let bytes = stl_fixture();
        fs::write(&source, &bytes).unwrap();
        let hash = hex_digest(&bytes);
        let destination = db.root().join("originals").join(format!("{hash}.stl"));
        let sentinel = vec![b'x'; bytes.len()];
        fs::write(&destination, &sentinel).unwrap();
        assert!(db.import_file(&source, None, None).is_err());
        assert_eq!(fs::read(&destination).unwrap(), sentinel);
        assert!(db.list_models(None, None).unwrap().is_empty());
        drop(db);
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = temp_root("import-hardening-outside");
            fs::create_dir_all(&outside).unwrap();
            fs::remove_file(&destination).unwrap();
            fs::remove_dir(root.join("originals")).unwrap();
            symlink(&outside, root.join("originals")).unwrap();
            let reopened = LibraryDb::open(&root);
            assert!(reopened.is_err());
            assert!(outside.read_dir().unwrap().next().is_none());
            let _ = fs::remove_file(root.join("originals"));
            let _ = fs::remove_dir_all(outside);
        }
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(sources);
    }

    #[cfg(unix)]
    #[test]
    fn library_open_rejects_symlinked_database_file() {
        use std::os::unix::fs::symlink;
        let root = temp_root("db-symlink");
        let outside = temp_root("db-symlink-outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("database"), b"not sqlite").unwrap();
        symlink(outside.join("database"), root.join("library.sqlite3")).unwrap();
        assert!(LibraryDb::open(&root).is_err());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn indexed_scan_is_in_place_and_reconciles_change_rename_missing_and_duplicates() {
        let library_root = temp_root("indexed-library");
        let source_root = temp_root("indexed-source");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(source_root.join("unique.stl"), b"unique-v1").unwrap();
        fs::write(source_root.join("duplicate-a.stl"), b"duplicate").unwrap();
        fs::write(source_root.join("duplicate-b.stl"), b"duplicate").unwrap();

        let db = LibraryDb::open(&library_root).unwrap();
        let root = db.add_index_root(&source_root, 15, 1_000_000).unwrap();
        let initial = vec![
            ScannedModel {
                relative_path: "unique.stl".into(),
                content_hash: hash(1),
                bytes: 9,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("Unique metadata".into()),
                archive_member: None,
                outer_fingerprint: None,
            },
            ScannedModel {
                relative_path: "duplicate-a.stl".into(),
                content_hash: hash(2),
                bytes: 9,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: None,
                archive_member: None,
                outer_fingerprint: None,
            },
            ScannedModel {
                relative_path: "duplicate-b.stl".into(),
                content_hash: hash(2),
                bytes: 9,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: None,
                archive_member: None,
                outer_fingerprint: None,
            },
        ];
        assert_eq!(db.record_scan_entries(&root.id, &initial).unwrap().added, 3);
        db.complete_scan(
            &root.id,
            &initial
                .iter()
                .map(|entry| entry.relative_path.clone())
                .collect::<Vec<_>>(),
            "scan-1",
        )
        .unwrap();
        let first_models = db.list_models(None, None).unwrap();
        let unique = first_models
            .iter()
            .find(|model| model.display_name == "Unique metadata")
            .unwrap()
            .clone();
        let duplicate_ids = first_models
            .iter()
            .filter(|model| model.content_hash.as_deref() == Some(hash(2).as_str()))
            .map(|model| model.id.clone())
            .collect::<HashSet<_>>();
        assert_eq!(duplicate_ids.len(), 2);
        assert!(library_root
            .join("originals")
            .read_dir()
            .unwrap()
            .next()
            .is_none());
        assert_eq!(
            fs::read(source_root.join("unique.stl")).unwrap(),
            b"unique-v1"
        );

        fs::rename(
            source_root.join("unique.stl"),
            source_root.join("renamed.stl"),
        )
        .unwrap();
        let renamed = ScannedModel {
            relative_path: "renamed.stl".into(),
            content_hash: hash(1),
            bytes: 9,
            format: "stl-binary".into(),
            modified_at: 2,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        let rename_batch = db
            .record_scan_entries(&root.id, std::slice::from_ref(&renamed))
            .unwrap();
        let rename_result = db
            .complete_scan_with_new_models(
                &root.id,
                &[
                    renamed.relative_path.clone(),
                    "duplicate-a.stl".into(),
                    "duplicate-b.stl".into(),
                ],
                &rename_batch.added_model_ids,
                "scan-2",
            )
            .unwrap();
        assert_eq!(rename_result.renamed, 1);
        let after_rename = db.get_model(&unique.id).unwrap().unwrap();
        assert_eq!(after_rename.display_name, "Unique metadata");
        assert!(after_rename.metadata_version > unique.metadata_version);
        assert!(matches!(
            db.remove_models_recoverably(&[VersionedTarget {
                model_id: unique.id.clone(),
                expected_version: unique.metadata_version,
            }]),
            Err(LibraryError::MetadataConflict { .. })
        ));
        assert!(after_rename
            .source_path
            .as_deref()
            .unwrap()
            .ends_with("renamed.stl"));
        assert_eq!(
            db.resolve_model_source(&unique.id).unwrap(),
            source_root.join("renamed.stl").canonicalize().unwrap()
        );

        let thumbnail_relative = PathBuf::from("thumbnails/unique.png");
        let thumbnail_bytes = png_fixture();
        let thumbnail_hash = hex_digest(&thumbnail_bytes);
        fs::write(library_root.join(&thumbnail_relative), &thumbnail_bytes).unwrap();
        assert!(matches!(
            db.set_thumbnail_for_source(&unique.id, &hash(8), &thumbnail_hash, &thumbnail_relative),
            Err(LibraryError::SourceChanged)
        ));
        db.set_thumbnail_for_source(&unique.id, &hash(1), &thumbnail_hash, &thumbnail_relative)
            .unwrap();
        let (thumbnail, png_hash) = db
            .thumbnail_for_model_with_hash(&unique.id)
            .unwrap()
            .unwrap();
        assert_eq!(
            thumbnail,
            library_root
                .join(&thumbnail_relative)
                .canonicalize()
                .unwrap()
        );
        assert_eq!(png_hash, thumbnail_hash);
        fs::write(source_root.join("renamed.stl"), b"unique-v2").unwrap();
        let changed = ScannedModel {
            relative_path: "renamed.stl".into(),
            content_hash: hash(3),
            bytes: 9,
            format: "stl-binary".into(),
            modified_at: 3,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        db.record_scan_entries(&root.id, std::slice::from_ref(&changed))
            .unwrap();
        let after_content_change = db.get_model(&unique.id).unwrap().unwrap();
        assert_eq!(
            after_content_change.content_hash.as_deref(),
            Some(hash(3).as_str())
        );
        assert!(after_content_change.metadata_version > after_rename.metadata_version);
        assert!(matches!(
            db.remove_models_recoverably(&[VersionedTarget {
                model_id: unique.id.clone(),
                expected_version: after_rename.metadata_version,
            }]),
            Err(LibraryError::MetadataConflict { .. })
        ));
        assert!(db.thumbnail_for_model(&unique.id).unwrap().is_none());
        assert_eq!(
            db.purge_stale_thumbnails().unwrap(),
            vec![library_root
                .canonicalize()
                .unwrap()
                .join(&thumbnail_relative)]
        );

        db.record_scan_entries(&root.id, std::slice::from_ref(&changed))
            .unwrap();
        let unchanged = db.get_model(&unique.id).unwrap().unwrap();
        assert!(!unchanged.source_missing);
        assert_eq!(
            unchanged.metadata_version,
            after_content_change.metadata_version
        );
        let timestamp_only = ScannedModel {
            modified_at: changed.modified_at + 1,
            ..changed.clone()
        };
        db.record_scan_entries(&root.id, std::slice::from_ref(&timestamp_only))
            .unwrap();
        assert_eq!(
            db.get_model(&unique.id).unwrap().unwrap().metadata_version,
            after_content_change.metadata_version
        );
        let completed = db
            .complete_scan(&root.id, &["renamed.stl".into()], "scan-3")
            .unwrap();
        assert_eq!(completed.missing, 2);
        assert_eq!(
            db.list_models(None, None)
                .unwrap()
                .iter()
                .filter(|model| model.source_missing)
                .count(),
            2
        );
        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(source_root);
    }

    #[test]
    fn schema_v3_migration_backs_up_and_adds_filament_catalog() {
        let root = temp_root("schema-v3-filaments");
        fs::create_dir_all(&root).unwrap();
        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(
            "DROP TABLE capture_downloads; DROP TABLE capture_items;
             DROP INDEX thumbnails_active_model_idx; DROP INDEX thumbnails_model_idx;
             ALTER TABLE thumbnails DROP COLUMN is_active;
             ALTER TABLE thumbnails DROP COLUMN ordinal;
             CREATE INDEX thumbnails_model_idx ON thumbnails(model_id,is_custom);
             ALTER TABLE models DROP COLUMN catalog_kind;
             DROP TABLE model_filaments; DROP TABLE filaments; DROP TABLE filament_sources;
             ALTER TABLE models DROP COLUMN favorite; ALTER TABLE models DROP COLUMN rating;
             PRAGMA user_version=3;",
        )
        .unwrap();
        drop(conn);
        let db = LibraryDb::open(&root).unwrap();
        assert_eq!(db.list_filaments(true).unwrap(), Vec::<Filament>::new());
        let version: i64 = db
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 6);
        let backup = root.join("library.sqlite3.schema-v3.backup");
        assert!(backup.is_file());
        let backup_conn = Connection::open(backup).unwrap();
        let backup_version: i64 = backup_conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(backup_version, 3);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn schema_v4_migration_backs_up_and_seeds_gallery_selection() {
        let root = temp_root("schema-v4-gallery");
        fs::create_dir_all(&root).unwrap();
        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(
            "DROP TABLE capture_downloads; DROP TABLE capture_items;
             DROP INDEX thumbnails_active_model_idx; DROP INDEX thumbnails_model_idx;
             ALTER TABLE thumbnails DROP COLUMN is_active;
             ALTER TABLE thumbnails DROP COLUMN ordinal;
             CREATE INDEX thumbnails_model_idx ON thumbnails(model_id,is_custom);
             ALTER TABLE models DROP COLUMN catalog_kind;
             ALTER TABLE models DROP COLUMN favorite; ALTER TABLE models DROP COLUMN rating;
             INSERT INTO models(id,display_name,description,folder_id,metadata_version,created_at,updated_at)
               VALUES('gallery-model','Gallery','', 'root',1,'1','1');
             INSERT INTO thumbnails(id,model_id,relative_path,content_hash,is_custom,source_content_hash)
               VALUES('generated','gallery-model','thumbnails/generated.png','aa',0,NULL),
                     ('custom','gallery-model','thumbnails/custom.png','bb',1,NULL);
             PRAGMA user_version=4;",
        )
        .unwrap();
        drop(conn);
        let db = LibraryDb::open(&root).unwrap();
        let gallery = db.list_thumbnail_gallery("gallery-model").unwrap();
        assert_eq!(gallery.len(), 2);
        assert_eq!(gallery.iter().filter(|item| item.is_active).count(), 1);
        assert!(
            gallery
                .iter()
                .find(|item| item.id == "custom")
                .unwrap()
                .is_active
        );
        assert_eq!(
            db.get_model("gallery-model").unwrap().unwrap().catalog_kind,
            "file"
        );
        let backup = root.join("library.sqlite3.schema-v4.backup");
        assert!(backup.is_file());
        let version: i64 = db
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 6);
        let version: i64 = Connection::open(backup)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn schema_v5_migration_backs_up_and_adds_rating_and_favorite_defaults() {
        let root = temp_root("schema-v5-rating");
        fs::create_dir_all(&root).unwrap();
        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(
            "ALTER TABLE models DROP COLUMN favorite;
             ALTER TABLE models DROP COLUMN rating;
             INSERT INTO models(id,display_name,description,folder_id,metadata_version,created_at,updated_at)
             VALUES('legacy-model','Legacy','', 'root',1,'1','1');
             PRAGMA user_version=5;",
        )
        .unwrap();
        drop(conn);

        let db = LibraryDb::open(&root).unwrap();
        let model = db.get_model("legacy-model").unwrap().unwrap();
        assert_eq!(model.metadata.rating, 0);
        assert!(!model.metadata.favorite);
        let backup = root.join("library.sqlite3.schema-v5.backup");
        assert!(backup.is_file());
        let backup_version: i64 = Connection::open(backup)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(backup_version, 5);
        let version: i64 = db
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 6);
        drop(db);

        let db = LibraryDb::open(&root).unwrap();
        let model = db.get_model("legacy-model").unwrap().unwrap();
        let updated = db
            .update_model_metadata(
                &model.id,
                model.metadata_version,
                &MetadataPatch {
                    rating: Some(5),
                    favorite: Some(true),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        assert_eq!(updated.metadata.rating, 5);
        assert!(updated.metadata.favorite);
        let (filtered, _) = db
            .list_models_page(
                &ModelPageQuery {
                    minimum_rating: Some(5),
                    favorite: Some(true),
                    ..ModelPageQuery::default()
                },
                None,
                None,
                10,
            )
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, model.id);
        assert!(db
            .update_model_metadata(
                &updated.id,
                updated.metadata_version,
                &MetadataPatch {
                    rating: Some(6),
                    ..MetadataPatch::default()
                },
            )
            .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn gallery_and_capture_receipts_are_versioned_bounded_and_idempotent() {
        use crate::models::{CaptureDownloadInput, CapturePageInput};

        let root = temp_root("gallery-capture");
        let db = LibraryDb::open(&root).unwrap();
        let model = db.create_model("Gallery", None).unwrap();
        let png = png_fixture();
        let png_hash = hex_digest(&png);
        let first_path = PathBuf::from("thumbnails/first.png");
        let second_path = PathBuf::from("thumbnails/second.png");
        fs::write(root.join(&first_path), &png).unwrap();
        fs::write(root.join(&second_path), &png).unwrap();
        let (model, first) = db
            .add_custom_thumbnail(&model.id, model.metadata_version, &png_hash, &first_path)
            .unwrap();
        let (model, second) = db
            .add_custom_thumbnail(&model.id, model.metadata_version, &png_hash, &second_path)
            .unwrap();
        assert!(second.is_active);
        let gallery = db.list_thumbnail_gallery(&model.id).unwrap();
        assert_eq!(gallery.len(), 2);
        assert!(
            !gallery
                .iter()
                .find(|item| item.id == first.id)
                .unwrap()
                .is_active
        );
        let model = db
            .set_active_thumbnail(&model.id, model.metadata_version, &first.id)
            .unwrap();
        assert!(db
            .delete_thumbnail_gallery_item(&model.id, model.metadata_version, &first.id)
            .is_err());
        let (model, deleted_path) = db
            .delete_thumbnail_gallery_item(&model.id, model.metadata_version, &second.id)
            .unwrap();
        assert_eq!(
            deleted_path,
            root.canonicalize().unwrap().join(&second_path)
        );
        assert_eq!(db.list_thumbnail_gallery(&model.id).unwrap().len(), 1);

        let receipt = CaptureReceiptInput {
            item_id: "item-1".into(),
            payload_hash: hash(7),
            created_at: "2026-10-11T00:00:00Z".into(),
            page: CapturePageInput {
                url: "https://example.test/model".into(),
                title: "Captured / Bookmark".into(),
                designer: Some("Designer".into()),
                model_license: Some("CC0".into()),
                notes: Some("Captured while offline".into()),
                parent_external_id: None,
            },
            downloads: vec![
                CaptureDownloadInput {
                    capture_id: "download-1".into(),
                    file_name: "part.stl".into(),
                    sha256: Some(hash(8)),
                    bytes: Some(84),
                },
                CaptureDownloadInput {
                    capture_id: "download-2".into(),
                    file_name: "other.3mf".into(),
                    sha256: None,
                    bytes: None,
                },
            ],
        };
        let created = db.create_or_get_capture_receipt(&receipt).unwrap();
        let bookmark_id = created.page_model_id.clone().unwrap();
        let bookmark = db.get_model(&bookmark_id).unwrap().unwrap();
        assert_eq!(bookmark.catalog_kind, "bookmark");
        assert!(bookmark.content_hash.is_none());
        assert_eq!(created.status, "partial");
        assert_eq!(
            db.create_or_get_capture_receipt(&receipt)
                .unwrap()
                .page_model_id,
            Some(bookmark_id.clone())
        );
        let mut conflicting = receipt.clone();
        conflicting.payload_hash = hash(9);
        assert!(db.create_or_get_capture_receipt(&conflicting).is_err());
        let mut second_receipt = receipt.clone();
        second_receipt.item_id = "item-2".into();
        second_receipt.payload_hash = hash(10);
        second_receipt.created_at = "2026-10-11T00:00:01Z".into();
        for (index, download) in second_receipt.downloads.iter_mut().enumerate() {
            download.capture_id = format!("item-2-download-{index}");
        }
        db.create_or_get_capture_receipt(&second_receipt).unwrap();
        let (first_page, more) = db.list_capture_summaries_page(None, None, 1).unwrap();
        assert!(more);
        assert_eq!(first_page[0].item_id, "item-2");
        assert_eq!(first_page[0].download_count, 2);
        let (second_page, more) = db
            .list_capture_summaries_page(
                Some(&first_page[0].created_at),
                Some(&first_page[0].item_id),
                1,
            )
            .unwrap();
        assert!(!more);
        assert_eq!(second_page[0].item_id, "item-1");
        let imported = db.create_model("Imported part", None).unwrap();
        let receipt = db
            .update_capture_download_result("item-1", "download-1", Some(&imported.id), None)
            .unwrap();
        assert_eq!(receipt.status, "partial");
        let receipt = db
            .update_capture_download_result("item-1", "download-2", None, Some("needs_upload"))
            .unwrap();
        assert_eq!(receipt.status, "failed");
        let other = db.create_model("Other part", None).unwrap();
        let receipt = db
            .update_capture_download_result("item-1", "download-2", Some(&other.id), None)
            .unwrap();
        assert_eq!(receipt.status, "imported");
        db.remove_models_recoverably(&[VersionedTarget {
            model_id: imported.id.clone(),
            expected_version: imported.metadata_version,
        }])
        .unwrap();
        let receipt = db.get_capture_receipt("item-1").unwrap().unwrap();
        assert_eq!(receipt.downloads[0].status, "imported_removed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manual_filaments_assign_atomically_archive_references_and_preserve_unknown_history() {
        let db = LibraryDb::open_in_memory().unwrap();
        let model = db.create_model("Filament model", None).unwrap();
        let red = db
            .create_filament(&FilamentInput {
                brand: "Example".into(),
                material: "PLA".into(),
                color: "#FF0000".into(),
                name: "Red".into(),
                notes: "local".into(),
            })
            .unwrap();
        let assigned = db
            .set_model_filaments(
                &model.id,
                model.metadata_version,
                std::slice::from_ref(&red.id),
            )
            .unwrap();
        assert_eq!(assigned.filament_ids, std::slice::from_ref(&red.id));
        assert!(matches!(
            db.set_model_filaments(&model.id, model.metadata_version, &[]),
            Err(LibraryError::MetadataConflict { .. })
        ));
        let attempt = db
            .create_print_attempt(
                &model.id,
                &PrintAttemptInput {
                    printed_at: "2026-10-10T12:00:00Z".into(),
                    quantity: 1,
                    notes: String::new(),
                    filament_ids: vec!["legacy-unknown".into()],
                    outcome: PrintOutcome::Printed,
                },
            )
            .unwrap();
        assert_eq!(attempt.filament_ids, ["legacy-unknown"]);
        let archived = db.delete_filament(&red.id, red.version).unwrap().unwrap();
        assert!(archived.archived);
        assert_eq!(
            db.get_print_attempt(&attempt.id)
                .unwrap()
                .unwrap()
                .filament_ids,
            ["legacy-unknown"]
        );
        let extra = (0..5)
            .map(|number| {
                db.create_filament(&FilamentInput {
                    name: format!("F{number}"),
                    ..FilamentInput::default()
                })
                .unwrap()
            })
            .collect::<Vec<_>>();
        let current = db.get_model(&model.id).unwrap().unwrap();
        assert!(db
            .set_model_filaments(
                &model.id,
                current.metadata_version,
                &extra.iter().map(|item| item.id.clone()).collect::<Vec<_>>()
            )
            .is_ok());
        let current = db.get_model(&model.id).unwrap().unwrap();
        let mut six = current.filament_ids.clone();
        six.push(red.id);
        assert!(db
            .set_model_filaments(&model.id, current.metadata_version, &six)
            .is_err());
        let unreferenced = db
            .create_filament(&FilamentInput {
                name: "Disposable".into(),
                ..FilamentInput::default()
            })
            .unwrap();
        assert!(db
            .delete_filament(&unreferenced.id, unreferenced.version)
            .unwrap()
            .is_none());
    }

    #[test]
    fn heterogeneous_filament_assignments_prevalidate_every_target_atomically() {
        let db = LibraryDb::open_in_memory().unwrap();
        let a = db.create_model("A", None).unwrap();
        let b = db.create_model("B", None).unwrap();
        let make = |name: &str| {
            db.create_filament(&FilamentInput {
                name: name.into(),
                ..FilamentInput::default()
            })
            .unwrap()
        };
        let first = make("First");
        let second = make("Second");
        let shared = make("Shared");
        let a = db
            .set_model_filaments(&a.id, a.metadata_version, std::slice::from_ref(&first.id))
            .unwrap();
        let b = db
            .set_model_filaments(&b.id, b.metadata_version, std::slice::from_ref(&second.id))
            .unwrap();
        let changed_b = db
            .update_model_metadata(
                &b.id,
                b.metadata_version,
                &MetadataPatch {
                    notes: Some("changed".into()),
                    ..MetadataPatch::default()
                },
            )
            .unwrap();
        assert!(matches!(
            db.bulk_assign_model_filaments(&[
                ModelFilamentAssignment {
                    target: VersionedTarget {
                        model_id: a.id.clone(),
                        expected_version: a.metadata_version,
                    },
                    filament_ids: vec![first.id.clone(), shared.id.clone()],
                },
                ModelFilamentAssignment {
                    target: VersionedTarget {
                        model_id: b.id.clone(),
                        expected_version: b.metadata_version,
                    },
                    filament_ids: vec![second.id.clone(), shared.id.clone()],
                },
            ]),
            Err(LibraryError::MetadataConflict { .. })
        ));
        assert_eq!(
            db.get_model(&a.id).unwrap().unwrap().filament_ids,
            std::slice::from_ref(&first.id)
        );
        assert!(db
            .bulk_assign_model_filaments(&[
                ModelFilamentAssignment {
                    target: VersionedTarget {
                        model_id: a.id.clone(),
                        expected_version: a.metadata_version,
                    },
                    filament_ids: vec![first.id.clone(), shared.id.clone()],
                },
                ModelFilamentAssignment {
                    target: VersionedTarget {
                        model_id: b.id.clone(),
                        expected_version: changed_b.metadata_version,
                    },
                    filament_ids: vec!["unknown".into()],
                },
            ])
            .is_err());
        assert_eq!(
            db.get_model(&a.id).unwrap().unwrap().filament_ids,
            std::slice::from_ref(&first.id)
        );
        let updated = db
            .bulk_assign_model_filaments(&[
                ModelFilamentAssignment {
                    target: VersionedTarget {
                        model_id: a.id.clone(),
                        expected_version: a.metadata_version,
                    },
                    filament_ids: vec![first.id.clone(), shared.id.clone()],
                },
                ModelFilamentAssignment {
                    target: VersionedTarget {
                        model_id: b.id,
                        expected_version: changed_b.metadata_version,
                    },
                    filament_ids: vec![second.id.clone(), shared.id.clone()],
                },
            ])
            .unwrap();
        assert_eq!(updated[0].filament_ids, [first.id, shared.id.clone()]);
        assert_eq!(updated[1].filament_ids, [second.id, shared.id]);
    }

    #[test]
    fn spoolman_complete_snapshots_keep_ids_and_references_across_missing_and_reappearance() {
        let db = LibraryDb::open_in_memory().unwrap();
        let source = db
            .upsert_spoolman_source("http://spoolman.local/api/v1/")
            .unwrap();
        let filament = SpoolmanFilamentRecord {
            external_id: "1".into(),
            brand: "Vendor".into(),
            material: "PLA".into(),
            color: "FF0000".into(),
            name: "Red PLA".into(),
            notes: String::new(),
        };
        let spool = SpoolmanSpoolRecord {
            external_id: "10".into(),
            filament_external_id: "1".into(),
            brand: filament.brand.clone(),
            material: filament.material.clone(),
            color: filament.color.clone(),
            name: filament.name.clone(),
            notes: "Shelf A".into(),
            archived: true,
        };
        let first = db
            .sync_spoolman_snapshot(
                &source.id,
                std::slice::from_ref(&filament),
                std::slice::from_ref(&spool),
                "2026-10-10T12:00:00Z",
            )
            .unwrap();
        assert_eq!(first.added, 2);
        let imported = db.list_filaments(true).unwrap();
        let type_id = imported
            .iter()
            .find(|item| item.resource_kind == FilamentResourceKind::Filament)
            .unwrap()
            .id
            .clone();
        let spool_row = imported
            .iter()
            .find(|item| item.resource_kind == FilamentResourceKind::Spool)
            .unwrap()
            .clone();
        assert_eq!(
            spool_row.parent_filament_id.as_deref(),
            Some(type_id.as_str())
        );
        assert!(spool_row.archived);
        let repeated = db
            .sync_spoolman_snapshot(
                &source.id,
                std::slice::from_ref(&filament),
                std::slice::from_ref(&spool),
                "2026-10-10T12:01:00Z",
            )
            .unwrap();
        assert_eq!(
            (repeated.added, repeated.updated, repeated.missing),
            (0, 0, 0)
        );
        let model = db.create_model("Synced", None).unwrap();
        db.set_model_filaments(
            &model.id,
            model.metadata_version,
            std::slice::from_ref(&spool_row.id),
        )
        .unwrap();
        let malformed = SpoolmanSpoolRecord {
            filament_external_id: "missing".into(),
            ..spool.clone()
        };
        assert!(db
            .sync_spoolman_snapshot(
                &source.id,
                std::slice::from_ref(&filament),
                &[malformed],
                "2026-10-10T12:02:00Z"
            )
            .is_err());
        assert!(
            !db.get_filament(&spool_row.id)
                .unwrap()
                .unwrap()
                .source_missing
        );
        let missing = db
            .sync_spoolman_snapshot(&source.id, &[], &[], "2026-10-10T12:03:00Z")
            .unwrap();
        assert_eq!(missing.missing, 2);
        assert!(
            db.get_filament(&spool_row.id)
                .unwrap()
                .unwrap()
                .source_missing
        );
        assert_eq!(
            db.get_model(&model.id).unwrap().unwrap().filament_ids,
            std::slice::from_ref(&spool_row.id)
        );
        db.sync_spoolman_snapshot(
            &source.id,
            std::slice::from_ref(&filament),
            std::slice::from_ref(&spool),
            "2026-10-10T12:04:00Z",
        )
        .unwrap();
        assert!(
            !db.get_filament(&spool_row.id)
                .unwrap()
                .unwrap()
                .source_missing
        );
        assert_eq!(
            db.get_filament(&spool_row.id).unwrap().unwrap().id,
            spool_row.id
        );
        let other = db
            .upsert_spoolman_source("https://other.example/api/v1")
            .unwrap();
        db.sync_spoolman_snapshot(
            &other.id,
            std::slice::from_ref(&filament),
            std::slice::from_ref(&spool),
            "2026-10-10T12:05:00Z",
        )
        .unwrap();
        assert_eq!(db.list_filaments(true).unwrap().len(), 4);
    }

    #[test]
    fn index_roots_reject_overlap_and_removed_roots_reject_stale_scan_writes() {
        let library_root = temp_root("root-validation-library");
        let source_root = temp_root("root-validation-source");
        fs::create_dir_all(source_root.join("nested")).unwrap();
        fs::write(source_root.join("part.stl"), b"part").unwrap();
        let db = LibraryDb::open(&library_root).unwrap();
        assert!(db.add_index_root(&source_root, 0, 1024).is_err());
        assert!(db.add_index_root(&source_root, 1_441, 1024).is_err());
        assert!(db
            .add_index_root(&source_root, 10, MAX_INDEXED_FILE_BYTES + 1)
            .is_err());
        let root = db.add_index_root(&source_root, 10, 1024).unwrap();
        assert!(db
            .add_index_root(source_root.join("nested"), 10, 1024)
            .is_err());
        db.remove_index_root(&root.id).unwrap();
        let entry = ScannedModel {
            relative_path: "part.stl".into(),
            content_hash: hash(4),
            bytes: 4,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        assert!(matches!(
            db.record_scan_entries(&root.id, &[entry]),
            Err(LibraryError::IndexRootNotFound(_))
        ));
        assert!(matches!(
            db.complete_scan(&root.id, &[], "cancelled"),
            Err(LibraryError::IndexRootNotFound(_))
        ));
        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(source_root);
    }

    #[test]
    fn persisted_index_root_values_are_checked_before_scanner_use() {
        let library_root = temp_root("corrupt-root-settings-library");
        let source_root = temp_root("corrupt-root-settings-source");
        fs::create_dir_all(&source_root).unwrap();
        let db = LibraryDb::open(&library_root).unwrap();
        let root = db.add_index_root(&source_root, 60, 1024).unwrap();
        db.conn
            .execute_batch("PRAGMA ignore_check_constraints = ON;")
            .unwrap();

        db.conn
            .execute(
                "UPDATE index_roots SET max_file_bytes = ?1 WHERE id = ?2",
                params![MAX_INDEXED_FILE_BYTES as i64 + 1, root.id],
            )
            .unwrap();
        assert!(db.list_index_roots().is_err());

        db.conn
            .execute(
                "UPDATE index_roots SET max_file_bytes = 1024, scan_interval_minutes = 4294967356
                 WHERE id = ?1",
                params![root.id],
            )
            .unwrap();
        assert!(db.list_index_roots().is_err());

        db.conn
            .execute(
                "UPDATE index_roots SET scan_interval_minutes = 60, enabled = 2 WHERE id = ?1",
                params![root.id],
            )
            .unwrap();
        assert!(db.list_index_roots().is_err());
        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(source_root);
    }

    #[test]
    fn offline_index_root_settings_can_be_updated_without_changing_catalog_identity() {
        let library_root = temp_root("offline-settings-library");
        let source_root = temp_root("offline-settings-source");
        let offline_location = temp_root("offline-settings-unmounted");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(source_root.join("part.stl"), b"part").unwrap();
        let db = LibraryDb::open(&library_root).unwrap();
        let root = db.add_index_root(&source_root, 10, 1024).unwrap();
        let entry = ScannedModel {
            relative_path: "part.stl".into(),
            content_hash: hash(4),
            bytes: 4,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: Some("Offline part".into()),
            archive_member: None,
            outer_fingerprint: None,
        };
        let added = db.record_scan_entries(&root.id, &[entry]).unwrap();
        let model_id = added.added_model_ids[0].clone();
        db.complete_scan_with_new_models(
            &root.id,
            &["part.stl".into()],
            &added.added_model_ids,
            "scan-1",
        )
        .unwrap();
        fs::rename(&source_root, &offline_location).unwrap();

        let updated = db
            .update_index_root(&IndexRoot {
                enabled: false,
                scan_interval_minutes: 120,
                max_file_bytes: 2048,
                ..root
            })
            .unwrap();
        assert!(!updated.enabled);
        assert_eq!(updated.scan_interval_minutes, 120);
        assert_eq!(updated.max_file_bytes, 2048);
        let model = db.get_model(&model_id).unwrap().unwrap();
        assert_eq!(model.id, model_id);
        assert!(!model.source_missing);
        assert_eq!(db.list_models(None, None).unwrap().len(), 1);
        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(offline_location);
    }

    #[test]
    fn restored_root_mapping_rewrites_links_and_disables_unmapped_roots() {
        let temp = temp_root("restore-root-map");
        let old = temp.join("old");
        let new = temp.join("new");
        let other = temp.join("other");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(old.join("part.stl"), vec![0_u8; 84]).unwrap();
        let db = LibraryDb::open(temp.join("library")).unwrap();
        let root = db.add_index_root(&old, 60, 1024).unwrap();
        let unmapped = db.add_index_root(&other, 60, 1024).unwrap();
        db.record_scan_entries(
            &root.id,
            &[ScannedModel {
                relative_path: "part.stl".into(),
                content_hash: hash(1),
                bytes: 84,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: None,
                archive_member: None,
                outer_fingerprint: None,
            }],
        )
        .unwrap();
        db.remap_restored_index_roots(&[(
            root.id.clone(),
            "operator-root".into(),
            new.canonicalize().unwrap(),
            true,
        )])
        .unwrap();
        let roots = db.list_index_roots().unwrap();
        let mapped = roots
            .iter()
            .find(|root| root.id == "operator-root")
            .unwrap();
        assert_eq!(mapped.path, new.canonicalize().unwrap());
        assert!(mapped.enabled);
        assert!(
            !roots
                .iter()
                .find(|root| root.id == unmapped.id)
                .unwrap()
                .enabled
        );
        let model = db.list_models(None, None).unwrap().remove(0);
        assert_eq!(model.source_root_id.as_deref(), Some("operator-root"));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn duplicate_survivor_is_not_consumed_as_a_rename_and_keeps_its_identity() {
        let library_root = temp_root("duplicate-survivor-library");
        let source_root = temp_root("duplicate-survivor-source");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(source_root.join("a.stl"), b"same").unwrap();
        fs::write(source_root.join("b.stl"), b"same").unwrap();
        let db = LibraryDb::open(&library_root).unwrap();
        let root = db.add_index_root(&source_root, 10, 1024).unwrap();
        let entries = [
            ScannedModel {
                relative_path: "a.stl".into(),
                content_hash: hash(6),
                bytes: 4,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("Metadata A".into()),
                archive_member: None,
                outer_fingerprint: None,
            },
            ScannedModel {
                relative_path: "b.stl".into(),
                content_hash: hash(6),
                bytes: 4,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("Metadata B".into()),
                archive_member: None,
                outer_fingerprint: None,
            },
        ];
        let added = db.record_scan_entries(&root.id, &entries).unwrap();
        db.complete_scan_with_new_models(
            &root.id,
            &["a.stl".into(), "b.stl".into()],
            &added.added_model_ids,
            "scan-1",
        )
        .unwrap();
        let survivor_before = db
            .list_models(None, Some("Metadata B"))
            .unwrap()
            .pop()
            .unwrap();
        fs::remove_file(source_root.join("a.stl")).unwrap();

        let completed = db
            .complete_scan_with_new_models(&root.id, &["b.stl".into()], &[], "scan-2")
            .unwrap();
        assert_eq!(completed.renamed, 0);
        let survivor_after = db.get_model(&survivor_before.id).unwrap().unwrap();
        assert_eq!(survivor_after.display_name, "Metadata B");
        assert!(!survivor_after.source_missing);
        assert_eq!(
            db.list_models(None, None)
                .unwrap()
                .iter()
                .filter(|model| model.source_missing)
                .count(),
            1
        );
        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(source_root);
    }

    #[test]
    fn cancelled_scan_additions_are_not_later_treated_as_new_rename_targets() {
        let library_root = temp_root("cancelled-rename-library");
        let source_root = temp_root("cancelled-rename-source");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(source_root.join("old.stl"), b"same").unwrap();
        let db = LibraryDb::open(&library_root).unwrap();
        let root = db.add_index_root(&source_root, 10, 1024).unwrap();
        let old = ScannedModel {
            relative_path: "old.stl".into(),
            content_hash: hash(7),
            bytes: 4,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: Some("Old metadata".into()),
            archive_member: None,
            outer_fingerprint: None,
        };
        let first = db
            .record_scan_entries(&root.id, std::slice::from_ref(&old))
            .unwrap();
        db.complete_scan_with_new_models(
            &root.id,
            &["old.stl".into()],
            &first.added_model_ids,
            "scan-1",
        )
        .unwrap();
        fs::rename(source_root.join("old.stl"), source_root.join("new.stl")).unwrap();
        let new = ScannedModel {
            relative_path: "new.stl".into(),
            content_hash: hash(7),
            bytes: 4,
            format: "stl-binary".into(),
            modified_at: 2,
            display_name: Some("Cancelled identity".into()),
            archive_member: None,
            outer_fingerprint: None,
        };
        let cancelled = db
            .record_scan_entries(&root.id, std::slice::from_ref(&new))
            .unwrap();
        assert_eq!(cancelled.added_model_ids.len(), 1);

        let resumed = db
            .record_scan_entries(&root.id, std::slice::from_ref(&new))
            .unwrap();
        assert!(resumed.added_model_ids.is_empty());
        let completed = db
            .complete_scan_with_new_models(
                &root.id,
                &["new.stl".into()],
                &resumed.added_model_ids,
                "scan-2",
            )
            .unwrap();
        assert_eq!(completed.renamed, 0);
        assert_eq!(db.list_models(None, None).unwrap().len(), 2);
        assert!(db
            .get_model(&cancelled.added_model_ids[0])
            .unwrap()
            .is_some());
        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(source_root);
    }

    #[cfg(unix)]
    #[test]
    fn indexed_sources_reject_parent_escape_and_symlink_escape() {
        use std::os::unix::fs::symlink;

        let library_root = temp_root("escape-library");
        let source_root = temp_root("escape-source");
        let outside_root = temp_root("escape-outside");
        fs::create_dir_all(&source_root).unwrap();
        fs::create_dir_all(&outside_root).unwrap();
        fs::write(outside_root.join("outside.stl"), b"outside").unwrap();
        symlink(
            outside_root.join("outside.stl"),
            source_root.join("link.stl"),
        )
        .unwrap();
        let db = LibraryDb::open(&library_root).unwrap();
        let root = db.add_index_root(&source_root, 10, 1024).unwrap();
        let escaped = ScannedModel {
            relative_path: "../outside.stl".into(),
            content_hash: hash(5),
            bytes: 7,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        assert!(db.record_scan_entries(&root.id, &[escaped]).is_err());
        let linked = ScannedModel {
            relative_path: "link.stl".into(),
            content_hash: hash(5),
            bytes: 7,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        assert!(db.record_scan_entries(&root.id, &[linked]).is_err());
        fs::remove_file(source_root.join("link.stl")).unwrap();
        let moved_root = temp_root("escape-source-moved");
        fs::rename(&source_root, &moved_root).unwrap();
        symlink(&outside_root, &source_root).unwrap();
        let replaced_root = ScannedModel {
            relative_path: "outside.stl".into(),
            content_hash: hash(5),
            bytes: 7,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        };
        assert!(db.record_scan_entries(&root.id, &[replaced_root]).is_err());
        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_file(source_root);
        let _ = fs::remove_dir_all(moved_root);
        let _ = fs::remove_dir_all(outside_root);
    }

    #[test]
    fn purge_thumbnails_removes_generated_records_but_preserves_custom_records() {
        let root = temp_root("purge-thumbnails");
        let source_dir = temp_root("purge-thumbnails-source");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("part.stl");
        fs::write(&source, stl_fixture()).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        let generated_bytes = png_fixture();
        let generated_hash = hex_digest(&generated_bytes);
        fs::write(root.join("thumbnails/generated.png"), &generated_bytes).unwrap();
        fs::write(root.join("thumbnails/custom.png"), b"custom").unwrap();
        db.set_thumbnail(
            &model.id,
            &generated_hash,
            Path::new("thumbnails/generated.png"),
        )
        .unwrap();
        db.conn.execute("INSERT INTO thumbnails (id, model_id, relative_path, content_hash, is_custom) VALUES ('custom', ?1, 'thumbnails/custom.png', 'custom-hash', 1)", params![model.id]).unwrap();
        let purged = db.purge_thumbnails().unwrap();
        assert_eq!(
            purged,
            vec![root
                .canonicalize()
                .unwrap()
                .join("thumbnails/generated.png")]
        );
        assert_eq!(
            db.thumbnail_for_model(&model.id).unwrap(),
            Some(root.join("thumbnails/custom.png").canonicalize().unwrap())
        );
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_dir);
    }

    #[test]
    fn purge_removes_generated_row_but_preserves_file_shared_with_custom_row() {
        let root = temp_root("purge-shared-custom");
        let source_dir = temp_root("purge-shared-custom-source");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("part.stl");
        fs::write(&source, stl_fixture()).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        let thumbnail_bytes = png_fixture();
        let thumbnail_hash = hex_digest(&thumbnail_bytes);
        let relative = "thumbnails/shared.png";
        let absolute = db.root().join(relative);
        fs::write(&absolute, &thumbnail_bytes).unwrap();
        db.set_thumbnail(&model.id, &thumbnail_hash, Path::new(relative))
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO thumbnails
                 (id, model_id, relative_path, content_hash, is_custom)
                 VALUES ('shared-custom', ?1, ?2, ?3, 1)",
                params![model.id, relative, thumbnail_hash],
            )
            .unwrap();

        let preferred = db
            .thumbnail_cache_entry_for_model(&model.id)
            .unwrap()
            .unwrap();
        assert!(preferred.is_custom);
        assert_eq!(preferred.png_hash, thumbnail_hash);

        let physical_deletes = db.purge_thumbnails().unwrap();
        for path in physical_deletes {
            fs::remove_file(path).unwrap();
        }
        assert!(absolute.is_file());
        let generated: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE is_custom = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let custom: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE id = 'shared-custom'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generated, 0);
        assert_eq!(custom, 1);
        assert!(db.thumbnail_for_model(&model.id).unwrap().is_some());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_dir);
    }

    #[test]
    fn purge_refuses_wrong_namespace_before_deleting_rows_or_originals() {
        let root = temp_root("purge-wrong-namespace");
        let source_dir = temp_root("purge-wrong-namespace-source");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("part.stl");
        let source_bytes = stl_fixture();
        fs::write(&source, &source_bytes).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        let original_relative = model.storage_path.clone().unwrap();
        let original_path = db.root().join(&original_relative);
        let before = fs::read(&original_path).unwrap();
        db.conn
            .execute(
                "INSERT INTO thumbnails
                 (id, model_id, relative_path, content_hash, is_custom, source_content_hash)
                 VALUES ('malicious-original', ?1, ?2, ?3, 0, ?4)",
                params![
                    model.id,
                    original_relative,
                    hex_digest(&source_bytes),
                    model.content_hash
                ],
            )
            .unwrap();

        assert!(matches!(
            db.purge_thumbnails(),
            Err(LibraryError::InvalidInput(_))
        ));
        assert_eq!(fs::read(&original_path).unwrap(), before);
        let retained: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE id = 'malicious-original'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, 1);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_dir);
    }

    #[cfg(unix)]
    #[test]
    fn purge_refuses_symlinked_thumbnail_ancestor_and_retains_outside_file_and_row() {
        use std::os::unix::fs::symlink;

        let root = temp_root("purge-symlink-ancestor");
        let source_dir = temp_root("purge-symlink-ancestor-source");
        let outside = temp_root("purge-symlink-ancestor-outside");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let source = source_dir.join("part.stl");
        fs::write(&source, stl_fixture()).unwrap();
        let db = LibraryDb::open(&root).unwrap();
        let model = db.import_file(&source, None, None).unwrap().model;
        let outside_bytes = png_fixture();
        let outside_path = outside.join("outside.png");
        fs::write(&outside_path, &outside_bytes).unwrap();
        symlink(&outside, db.root().join("thumbnails/link")).unwrap();
        db.conn
            .execute(
                "INSERT INTO thumbnails
                 (id, model_id, relative_path, content_hash, is_custom, source_content_hash)
                 VALUES ('malicious-symlink', ?1, 'thumbnails/link/outside.png', ?2, 0, ?3)",
                params![model.id, hex_digest(&outside_bytes), model.content_hash],
            )
            .unwrap();

        assert!(matches!(
            db.purge_thumbnails(),
            Err(LibraryError::InvalidInput(_))
        ));
        assert_eq!(fs::read(&outside_path).unwrap(), outside_bytes);
        let retained: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM thumbnails WHERE id = 'malicious-symlink'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, 1);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_dir);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn nonempty_folder_cannot_be_deleted() {
        let db = LibraryDb::open_in_memory().unwrap();
        let folder = db.create_folder("Nested", None).unwrap();
        db.create_model("Part", Some(&folder.id)).unwrap();
        assert!(matches!(
            db.delete_folder(&folder.id),
            Err(LibraryError::FolderNotEmpty)
        ));
    }

    #[test]
    fn moving_a_folder_rejects_descendant_cycles() {
        let db = LibraryDb::open_in_memory().unwrap();
        let parent = db.create_folder("Parent", None).unwrap();
        let child = db.create_folder("Child", Some(&parent.id)).unwrap();
        assert!(matches!(
            db.move_folder(&parent.id, &child.id),
            Err(LibraryError::FolderCycle)
        ));
        assert_eq!(
            db.move_folder(&child.id, ROOT_FOLDER_ID)
                .unwrap()
                .parent_id
                .as_deref(),
            Some(ROOT_FOLDER_ID)
        );
    }
}
