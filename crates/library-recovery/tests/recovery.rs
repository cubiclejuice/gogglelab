use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;

use library_recovery::{export_library_zip, export_raw_colorway_json, inspect_library};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use zip::ZipArchive;

const ORIGINAL: &[u8] = b"original model bytes\x00\xff";
const MANUAL: &[u8] = b"manual bytes";
const THUMBNAIL: &[u8] = b"thumbnail bytes";

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn fixture() -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    fs::create_dir(&root).unwrap();
    for directory in ["originals", "manuals", "thumbnails"] {
        fs::create_dir(root.join(directory)).unwrap();
    }
    fs::write(root.join("originals/model.stl"), ORIGINAL).unwrap();
    fs::write(root.join("manuals/manual.pdf"), MANUAL).unwrap();
    fs::write(root.join("thumbnails/thumb.png"), THUMBNAIL).unwrap();

    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.execute_batch(
        "CREATE TABLE libraries (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL);
         CREATE TABLE folders (id TEXT PRIMARY KEY, name TEXT NOT NULL, parent_id TEXT);
         CREATE TABLE blobs (hash TEXT PRIMARY KEY, relative_path TEXT NOT NULL, bytes INTEGER NOT NULL);
         CREATE TABLE models (id TEXT PRIMARY KEY, display_name TEXT NOT NULL, description TEXT NOT NULL,
           folder_id TEXT NOT NULL, current_revision_id TEXT, metadata_version INTEGER NOT NULL,
           deleted_at TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
         CREATE TABLE revisions (id TEXT PRIMARY KEY, model_id TEXT NOT NULL, blob_hash TEXT NOT NULL,
           original_name TEXT NOT NULL, format TEXT NOT NULL, bytes INTEGER NOT NULL, created_at TEXT NOT NULL);
         CREATE TABLE tags (id TEXT PRIMARY KEY, name TEXT NOT NULL);
         CREATE TABLE model_tags (model_id TEXT NOT NULL, tag_id TEXT NOT NULL);
         CREATE TABLE manuals (model_id TEXT PRIMARY KEY, relative_path TEXT NOT NULL, content_hash TEXT NOT NULL);
         CREATE TABLE thumbnails (id TEXT PRIMARY KEY, model_id TEXT NOT NULL, relative_path TEXT NOT NULL,
           content_hash TEXT NOT NULL, is_custom INTEGER NOT NULL);
         CREATE TABLE jobs (id TEXT PRIMARY KEY, model_id TEXT, kind TEXT NOT NULL, status TEXT NOT NULL,
           retry_count INTEGER NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
         PRAGMA user_version = 1;
         INSERT INTO libraries VALUES ('local', 'Local Library', '1970-01-01T00:00:00Z');
         INSERT INTO folders VALUES ('root', 'All Models', NULL);
         INSERT INTO folders VALUES ('folder-1', 'Projects', 'root');
         INSERT INTO models VALUES ('model-1', 'Bracket', 'first model', 'folder-1', 'revision-1', 3,
           '2026-01-01', '2025-01-01', '2026-01-01');
         INSERT INTO tags VALUES ('tag-1', 'Prototype');
         INSERT INTO model_tags VALUES ('model-1', 'tag-1');",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO blobs VALUES (?1, 'originals/model.stl', ?2)",
        params![sha(ORIGINAL), ORIGINAL.len() as i64],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO revisions VALUES ('revision-1', 'model-1', ?1, 'bracket.stl', 'stl-binary', ?2, '2025-01-01')",
        params![sha(ORIGINAL), ORIGINAL.len() as i64],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO manuals VALUES ('model-1', 'manuals/manual.pdf', ?1)",
        params![sha(MANUAL)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO thumbnails VALUES ('thumb-1', 'model-1', 'thumbnails/thumb.png', ?1, 1)",
        params![sha(THUMBNAIL)],
    )
    .unwrap();
    drop(conn);
    (tmp, root)
}

fn zip_bytes(archive: &mut ZipArchive<File>, name: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    archive
        .by_name(name)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

#[test]
fn exports_complete_metadata_and_exact_verified_assets_without_touching_database() {
    let (tmp, root) = fixture();
    let database = root.join("library.sqlite3");
    let before = fs::read(&database).unwrap();
    let destination = tmp.path().join("recovered.zip");

    let manifest = export_library_zip(&root, &["model-1".into()], &destination).unwrap();
    assert_eq!(manifest.manifest_version, 1);
    assert_eq!(manifest.library_schema_version, 1);
    assert_eq!(manifest.models.len(), 1);
    let model = &manifest.models[0];
    assert_eq!(model.source_identity, "library:model-1");
    assert_eq!(model.revisions[0].original.sha256, sha(ORIGINAL));

    let mut archive = ZipArchive::new(File::open(destination).unwrap()).unwrap();
    let original_path = model.revisions[0]
        .original
        .exported_path
        .as_deref()
        .unwrap();
    assert_eq!(zip_bytes(&mut archive, original_path), ORIGINAL);
    let manifest_json: serde_json::Value =
        serde_json::from_slice(&zip_bytes(&mut archive, "manifest.json")).unwrap();
    assert_eq!(manifest_json["models"][0]["deleted_at"], "2026-01-01");
    assert_eq!(
        manifest_json["models"][0]["current_revision_id"],
        "revision-1"
    );
    assert_eq!(manifest_json["models"][0]["manual"]["sha256"], sha(MANUAL));
    assert_eq!(
        manifest_json["models"][0]["thumbnails"][0]["asset"]["sha256"],
        sha(THUMBNAIL)
    );
    assert_eq!(manifest_json["model_tags"][0]["tag_id"], "tag-1");
    let manual_path = manifest_json["models"][0]["manual"]["exported_path"]
        .as_str()
        .unwrap();
    let thumbnail_path = manifest_json["models"][0]["thumbnails"][0]["asset"]["exported_path"]
        .as_str()
        .unwrap();
    assert_eq!(zip_bytes(&mut archive, manual_path), MANUAL);
    assert_eq!(zip_bytes(&mut archive, thumbnail_path), THUMBNAIL);
    assert_eq!(fs::read(&database).unwrap(), before);
    let conn =
        Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn rejects_unknown_or_incomplete_schema_without_creating_output() {
    let (tmp, root) = fixture();
    let destination = tmp.path().join("recovered.zip");
    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.pragma_update(None, "user_version", 2).unwrap();
    drop(conn);
    assert!(inspect_library(&root).is_err());
    assert!(export_library_zip(&root, &["model-1".into()], &destination).is_err());
    assert!(!destination.exists());

    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.pragma_update(None, "user_version", 1).unwrap();
    conn.execute_batch(
        "DROP TABLE thumbnails;
         CREATE TABLE thumbnails (id TEXT PRIMARY KEY, model_id TEXT, relative_path TEXT, content_hash TEXT);",
    )
    .unwrap();
    drop(conn);
    assert!(inspect_library(&root).is_err());
    assert!(!destination.exists());
}

#[test]
fn rejects_traversal_and_does_not_replace_existing_destination() {
    let (tmp, root) = fixture();
    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.execute("UPDATE blobs SET relative_path = '../outside.stl'", [])
        .unwrap();
    drop(conn);
    let destination = tmp.path().join("recovered.zip");
    assert!(export_library_zip(&root, &["model-1".into()], &destination).is_err());
    assert!(!destination.exists());

    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.execute("UPDATE blobs SET relative_path = 'originals/model.stl'", [])
        .unwrap();
    drop(conn);
    fs::write(&destination, b"keep me").unwrap();
    assert!(export_library_zip(&root, &["model-1".into()], &destination).is_err());
    assert_eq!(fs::read(destination).unwrap(), b"keep me");
}

#[cfg(unix)]
#[test]
fn rejects_symlink_that_escapes_library_root() {
    let (tmp, root) = fixture();
    let outside = tmp.path().join("outside.stl");
    fs::write(&outside, ORIGINAL).unwrap();
    fs::remove_file(root.join("originals/model.stl")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("originals/model.stl")).unwrap();
    let destination = tmp.path().join("recovered.zip");
    assert!(export_library_zip(&root, &["model-1".into()], &destination).is_err());
    assert!(!destination.exists());
}

#[test]
fn raw_colorway_export_keeps_malformed_json_bytes_and_refuses_overwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let destination = tmp.path().join("colorways.json");
    let raw = "{\"name\":\"é\",invalid";
    assert_eq!(
        export_raw_colorway_json(raw, &destination).unwrap(),
        raw.len() as u64
    );
    assert_eq!(fs::read(&destination).unwrap(), raw.as_bytes());
    assert!(export_raw_colorway_json("replacement", &destination).is_err());
    assert_eq!(fs::read(destination).unwrap(), raw.as_bytes());
}

#[test]
fn refuses_colorway_payload_above_limit_without_creating_file() {
    let tmp = tempfile::tempdir().unwrap();
    let destination = tmp.path().join("colorways.json");
    assert!(export_raw_colorway_json(&"x".repeat(8 * 1024 * 1024 + 1), &destination).is_err());
    assert!(!destination.exists());
}

#[test]
fn selected_export_is_independent_of_an_unselected_missing_asset() {
    let (tmp, root) = fixture();
    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO models VALUES ('model-2', 'Missing', '', 'root', 'revision-2', 1,
           NULL, '2025-01-01', '2025-01-01');
         INSERT INTO blobs VALUES ('0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
           'originals/missing.stl', 3);
         INSERT INTO revisions VALUES ('revision-2', 'model-2',
           '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
           'missing.stl', 'stl-binary', 3, '2025-01-01');",
    ).unwrap();
    drop(conn);
    assert!(inspect_library(&root).is_err());
    let destination = tmp.path().join("selected.zip");
    let manifest = export_library_zip(&root, &["model-1".into()], &destination).unwrap();
    assert_eq!(manifest.models.len(), 1);
    assert_eq!(manifest.models[0].id, "model-1");
}

#[test]
fn rejects_corrupt_original_hash_and_removes_incomplete_archive() {
    let (tmp, root) = fixture();
    fs::write(root.join("originals/model.stl"), vec![b'x'; ORIGINAL.len()]).unwrap();
    let destination = tmp.path().join("corrupt.zip");
    assert!(export_library_zip(&root, &["model-1".into()], &destination).is_err());
    assert!(!destination.exists());
}

#[test]
fn repeated_exports_have_identical_zip_bytes() {
    let (tmp, root) = fixture();
    let first = tmp.path().join("first.zip");
    let second = tmp.path().join("second.zip");
    export_library_zip(&root, &["model-1".into()], &first).unwrap();
    export_library_zip(&root, &["model-1".into()], &second).unwrap();
    assert_eq!(fs::read(first).unwrap(), fs::read(second).unwrap());
}

#[test]
fn live_wal_reader_uses_committed_snapshot_without_changing_database() {
    let (_tmp, root) = fixture();
    let database = root.join("library.sqlite3");
    let writer = Connection::open(&database).unwrap();
    writer.pragma_update(None, "journal_mode", "WAL").unwrap();
    writer
        .execute_batch(
            "BEGIN IMMEDIATE;
         UPDATE models SET display_name = 'Uncommitted Name' WHERE id = 'model-1';",
        )
        .unwrap();
    let before = fs::read(&database).unwrap();

    let catalog = inspect_library(&root).unwrap();
    assert_eq!(catalog.models[0].display_name, "Bracket");
    assert_eq!(fs::read(&database).unwrap(), before);
    assert_eq!(
        writer
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    writer.execute_batch("ROLLBACK").unwrap();
}
