#[cfg(test)]
use std::fs;
#[cfg(test)]
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
use library_core::{IndexRoot, ScanReconcile, ScannedModel};
use tauri::Manager;

use crate::library_commands::LibraryState;

#[cfg(test)]
use library_runtime::scanner::{is_model_path, modified_nanos, root_should_scan, scan_file};
pub use library_runtime::scanner::{LibraryScanStatus, ScanControl};

pub fn start_background_scanner(app: tauri::AppHandle) {
    let runtime = app.state::<LibraryState>().runtime().clone();
    library_runtime::scanner::start_background_scanner(&runtime);
}

pub(crate) fn start_manual_scan(
    state: LibraryState,
    root_id: Option<String>,
) -> Result<(), String> {
    library_runtime::scanner::start_manual_scan(state.runtime().clone(), root_id)
}

#[cfg(test)]
fn scan_root(state: &LibraryState, root: &IndexRoot) -> Result<ScanReconcile, String> {
    library_runtime::scanner::scan_root(state.runtime(), root)
}

#[cfg(test)]
fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::{Cursor, Write};
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
    use std::thread;
    use std::time::Duration;

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "gogglelab-scan-{label}-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn triangle_stl() -> Vec<u8> {
        let mut bytes = vec![0_u8; 84];
        bytes[80..84].copy_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&[0_u8; 50]);
        bytes
    }

    fn three_mf_cube() -> Vec<u8> {
        let model = r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
<resources><object id="1" type="model"><mesh><vertices>
<vertex x="0" y="0" z="0"/><vertex x="20" y="0" z="0"/>
<vertex x="20" y="20" z="0"/><vertex x="0" y="20" z="0"/>
<vertex x="0" y="0" z="20"/><vertex x="20" y="0" z="20"/>
<vertex x="20" y="20" z="20"/><vertex x="0" y="20" z="20"/>
</vertices><triangles>
<triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="3" v3="2"/>
<triangle v1="4" v2="5" v3="6"/><triangle v1="4" v2="6" v3="7"/>
<triangle v1="0" v2="1" v3="5"/><triangle v1="0" v2="5" v3="4"/>
<triangle v1="1" v2="2" v3="6"/><triangle v1="1" v2="6" v3="5"/>
<triangle v1="2" v2="3" v3="7"/><triangle v1="2" v2="7" v3="6"/>
<triangle v1="3" v2="0" v3="4"/><triangle v1="3" v2="4" v3="7"/>
</triangles></mesh></object></resources><build><item objectid="1"/></build></model>"#;
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file("3D/3dmodel.model", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(model.as_bytes()).unwrap();
        archive.finish().unwrap().into_inner()
    }

    #[test]
    fn scan_file_does_not_modify_source_timestamp() {
        let root = temp_root("timestamp");
        let path = root.join("fixture.stl");
        fs::write(&path, triangle_stl()).unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        let relative = PathBuf::from("fixture.stl");
        let parsed = scan_file(
            &path,
            relative.clone(),
            134,
            modified_nanos(&fs::metadata(&path).unwrap()),
        )
        .unwrap();
        let after = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(parsed.relative_path, relative);
        assert_eq!(before, after);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn model_extension_matching_is_case_insensitive_and_narrow() {
        assert!(is_model_path(Path::new("part.STL")));
        assert!(is_model_path(Path::new("part.3mf")));
        assert!(!is_model_path(Path::new("part.stl.txt")));
    }

    #[test]
    fn startup_scans_enabled_roots_even_when_interval_is_not_due() {
        let root = IndexRoot {
            id: "root-1".into(),
            path: PathBuf::from("/tmp/example"),
            enabled: true,
            last_scanned_at: Some(now_seconds().to_string()),
            scan_interval_minutes: 60,
            max_file_bytes: 1024,
            scan_zip: false,
        };
        assert!(root_should_scan(&root, true));
        assert!(!root_should_scan(&root, false));
        assert!(!root_should_scan(
            &IndexRoot {
                enabled: false,
                ..root
            },
            true
        ));
    }

    #[test]
    fn cancelling_an_idle_completed_job_does_not_leave_cancelling_set() {
        let control = ScanControl::new();
        assert!(control.try_begin(0, "thumbnails"));
        control.finish();
        control.cancel();
        let status = control.status();
        assert!(!status.running);
        assert!(!status.cancelling);
        assert_eq!(status.job_kind.as_deref(), Some("thumbnails"));
    }

    #[test]
    fn cancelled_scan_does_not_mark_sources_missing_but_completed_scan_does() {
        let catalog = temp_root("cancel-catalog");
        let source = temp_root("cancel-source");
        fs::write(source.join("part.stl"), triangle_stl()).unwrap();
        let state = LibraryState::new(catalog.clone());
        let root = state
            .with_db(|db| {
                db.add_index_root(&source, 60, 1024 * 1024)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        scan_root(&state, &root).unwrap();
        let model_id = state
            .with_db(|db| {
                Ok(db
                    .list_models(None, None)
                    .map_err(super::super::library_commands::library_error)?[0]
                    .id
                    .clone())
            })
            .unwrap();

        fs::remove_file(source.join("part.stl")).unwrap();
        state.scan_control().set_cancelled_for_test(true);
        scan_root(&state, &root).unwrap();
        let missing_after_cancel = state
            .with_db(|db| {
                Ok(db
                    .get_model(&model_id)
                    .map_err(super::super::library_commands::library_error)?
                    .unwrap()
                    .source_missing)
            })
            .unwrap();
        assert!(!missing_after_cancel);

        state.scan_control().set_cancelled_for_test(false);
        scan_root(&state, &root).unwrap();
        let missing_after_complete = state
            .with_db(|db| {
                Ok(db
                    .get_model(&model_id)
                    .map_err(super::super::library_commands::library_error)?
                    .unwrap()
                    .source_missing)
            })
            .unwrap();
        assert!(missing_after_complete);
        let _ = fs::remove_dir_all(catalog);
        let _ = fs::remove_dir_all(source);
    }

    #[test]
    fn same_size_source_update_is_reindexed() {
        let catalog = temp_root("update-catalog");
        let source = temp_root("update-source");
        let path = source.join("part.stl");
        let first = triangle_stl();
        fs::write(&path, &first).unwrap();
        let state = LibraryState::new(catalog.clone());
        let root = state
            .with_db(|db| {
                db.add_index_root(&source, 60, 1024 * 1024)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        scan_root(&state, &root).unwrap();
        let (original_hash, original_thumbnail) = state
            .with_db(|db| {
                let model = db
                    .list_models(None, None)
                    .map_err(super::super::library_commands::library_error)?
                    .remove(0);
                Ok((model.content_hash.unwrap(), model.thumbnail_path.unwrap()))
            })
            .unwrap();

        let mut changed = first;
        changed[100] = 1;
        thread::sleep(Duration::from_millis(2));
        fs::write(&path, changed).unwrap();
        scan_root(&state, &root).unwrap();
        let (changed_hash, changed_thumbnail) = state
            .with_db(|db| {
                let model = db
                    .list_models(None, None)
                    .map_err(super::super::library_commands::library_error)?
                    .remove(0);
                Ok((model.content_hash.unwrap(), model.thumbnail_path.unwrap()))
            })
            .unwrap();
        assert_ne!(original_hash, changed_hash);
        assert_ne!(original_thumbnail, changed_thumbnail);
        let _ = fs::remove_dir_all(catalog);
        let _ = fs::remove_dir_all(source);
    }

    #[test]
    fn failed_file_parse_prevents_missing_reconciliation() {
        let catalog = temp_root("parse-error-catalog");
        let source = temp_root("parse-error-source");
        fs::write(source.join("broken.stl"), triangle_stl()).unwrap();
        fs::write(source.join("removed.stl"), triangle_stl()).unwrap();
        let state = LibraryState::new(catalog.clone());
        let root = state
            .with_db(|db| {
                db.add_index_root(&source, 60, 1024 * 1024)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        scan_root(&state, &root).unwrap();
        fs::write(source.join("broken.stl"), b"not a model").unwrap();
        fs::remove_file(source.join("removed.stl")).unwrap();

        scan_root(&state, &root).unwrap();
        let models = state
            .with_db(|db| {
                db.list_models(None, None)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        assert_eq!(models.len(), 2);
        assert!(models.iter().all(|model| !model.source_missing));
        let _ = fs::remove_dir_all(catalog);
        let _ = fs::remove_dir_all(source);
    }

    #[test]
    fn oversized_changed_source_keeps_scan_incomplete_and_is_refused_for_open() {
        let catalog = temp_root("oversized-catalog");
        let source = temp_root("oversized-source");
        let path = source.join("growing.stl");
        fs::write(&path, triangle_stl()).unwrap();
        let state = LibraryState::new(catalog.clone());
        let root = state
            .with_db(|db| {
                db.add_index_root(&source, 60, 150)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        scan_root(&state, &root).unwrap();
        let (model_id, original_hash, last_scanned_at) = state
            .with_db(|db| {
                let model = db
                    .list_models(None, None)
                    .map_err(super::super::library_commands::library_error)?
                    .remove(0);
                let scanned = db
                    .list_index_roots()
                    .map_err(super::super::library_commands::library_error)?
                    .remove(0)
                    .last_scanned_at;
                Ok((model.id, model.content_hash.unwrap(), scanned))
            })
            .unwrap();
        use std::fs::OpenOptions;
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&[0_u8; 100]).unwrap();
        drop(file);

        scan_root(&state, &root).unwrap();
        state
            .with_db(|db| {
                let model = db
                    .get_model(&model_id)
                    .map_err(super::super::library_commands::library_error)?
                    .unwrap();
                assert_eq!(model.content_hash.as_deref(), Some(original_hash.as_str()));
                let scanned = db
                    .list_index_roots()
                    .map_err(super::super::library_commands::library_error)?
                    .remove(0);
                assert_eq!(scanned.last_scanned_at, last_scanned_at);
                let resolved = db
                    .resolve_model_source_ref(&model_id)
                    .map_err(super::super::library_commands::library_error)?;
                assert!(crate::library_commands::validate_indexed_source_limit(
                    &resolved,
                    Some(root.max_file_bytes)
                )
                .is_err());
                Ok(())
            })
            .unwrap();
        let _ = fs::remove_dir_all(catalog);
        let _ = fs::remove_dir_all(source);
    }

    #[test]
    fn zip_scan_indexes_members_as_one_bundle_and_cleans_thumbnail_extractions() {
        let catalog = temp_root("zip-catalog");
        let source = temp_root("zip-source");
        let archive_path = source.join("bundle.zip");
        let file = fs::File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        for name in ["parts/one.stl", "parts/two.stl"] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(&triangle_stl()).unwrap();
        }
        archive.finish().unwrap();
        let source_modified = fs::metadata(&archive_path).unwrap().modified().unwrap();
        let state = LibraryState::new(catalog.clone());
        let mut root = state
            .with_db(|db| {
                db.add_index_root(&source, 60, 1024 * 1024)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        root.scan_zip = true;
        let root = state
            .with_db(|db| {
                db.update_index_root(&root)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();

        scan_root(&state, &root).unwrap();
        let models = state
            .with_db(|db| {
                db.list_models(None, None)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        assert_eq!(models.len(), 2);
        assert!(models.iter().all(|model| model.bundle_id.is_some()));
        assert_eq!(models[0].bundle_id, models[1].bundle_id);
        assert!(models
            .iter()
            .all(|model| model.source_archive_member.is_some()));
        assert!(models.iter().all(|model| model.thumbnail_path.is_some()));
        assert_eq!(
            source_modified,
            fs::metadata(&archive_path).unwrap().modified().unwrap()
        );
        assert!(!fs::read_dir(catalog.join("staging"))
            .unwrap()
            .flatten()
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with("archive-thumbnail-")));

        let mut paused_root = root.clone();
        paused_root.scan_zip = false;
        let paused_root = state
            .with_db(|db| {
                db.update_index_root(&paused_root)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        fs::remove_file(&archive_path).unwrap();
        scan_root(&state, &paused_root).unwrap();
        let paused_models = state
            .with_db(|db| {
                db.list_models(None, None)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        assert!(paused_models.iter().all(|model| !model.source_missing));

        let mut enabled_root = paused_root;
        enabled_root.scan_zip = true;
        let enabled_root = state
            .with_db(|db| {
                db.update_index_root(&enabled_root)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        scan_root(&state, &enabled_root).unwrap();
        let missing_models = state
            .with_db(|db| {
                db.list_models(None, None)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        assert!(missing_models.iter().all(|model| model.source_missing));
        let _ = fs::remove_dir_all(catalog);
        let _ = fs::remove_dir_all(source);
    }

    #[test]
    fn zip_scan_repairs_a_partial_prior_member_batch() {
        let catalog = temp_root("zip-partial-catalog");
        let source = temp_root("zip-partial-source");
        let archive_path = source.join("bundle.zip");
        let file = fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        for name in ["parts/one.stl", "parts/two.stl"] {
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&triangle_stl()).unwrap();
        }
        writer.finish().unwrap();
        let state = LibraryState::new(catalog.clone());
        let mut root = state
            .with_db(|db| {
                db.add_index_root(&source, 60, 1024 * 1024)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        root.scan_zip = true;
        let root = state
            .with_db(|db| {
                db.update_index_root(&root)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        let archive_scan =
            crate::library_archives::scan_archive(&archive_path, root.max_file_bytes).unwrap();
        let first = &archive_scan.members[0];
        state
            .with_db(|db| {
                db.record_scan_entries(
                    &root.id,
                    &[ScannedModel {
                        relative_path: PathBuf::from("bundle.zip"),
                        content_hash: first.content_hash.clone(),
                        bytes: first.bytes,
                        format: first.format.clone(),
                        modified_at: modified_nanos(&fs::metadata(&archive_path).unwrap()),
                        display_name: first.display_name.clone(),
                        archive_member: Some(first.member_path.clone()),
                        outer_fingerprint: Some(archive_scan.outer_fingerprint.clone()),
                    }],
                )
                .map(|_| ())
                .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        assert_eq!(
            state
                .with_db(|db| db
                    .list_models(None, None)
                    .map_err(super::super::library_commands::library_error))
                .unwrap()
                .len(),
            1
        );

        scan_root(&state, &root).unwrap();
        assert_eq!(
            state
                .with_db(|db| db
                    .list_models(None, None)
                    .map_err(super::super::library_commands::library_error))
                .unwrap()
                .len(),
            2
        );
        let _ = fs::remove_dir_all(catalog);
        let _ = fs::remove_dir_all(source);
    }

    #[test]
    fn zip_scan_renders_and_extracts_a_real_3mf_member_without_mutating_outer_archive() {
        let catalog = temp_root("zip-3mf-catalog");
        let source = temp_root("zip-3mf-source");
        let archive_path = source.join("models.zip");
        let member_bytes = three_mf_cube();
        let file = fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("nested/cube.3mf", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&member_bytes).unwrap();
        writer.finish().unwrap();
        let outer_before = fs::read(&archive_path).unwrap();
        let outer_hash_before = Sha256::digest(&outer_before);
        let outer_modified_before = fs::metadata(&archive_path).unwrap().modified().unwrap();

        let state = LibraryState::new(catalog.clone());
        let mut root = state
            .with_db(|db| {
                db.add_index_root(&source, 60, 100 * 1024 * 1024)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        root.scan_zip = true;
        let root = state
            .with_db(|db| {
                db.update_index_root(&root)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        scan_root(&state, &root).unwrap();

        let model = state
            .with_db(|db| {
                Ok(db
                    .list_models(None, None)
                    .map_err(super::super::library_commands::library_error)?
                    .remove(0))
            })
            .unwrap();
        assert_eq!(model.format.as_deref(), Some("3mf"));
        assert_eq!(
            model.source_archive_member.as_deref(),
            Some("nested/cube.3mf")
        );
        let thumbnail = model.thumbnail_path.as_deref().unwrap();
        let thumbnail_bytes = fs::read(catalog.join(thumbnail)).unwrap();
        assert!(thumbnail_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(thumbnail_bytes.len() > 500);

        let resolved = state
            .with_db(|db| {
                db.resolve_model_source_ref(&model.id)
                    .map_err(super::super::library_commands::library_error)
            })
            .unwrap();
        let cached = crate::library_archives::extract_member_to_cache(
            &catalog.canonicalize().unwrap(),
            &resolved.outer_path,
            resolved.archive_member.as_deref().unwrap(),
            resolved.outer_fingerprint.as_deref().unwrap(),
            &resolved.content_hash,
            resolved.bytes,
        )
        .unwrap();
        assert_eq!(fs::read(&cached).unwrap(), member_bytes);
        let parsed = stl_core::parse_model(&fs::read(cached).unwrap()).unwrap();
        assert_eq!(parsed.format, stl_core::StlFormat::ThreeMf);
        assert_eq!(parsed.triangle_count, 12);
        assert_eq!(
            outer_hash_before.as_slice(),
            Sha256::digest(fs::read(&archive_path).unwrap()).as_slice()
        );
        assert_eq!(
            outer_modified_before,
            fs::metadata(&archive_path).unwrap().modified().unwrap()
        );
        assert!(!fs::read_dir(catalog.join("staging"))
            .unwrap()
            .flatten()
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with("archive-thumbnail-")));
        let _ = fs::remove_dir_all(catalog);
        let _ = fs::remove_dir_all(source);
    }
}
