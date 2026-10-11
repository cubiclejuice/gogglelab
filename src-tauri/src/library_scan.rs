use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Weak};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use library_core::{IndexRoot, ScanReconcile, ScannedModel, SourceRef};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::Manager;

use crate::extension_api::AppError;
use crate::library_commands::{LibraryState, LibraryStateInner};

const MAX_SCAN_FILES: usize = 250_000;
const MAX_REPORTED_ERRORS: usize = 100;
const BACKGROUND_POLL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryScanStatus {
    pub running: bool,
    pub cancelling: bool,
    pub job_kind: Option<String>,
    pub current_root_id: Option<String>,
    pub roots_total: usize,
    pub roots_completed: usize,
    pub files_visited: usize,
    pub files_indexed: usize,
    pub thumbnails_total: usize,
    pub thumbnails_completed: usize,
    pub errors: Vec<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

pub(crate) struct ScanControl {
    status: Mutex<LibraryScanStatus>,
    cancel: AtomicBool,
}

impl ScanControl {
    pub(crate) fn new() -> Self {
        Self {
            status: Mutex::new(LibraryScanStatus::default()),
            cancel: AtomicBool::new(false),
        }
    }

    pub(crate) fn status(&self) -> LibraryScanStatus {
        self.status.lock().unwrap().clone()
    }

    pub(crate) fn cancel(&self) {
        let mut status = self.status.lock().unwrap();
        if status.running {
            self.cancel.store(true, Ordering::Release);
            status.cancelling = true;
        }
    }

    pub(crate) fn try_begin(&self, roots_total: usize, job_kind: &str) -> bool {
        let mut status = self.status.lock().unwrap();
        if status.running {
            return false;
        }
        self.cancel.store(false, Ordering::Release);
        *status = LibraryScanStatus {
            running: true,
            job_kind: Some(job_kind.into()),
            roots_total,
            started_at: Some(timestamp()),
            ..LibraryScanStatus::default()
        };
        true
    }

    pub(crate) fn finish(&self) {
        let mut status = self.status.lock().unwrap();
        status.running = false;
        status.cancelling = false;
        status.current_root_id = None;
        status.finished_at = Some(timestamp());
    }

    pub(crate) fn failed_to_spawn(&self, message: String) {
        self.error(message);
        self.finish();
    }

    pub(crate) fn error(&self, message: String) {
        let mut status = self.status.lock().unwrap();
        if status.errors.len() < MAX_REPORTED_ERRORS {
            status.errors.push(message);
        } else if status.errors.len() == MAX_REPORTED_ERRORS {
            status
                .errors
                .push("Additional scan errors were omitted".into());
        }
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    pub(crate) fn set_job_kind(&self, kind: &str) {
        self.status.lock().unwrap().job_kind = Some(kind.into());
    }

    pub(crate) fn set_thumbnail_progress(&self, total: usize) {
        let mut status = self.status.lock().unwrap();
        status.thumbnails_total = total;
        status.thumbnails_completed = 0;
    }

    pub(crate) fn thumbnail_completed(&self) {
        self.status.lock().unwrap().thumbnails_completed += 1;
    }
}

pub fn start_background_scanner(app: tauri::AppHandle) {
    let weak = {
        let state = app.state::<LibraryState>();
        state.weak_inner()
    };
    let error_state = weak.clone();
    if let Err(error) = thread::Builder::new()
        .name("gogglelab-library-scheduler".into())
        .spawn(move || background_loop(weak))
    {
        if let Some(inner) = error_state.upgrade() {
            inner
                .scan
                .error(format!("Could not start background scanner: {error}"));
        }
    }
}

fn background_loop(state: Weak<LibraryStateInner>) {
    let mut startup = true;
    while let Some(inner) = state.upgrade() {
        let library = LibraryState::from_inner(inner);
        let roots = library
            .with_db(|db| {
                db.list_index_roots()
                    .map_err(super::library_commands::library_error)
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|root| root_should_scan(root, startup))
            .map(|root| root.id)
            .collect::<Vec<_>>();
        if !roots.is_empty() {
            let _ = start_scan(library.clone(), None, Some(roots));
        }
        startup = false;
        drop(library);

        let mut waited = Duration::ZERO;
        while waited < BACKGROUND_POLL {
            thread::sleep(Duration::from_secs(1));
            waited += Duration::from_secs(1);
            if state.upgrade().is_none() {
                return;
            }
        }
    }
}

fn root_should_scan(root: &IndexRoot, startup: bool) -> bool {
    root.enabled && (startup || root_is_due(root))
}

fn root_is_due(root: &IndexRoot) -> bool {
    if !root.enabled {
        return false;
    }
    let Some(last) = root
        .last_scanned_at
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return true;
    };
    now_seconds().saturating_sub(last) >= u64::from(root.scan_interval_minutes).saturating_mul(60)
}

pub(crate) fn start_manual_scan(
    state: LibraryState,
    root_id: Option<String>,
) -> Result<(), String> {
    start_scan(state, root_id, None)
}

fn start_scan(
    state: LibraryState,
    root_id: Option<String>,
    selected_ids: Option<Vec<String>>,
) -> Result<(), String> {
    let roots = state
        .with_db(|db| {
            db.list_index_roots()
                .map_err(super::library_commands::library_error)
        })
        .map_err(app_error_text)?
        .into_iter()
        .filter(|root| root.enabled)
        .filter(|root| root_id.as_ref().is_none_or(|id| &root.id == id))
        .filter(|root| {
            selected_ids
                .as_ref()
                .is_none_or(|ids| ids.iter().any(|id| id == &root.id))
        })
        .collect::<Vec<_>>();
    if root_id.is_some() && roots.is_empty() {
        return Err("Index root was not found or is disabled".into());
    }
    if !state.scan_control().try_begin(roots.len(), "scan") {
        return Err("A library scan is already running".into());
    }
    let scanner_state = state.clone();
    if let Err(error) = thread::Builder::new()
        .name("gogglelab-library-scan".into())
        .spawn(move || run_scan(state, roots))
    {
        let message = error.to_string();
        scanner_state
            .scan_control()
            .failed_to_spawn(message.clone());
        return Err(message);
    }
    Ok(())
}

fn run_scan(state: LibraryState, roots: Vec<IndexRoot>) {
    for root in roots {
        if state.scan_control().cancel.load(Ordering::Acquire) {
            break;
        }
        state.scan_control().status.lock().unwrap().current_root_id = Some(root.id.clone());
        if let Err(error) = scan_root(&state, &root) {
            state
                .scan_control()
                .error(format!("{}: {error}", root.path.display()));
        }
        state.scan_control().status.lock().unwrap().roots_completed += 1;
    }
    state.scan_control().finish();
}

fn scan_root(state: &LibraryState, root: &IndexRoot) -> Result<ScanReconcile, String> {
    let canonical_root = root
        .path
        .canonicalize()
        .map_err(|error| format!("Root is offline or unreadable: {error}"))?;
    if canonical_root != root.path {
        return Err("Index root identity changed; remove and add the root again".into());
    }
    if !canonical_root.is_dir() {
        return Err("Index root is not a directory".into());
    }

    let mut stack = vec![canonical_root.clone()];
    let batch_size = state
        .with_db(|db| {
            db.get_catalog_settings()
                .map(|settings| settings.scan_batch_size.clamp(16, 256) as usize)
                .map_err(super::library_commands::library_error)
        })
        .map_err(app_error_text)?;
    let mut seen = Vec::<SourceRef>::new();
    if !root.scan_zip {
        preserve_paused_archive_sources(state, root, &mut seen)?;
    }
    let mut batch = Vec::<ScannedModel>::with_capacity(batch_size);
    let mut reconciled = ScanReconcile::default();
    let mut complete = true;

    while let Some(directory) = stack.pop() {
        if state.scan_control().cancel.load(Ordering::Acquire) {
            complete = false;
            break;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                complete = false;
                state.scan_control().error(format!(
                    "Could not enumerate {}: {error}",
                    directory.display()
                ));
                continue;
            }
        };
        for entry in entries {
            if state.scan_control().cancel.load(Ordering::Acquire) {
                complete = false;
                break;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    complete = false;
                    state
                        .scan_control()
                        .error(format!("Could not read a directory entry: {error}"));
                    continue;
                }
            };
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    complete = false;
                    state
                        .scan_control()
                        .error(format!("Could not inspect {}: {error}", path.display()));
                    continue;
                }
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                stack.push(path);
                continue;
            }
            let is_archive = root.scan_zip && is_zip_path(&path);
            if !metadata.is_file() || (!is_model_path(&path) && !is_archive) {
                continue;
            }
            state.scan_control().status.lock().unwrap().files_visited += 1;
            if seen.len() == MAX_SCAN_FILES {
                complete = false;
                state.scan_control().error(format!(
                    "Root exceeds the safety limit of {MAX_SCAN_FILES} model files"
                ));
                stack.clear();
                break;
            }
            let canonical_path = match path.canonicalize() {
                Ok(path) if path.starts_with(&canonical_root) => path,
                Ok(_) => {
                    complete = false;
                    state.scan_control().error(format!(
                        "Skipped a path outside the configured root: {}",
                        path.display()
                    ));
                    continue;
                }
                Err(error) => {
                    complete = false;
                    state
                        .scan_control()
                        .error(format!("Could not validate {}: {error}", path.display()));
                    continue;
                }
            };
            let relative = canonical_path
                .strip_prefix(&canonical_root)
                .expect("canonical containment was checked")
                .to_path_buf();
            if is_archive {
                match scan_archive_file(
                    state,
                    root,
                    &canonical_path,
                    &relative,
                    &metadata,
                    &mut seen,
                    &mut batch,
                    batch_size,
                    &mut reconciled,
                ) {
                    Ok(()) => {}
                    Err(error) => {
                        complete = false;
                        state
                            .scan_control()
                            .error(format!("{}: {error}", path.display()));
                    }
                }
                continue;
            }
            seen.push(SourceRef {
                relative_path: relative.clone(),
                archive_member: None,
            });
            if metadata.len() > root.max_file_bytes {
                complete = false;
                state.scan_control().error(format!(
                    "Skipped {} because it exceeds the {} byte limit",
                    path.display(),
                    root.max_file_bytes
                ));
                continue;
            }
            let modified_at = modified_nanos(&metadata);
            let unchanged = state
                .with_db(|db| {
                    db.indexed_source_metadata(&root.id, &relative, None)
                        .map_err(super::library_commands::library_error)
                })
                .map_err(app_error_text)?
                .is_some_and(|existing| {
                    existing.bytes == metadata.len() && existing.modified_at == modified_at
                });
            if unchanged {
                continue;
            }
            match scan_file(&canonical_path, relative, metadata.len(), modified_at) {
                Ok(model) => batch.push(model),
                Err(error) => {
                    complete = false;
                    state
                        .scan_control()
                        .error(format!("{}: {error}", path.display()));
                }
            }
            if batch.len() == batch_size {
                let result = flush_batch(state, &root.id, &mut batch)?;
                add_reconcile(&mut reconciled, &result);
            }
        }
    }

    if !batch.is_empty() {
        let result = flush_batch(state, &root.id, &mut batch)?;
        add_reconcile(&mut reconciled, &result);
    }
    if complete && !state.scan_control().cancel.load(Ordering::Acquire) {
        let result = state
            .with_db(|db| {
                db.complete_scan_sources_with_new_models(
                    &root.id,
                    &seen,
                    &reconciled.added_model_ids,
                    &timestamp(),
                )
                .map_err(super::library_commands::library_error)
            })
            .map_err(app_error_text)?;
        add_reconcile(&mut reconciled, &result);
        if !state.scan_control().cancelled() {
            crate::library_commands::render_thumbnails_for_root(state, &root.id);
        }
    }
    Ok(reconciled)
}

