//! One debounced recursive filesystem watch for the folder currently selected
//! in the file tree. The webview receives a root-level invalidation signal; it
//! deliberately never receives changed descendant names.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::Emitter;

use crate::error::AppError;

const DEBOUNCE: Duration = Duration::from_millis(250);
const MAX_DEBOUNCE: Duration = Duration::from_secs(1);
const EVENT_QUEUE_CAPACITY: usize = 128;

#[derive(Debug, Clone, Serialize)]
pub struct FolderChangeEvent {
    pub root: String,
    pub watch_id: u64,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct FolderWatchState {
    inner: Mutex<WatchState>,
}

#[derive(Default)]
struct WatchState {
    latest_watch_id: Option<u64>,
    active: Option<FolderWatch>,
    selected_root: Option<SelectedRootState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RootIdentity {
    device: u64,
    inode: u64,
}

struct SelectedRootState {
    path: PathBuf,
    identity: RootIdentity,
}

/// Opened directory handle for an authorized selected root.
///
/// The handle is intentionally opaque outside the core crate. Consumers can
/// perform a filesystem operation relative to the selected directory without
/// rebuilding authority from a frontend-provided path.
pub struct SelectedRootHandle {
    path: PathBuf,
    identity: RootIdentity,
    directory: File,
}

impl SelectedRootHandle {
    pub(crate) fn directory(&self) -> &File {
        &self.directory
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Confirms the selected path still resolves to the object held open by
    /// this handle. This detects replacement and symlink substitution.
    pub fn verify_current(&self) -> Result<(), String> {
        verify_root_path(&self.path, self.identity)
    }

    /// Atomically creates a child directory relative to the opened root.
    /// The operation never follows a replaced root path or overwrites an
    /// existing child. Callers own product-specific name policy.
    /// Move only a regular ZIP under this selected root to the operating-system Trash.
    pub fn move_zip_to_trash(&self, path: &Path) -> Result<(), String> {
        crate::trash::move_zip_to_trash(self, path)
    }

    pub fn create_child_directory(&self, name: &str) -> Result<PathBuf, String> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
            || name.contains('\0')
        {
            return Err("Folder name must be a single path segment.".into());
        }

        self.verify_current()?;
        #[cfg(windows)]
        {
            // The held root denies delete/rename and rejects reparse points.
            // create_dir is atomic and fails if the child already exists.
            fs::create_dir(self.path.join(name))
                .map_err(|e| format!("Could not create folder: {e}"))?;
        }
        #[cfg(not(windows))]
        create_directory_at(&self.directory, name)?;
        self.verify_current()?;
        Ok(self.path.join(name))
    }
}

#[cfg(unix)]
fn root_identity(file: &File) -> Result<RootIdentity, String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    Ok(RootIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(windows)]
fn root_identity(file: &File) -> Result<RootIdentity, String> {
    let (device, inode) =
        crate::fs_portability::windows_identity(file).map_err(|e| e.to_string())?;
    Ok(RootIdentity { device, inode })
}

#[cfg(not(any(unix, windows)))]
fn root_identity(_file: &File) -> Result<RootIdentity, String> {
    Err("This platform cannot verify the selected folder identity.".into())
}

fn verify_root_path(path: &Path, expected: RootIdentity) -> Result<(), String> {
    let link_metadata = fs::symlink_metadata(path)
        .map_err(|_| "The selected folder is no longer available.".to_string())?;
    if link_metadata.file_type().is_symlink() || !link_metadata.is_dir() {
        return Err("The selected folder was replaced or is no longer a directory.".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "The selected folder is no longer available.".to_string())?;
    if canonical != path {
        return Err("The selected folder path changed unexpectedly.".into());
    }
    if root_identity(&crate::fs_portability::open_directory(path).map_err(|e| e.to_string())?)?
        != expected
    {
        return Err("The selected folder was replaced. Select it again and retry.".into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn create_directory_at(directory: &File, name: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        use std::os::raw::{c_char, c_int};

        #[cfg(target_os = "macos")]
        type ModeT = u16;
        #[cfg(not(target_os = "macos"))]
        type ModeT = u32;

        extern "C" {
            fn mkdirat(directory: c_int, path: *const c_char, mode: ModeT) -> c_int;
        }

        let name = CString::new(name.as_bytes())
            .map_err(|_| "Folder name contains an invalid byte.".to_string())?;
        // mkdirat is rooted at the already-open selected folder. Its atomic
        // create semantics reject collisions, including links, without a
        // check-then-create race.
        let result = unsafe { mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o755 as ModeT) };
        if result == 0 {
            Ok(())
        } else {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Err("A file or folder with that name already exists.".into())
            } else {
                Err(format!("Could not create folder: {error}"))
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (directory, name);
        Err("Secure folder creation is unavailable on this platform.".into())
    }
}

/// A native watcher and its debounce worker. Dropping this object stops both:
/// notify drops the callback sender, which wakes the worker rather than making
/// a Tauri command wait for a thread join.
struct FolderWatch {
    alive: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
    _watcher: RecommendedWatcher,
    _worker: thread::JoinHandle<()>,
}

impl Drop for FolderWatch {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        self.enabled.store(false, Ordering::Release);
    }
}

impl FolderWatch {
    fn start<F>(root: PathBuf, watch_id: u64, emit: F) -> Result<Self, String>
    where
        F: Fn(FolderChangeEvent) + Send + Sync + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
        let alive = Arc::new(AtomicBool::new(true));
        let enabled = Arc::new(AtomicBool::new(false));
        let worker_alive = alive.clone();
        let worker_enabled = enabled.clone();
        let worker_root = root.clone();
        let worker = thread::Builder::new()
            .name("gogglelab-folder-watch".into())
            .spawn(move || {
                debounce_events(
                    receiver,
                    worker_root,
                    watch_id,
                    worker_alive,
                    worker_enabled,
                    emit,
                )
            })
            .map_err(|error| format!("could not start folder-watch worker: {error}"))?;

        let callback_root = root.clone();
        let mut watcher = RecommendedWatcher::new(
            move |event: notify::Result<Event>| {
                // Read/access noise must not fill the bounded queue and crowd
                // out the only creation event in a burst.
                if event
                    .as_ref()
                    .is_ok_and(|event| !relevant(event, &callback_root))
                {
                    return;
                }
                enqueue_event(&sender, event);
            },
            Config::default().with_follow_symlinks(false),
        )
        .map_err(|error| format!("could not create folder watcher: {error}"))?;
        if let Err(error) = watcher.watch(&root, RecursiveMode::Recursive) {
            drop(watcher);
            // The sender is gone after `watcher` drops; the worker will wake
            // immediately. Do not join it on the command/UI path.
            return Err(format!("could not watch {}: {error}", root.display()));
        }
        Ok(Self {
            alive,
            enabled,
            _watcher: watcher,
            _worker: worker,
        })
    }

    fn activate(&self) {
        self.enabled.store(true, Ordering::Release);
    }
}

fn enqueue_event(sender: &SyncSender<notify::Result<Event>>, event: notify::Result<Event>) {
    // The worker only needs one signal per burst. A bounded queue prevents an
    // editor's event storm from becoming unbounded memory while preserving an
    // already-enqueued invalidation.
    let _ = sender.try_send(event);
}

fn relevant(event: &Event, root: &Path) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event.paths.is_empty() || event.paths.iter().any(|path| path.starts_with(root))
}

fn send_change<F>(
    emit: &F,
    root: &Path,
    watch_id: u64,
    alive: &AtomicBool,
    enabled: &AtomicBool,
    error: Option<String>,
) where
    F: Fn(FolderChangeEvent),
{
    if !alive.load(Ordering::Acquire) || !enabled.load(Ordering::Acquire) {
        return;
    }
    emit(FolderChangeEvent {
        root: root.to_string_lossy().into_owned(),
        watch_id,
        error,
    });
}

fn debounce_events<F>(
    receiver: Receiver<notify::Result<Event>>,
    root: PathBuf,
    watch_id: u64,
    alive: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
    emit: F,
) where
    F: Fn(FolderChangeEvent),
{
    let mut pending = false;
    let mut flush_at: Option<Instant> = None;
    let mut latest_flush_at: Option<Instant> = None;

    loop {
        let timeout = flush_at
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(60));
        match receiver.recv_timeout(timeout) {
            Ok(Ok(event)) if relevant(&event, &root) => {
                let now = Instant::now();
                let cap = *latest_flush_at.get_or_insert(now + MAX_DEBOUNCE);
                pending = true;
                flush_at = Some(std::cmp::min(now + DEBOUNCE, cap));
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => send_change(
                &emit,
                &root,
                watch_id,
                &alive,
                &enabled,
                Some(format!("folder watch error: {error}")),
            ),
            Err(RecvTimeoutError::Timeout) => {
                if pending {
                    send_change(&emit, &root, watch_id, &alive, &enabled, None);
                    pending = false;
                    flush_at = None;
                    latest_flush_at = None;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if !alive.load(Ordering::Acquire) {
            return;
        }
        // recv_timeout(0) still receives queued events. Check the deadline
        // explicitly so continuous writes cannot postpone a refresh forever.
        if pending && flush_at.is_some_and(|deadline| Instant::now() >= deadline) {
            send_change(&emit, &root, watch_id, &alive, &enabled, None);
            pending = false;
            flush_at = None;
            latest_flush_at = None;
        }
    }
}

fn canonical_root(root: &str) -> Result<(PathBuf, RootIdentity), AppError> {
    let root = Path::new(root)
        .canonicalize()
        .map_err(|_| AppError::Watch {
            message: "The selected folder no longer exists or cannot be read.".into(),
        })?;
    if !root.is_dir() {
        return Err(AppError::Watch {
            message: "The selected path is not a folder.".into(),
        });
    }
    let identity = selected_root_identity(&root)?;
    Ok((root, identity))
}

fn selected_root_identity(root: &Path) -> Result<RootIdentity, AppError> {
    let directory = crate::fs_portability::open_directory(root).map_err(|_| AppError::Watch {
        message: "The selected folder is no longer available.".into(),
    })?;
    let identity = root_identity(&directory).map_err(|message| AppError::Watch { message })?;
    verify_root_path(root, identity).map_err(|message| AppError::Watch { message })?;
    Ok(identity)
}

impl FolderWatchState {
    #[cfg(test)]
    pub(crate) fn select_root_for_test(&self, root: &Path) {
        let (root, identity) = canonical_root(&root.to_string_lossy()).unwrap();
        assert!(self.register(1));
        assert!(self.select_root_if_latest(1, root, identity));
    }

    /// Returns the latest canonical root accepted by the native backend.
    /// Browsing and safe file operations remain available if filesystem watch
    /// setup fails for an otherwise readable selected folder.
    pub fn selected_root(&self) -> Option<PathBuf> {
        self.inner
            .lock()
            .unwrap()
            .selected_root
            .as_ref()
            .map(|selected| selected.path.clone())
    }

    /// Runs an operation against an opened handle for the currently selected
    /// root while holding the selection lock. The requested parent must match
    /// the selected canonical path and the directory identity captured when
    /// that root was selected.
    pub fn with_selected_root<T>(
        &self,
        expected_root: &Path,
        operation: impl FnOnce(&SelectedRootHandle) -> Result<T, String>,
    ) -> Result<T, String> {
        let state = self.inner.lock().unwrap();
        let selected = state
            .selected_root
            .as_ref()
            .ok_or_else(|| "No folder is currently selected.".to_string())?;
        if expected_root != selected.path.as_path() {
            return Err("The requested folder is no longer the selected file-tree root.".into());
        }

        verify_root_path(&selected.path, selected.identity)?;
        let directory = crate::fs_portability::open_directory(&selected.path)
            .map_err(|_| "The selected folder is no longer available.".to_string())?;
        if root_identity(&directory)? != selected.identity {
            return Err("The selected folder was replaced. Select it again and retry.".into());
        }
        let root = SelectedRootHandle {
            path: selected.path.clone(),
            identity: selected.identity,
            directory,
        };
        operation(&root)
    }

    /// Records an intent before setup starts, so a slow canonicalize/watcher
    /// creation cannot replace a newer root.
    fn register(&self, watch_id: u64) -> bool {
        let mut state = self.inner.lock().unwrap();
        if state
            .latest_watch_id
            .is_some_and(|latest| watch_id <= latest)
        {
            return false;
        }
        state.latest_watch_id = Some(watch_id);
        true
    }

    fn clear_selection_if_latest(&self, watch_id: u64) {
        let old = {
            let mut state = self.inner.lock().unwrap();
            if state.latest_watch_id != Some(watch_id) {
                return;
            }
            state.selected_root = None;
            state.active.take()
        };
        drop(old);
    }

    /// Commits a successfully canonicalized selection before watch setup.
    /// Any watcher for the previous selection is retired immediately.
    fn select_root_if_latest(&self, watch_id: u64, root: PathBuf, identity: RootIdentity) -> bool {
        let old = {
            let mut state = self.inner.lock().unwrap();
            if state.latest_watch_id != Some(watch_id) {
                return false;
            }
            state.selected_root = Some(SelectedRootState {
                path: root,
                identity,
            });
            state.active.take()
        };
        drop(old);
        true
    }

    /// A watch failure does not invalidate a successfully canonicalized root.
    fn watch_failed_if_latest(&self, watch_id: u64) {
        let old = {
            let mut state = self.inner.lock().unwrap();
            if state.latest_watch_id != Some(watch_id) {
                return;
            }
            state.active.take()
        };
        drop(old);
    }

    fn replace_if_latest(&self, watch_id: u64, next: FolderWatch) -> bool {
        let old = {
            let mut state = self.inner.lock().unwrap();
            if state.latest_watch_id != Some(watch_id) {
                return false;
            }
            next.activate();
            state.active.replace(next)
        };
        drop(old);
        true
    }
}

/// Starts/replaces the selected-root watch, or stops it with `root: None`.
/// Stale intents are harmless no-ops (`Ok(None)`): the caller's newer intent
/// owns the active watcher and will receive its own result/event.
#[tauri::command]
pub fn watch_folder(
    app: tauri::AppHandle,
    state: tauri::State<'_, FolderWatchState>,
    root: Option<String>,
    watch_id: u64,
) -> Result<Option<String>, AppError> {
    if !state.register(watch_id) {
        return Ok(None);
    }
    let Some(requested_root) = root else {
        state.clear_selection_if_latest(watch_id);
        return Ok(None);
    };

    let (root, identity) = match canonical_root(&requested_root) {
        Ok(root) => root,
        Err(error) => {
            // A current failed root-selection should not leave a watcher doing
            // pointless work for a folder the UI no longer selected.
            state.clear_selection_if_latest(watch_id);
            return Err(error);
        }
    };
    if !state.select_root_if_latest(watch_id, root.clone(), identity) {
        return Ok(None);
    }
    let canonical = root.to_string_lossy().into_owned();
    let app_for_events = app.clone();
    let next = match FolderWatch::start(root.clone(), watch_id, move |event| {
        let _ = app_for_events.emit("folder-changed", event);
    }) {
        Ok(watcher) => watcher,
        Err(message) => {
            state.watch_failed_if_latest(watch_id);
            return Err(AppError::Watch { message });
        }
    };
    if state.replace_if_latest(watch_id, next) {
        Ok(Some(canonical))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc::Receiver;

    const WAIT: Duration = Duration::from_secs(5);

    fn temp(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("gogglelab-watch-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    fn watcher(root: &Path) -> (FolderWatch, Receiver<FolderChangeEvent>) {
        let (sender, receiver) = mpsc::channel();
        let watcher = FolderWatch::start(root.to_path_buf(), 42, move |event| {
            let _ = sender.send(event);
        })
        .unwrap();
        watcher.activate();
        (watcher, receiver)
    }

    fn changed(receiver: &Receiver<FolderChangeEvent>) -> FolderChangeEvent {
        receiver
            .recv_timeout(WAIT)
            .expect("watcher should emit before the bounded deadline")
    }

    fn no_event(result: Result<FolderChangeEvent, RecvTimeoutError>) -> bool {
        matches!(
            result,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected)
        )
    }

    #[test]
    fn reports_recursive_new_files_and_directories_without_descendant_paths() {
        let root = temp("recursive");
        let (_watcher, receiver) = watcher(&root);
        let nested = root.join("new/subfolder");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("part.stl"), b"mesh").unwrap();

        let event = changed(&receiver);
        assert_eq!(event.root, root.to_string_lossy());
        assert_eq!(event.watch_id, 42);
        assert_eq!(event.error, None);
    }

    #[test]
    fn coalesces_a_burst_into_one_refresh() {
        let root = temp("burst");
        let (_watcher, receiver) = watcher(&root);
        for index in 0..8 {
            fs::write(root.join(format!("part-{index}.stl")), b"mesh").unwrap();
        }

        let _ = changed(&receiver);
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(700)),
            Err(RecvTimeoutError::Timeout)
        ));
    }

    #[test]
    fn replacement_and_stop_do_not_report_old_root_events() {
        let first = temp("first");
        let second = temp("second");
        let (first_watch, first_events) = watcher(&first);
        drop(first_watch);
        let (second_watch, second_events) = watcher(&second);
        fs::write(first.join("old.stl"), b"mesh").unwrap();
        assert!(no_event(
            first_events.recv_timeout(Duration::from_millis(700))
        ));
        fs::write(second.join("new.stl"), b"mesh").unwrap();
        assert_eq!(changed(&second_events).root, second.to_string_lossy());
        drop(second_watch);
        fs::write(second.join("stopped.stl"), b"mesh").unwrap();
        assert!(no_event(
            second_events.recv_timeout(Duration::from_millis(700))
        ));
    }

    #[test]
    fn canonical_root_normalizes_paths_and_rejects_files() {
        let root = temp("canonical");
        let dotted = root.join(".");
        let normalized = canonical_root(&dotted.to_string_lossy()).unwrap();
        assert_eq!(normalized.0, root);
        let file = root.join("file.stl");
        fs::write(&file, b"mesh").unwrap();
        assert!(canonical_root(&file.to_string_lossy()).is_err());
    }

    #[test]
    fn stale_watch_intents_cannot_replace_or_stop_the_latest_watch() {
        let root = temp("intent-order");
        let state = FolderWatchState::default();
        assert!(state.register(2));
        assert!(!state.register(1));
        assert!(!state.register(2));
        assert!(state.select_root_if_latest(
            2,
            root.clone(),
            selected_root_identity(&root).unwrap()
        ));
        let (active, _) = watcher(&root);
        assert!(state.replace_if_latest(2, active));
        assert_eq!(state.selected_root(), Some(root.clone()));
        state.clear_selection_if_latest(1);
        assert!(state.inner.lock().unwrap().active.is_some());
        assert_eq!(state.selected_root(), Some(root.clone()));
        assert!(state.register(3));
        let (stale, _) = watcher(&root);
        assert!(!state.replace_if_latest(2, stale));
        assert_eq!(state.selected_root(), Some(root));
        state.clear_selection_if_latest(3);
        assert!(state.inner.lock().unwrap().active.is_none());
        assert_eq!(state.selected_root(), None);
    }

    #[test]
    fn selected_root_survives_watcher_startup_failure() {
        let root = temp("watch-start-failure");
        let state = FolderWatchState::default();
        assert!(state.register(1));
        assert!(state.select_root_if_latest(
            1,
            root.clone(),
            selected_root_identity(&root).unwrap()
        ));

        state.watch_failed_if_latest(1);

        assert_eq!(state.selected_root(), Some(root));
        assert!(state.inner.lock().unwrap().active.is_none());
    }

    #[test]
    fn stale_intent_cannot_change_or_clear_selected_root() {
        let first = temp("selected-first");
        let second = temp("selected-second");
        let stale = temp("selected-stale");
        let state = FolderWatchState::default();

        assert!(state.register(1));
        assert!(state.select_root_if_latest(
            1,
            first.clone(),
            selected_root_identity(&first).unwrap()
        ));
        assert!(state.register(2));
        assert_eq!(state.selected_root(), Some(first.clone()));
        assert!(!state.select_root_if_latest(
            1,
            stale.clone(),
            selected_root_identity(&stale).unwrap()
        ));
        state.watch_failed_if_latest(1);
        assert_eq!(state.selected_root(), Some(first));

        assert!(state.select_root_if_latest(
            2,
            second.clone(),
            selected_root_identity(&second).unwrap()
        ));
        state.watch_failed_if_latest(1);
        assert_eq!(state.selected_root(), Some(second));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn extraction_publishing_notifies_the_selected_root() {
        use std::io::Write;
        let root = temp("extraction");
        let path = root.join("kit.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file("parts/model.stl", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"model data").unwrap();
        zip.finish().unwrap();
        let (_watcher, receiver) = watcher(&root);
        let result =
            crate::archive::extract_zip_core(root.to_str().unwrap(), path.to_str().unwrap())
                .unwrap();
        let event = changed(&receiver);
        assert_eq!(event.root, root.to_string_lossy());
        assert!(Path::new(&result.destination)
            .join("parts/model.stl")
            .is_file());
        let entries = crate::filetree::list_dir(event.root.clone(), event.root).unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.is_dir && entry.name == "kit"));
        assert!(entries
            .iter()
            .any(|entry| !entry.is_dir && entry.name == "kit.zip"));
        assert!(!entries
            .iter()
            .any(|entry| entry.name.starts_with(".gogglelab-extract-")));
    }
}
