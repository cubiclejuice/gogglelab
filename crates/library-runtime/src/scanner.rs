use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::LibraryRuntime;
use library_core::{IndexRoot, ScanReconcile, ScannedModel, SourceRef};
use serde::Serialize;
use sha2::{Digest, Sha256};

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

pub struct ScanControl {
    status: Mutex<LibraryScanStatus>,
    cancel: AtomicBool,
}

impl Default for ScanControl {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanControl {
    pub fn new() -> Self {
        Self {
            status: Mutex::new(LibraryScanStatus::default()),
            cancel: AtomicBool::new(false),
        }
    }

    pub fn status(&self) -> LibraryScanStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn cancel(&self) {
        let mut status = self.status.lock().unwrap();
        if status.running {
            self.cancel.store(true, Ordering::Release);
            status.cancelling = true;
        }
    }

    pub fn try_begin(&self, roots_total: usize, job_kind: &str) -> bool {
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

    pub fn finish(&self) {
        let mut status = self.status.lock().unwrap();
        status.running = false;
        status.cancelling = false;
        status.current_root_id = None;
        status.finished_at = Some(timestamp());
    }

    pub fn failed_to_spawn(&self, message: String) {
        self.error(message);
        self.finish();
    }

    pub fn error(&self, message: String) {
        let mut status = self.status.lock().unwrap();
        if status.errors.len() < MAX_REPORTED_ERRORS {
            status.errors.push(message);
        } else if status.errors.len() == MAX_REPORTED_ERRORS {
            status
                .errors
                .push("Additional scan errors were omitted".into());
        }
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    #[doc(hidden)]
    pub fn set_cancelled_for_test(&self, cancelled: bool) {
        self.cancel.store(cancelled, Ordering::Release);
    }

    pub fn set_job_kind(&self, kind: &str) {
        self.status.lock().unwrap().job_kind = Some(kind.into());
    }

    pub fn set_thumbnail_progress(&self, total: usize) {
        let mut status = self.status.lock().unwrap();
        status.thumbnails_total = total;
        status.thumbnails_completed = 0;
    }

    pub fn thumbnail_completed(&self) {
        self.status.lock().unwrap().thumbnails_completed += 1;
    }
}

pub fn start_background_scanner(runtime: &Arc<LibraryRuntime>) {
    if !runtime.capability_enabled(library_protocol::Capability::Scan) {
        return;
    }
    let weak = Arc::downgrade(runtime);
    let error_state = weak.clone();
    if let Err(error) = thread::Builder::new()
        .name("gogglelab-library-scheduler".into())
        .spawn(move || background_loop(weak))
    {
        if let Some(runtime) = error_state.upgrade() {
            runtime
                .scan_control()
                .error(format!("Could not start background scanner: {error}"));
        }
    }
}

fn background_loop(state: Weak<LibraryRuntime>) {
    let mut startup = true;
    while let Some(runtime) = state.upgrade() {
        let roots = runtime
            .read_db(|db| db.list_index_roots())
            .unwrap_or_default()
            .into_iter()
            .filter(|root| root_should_scan(root, startup))
            .filter_map(|root| match runtime.validate_scan_root(&root) {
                Ok(()) => Some(root.id),
                Err(error) => {
                    runtime.scan_control().error(format!(
                        "Skipped unadmitted background root {}: {}",
                        root.id, error.message
                    ));
                    None
                }
            })
            .collect::<Vec<_>>();
        if !roots.is_empty() {
            let _ = start_scan(runtime.clone(), None, Some(roots));
        }
        startup = false;
        drop(runtime);

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

pub fn root_should_scan(root: &IndexRoot, startup: bool) -> bool {
    root.enabled && (startup || root_is_due(root))
}

pub fn root_is_due(root: &IndexRoot) -> bool {
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

pub fn start_manual_scan(
    state: Arc<LibraryRuntime>,
    root_id: Option<String>,
) -> Result<(), String> {
    start_scan(state, root_id, None)
}

pub fn start_manual_scan_subdirectory(
    state: Arc<LibraryRuntime>,
    root_id: String,
    directory: String,
) -> Result<(), String> {
    if directory.is_empty() {
        return start_manual_scan(state, Some(root_id));
    }
    if state.maintenance_active() {
        return Err("Library maintenance is in progress".into());
    }
    let root = state
        .read_db(|db| {
            db.list_index_roots()
                .map(|roots| roots.into_iter().find(|root| root.id == root_id))
        })
        .map_err(runtime_error_text)?
        .ok_or_else(|| "Index root was not found".to_string())?;
    if !root.enabled {
        return Err("Index root is disabled".into());
    }
    state
        .validate_scan_root(&root)
        .map_err(runtime_error_text)?;
    let relative = PathBuf::from(&directory);
    validate_scan_subdirectory(&root.path, &relative)?;
    if !state.scan_control().try_begin(1, "subdirectoryScan") {
        return Err("A library scan is already running".into());
    }
    if state.maintenance_active() {
        state.scan_control().finish();
        return Err("Library maintenance is in progress".into());
    }
    let scanner_state = Arc::clone(&state);
    if let Err(error) = thread::Builder::new()
        .name("gogglelab-library-subdirectory-scan".into())
        .spawn(move || {
            state.scan_control().status.lock().unwrap().current_root_id = Some(root.id.clone());
            if let Err(error) = scan_subdirectory(&state, &root, &relative) {
                state
                    .scan_control()
                    .error(format!("{}: {error}", root.path.display()));
            }
            state.scan_control().status.lock().unwrap().roots_completed += 1;
            state.scan_control().finish();
        })
    {
        let message = error.to_string();
        scanner_state
            .scan_control()
            .failed_to_spawn(message.clone());
        return Err(message);
    }
    Ok(())
}

pub fn start_selected_scan(
    state: Arc<LibraryRuntime>,
    selected_ids: Vec<String>,
) -> Result<(), String> {
    start_scan(state, None, Some(selected_ids))
}

fn start_scan(
    state: Arc<LibraryRuntime>,
    root_id: Option<String>,
    selected_ids: Option<Vec<String>>,
) -> Result<(), String> {
    if state.maintenance_active() {
        return Err("Library maintenance is in progress".into());
    }
    let roots = state
        .read_db(|db| db.list_index_roots())
        .map_err(runtime_error_text)?
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
    for root in &roots {
        state.validate_scan_root(root).map_err(runtime_error_text)?;
    }
    if !state.scan_control().try_begin(roots.len(), "scan") {
        return Err("A library scan is already running".into());
    }
    if state.maintenance_active() {
        state.scan_control().finish();
        return Err("Library maintenance is in progress".into());
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

fn run_scan(state: Arc<LibraryRuntime>, roots: Vec<IndexRoot>) {
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

pub fn scan_root(state: &Arc<LibraryRuntime>, root: &IndexRoot) -> Result<ScanReconcile, String> {
    scan_root_scope(state, root, None)
}

pub fn scan_subdirectory(
    state: &Arc<LibraryRuntime>,
    root: &IndexRoot,
    relative_directory: &Path,
) -> Result<ScanReconcile, String> {
    if relative_directory.as_os_str().is_empty() {
        return scan_root_scope(state, root, None);
    }
    scan_root_scope(state, root, Some(relative_directory))
}

fn scan_root_scope(
    state: &Arc<LibraryRuntime>,
    root: &IndexRoot,
    relative_directory: Option<&Path>,
) -> Result<ScanReconcile, String> {
    state.validate_scan_root(root).map_err(runtime_error_text)?;
    let _io_guard = state.begin_io().map_err(runtime_error_text)?;
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

    let selected_directory = match relative_directory {
        Some(relative) => validate_scan_subdirectory(&canonical_root, relative)?,
        None => canonical_root.clone(),
    };
    let is_full_root = relative_directory.is_none();
    let scan_scope = if is_full_root {
        None
    } else {
        Some(
            selected_directory
                .strip_prefix(&canonical_root)
                .map_err(|_| "Scan directory escaped its configured root".to_string())?
                .to_path_buf(),
        )
    };

    let mut stack = vec![selected_directory];
    let batch_size = state
        .read_db(|db| {
            db.get_catalog_settings()
                .map(|settings| settings.scan_batch_size.clamp(16, 256) as usize)
        })
        .map_err(runtime_error_text)?;
    let mut seen = Vec::<SourceRef>::new();
    if !root.scan_zip {
        preserve_paused_archive_sources(state, root, scan_scope.as_deref(), &mut seen)?;
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
                .read_db(|db| db.indexed_source_metadata(&root.id, &relative, None))
                .map_err(runtime_error_text)?
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
            .mutate_db(|db| match scan_scope.as_deref() {
                Some(scope) => db.complete_subdirectory_scan_sources_with_new_models(
                    &root.id,
                    scope,
                    &seen,
                    &reconciled.added_model_ids,
                ),
                None => db.complete_scan_sources_with_new_models(
                    &root.id,
                    &seen,
                    &reconciled.added_model_ids,
                    &timestamp(),
                ),
            })
            .map_err(runtime_error_text)?;
        add_reconcile(&mut reconciled, &result);
        state.notify_scan_completed(&root.id);
    }
    Ok(reconciled)
}

fn validate_scan_subdirectory(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || relative.to_string_lossy().contains('\\')
        || relative.to_string_lossy().contains('\0')
    {
        return Err("Scan directory must be a safe root-relative path".into());
    }
    if root.canonicalize().map_err(|error| error.to_string())? != root {
        return Err("Configured root identity changed".into());
    }
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err("Scan directory contains an unsafe component".into());
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("Scan directory must contain only real directories".into());
        }
    }
    let canonical = current.canonicalize().map_err(|error| error.to_string())?;
    if !canonical.starts_with(root) {
        return Err("Scan directory escaped its configured root".into());
    }
    Ok(canonical)
}

fn preserve_paused_archive_sources(
    state: &Arc<LibraryRuntime>,
    root: &IndexRoot,
    scope: Option<&Path>,
    seen: &mut Vec<SourceRef>,
) -> Result<(), String> {
    let archive_sources = state
        .read_db(|db| db.indexed_archive_source_refs(&root.id, scope))
        .map_err(runtime_error_text)?;
    if archive_sources.len() > MAX_SCAN_FILES.saturating_sub(seen.len()) {
        return Err(format!(
            "Root exceeds the safety limit of {MAX_SCAN_FILES} model files"
        ));
    }
    seen.extend(archive_sources);
    Ok(())
}

pub fn scan_file(
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
    state: &Arc<LibraryRuntime>,
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
    let inventory = crate::archives::archive_inventory_with_cancel_typed(
        archive_path,
        root.max_file_bytes,
        &cancelled,
    )
    .map_err(|error| error.to_string())?;
    let existing = state
        .read_db(|db| db.indexed_archive_metadata(&root.id, relative_path))
        .map_err(runtime_error_text)?;
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

    let scan = crate::archives::scan_archive_with_cancel_typed(
        archive_path,
        root.max_file_bytes,
        &cancelled,
    )
    .map_err(|error| error.to_string())?;
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
    state: &Arc<LibraryRuntime>,
    root_id: &str,
    batch: &mut Vec<ScannedModel>,
) -> Result<ScanReconcile, String> {
    let result = state
        .mutate_db(|db| db.record_scan_entries(root_id, batch))
        .map_err(runtime_error_text)?;
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

fn runtime_error_text(error: library_protocol::RuntimeError) -> String {
    error.message
}

pub fn is_model_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("stl") || extension.eq_ignore_ascii_case("3mf")
        })
}

pub fn is_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

/// Full-resolution Unix timestamp used by schema-2 `source_modified_at`.
/// Legacy second-resolution values remain readable but will be re-indexed once.
pub fn modified_nanos(metadata: &fs::Metadata) -> u64 {
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