fn preserve_paused_archive_sources(
    state: &LibraryState,
    root: &IndexRoot,
    seen: &mut Vec<SourceRef>,
) -> Result<(), String> {
    let models = state
        .with_db(|db| {
            db.list_models(None, None)
                .map_err(super::library_commands::library_error)
        })
        .map_err(app_error_text)?;
    for model in models {
        if model.source_root_id.as_deref() != Some(&root.id) {
            continue;
        }
        let Some(archive_member) = model.source_archive_member else {
            continue;
        };
        let source_path = model
            .source_path
            .as_deref()
            .ok_or_else(|| "Archive model has no source path".to_string())?;
        let source_path = Path::new(source_path);
        let relative_path = if source_path.is_absolute() {
            source_path
                .strip_prefix(&root.path)
                .map_err(|_| "Archive model source escaped its configured root".to_string())?
                .to_path_buf()
        } else {
            source_path.to_path_buf()
        };
        if relative_path.as_os_str().is_empty()
            || relative_path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err("Archive model source path is invalid".into());
        }
        if seen.len() == MAX_SCAN_FILES {
            return Err(format!(
                "Root exceeds the safety limit of {MAX_SCAN_FILES} model files"
            ));
        }
        seen.push(SourceRef {
            relative_path,
            archive_member: Some(archive_member),
        });
    }
    Ok(())
}

