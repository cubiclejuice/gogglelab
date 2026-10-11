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

#[test]
fn schema_v2_recovery_preserves_indexed_references_without_accessing_source_roots() {
    let (tmp, root) = fixture();
    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.execute_batch(
        "ALTER TABLE thumbnails ADD source_content_hash TEXT;
         ALTER TABLE models ADD source_kind TEXT NOT NULL DEFAULT 'managed';
         ALTER TABLE models ADD source_path TEXT;
         ALTER TABLE models ADD source_root_id TEXT;
         ALTER TABLE models ADD source_missing INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE models ADD source_modified_at INTEGER;
         CREATE TABLE index_roots (id TEXT PRIMARY KEY, path TEXT, enabled INTEGER,
           last_scanned_at TEXT, scan_interval_minutes INTEGER, max_file_bytes INTEGER);
         CREATE TABLE indexed_model_data (model_id TEXT PRIMARY KEY, content_hash TEXT,
           original_name TEXT, format TEXT, bytes INTEGER);
         INSERT INTO index_roots VALUES ('source-1','/unavailable/qa-models',1,NULL,60,52428800);
         INSERT INTO models (id,display_name,description,folder_id,current_revision_id,
           metadata_version,deleted_at,created_at,updated_at,source_kind,source_path,
           source_root_id,source_missing,source_modified_at)
           VALUES ('indexed-1','Indexed gear','keep this','root',NULL,1,NULL,'1','2',
           'indexed','parts/gear.stl','source-1',1,100);
         PRAGMA user_version=2;",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO indexed_model_data VALUES ('indexed-1',?1,'gear.stl','stl-binary',134)",
        [sha(b"external model")],
    )
    .unwrap();
    conn.execute_batch("CREATE TABLE catalog_settings(id TEXT PRIMARY KEY,thumbnail_size INTEGER,thumbnail_background TEXT,thumbnail_concurrency INTEGER,scan_batch_size INTEGER);
        INSERT INTO catalog_settings VALUES ('local',512,'#112233',1,32);").unwrap();
    conn.execute(
        "UPDATE thumbnails SET source_content_hash=?1",
        [sha(ORIGINAL)],
    )
    .unwrap();
    drop(conn);
    let before = fs::read(root.join("library.sqlite3")).unwrap();
    let manifest = inspect_library(&root).unwrap();
    assert_eq!(manifest.library_schema_version, 2);
    assert_eq!(manifest.index_roots.len(), 1);
    assert_eq!(
        manifest.catalog_settings.as_ref().unwrap().thumbnail_size,
        512
    );
    assert_eq!(
        manifest
            .catalog_settings
            .as_ref()
            .unwrap()
            .thumbnail_background,
        "#112233"
    );
    let managed = manifest.models.iter().find(|m| m.id == "model-1").unwrap();
    assert_eq!(managed.thumbnails[0].asset.sha256, sha(THUMBNAIL));
    assert_eq!(
        managed.thumbnails[0].source_content_hash.as_deref(),
        Some(sha(ORIGINAL).as_str())
    );
    let indexed = manifest
        .models
        .iter()
        .find(|m| m.id == "indexed-1")
        .unwrap();
    let source = indexed.indexed_source.as_ref().unwrap();
    assert_eq!(source.relative_path, "parts/gear.stl");
    assert_eq!(source.root_id, "source-1");
    assert!(source.missing);
    assert_eq!(source.bytes, 134);
    let exported = export_library_zip(&root, &[], &tmp.path().join("v2.zip")).unwrap();
    assert_eq!(exported.models.len(), 2);
    assert_eq!(fs::read(root.join("library.sqlite3")).unwrap(), before);
}

#[test]
fn schema_v2_recovery_rejects_unknown_source_kind_and_out_of_range_root() {
    let (_tmp, root) = fixture();
    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.execute_batch(
        "ALTER TABLE models ADD source_kind TEXT NOT NULL DEFAULT 'managed';
         ALTER TABLE models ADD source_path TEXT;
         ALTER TABLE models ADD source_root_id TEXT;
         ALTER TABLE models ADD source_missing INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE models ADD source_modified_at INTEGER;
         ALTER TABLE thumbnails ADD source_content_hash TEXT;
         CREATE TABLE index_roots(id TEXT PRIMARY KEY,path TEXT,enabled INTEGER,last_scanned_at TEXT,scan_interval_minutes INTEGER,max_file_bytes INTEGER);
         CREATE TABLE indexed_model_data(model_id TEXT PRIMARY KEY,content_hash TEXT,original_name TEXT,format TEXT,bytes INTEGER);
         PRAGMA user_version=2;
         UPDATE models SET source_kind='unexpected';"
    ).unwrap();
    assert!(inspect_library(&root).is_err());
    conn.execute("UPDATE models SET source_kind='managed'", [])
        .unwrap();
    conn.execute(
        "INSERT INTO index_roots VALUES ('invalid','/qa/models',1,NULL,60,104857601)",
        [],
    )
    .unwrap();
    assert!(inspect_library(&root).is_err());
}

#[test]
fn schema_v3_recovery_preserves_metadata_history_archive_refs_and_categories() {
    let (tmp, root) = fixture();
    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.execute_batch(
        "ALTER TABLE models ADD source_kind TEXT NOT NULL DEFAULT 'managed';
         ALTER TABLE models ADD source_path TEXT;
         ALTER TABLE models ADD source_root_id TEXT;
         ALTER TABLE models ADD source_missing INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE models ADD source_modified_at INTEGER;
         ALTER TABLE models ADD source_archive_member TEXT;
         ALTER TABLE models ADD designer TEXT;
         ALTER TABLE models ADD source_url TEXT;
         ALTER TABLE models ADD notes TEXT;
         ALTER TABLE models ADD model_license TEXT;
         ALTER TABLE models ADD print_status TEXT NOT NULL DEFAULT 'unprinted';
         ALTER TABLE models ADD parent_model_id TEXT;
         ALTER TABLE models ADD last_printed_at TEXT;
         ALTER TABLE models ADD print_count INTEGER NOT NULL DEFAULT 0;
         ALTER TABLE models ADD bundle_id TEXT;
         ALTER TABLE tags ADD category TEXT;
         ALTER TABLE thumbnails ADD source_content_hash TEXT;
         CREATE TABLE index_roots(id TEXT PRIMARY KEY,path TEXT,enabled INTEGER,last_scanned_at TEXT,scan_interval_minutes INTEGER,max_file_bytes INTEGER,scan_zip INTEGER);
         CREATE TABLE indexed_model_data(model_id TEXT PRIMARY KEY,content_hash TEXT,original_name TEXT,format TEXT,bytes INTEGER,outer_fingerprint TEXT);
         CREATE TABLE bundles(id TEXT PRIMARY KEY,source_root_id TEXT,source_path TEXT,created_at TEXT);
         CREATE TABLE print_attempts(id TEXT PRIMARY KEY,model_id TEXT,printed_at TEXT,quantity INTEGER,notes TEXT,outcome TEXT,created_at TEXT,updated_at TEXT);
         CREATE TABLE print_attempt_filaments(attempt_id TEXT,filament_id TEXT,ordinal INTEGER);
         INSERT INTO index_roots VALUES('r3','/unavailable/archive-models',1,NULL,60,52428800,1);
         INSERT INTO bundles VALUES('bundle3','r3','pack.zip','1');
         INSERT INTO bundles VALUES('manual3',NULL,NULL,'1');
         INSERT INTO models (id,display_name,description,folder_id,current_revision_id,metadata_version,deleted_at,created_at,updated_at,source_kind,source_path,source_root_id,source_missing,source_modified_at,source_archive_member,print_status,print_count,bundle_id)
           VALUES('archive-member','Gear','', 'root',NULL,1,NULL,'1','1','indexed','pack.zip','r3',0,1,'parts/gear.stl','unprinted',0,'bundle3');
         INSERT INTO indexed_model_data VALUES('archive-member','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','gear.stl','stl-binary',84,'zipmeta:test');
         UPDATE models SET designer='Ada',source_url='https://example.test/model',notes='preserve notes',
            model_license='CC0',print_status='queued',last_printed_at='2026-10-10T17:00:00.000Z',
            print_count=2,bundle_id='manual3' WHERE id='model-1';
         UPDATE tags SET category='Prototype';
         INSERT INTO print_attempts VALUES('attempt3','model-1','2026-10-10T17:00:00.000Z',2,'good print','printed','1','1');
         INSERT INTO print_attempt_filaments VALUES('attempt3','PLA-blue',0);
         PRAGMA user_version=3;"
    ).unwrap();
    let boundary_tag = "T".repeat(256);
    let boundary_filament = "F".repeat(256);
    conn.execute("UPDATE tags SET name=?1 WHERE id='tag-1'", [&boundary_tag])
        .unwrap();
    conn.execute(
        "UPDATE print_attempt_filaments SET filament_id=?1 WHERE attempt_id='attempt3'",
        [&boundary_filament],
    )
    .unwrap();
    conn.execute(
        "UPDATE models SET notes=?1 WHERE id='model-1'",
        ["line one\nline two\tindented"],
    )
    .unwrap();
    drop(conn);
    let before = fs::read(root.join("library.sqlite3")).unwrap();
    let manifest = inspect_library(&root).unwrap();
    assert_eq!(manifest.library_schema_version, 3);
    let organized = manifest
        .models
        .iter()
        .find(|model| model.id == "model-1")
        .unwrap();
    assert_eq!(
        organized.organization.as_ref().unwrap().designer.as_deref(),
        Some("Ada")
    );
    assert_eq!(
        organized.organization.as_ref().unwrap().print_status,
        "queued"
    );
    assert_eq!(organized.print_attempts[0].quantity, 2);
    assert_eq!(
        organized.organization.as_ref().unwrap().notes.as_deref(),
        Some("line one\nline two\tindented")
    );
    assert_eq!(
        organized.print_attempts[0].filament_ids,
        [boundary_filament]
    );
    assert_eq!(manifest.tags[0].name, boundary_tag);
    assert_eq!(manifest.tags[0].category.as_deref(), Some("Prototype"));
    assert_eq!(manifest.bundles[0].id, "bundle3");
    assert!(manifest.index_roots[0].scan_zip);
    let archive_model = manifest
        .models
        .iter()
        .find(|model| model.id == "archive-member")
        .unwrap();
    assert_eq!(
        archive_model
            .indexed_source
            .as_ref()
            .unwrap()
            .archive_member
            .as_deref(),
        Some("parts/gear.stl")
    );
    assert_eq!(
        archive_model
            .indexed_source
            .as_ref()
            .unwrap()
            .outer_fingerprint
            .as_deref(),
        Some("zipmeta:test")
    );
    let exported = export_library_zip(&root, &[], &tmp.path().join("v3.zip")).unwrap();
    assert_eq!(
        exported
            .models
            .iter()
            .find(|model| model.id == "model-1")
            .unwrap()
            .print_attempts
            .len(),
        1
    );
    assert_eq!(fs::read(root.join("library.sqlite3")).unwrap(), before);
    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.execute(
        "UPDATE models SET parent_model_id='model-1' WHERE id='model-1'",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(inspect_library(&root).is_err());
    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.execute(
        "UPDATE models SET parent_model_id=NULL WHERE id='model-1'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE print_attempts SET printed_at='not-a-date',quantity=10001 WHERE id='attempt3'",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(inspect_library(&root).is_err());
    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.execute("UPDATE print_attempts SET printed_at='2026-10-10T17:00:00.000Z',quantity=2 WHERE id='attempt3'", []).unwrap();
    conn.execute(
        "UPDATE print_attempt_filaments SET ordinal=2 WHERE attempt_id='attempt3'",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(inspect_library(&root).is_err());
    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.execute(
        "UPDATE print_attempt_filaments SET ordinal=0 WHERE attempt_id='attempt3'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE indexed_model_data SET outer_fingerprint='' WHERE model_id='archive-member'",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(inspect_library(&root).is_err());
    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.execute("UPDATE indexed_model_data SET outer_fingerprint='zipmeta:test' WHERE model_id='archive-member'", []).unwrap();
    conn.execute(
        "UPDATE bundles SET source_path='wrong.zip' WHERE id='bundle3'",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(inspect_library(&root).is_err());
    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.execute(
        "UPDATE bundles SET source_root_id=NULL,source_path=NULL WHERE id='bundle3'",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(inspect_library(&root).is_ok());
}

#[test]
fn schema_v4_recovery_preserves_catalog_assignments_sources_and_unknown_history_ids() {
    use library_core::{
        FilamentInput, LibraryDb, PrintAttemptInput, PrintOutcome, SpoolmanFilamentRecord,
        SpoolmanSpoolRecord,
    };

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library-v4");
    let db = LibraryDb::open(&root).unwrap();
    let model = db.create_model("Filament recovery", None).unwrap();
    let manual = db
        .create_filament(&FilamentInput {
            brand: "Local".into(),
            material: "PETG".into(),
            color: "#00AAFF".into(),
            name: "Ocean".into(),
            notes: "line one\nline two".into(),
        })
        .unwrap();
    let source = db
        .upsert_spoolman_source("http://spoolman.local/api/v1")
        .unwrap();
    db.sync_spoolman_snapshot(
        &source.id,
        &[SpoolmanFilamentRecord {
            external_id: "1".into(),
            brand: "Remote".into(),
            material: "PLA".into(),
            color: "FF0000".into(),
            name: "Red".into(),
            notes: String::new(),
        }],
        &[SpoolmanSpoolRecord {
            external_id: "9".into(),
            filament_external_id: "1".into(),
            brand: "Remote".into(),
            material: "PLA".into(),
            color: "FF0000".into(),
            name: "Red".into(),
            notes: "Shelf A".into(),
            archived: true,
        }],
        "2026-10-10T12:00:00Z",
    )
    .unwrap();
    let remote = db.list_filaments(true).unwrap();
    let spool = remote
        .iter()
        .find(|filament| filament.external_id.as_deref() == Some("9"))
        .unwrap();
    let model = db
        .set_model_filaments(
            &model.id,
            model.metadata_version,
            &[manual.id.clone(), spool.id.clone()],
        )
        .unwrap();
    db.create_print_attempt(
        &model.id,
        &PrintAttemptInput {
            printed_at: "2026-10-10T12:30:00Z".into(),
            quantity: 1,
            notes: String::new(),
            filament_ids: vec![manual.id.clone(), "legacy-unknown".into()],
            outcome: PrintOutcome::Printed,
        },
    )
    .unwrap();
    assert!(
        db.delete_filament(&manual.id, manual.version)
            .unwrap()
            .unwrap()
            .archived
    );
    drop(db);

    let manifest = inspect_library(&root).unwrap();
    assert_eq!(manifest.library_schema_version, 4);
    assert_eq!(manifest.filament_sources.len(), 1);
    assert_eq!(manifest.filaments.len(), 3);
    assert!(
        manifest
            .filaments
            .iter()
            .find(|filament| filament.id == manual.id)
            .unwrap()
            .archived
    );
    let recovered = manifest
        .models
        .iter()
        .find(|item| item.id == model.id)
        .unwrap();
    assert_eq!(
        recovered.filament_ids,
        [manual.id.clone(), spool.id.clone()]
    );
    assert_eq!(
        recovered.print_attempts[0].filament_ids,
        [manual.id.clone(), "legacy-unknown".into()]
    );
    let selected = export_library_zip(&root, &[model.id], &tmp.path().join("v4.zip")).unwrap();
    assert_eq!(selected.filaments.len(), 3);
    assert_eq!(selected.filament_sources.len(), 1);

    let conn = Connection::open(root.join("library.sqlite3")).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
    conn.execute(
        "UPDATE model_filaments SET ordinal=4 WHERE model_id=?1 AND ordinal=1",
        [&recovered.id],
    )
    .unwrap();
    assert!(inspect_library(&root).is_err());
    conn.execute(
        "UPDATE model_filaments SET ordinal=1 WHERE model_id=?1 AND ordinal=4",
        [&recovered.id],
    )
    .unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints=ON;")
        .unwrap();
    conn.execute("UPDATE filaments SET version=0 WHERE id=?1", [&manual.id])
        .unwrap();
    assert!(inspect_library(&root).is_err());
    conn.execute("UPDATE filaments SET version=1 WHERE id=?1", [&manual.id])
        .unwrap();
    conn.execute_batch("PRAGMA ignore_check_constraints=OFF;")
        .unwrap();
    conn.execute(
        "UPDATE model_filaments SET filament_id='unknown-filament' WHERE model_id=?1 AND filament_id=?2",
        params![recovered.id, manual.id],
    )
    .unwrap();
    assert!(inspect_library(&root).is_err());
    conn.execute(
        "UPDATE model_filaments SET filament_id=?1 WHERE model_id=?2 AND filament_id='unknown-filament'",
        params![manual.id, recovered.id],
    )
    .unwrap();
    conn.execute(
        "UPDATE filaments SET parent_filament_id=?1 WHERE id=?2",
        params![manual.id, spool.id],
    )
    .unwrap();
    assert!(inspect_library(&root).is_err());
    conn.execute(
        "UPDATE filaments SET parent_filament_id=(SELECT id FROM filaments WHERE resource_kind='filament' LIMIT 1) WHERE id=?1",
        [&spool.id],
    )
    .unwrap();
    conn.execute(
        "UPDATE filament_sources SET base_url='http://secret@example.test'",
        [],
    )
    .unwrap();
    assert!(inspect_library(&root).is_err());
    conn.execute(
        "UPDATE filament_sources SET base_url='http://spoolman.local/api/v1'",
        [],
    )
    .unwrap();
    conn.execute_batch(
        "PRAGMA ignore_check_constraints=ON;
         INSERT INTO filaments(id,version,brand,material,color,name,notes,origin,resource_kind,archived,source_missing,created_at,updated_at)
           VALUES('extra-1',1,'','','','Extra 1','','manual','manual',0,0,'1','1'),
                 ('extra-2',1,'','','','Extra 2','','manual','manual',0,0,'1','1'),
                 ('extra-3',1,'','','','Extra 3','','manual','manual',0,0,'1','1'),
                 ('extra-4',1,'','','','Extra 4','','manual','manual',0,0,'1','1');",
    )
    .unwrap();
    for (ordinal, id) in [
        (2, "extra-1"),
        (3, "extra-2"),
        (4, "extra-3"),
        (5, "extra-4"),
    ] {
        conn.execute(
            "INSERT INTO model_filaments(model_id,filament_id,ordinal) VALUES(?1,?2,?3)",
            params![recovered.id, id, ordinal],
        )
        .unwrap();
    }
    assert!(inspect_library(&root).is_err());
}

#[test]
fn recovery_accepts_core_created_folder_bundle_without_reading_external_sources() {
    use library_core::{LibraryDb, ScannedModel, SourceRef};
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let source = tmp.path().join("source");
    fs::create_dir_all(source.join("folder")).unwrap();
    fs::write(source.join("folder/a.stl"), b"a").unwrap();
    fs::write(source.join("folder/b.stl"), b"b").unwrap();
    fs::write(source.join("folder/c.stl"), b"c").unwrap();
    fs::write(source.join("folder/d.stl"), b"d").unwrap();
    let db = LibraryDb::open(&library).unwrap();
    let root = db.add_index_root(&source, 60, 1024).unwrap();
    let entries = [
        ScannedModel {
            relative_path: "folder/a.stl".into(),
            content_hash: "11".repeat(32),
            bytes: 1,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        },
        ScannedModel {
            relative_path: "folder/c.stl".into(),
            content_hash: "33".repeat(32),
            bytes: 1,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        },
        ScannedModel {
            relative_path: "folder/d.stl".into(),
            content_hash: "44".repeat(32),
            bytes: 1,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        },
        ScannedModel {
            relative_path: "folder/b.stl".into(),
            content_hash: "22".repeat(32),
            bytes: 1,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        },
    ];
    let added = db.record_scan_entries(&root.id, &entries).unwrap();
    let seen = entries
        .iter()
        .map(|entry| SourceRef {
            relative_path: entry.relative_path.clone(),
            archive_member: None,
        })
        .collect::<Vec<_>>();
    db.complete_scan_sources_with_new_models(&root.id, &seen, &added.added_model_ids, "1")
        .unwrap();
    let old_bundle = db.list_models(None, None).unwrap()[0]
        .bundle_id
        .clone()
        .unwrap();
    fs::create_dir_all(source.join("split")).unwrap();
    fs::rename(source.join("folder/c.stl"), source.join("split/c.stl")).unwrap();
    fs::rename(source.join("folder/d.stl"), source.join("split/d.stl")).unwrap();
    let split_entries = [
        ScannedModel {
            relative_path: "folder/a.stl".into(),
            content_hash: "11".repeat(32),
            bytes: 1,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        },
        ScannedModel {
            relative_path: "folder/b.stl".into(),
            content_hash: "22".repeat(32),
            bytes: 1,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        },
        ScannedModel {
            relative_path: "split/c.stl".into(),
            content_hash: "33".repeat(32),
            bytes: 1,
            format: "stl-binary".into(),
            modified_at: 2,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        },
        ScannedModel {
            relative_path: "split/d.stl".into(),
            content_hash: "44".repeat(32),
            bytes: 1,
            format: "stl-binary".into(),
            modified_at: 2,
            display_name: None,
            archive_member: None,
            outer_fingerprint: None,
        },
    ];
    let batch = db.record_scan_entries(&root.id, &split_entries).unwrap();
    let split_seen = split_entries
        .iter()
        .map(|entry| SourceRef {
            relative_path: entry.relative_path.clone(),
            archive_member: None,
        })
        .collect::<Vec<_>>();
    db.complete_scan_sources_with_new_models(&root.id, &split_seen, &batch.added_model_ids, "2")
        .unwrap();
    drop(db);
    fs::remove_dir_all(&source).unwrap();
    let manifest = inspect_library(&library).unwrap();
    let bundle_ids = manifest
        .models
        .iter()
        .map(|model| model.organization.as_ref().unwrap().bundle_id.clone())
        .collect::<Vec<_>>();
    assert!(bundle_ids[0].is_some());
    let folder_models = manifest
        .models
        .iter()
        .filter(|model| {
            model
                .indexed_source
                .as_ref()
                .unwrap()
                .relative_path
                .starts_with("folder/")
        })
        .collect::<Vec<_>>();
    let split_models = manifest
        .models
        .iter()
        .filter(|model| {
            model
                .indexed_source
                .as_ref()
                .unwrap()
                .relative_path
                .starts_with("split/")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        folder_models[0].organization.as_ref().unwrap().bundle_id,
        folder_models[1].organization.as_ref().unwrap().bundle_id
    );
    assert_eq!(
        folder_models[0]
            .organization
            .as_ref()
            .unwrap()
            .bundle_id
            .as_deref(),
        Some(old_bundle.as_str())
    );
    assert_eq!(
        split_models[0].organization.as_ref().unwrap().bundle_id,
        split_models[1].organization.as_ref().unwrap().bundle_id
    );
    assert_ne!(
        folder_models[0].organization.as_ref().unwrap().bundle_id,
        split_models[0].organization.as_ref().unwrap().bundle_id
    );
    let bundle = manifest
        .bundles
        .iter()
        .find(|bundle| bundle.id == old_bundle)
        .unwrap();
    assert_eq!(bundle.source_path.as_deref(), Some("folder/"));
}

#[test]
fn recovery_accepts_core_reconciled_archive_rename_with_stable_bundle() {
    use library_core::{LibraryDb, ScannedModel, SourceRef};
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let source = tmp.path().join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("pack.zip"), b"one").unwrap();
    let db = LibraryDb::open(&library).unwrap();
    let root = db.add_index_root(&source, 60, 1024).unwrap();
    let first = ScannedModel {
        relative_path: "pack.zip".into(),
        content_hash: "33".repeat(32),
        bytes: 84,
        format: "stl-binary".into(),
        modified_at: 1,
        display_name: None,
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
    fs::rename(source.join("pack.zip"), source.join("renamed.zip")).unwrap();
    let renamed = ScannedModel {
        relative_path: "renamed.zip".into(),
        modified_at: 2,
        outer_fingerprint: Some("fp2".into()),
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
    drop(db);
    fs::remove_dir_all(&source).unwrap();
    let manifest = inspect_library(&library).unwrap();
    let model = manifest
        .models
        .iter()
        .find(|model| model.id == original.id)
        .unwrap();
    assert_eq!(
        model.organization.as_ref().unwrap().bundle_id.as_deref(),
        Some(bundle.as_str())
    );
    let bundle = manifest
        .bundles
        .iter()
        .find(|item| item.id == bundle)
        .unwrap();
    assert_eq!(bundle.source_path.as_deref(), Some("renamed.zip"));
}

#[test]
fn recovery_accepts_partial_archive_move_with_missing_old_member_and_split_bundles() {
    use library_core::{LibraryDb, ScannedModel, SourceRef};
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let source = tmp.path().join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("old.zip"), b"old").unwrap();
    fs::write(source.join("new.zip"), b"new").unwrap();
    let db = LibraryDb::open(&library).unwrap();
    let root = db.add_index_root(&source, 60, 1024).unwrap();
    let old = [
        ScannedModel {
            relative_path: "old.zip".into(),
            content_hash: "55".repeat(32),
            bytes: 84,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: Some("a.stl".into()),
            outer_fingerprint: Some("oldfp".into()),
        },
        ScannedModel {
            relative_path: "old.zip".into(),
            content_hash: "66".repeat(32),
            bytes: 84,
            format: "stl-binary".into(),
            modified_at: 1,
            display_name: None,
            archive_member: Some("b.stl".into()),
            outer_fingerprint: Some("oldfp".into()),
        },
    ];
    let added = db.record_scan_entries(&root.id, &old).unwrap();
    let seen = old
        .iter()
        .map(|entry| SourceRef {
            relative_path: entry.relative_path.clone(),
            archive_member: entry.archive_member.clone(),
        })
        .collect::<Vec<_>>();
    db.complete_scan_sources_with_new_models(&root.id, &seen, &added.added_model_ids, "1")
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
        content_hash: "55".repeat(32),
        bytes: 84,
        format: "stl-binary".into(),
        modified_at: 2,
        display_name: None,
        archive_member: Some("a.stl".into()),
        outer_fingerprint: Some("newfp".into()),
    };
    let batch = db
        .record_scan_entries(&root.id, std::slice::from_ref(&moved))
        .unwrap();
    db.complete_scan_sources_with_new_models(
        &root.id,
        &[SourceRef {
            relative_path: "new.zip".into(),
            archive_member: Some("a.stl".into()),
        }],
        &batch.added_model_ids,
        "2",
    )
    .unwrap();
    drop(db);
    fs::remove_dir_all(&source).unwrap();
    let manifest = inspect_library(&library).unwrap();
    let a = manifest.models.iter().find(|m| m.id == a_id).unwrap();
    let b = manifest.models.iter().find(|m| m.id == b_id).unwrap();
    assert_ne!(
        a.organization.as_ref().unwrap().bundle_id.as_deref(),
        Some(old_bundle.as_str())
    );
    assert_eq!(
        b.organization.as_ref().unwrap().bundle_id.as_deref(),
        Some(old_bundle.as_str())
    );
    assert!(b.indexed_source.as_ref().unwrap().missing);
    let exported = export_library_zip(&library, &[], &tmp.path().join("partial.zip")).unwrap();
    assert_eq!(exported.models.len(), 2);
}
