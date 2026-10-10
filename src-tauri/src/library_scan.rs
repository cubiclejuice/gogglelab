use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Weak};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use library_core::{IndexRoot, ScanReconcile, ScannedModel};
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
    let mut seen = Vec::<PathBuf>::new();
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
            if !metadata.is_file() || !is_model_path(&path) {
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
            seen.push(relative.clone());
            if metadata.len() > root.max_file_bytes {
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
                    db.indexed_source_metadata(&root.id, &relative)
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
                db.complete_scan_with_new_models(
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

fn scan_file(
    path: &Path,
    relative_path: PathBuf,
    expected_bytes: u64,
    modified_at: u64,
) -> Result<ScannedModel, String> {
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
    })
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
}