fn scan_file(
    path: &Path,
    relative_path: PathBuf,
    expected_bytes: u64,
    modified_at: u64,
) -> Result<ScannedModel, String> {
    if expected_bytes > stl_core::limits::MAX_FILE_BYTES {
        return Err("Model exceeds the native 100 MiB scan limit".into());
    }
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(expected_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 != expected_bytes {
        return Err("File changed while it was being indexed".into());
    }
    let after = file.metadata().map_err(|error| error.to_string())?;
    if after.len() != expected_bytes || modified_nanos(&after) != modified_at {
        return Err("File changed while it was being indexed".into());
    }
    let parsed = stl_core::parse_model(&bytes).map_err(|error| error.to_string())?;
    let format = match parsed.format {
        stl_core::StlFormat::Binary => "stl-binary",
        stl_core::StlFormat::Ascii => "stl-ascii",
        stl_core::StlFormat::ThreeMf => "3mf",
    };
    let content_hash = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let display_name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .map(str::to_owned);
    Ok(ScannedModel {
        relative_path,
        content_hash,
        bytes: expected_bytes,
        format: format.into(),
        modified_at,
        display_name,
        archive_member: None,
        outer_fingerprint: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn scan_archive_file(
    state: &LibraryState,
    root: &IndexRoot,
    archive_path: &Path,
    relative_path: &Path,
    metadata: &fs::Metadata,
    seen: &mut Vec<SourceRef>,
    batch: &mut Vec<ScannedModel>,
    batch_size: usize,
    reconciled: &mut ScanReconcile,
) -> Result<(), String> {
    let cancelled = || state.scan_control().cancelled();
    let inventory = crate::library_archives::archive_inventory_with_cancel(
        archive_path,
        root.max_file_bytes,
        &cancelled,
    )?;
    let existing = state
        .with_db(|db| {
            db.indexed_archive_metadata(&root.id, relative_path)
                .map_err(super::library_commands::library_error)
        })
        .map_err(app_error_text)?;
    let expected_members = inventory
        .member_paths
        .iter()
        .collect::<std::collections::HashSet<_>>();
    let existing_members = existing
        .iter()
        .filter_map(|entry| entry.archive_member.as_ref())
        .collect::<std::collections::HashSet<_>>();
    if existing.len() == inventory.member_paths.len()
        && existing_members == expected_members
        && existing
            .iter()
            .all(|entry| entry.outer_fingerprint.as_deref() == Some(&inventory.outer_fingerprint))
    {
        for member in inventory.member_paths {
            if seen.len() == MAX_SCAN_FILES {
                return Err(format!(
                    "Root exceeds the safety limit of {MAX_SCAN_FILES} model files"
                ));
            }
            seen.push(SourceRef {
                relative_path: relative_path.to_path_buf(),
                archive_member: Some(member),
            });
        }
        return Ok(());
    }

    let scan = crate::library_archives::scan_archive_with_cancel(
        archive_path,
        root.max_file_bytes,
        &cancelled,
    )?;
    if scan.outer_fingerprint != inventory.outer_fingerprint {
        return Err("Archive changed between metadata and content scans".into());
    }
    let modified_at = modified_nanos(metadata);
    for member in scan.members {
        if state.scan_control().cancelled() {
            return Err("Archive scan was cancelled".into());
        }
        if seen.len() == MAX_SCAN_FILES {
            return Err(format!(
                "Root exceeds the safety limit of {MAX_SCAN_FILES} model files"
            ));
        }
        seen.push(SourceRef {
            relative_path: relative_path.to_path_buf(),
            archive_member: Some(member.member_path.clone()),
        });
        batch.push(ScannedModel {
            relative_path: relative_path.to_path_buf(),
            content_hash: member.content_hash,
            bytes: member.bytes,
            format: member.format,
            modified_at,
            display_name: member.display_name,
            archive_member: Some(member.member_path),
            outer_fingerprint: Some(scan.outer_fingerprint.clone()),
        });
        if batch.len() == batch_size {
            let result = flush_batch(state, &root.id, batch)?;
            add_reconcile(reconciled, &result);
        }
    }
    Ok(())
}

fn flush_batch(
    state: &LibraryState,
    root_id: &str,
    batch: &mut Vec<ScannedModel>,
) -> Result<ScanReconcile, String> {
    let result = state
        .with_db(|db| {
            db.record_scan_entries(root_id, batch)
                .map_err(super::library_commands::library_error)
        })
        .map_err(app_error_text)?;
    state.scan_control().status.lock().unwrap().files_indexed += batch.len();
    batch.clear();
    Ok(result)
}

fn add_reconcile(total: &mut ScanReconcile, next: &ScanReconcile) {
    total.seen += next.seen;
    total.added += next.added;
    total.updated += next.updated;
    total.renamed += next.renamed;
    total.missing += next.missing;
    total
        .added_model_ids
        .extend(next.added_model_ids.iter().cloned());
}

fn app_error_text(error: AppError) -> String {
    match error {
        AppError::Library { message } => message,
        other => format!("{other:?}"),
    }
}

fn is_model_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("stl") || extension.eq_ignore_ascii_case("3mf")
        })
}

fn is_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

/// Full-resolution Unix timestamp used by schema-2 `source_modified_at`.
/// Legacy second-resolution values remain readable but will be re-indexed once.
fn modified_nanos(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |value| {
            u64::try_from(value.as_nanos()).unwrap_or(u64::MAX)
        })
}

fn timestamp() -> String {
    now_seconds().to_string()
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

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
        state.scan_control().cancel.store(true, Ordering::Release);
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

        state.scan_control().cancel.store(false, Ordering::Release);
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
