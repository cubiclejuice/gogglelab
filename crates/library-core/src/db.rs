use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{io, LibraryError};
use crate::models::{
    Folder, ImportResult, IndexRoot, IndexedSourceMetadata, Model, ScanReconcile, ScannedModel,
};

pub const CURRENT_SCHEMA_VERSION: i64 = 2;
pub const ROOT_FOLDER_ID: &str = "root";
pub const MIN_SCAN_INTERVAL_MINUTES: u32 = 1;
pub const MAX_SCAN_INTERVAL_MINUTES: u32 = 1_440;
pub const MAX_INDEXED_FILE_BYTES: u64 = stl_core::limits::MAX_FILE_BYTES;

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct LibraryDb {
    conn: Connection,
    root: PathBuf,
}

impl LibraryDb {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, LibraryError> {
        let root = root.as_ref().to_path_buf();
        for dir in ["originals", "manuals", "thumbnails", "staging", "trash"] {
            std::fs::create_dir_all(root.join(dir)).map_err(|e| io(root.join(dir), e))?;
        }
        let root = root.canonicalize().map_err(|error| io(&root, error))?;
        let db_path = root.join("library.sqlite3");
        let conn = Connection::open(&db_path)?;
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
        }
        Ok(Self { conn, root })
    }

    pub fn root(&self) -> &Path {
        &self.root
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
            Ok(Some(model))
        } else {
            Ok(None)
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
        for model in &mut models {
            model.tags = tags.remove(&model.id).unwrap_or_default();
        }
        Ok(models)
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
        let metadata = std::fs::metadata(source).map_err(|e| io(source, e))?;
        if !metadata.is_file() {
            return Err(LibraryError::InvalidInput("source is not a file".into()));
        }
        let bytes = std::fs::read(source).map_err(|e| io(source, e))?;
        if metadata.len() != bytes.len() as u64 {
            return Err(LibraryError::SourceChanged);
        }
        let parsed = stl_core::parse_model(&bytes)?;
        let format = match parsed.format {
            stl_core::StlFormat::Binary => "stl-binary",
            stl_core::StlFormat::Ascii => "stl-ascii",
            stl_core::StlFormat::ThreeMf => "3mf",
        };
        let hash = hex_digest(&bytes);
        let extension = source
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("stl")
            .to_ascii_lowercase();
        let relative_path = format!("originals/{hash}.{extension}");
        let destination = self.root.join(&relative_path);
        let staging = self
            .root
            .join("staging")
            .join(format!("{}.part", Uuid::new_v4()));
        std::fs::write(&staging, &bytes).map_err(|e| io(&staging, e))?;
        let reused_blob = self
            .conn
            .query_row(
                "SELECT 1 FROM blobs WHERE hash = ?1",
                params![hash],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some();
        if !reused_blob {
            if let Err(error) = std::fs::rename(&staging, &destination) {
                let _ = std::fs::remove_file(&staging);
                return Err(io(&destination, error));
            }
        } else {
            let _ = std::fs::remove_file(&staging);
        }

        let original_name = source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("model.stl");
        let display_name = display_name
            .map(str::to_owned)
            .or_else(|| {
                source
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "Model".into());
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
            params![model_id, clean_name(&display_name, "Model")?, folder_id, revision_id, now],
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
        };
        self.conn.execute(
            "INSERT INTO index_roots
             (id, path, enabled, last_scanned_at, scan_interval_minutes, max_file_bytes)
             VALUES (?1, ?2, 1, NULL, ?3, ?4)",
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
            "SELECT id, path, enabled, last_scanned_at, scan_interval_minutes, max_file_bytes
             FROM index_roots ORDER BY lower(path), id",
        )?;
        let rows = stmt.query_map([], index_root_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
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
                    max_file_bytes = ?4
             WHERE id = ?5",
            params![
                existing.path.to_string_lossy(),
                root.enabled,
                i64::from(root.scan_interval_minutes),
                root.max_file_bytes as i64,
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

    pub fn indexed_source_metadata(
        &self,
        root_id: &str,
        relative_path: &Path,
    ) -> Result<Option<IndexedSourceMetadata>, LibraryError> {
        let relative_path = validated_relative_path(relative_path)?;
        self.conn
            .query_row(
                "SELECT d.bytes, m.source_modified_at, d.content_hash
                 FROM models m JOIN indexed_model_data d ON d.model_id = m.id
                 WHERE m.source_kind = 'indexed' AND m.source_root_id = ?1 AND m.source_path = ?2",
                params![root_id, relative_path],
                |row| {
                    Ok(IndexedSourceMetadata {
                        bytes: row.get::<_, i64>(0)? as u64,
                        modified_at: row.get::<_, i64>(1)? as u64,
                        content_hash: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
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
            if !paths.insert(relative.clone()) {
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
            validated.push((relative, entry));
        }

        let tx = self.conn.unchecked_transaction()?;
        let mut result = ScanReconcile {
            seen: validated.len(),
            ..ScanReconcile::default()
        };
        for (relative, entry) in validated {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT m.id
                     FROM models m JOIN indexed_model_data d ON d.model_id = m.id
                     WHERE m.source_kind = 'indexed' AND m.source_root_id = ?1 AND m.source_path = ?2",
                    params![root_id, relative],
                    |row| row.get(0),
                )
                .optional()?;
            let original_name = Path::new(&relative)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("model")
                .to_owned();
            if let Some(model_id) = existing {
                tx.execute(
                    "UPDATE indexed_model_data SET content_hash = ?1, original_name = ?2,
                            format = ?3, bytes = ?4 WHERE model_id = ?5",
                    params![
                        entry.content_hash,
                        original_name,
                        entry.format,
                        entry.bytes as i64,
                        model_id
                    ],
                )?;
                tx.execute(
                    "UPDATE models SET source_missing = 0, source_modified_at = ?1,
                            updated_at = ?2 WHERE id = ?3",
                    params![entry.modified_at as i64, timestamp(), model_id],
                )?;
                result.updated += 1;
            } else {
                let model_id = new_id("model");
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
                      source_modified_at)
                     VALUES (?1, ?2, '', ?3, 1, ?4, ?4, 'indexed', ?5, ?6, 0, ?7)",
                    params![
                        model_id,
                        display_name,
                        ROOT_FOLDER_ID,
                        now,
                        relative,
                        root_id,
                        entry.modified_at as i64
                    ],
                )?;
                tx.execute(
                    "INSERT INTO indexed_model_data
                     (model_id, content_hash, original_name, format, bytes)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        model_id,
                        entry.content_hash,
                        original_name,
                        entry.format,
                        entry.bytes as i64
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
        let seen = seen_relative_paths
            .iter()
            .map(|path| validated_relative_path(path))
            .collect::<Result<HashSet<_>, _>>()?;
        let tx = self.conn.unchecked_transaction()?;
        let mut indexed = Vec::new();
        {
            let mut stmt = tx.prepare(
                "SELECT m.id, m.source_path, d.content_hash
                 FROM models m JOIN indexed_model_data d ON d.model_id = m.id
                 WHERE m.source_kind = 'indexed' AND m.source_root_id = ?1",
            )?;
            let rows = stmt.query_map(params![root_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            indexed.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }

        let newly_added = newly_added_model_ids.iter().collect::<HashSet<_>>();
        let mut absent_by_hash: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut present_by_hash: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (id, path, hash) in &indexed {
            let target = if seen.contains(path) {
                &mut present_by_hash
            } else {
                &mut absent_by_hash
            };
            target
                .entry(hash.clone())
                .or_default()
                .push((id.clone(), path.clone()));
        }

        let mut result = ScanReconcile {
            seen: seen.len(),
            ..ScanReconcile::default()
        };
        for (hash, absent) in absent_by_hash.iter() {
            let Some(present) = present_by_hash.get(hash) else {
                continue;
            };
            if absent.len() != 1 || present.len() != 1 {
                continue;
            }
            let (old_id, _) = &absent[0];
            let (new_id, new_path) = &present[0];
            if old_id == new_id || !newly_added.contains(new_id) {
                continue;
            }
            let (modified_at, original_name, format, bytes): (i64, String, String, i64) = tx
                .query_row(
                    "SELECT m.source_modified_at, d.original_name, d.format, d.bytes
                     FROM models m JOIN indexed_model_data d ON d.model_id = m.id WHERE m.id = ?1",
                    params![new_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?;
            tx.execute("DELETE FROM models WHERE id = ?1", params![new_id])?;
            tx.execute(
                "UPDATE models SET source_path = ?1, source_missing = 0,
                        source_modified_at = ?2, updated_at = ?3 WHERE id = ?4",
                params![new_path, modified_at, timestamp(), old_id],
            )?;
            tx.execute(
                "UPDATE indexed_model_data SET original_name = ?1, format = ?2, bytes = ?3
                 WHERE model_id = ?4",
                params![original_name, format, bytes, old_id],
            )?;
            result.renamed += 1;
        }

        let mut ids_and_paths = Vec::new();
        {
            let mut stmt = tx.prepare(
                "SELECT id, source_path FROM models
                 WHERE source_kind = 'indexed' AND source_root_id = ?1",
            )?;
            let rows = stmt.query_map(params![root_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            ids_and_paths.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }
        for (id, path) in ids_and_paths {
            let missing = !seen.contains(&path);
            tx.execute(
                "UPDATE models SET source_missing = ?1 WHERE id = ?2",
                params![missing, id],
            )?;
            result.missing += usize::from(missing);
        }
        let changed = tx.execute(
            "UPDATE index_roots SET last_scanned_at = ?1 WHERE id = ?2",
            params![completed_at, root_id],
        )?;
        if changed == 0 {
            return Err(LibraryError::IndexRootNotFound(root_id.to_owned()));
        }
        tx.commit()?;
        Ok(result)
    }

    pub fn resolve_model_source(&self, model_id: &str) -> Result<PathBuf, LibraryError> {
        let source: Option<(String, Option<String>, Option<String>, bool, Option<String>)> = self
            .conn
            .query_row(
                "SELECT m.source_kind, m.source_path, m.source_root_id, m.source_missing,
                        b.relative_path
                 FROM models m
                 LEFT JOIN revisions r ON r.id = m.current_revision_id
                 LEFT JOIN blobs b ON b.hash = r.blob_hash
                 WHERE m.id = ?1",
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
        let (source_kind, source_path, source_root_id, source_missing, managed_path) =
            source.ok_or_else(|| LibraryError::ModelNotFound(model_id.to_owned()))?;
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
                resolve_under_root(&self.root, Path::new(&relative))
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
                resolve_under_root(&root.path, Path::new(&relative))
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
        let model = self
            .get_model(model_id)?
            .ok_or_else(|| LibraryError::ModelNotFound(model_id.to_owned()))?;
        let current_source_hash = model
            .content_hash
            .ok_or_else(|| LibraryError::InvalidInput("model has no source content hash".into()))?;
        if current_source_hash != source_content_hash {
            return Err(LibraryError::SourceChanged);
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM thumbnails WHERE model_id = ?1 AND is_custom = 0",
            params![model_id],
        )?;
        tx.execute(
            "INSERT INTO thumbnails
             (id, model_id, relative_path, content_hash, is_custom, source_content_hash)
             VALUES (?1, ?2, ?3, ?4, 0, ?5)",
            params![
                new_id("thumbnail"),
                model_id,
                relative,
                content_hash,
                source_content_hash
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
        let thumbnail: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT t.relative_path, t.content_hash FROM thumbnails t
                 LEFT JOIN revisions r ON r.id = (SELECT current_revision_id FROM models WHERE id = ?1)
                 LEFT JOIN indexed_model_data d ON d.model_id = ?1
                 WHERE t.model_id = ?1
                   AND (t.is_custom = 1 OR t.source_content_hash IS NULL
                        OR t.source_content_hash = COALESCE(r.blob_hash, d.content_hash))
                 ORDER BY t.is_custom DESC, t.id LIMIT 1",
                params![model_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        thumbnail
            .map(|(path, content_hash)| {
                resolve_under_root(&self.root, Path::new(&path))
                    .map(|absolute| (absolute, content_hash))
            })
            .transpose()
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
            "SELECT t.id, t.relative_path FROM thumbnails t
             JOIN models m ON m.id = t.model_id
             LEFT JOIN revisions r ON r.id = m.current_revision_id
             LEFT JOIN indexed_model_data d ON d.model_id = m.id
             WHERE {predicate}"
        );
        let mut records = Vec::new();
        {
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            records.extend(rows.collect::<Result<Vec<_>, _>>()?);
        }
        let tx = self.conn.unchecked_transaction()?;
        for (id, _) in &records {
            tx.execute("DELETE FROM thumbnails WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        records
            .into_iter()
            .map(|(_, path)| managed_relative_path(&self.root, Path::new(&path)))
            .collect()
    }

    fn get_index_root(&self, id: &str) -> Result<Option<IndexRoot>, LibraryError> {
        self.conn
            .query_row(
                "SELECT id, path, enabled, last_scanned_at, scan_interval_minutes, max_file_bytes
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
}

const MODEL_SELECT: &str = "SELECT m.id, m.display_name, m.description, m.folder_id,
            COALESCE(r.original_name, d.original_name), COALESCE(r.format, d.format),
            COALESCE(r.bytes, d.bytes, 0), COALESCE(r.blob_hash, d.content_hash),
            b.relative_path, m.metadata_version, m.created_at, m.updated_at,
            m.source_kind,
            CASE WHEN m.source_kind = 'indexed'
                 THEN rtrim(ir.path, '/') || '/' || m.source_path
                 ELSE b.relative_path END,
            m.source_root_id, m.source_missing, m.source_modified_at,
            (SELECT t.relative_path FROM thumbnails t
             WHERE t.model_id = m.id
               AND (t.is_custom = 1 OR t.source_content_hash IS NULL
                    OR t.source_content_hash = COALESCE(r.blob_hash, d.content_hash))
             ORDER BY t.is_custom DESC, t.id LIMIT 1)
     FROM models m
     LEFT JOIN revisions r ON r.id = m.current_revision_id
     LEFT JOIN blobs b ON b.hash = r.blob_hash
     LEFT JOIN indexed_model_data d ON d.model_id = m.id
     LEFT JOIN index_roots ir ON ir.id = m.source_root_id";

const SCHEMA: &str = r#"
CREATE TABLE libraries (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL);
INSERT INTO libraries (id, name, created_at) VALUES ('local', 'Local Library', '1970-01-01T00:00:00Z');
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
    source_modified_at INTEGER
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
CREATE TABLE tags (id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE COLLATE NOCASE);
CREATE TABLE model_tags (
    model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY(model_id, tag_id)
);
CREATE TABLE manuals (model_id TEXT PRIMARY KEY REFERENCES models(id) ON DELETE CASCADE, relative_path TEXT NOT NULL, content_hash TEXT NOT NULL);
CREATE TABLE thumbnails (id TEXT PRIMARY KEY, model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE, relative_path TEXT NOT NULL, content_hash TEXT NOT NULL, is_custom INTEGER NOT NULL DEFAULT 0, source_content_hash TEXT);
CREATE INDEX thumbnails_model_idx ON thumbnails(model_id, is_custom);
CREATE TABLE jobs (id TEXT PRIMARY KEY, model_id TEXT REFERENCES models(id) ON DELETE CASCADE, kind TEXT NOT NULL, status TEXT NOT NULL, retry_count INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
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
        source_kind: row.get(12)?,
        source_path: row.get(13)?,
        source_root_id: row.get(14)?,
        source_missing: row.get(15)?,
        source_modified_at: row.get::<_, Option<i64>>(16)?.map(|value| value as u64),
        thumbnail_path: row.get(17)?,
    })
}

fn index_root_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IndexRoot> {
    Ok(IndexRoot {
        id: row.get(0)?,
        path: PathBuf::from(row.get::<_, String>(1)?),
        enabled: row.get(2)?,
        last_scanned_at: row.get(3)?,
        scan_interval_minutes: row.get::<_, i64>(4)? as u32,
        max_file_bytes: row.get::<_, i64>(5)? as u64,
    })
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

fn managed_relative_path(root: &Path, relative: &Path) -> Result<PathBuf, LibraryError> {
    let relative = validated_relative_path(relative)?;
    Ok(root.join(relative))
}

fn validate_hash(hash: &str) -> Result<(), LibraryError> {
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(LibraryError::InvalidInput(
            "content hash must be a 64-character hexadecimal SHA-256 digest".into(),
        ));
    }
    Ok(())
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
        conn.execute("INSERT INTO models (id, display_name, description, folder_id, current_revision_id, metadata_version, created_at, updated_at) VALUES ('model-v1', 'Tagged Bracket', '', 'root', 'revision-v1', 1, '1', '1')", []).unwrap();
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
            },
            ScannedModel {
                relative_path: "duplicate-a.stl".into(),
                content_hash: hash(2),
                bytes: 9,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: None,
            },
            ScannedModel {
                relative_path: "duplicate-b.stl".into(),
                content_hash: hash(2),
                bytes: 9,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: None,
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
        fs::write(library_root.join(&thumbnail_relative), b"thumbnail").unwrap();
        assert!(matches!(
            db.set_thumbnail_for_source(&unique.id, &hash(8), &hash(9), &thumbnail_relative),
            Err(LibraryError::SourceChanged)
        ));
        db.set_thumbnail_for_source(&unique.id, &hash(1), &hash(9), &thumbnail_relative)
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
        assert_eq!(png_hash, hash(9));
        fs::write(source_root.join("renamed.stl"), b"unique-v2").unwrap();
        let changed = ScannedModel {
            relative_path: "renamed.stl".into(),
            content_hash: hash(3),
            bytes: 9,
            format: "stl-binary".into(),
            modified_at: 3,
            display_name: None,
        };
        db.record_scan_entries(&root.id, std::slice::from_ref(&changed))
            .unwrap();
        assert_eq!(
            db.get_model(&unique.id)
                .unwrap()
                .unwrap()
                .content_hash
                .as_deref(),
            Some(hash(3).as_str())
        );
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
        assert!(!db.get_model(&unique.id).unwrap().unwrap().source_missing);
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
            },
            ScannedModel {
                relative_path: "b.stl".into(),
                content_hash: hash(6),
                bytes: 4,
                format: "stl-binary".into(),
                modified_at: 1,
                display_name: Some("Metadata B".into()),
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
        };
        assert!(db.record_scan_entries(&root.id, &[escaped]).is_err());
        let linked = ScannedModel {
            relative_path: "link.stl".into(),
            content_hash: hash(5),
            bytes: 7,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
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
        fs::write(root.join("thumbnails/generated.png"), b"generated").unwrap();
        fs::write(root.join("thumbnails/custom.png"), b"custom").unwrap();
        db.set_thumbnail(&model.id, &hash(9), Path::new("thumbnails/generated.png"))
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
