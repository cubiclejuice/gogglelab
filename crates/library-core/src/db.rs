use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{io, LibraryError};
use crate::models::{Folder, ImportResult, Model};

pub const CURRENT_SCHEMA_VERSION: i64 = 1;
pub const ROOT_FOLDER_ID: &str = "root";

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
            let mut model = row?;
            model.tags = self.tags_for(&model.id)?;
            models.push(model);
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
}

const MODEL_SELECT: &str = "SELECT m.id, m.display_name, m.description, m.folder_id,
            r.original_name, r.format, COALESCE(r.bytes, 0), r.blob_hash,
            CASE WHEN b.relative_path IS NULL THEN NULL ELSE b.relative_path END,
            m.metadata_version, m.created_at, m.updated_at
     FROM models m
     LEFT JOIN revisions r ON r.id = m.current_revision_id
     LEFT JOIN blobs b ON b.hash = r.blob_hash";

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
    updated_at TEXT NOT NULL
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
CREATE TABLE thumbnails (id TEXT PRIMARY KEY, model_id TEXT NOT NULL REFERENCES models(id) ON DELETE CASCADE, relative_path TEXT NOT NULL, content_hash TEXT NOT NULL, is_custom INTEGER NOT NULL DEFAULT 0);
CREATE TABLE jobs (id TEXT PRIMARY KEY, model_id TEXT REFERENCES models(id) ON DELETE CASCADE, kind TEXT NOT NULL, status TEXT NOT NULL, retry_count INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("gogglelab-library-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
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
    fn existing_schema_v1_catalog_reopens_without_migration_or_data_loss() {
        let root = temp_root("existing-v1");
        let source_root = temp_root("existing-v1-source");
        fs::create_dir_all(&source_root).unwrap();
        let source = source_root.join("Tagged Bracket.stl");
        let source_bytes = stl_fixture();
        fs::write(&source, &source_bytes).unwrap();

        let db = LibraryDb::open(&root).unwrap();
        let folder = db.create_folder("Existing Projects", None).unwrap();
        let imported = db
            .import_file(&source, Some(&folder.id), Some("Tagged Bracket"))
            .unwrap();
        let model_id = imported.model.id.clone();
        let content_hash = imported.model.content_hash.clone().unwrap();
        let storage_path = imported.model.storage_path.clone().unwrap();
        drop(db);

        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        conn.execute(
            "INSERT INTO tags (id, name) VALUES ('tag-existing', 'Favorite')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO model_tags (model_id, tag_id) VALUES (?1, 'tag-existing')",
            params![model_id],
        )
        .unwrap();
        drop(conn);

        let db = LibraryDb::open(&root).unwrap();
        let existing = db.get_model(&model_id).unwrap().unwrap();
        assert_eq!(existing.id, model_id);
        assert_eq!(existing.display_name, "Tagged Bracket");
        assert_eq!(existing.folder_id, folder.id);
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
        assert_eq!(fs::read(root.join(storage_path)).unwrap(), source_bytes);
        drop(db);

        let conn = Connection::open(root.join("library.sqlite3")).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(source_root);
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
