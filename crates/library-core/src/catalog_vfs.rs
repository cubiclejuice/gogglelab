//! SQLite VFS wrapper that pins catalog I/O to the file opened by the caller.
//!
//! The registration must remain alive for at least as long as every connection
//! opened with [`CatalogVfs::vfs_name`]. The wrapper delegates platform locking,
//! temporary-file handling, and other VFS behavior to SQLite's default VFS.

use std::alloc::{alloc, dealloc, Layout};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::{ffi, Connection};
use uuid::Uuid;

static NEXT_VFS_ID: AtomicU64 = AtomicU64::new(1);
static REGISTRATIONS: OnceLock<Mutex<HashMap<usize, Arc<VfsState>>>> = OnceLock::new();

fn registrations() -> &'static Mutex<HashMap<usize, Arc<VfsState>>> {
    REGISTRATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileIdentity {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows { volume: u32, index: u64 },
    #[cfg(not(any(unix, windows)))]
    Unsupported,
}

impl FileIdentity {
    fn from_file(file: &File) -> io::Result<Option<Self>> {
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Ok(None);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Some(Self::Unix {
                device: metadata.dev(),
                inode: metadata.ino(),
            }))
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
                FILE_ATTRIBUTE_REPARSE_POINT,
            };

            let mut information = BY_HANDLE_FILE_INFORMATION::default();
            let result =
                unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) };
            if result == 0 {
                return Err(io::Error::last_os_error());
            }
            if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Ok(None);
            }
            let index = (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow);
            Ok(Some(Self::Windows {
                volume: information.dwVolumeSerialNumber,
                index,
            }))
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = file;
            Ok(None)
        }
    }
}

/// Opens a regular file without following a final symlink or reparse point,
/// then verifies that the visible entry still names that file.
pub fn open_pinned_file(path: &Path) -> io::Result<File> {
    let identity = path_identity(path)?;
    let file = open_no_follow(path)?;
    if FileIdentity::from_file(&file)? != Some(identity) || path_identity(path)? != identity {
        return Err(io::Error::other(
            "file identity changed while it was opened",
        ));
    }
    Ok(file)
}

/// Compares file handles by their stable platform volume and file identity.
pub fn same_file_identity(left: &File, right: &File) -> io::Result<bool> {
    let left = FileIdentity::from_file(left)?;
    let right = FileIdentity::from_file(right)?;
    Ok(left.is_some() && left == right)
}

struct VfsState {
    database_path: PathBuf,
    pinned_file: File,
    pinned_identity: FileIdentity,
    hold_path: PathBuf,
    journal_path: PathBuf,
    default_vfs: usize,
    name: CString,
    vfs: AtomicUsize,
    registered: AtomicBool,
    open_files: AtomicUsize,
    sidecars: Mutex<HashMap<PathBuf, FileIdentity>>,
    allowed_outputs: Mutex<HashSet<PathBuf>>,
    output_files: Mutex<HashMap<PathBuf, FileIdentity>>,
}

/// Owns an SQLite VFS registration that verifies catalog identity around I/O.
pub struct CatalogVfs {
    state: Arc<VfsState>,
    registered: bool,
}

impl CatalogVfs {
    /// Register a wrapper around the platform default VFS for `database_path`.
    ///
    /// `pinned_file` must already refer to the regular catalog file at that
    /// path. The wrapper rejects the registration if the path no longer names
    /// the pinned file or if the platform cannot identify file handles.
    pub fn new(database_path: &Path, pinned_file: &File) -> io::Result<Self> {
        if !cfg!(any(unix, windows)) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "catalog VFS is supported only on Unix and Windows",
            ));
        }

        let parent = database_path
            .parent()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "catalog path has no parent")
            })?
            .canonicalize()?;
        let filename = database_path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "catalog path has no filename")
        })?;
        let database_path = parent.join(filename);
        let pinned_identity = FileIdentity::from_file(pinned_file)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "catalog is not a regular identifiable file",
            )
        })?;
        if path_identity(&database_path)? != pinned_identity {
            return Err(io::Error::other(
                "catalog path does not identify the pinned file",
            ));
        }
        let (hold_path, journal_path) = create_catalog_hold(&database_path, pinned_identity)?;

        let default_vfs = unsafe { ffi::sqlite3_vfs_find(ptr::null()) };
        if default_vfs.is_null() {
            return Err(io::Error::other("SQLite default VFS is unavailable"));
        }
        let base = unsafe { &*default_vfs };
        if base.szOsFile < std::mem::size_of::<ffi::sqlite3_file>() as i32
            || base.xOpen.is_none()
            || base.xDelete.is_none()
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "SQLite default VFS is incomplete",
            ));
        }

        let id = NEXT_VFS_ID.fetch_add(1, Ordering::Relaxed);
        let name = CString::new(format!("gogglelab_catalog_{}_{}", std::process::id(), id))
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid VFS name"))?;
        let state = Arc::new(VfsState {
            database_path,
            pinned_file: pinned_file.try_clone()?,
            pinned_identity,
            hold_path,
            journal_path,
            default_vfs: default_vfs as usize,
            name,
            vfs: AtomicUsize::new(0),
            registered: AtomicBool::new(false),
            open_files: AtomicUsize::new(0),
            sidecars: Mutex::new(HashMap::new()),
            allowed_outputs: Mutex::new(HashSet::new()),
            output_files: Mutex::new(HashMap::new()),
        });

        adopt_existing_public_sidecars(&state)?;

        let mut wrapper = unsafe { ptr::read(default_vfs) };
        wrapper.pNext = ptr::null_mut();
        wrapper.zName = state.name.as_ptr();
        wrapper.szOsFile = std::mem::size_of::<GuardedFile>() as i32;
        wrapper.xOpen = Some(vfs_open);
        wrapper.xDelete = Some(vfs_delete);
        wrapper.xFullPathname = Some(vfs_full_pathname);
        let wrapper = Box::into_raw(Box::new(wrapper));
        let wrapper_key = wrapper as usize;
        // The field is immutable after registration. The integer avoids making
        // the shared callback state depend on raw-pointer Send/Sync behavior.
        state.vfs.store(wrapper_key, Ordering::Release);
        registrations()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(wrapper_key, state.clone());

        let result = unsafe { ffi::sqlite3_vfs_register(wrapper, 0) };
        if result != ffi::SQLITE_OK {
            registrations()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&wrapper_key);
            unsafe { drop(Box::from_raw(wrapper)) };
            return Err(io::Error::other("could not register catalog VFS"));
        }
        state.registered.store(true, Ordering::Release);
        Ok(Self {
            state,
            registered: true,
        })
    }

    /// The name to pass to `Connection::open_with_flags_and_vfs`.
    pub fn vfs_name(&self) -> &CStr {
        &self.state.name
    }

    /// Stable hardlink path used as SQLite's main filename. SQLite derives
    /// rollback journal, WAL, and SHM paths from this inode-specific alias.
    pub fn sqlite_path(&self) -> &Path {
        &self.state.hold_path
    }

    /// Journal documenting the persistent inode alias used by SQLite.
    pub fn hold_journal_path(&self) -> &Path {
        &self.state.journal_path
    }

    /// Admit one exact, currently absent SQLite destination for a `VACUUM INTO`
    /// operation on this registration. Call immediately before executing the
    /// SQL statement; the file identity is pinned when SQLite opens it.
    pub fn allow_output_path(&self, path: &Path) -> io::Result<()> {
        self.state.check_identity()?;
        let output = canonical_candidate(path)?;
        if output == self.state.database_path || output == self.state.hold_path {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "VACUUM output cannot be the catalog or its pinned alias",
            ));
        }
        match std::fs::symlink_metadata(&output) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "VACUUM output path must not already exist",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut allowed = self
            .state
            .allowed_outputs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self
            .state
            .output_files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(&output)
            || !allowed.insert(output)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "VACUUM output path is already admitted",
            ));
        }
        Ok(())
    }

    /// Check that the catalog pathname still identifies the pinned file.
    pub fn check_identity(&self) -> io::Result<()> {
        self.state.check_identity()
    }

    /// Check both pathname identity and SQLite's open main-file identity.
    pub fn check_connection(&self, connection: &Connection) -> io::Result<()> {
        self.check_identity()?;
        let mut moved = 0;
        let rc = unsafe {
            ffi::sqlite3_file_control(
                connection.handle(),
                c"main".as_ptr(),
                ffi::SQLITE_FCNTL_HAS_MOVED,
                (&mut moved as *mut i32).cast(),
            )
        };
        if rc != ffi::SQLITE_OK || moved != 0 {
            return Err(io::Error::other(
                "SQLite catalog connection no longer identifies the pinned file",
            ));
        }
        Ok(())
    }
}

impl Drop for CatalogVfs {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }
        let vfs = self.state.vfs.load(Ordering::Acquire) as *mut ffi::sqlite3_vfs;
        unsafe { ffi::sqlite3_vfs_unregister(vfs) };
        self.state.registered.store(false, Ordering::Release);
        self.registered = false;
        if self.state.open_files.load(Ordering::Acquire) == 0 {
            cleanup_unregistered_vfs(&self.state);
        }
        // If an SQLite file still owns the state, keep the VFS storage and
        // callback registration data alive until that file is closed.
    }
}

fn cleanup_unregistered_vfs(state: &Arc<VfsState>) {
    if state.registered.load(Ordering::Acquire) || state.open_files.load(Ordering::Acquire) != 0 {
        return;
    }
    let vfs = state.vfs.swap(0, Ordering::AcqRel);
    if vfs == 0 {
        return;
    }
    registrations()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&vfs);
    unsafe { drop(Box::from_raw(vfs as *mut ffi::sqlite3_vfs)) };
    // The identity-derived hardlink intentionally survives this registration.
    // Other connections and backup/recovery processes may still use the same
    // SQLite alias, and the extra name does not duplicate the catalog bytes.
    let _ = state;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidecarKind {
    Journal,
    Wal,
    Shm,
}

enum OpenRole {
    Other,
    Catalog,
    Sidecar(SidecarKind, PathBuf),
    Output(PathBuf),
}

enum FileRole {
    Other,
    Catalog,
    Output {
        path: PathBuf,
        identity: FileIdentity,
    },
    Sidecar {
        path: PathBuf,
        identity: FileIdentity,
    },
}

impl FileRole {
    fn is_tracked(&self) -> bool {
        !matches!(self, Self::Other)
    }

    fn expected_identity(&self, state: &VfsState) -> Option<FileIdentity> {
        match self {
            Self::Other => None,
            Self::Catalog => Some(state.pinned_identity),
            Self::Output { identity, .. } => Some(*identity),
            Self::Sidecar { identity, .. } => Some(*identity),
        }
    }
}

fn sqlite_path(name: *const std::ffi::c_char) -> Option<PathBuf> {
    if name.is_null() {
        return None;
    }
    let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(windows)]
    {
        String::from_utf8(bytes.to_vec()).ok().map(PathBuf::from)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = bytes;
        None
    }
}

fn canonical_candidate(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let parent = absolute
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "SQLite path has no parent"))?
        .canonicalize()?;
    let name = absolute.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "SQLite path has no filename")
    })?;
    Ok(parent.join(name))
}

fn classify_path(state: &VfsState, path: &Path) -> OpenRole {
    let Ok(candidate) = canonical_candidate(path) else {
        return OpenRole::Other;
    };
    if candidate == state.database_path || candidate == state.hold_path {
        return OpenRole::Catalog;
    }
    let Some(parent) = state.hold_path.parent() else {
        return OpenRole::Other;
    };
    let Some(filename) = state.hold_path.file_name().and_then(|name| name.to_str()) else {
        return OpenRole::Other;
    };
    for (suffix, kind) in [
        ("-journal", SidecarKind::Journal),
        ("-wal", SidecarKind::Wal),
        ("-shm", SidecarKind::Shm),
    ] {
        if candidate == parent.join(format!("{filename}{suffix}")) {
            return OpenRole::Sidecar(kind, candidate);
        }
    }
    let mut outputs: Vec<PathBuf> = state
        .output_files
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .keys()
        .cloned()
        .collect();
    outputs.extend(
        state
            .allowed_outputs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned(),
    );
    for output in outputs {
        if candidate == output {
            return OpenRole::Output(output);
        }
        let Some(output_parent) = output.parent() else {
            continue;
        };
        let Some(output_name) = output.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        for (suffix, kind) in [
            ("-journal", SidecarKind::Journal),
            ("-wal", SidecarKind::Wal),
            ("-shm", SidecarKind::Shm),
        ] {
            if candidate == output_parent.join(format!("{output_name}{suffix}")) {
                return OpenRole::Sidecar(kind, candidate);
            }
        }
    }
    OpenRole::Other
}

fn canonical_path_bytes(path: &Path) -> Option<Vec<u8>> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(path.as_os_str().as_bytes().to_vec())
    }
    #[cfg(windows)]
    {
        path.to_str().map(|path| path.as_bytes().to_vec())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        None
    }
}

fn sidecar_path_for(database_path: &Path, kind: SidecarKind) -> Option<PathBuf> {
    let parent = database_path.parent()?;
    let filename = database_path.file_name()?.to_str()?;
    let suffix = match kind {
        SidecarKind::Journal => "-journal",
        SidecarKind::Wal => "-wal",
        SidecarKind::Shm => "-shm",
    };
    Some(parent.join(format!("{filename}{suffix}")))
}

fn existing_identity(path: &Path) -> io::Result<Option<FileIdentity>> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
        Ok(_) => path_identity(path).map(Some),
    }
}

fn adopt_legacy_sidecar(state: &VfsState, kind: SidecarKind) -> io::Result<Option<FileIdentity>> {
    state.check_identity()?;
    let alias = sidecar_path_for(&state.hold_path, kind)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid alias sidecar path"))?;
    let visible = sidecar_path_for(&state.database_path, kind).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "invalid visible sidecar path")
    })?;
    let visible_identity = existing_identity(&visible)?;
    let alias_identity = existing_identity(&alias)?;
    if let (Some(visible), Some(alias)) = (visible_identity, alias_identity) {
        if visible != alias {
            return Err(io::Error::other(
                "visible and alias sidecars identify different files",
            ));
        }
    }
    let identity = if let Some(identity) = alias_identity {
        identity
    } else if let Some(identity) = visible_identity {
        match std::fs::hard_link(&visible, &alias) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        if existing_identity(&visible)? != Some(identity)
            || existing_identity(&alias)? != Some(identity)
        {
            return Err(io::Error::other(
                "legacy sidecar changed while linking it to the catalog alias",
            ));
        }
        identity
    } else {
        return Ok(None);
    };
    state.check_identity()?;
    state
        .sidecars
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(alias, identity);
    Ok(Some(identity))
}

fn adopt_existing_public_sidecars(state: &VfsState) -> io::Result<()> {
    for kind in [SidecarKind::Journal, SidecarKind::Wal, SidecarKind::Shm] {
        adopt_legacy_sidecar(state, kind)?;
    }
    Ok(())
}

fn write_path_journal(
    source: &Path,
    hold: &Path,
    expected: Option<FileIdentity>,
    status: &str,
) -> io::Result<PathBuf> {
    let parent = source
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "sidecar has no parent"))?;
    let base = source
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "sidecar has no filename"))?
        .to_string_lossy();
    for _ in 0..8 {
        let id = Uuid::new_v4();
        let journal = parent.join(format!(".{base}.gogglelab-operation-{id}.journal"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&journal) {
            Ok(mut file) => {
                writeln!(
                    file,
                    "format=1\noperation={id}\nsource={}\nhold={}\nidentity={expected:?}\nstatus={status}\n",
                    source.display(),
                    hold.display()
                )?;
                file.sync_all()?;
                sync_directory(parent)?;
                return Ok(journal);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate sidecar journal",
    ))
}

fn rename_no_replace(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = CString::new(source.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid sidecar path"))?;
        let destination = CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid hold path"))?;
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = CString::new(source.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid sidecar path"))?;
        let destination = CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid hold path"))?;
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        if unsafe { MoveFileW(source.as_ptr(), destination.as_ptr()) } != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (source, destination);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic no-replace rename is unsupported",
        ))
    }
}

fn guarded_sidecar_delete(
    state: &VfsState,
    source: &Path,
    expected_hint: Option<FileIdentity>,
    sync_dir: bool,
) -> io::Result<()> {
    guarded_sidecar_delete_after_move(state, source, expected_hint, sync_dir, |_, _| {})
}

fn guarded_sidecar_delete_after_move(
    state: &VfsState,
    source: &Path,
    expected_hint: Option<FileIdentity>,
    sync_dir: bool,
    after_move: impl FnOnce(&Path, &Path),
) -> io::Result<()> {
    match std::fs::symlink_metadata(source) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    state.check_identity()?;
    let before = match path_identity(source) {
        Ok(identity) => identity,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            let _ = write_path_journal(source, source, expected_hint, "refused-invalid-source");
            return Err(error);
        }
    };
    if expected_hint.is_none() {
        return preserve_untracked_sidecar(state, source, sync_dir);
    }
    if expected_hint.is_some_and(|expected| expected != before) {
        let _ = write_path_journal(source, source, expected_hint, "refused-identity-mismatch");
        return Err(io::Error::other("sidecar identity changed before deletion"));
    }
    let parent = source
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "sidecar has no parent"))?;
    let filename = source
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "sidecar has no filename"))?
        .to_string_lossy();
    let (hold, journal) = loop {
        let id = Uuid::new_v4();
        let hold = parent.join(format!(".{filename}.gogglelab-sidecar-hold-{id}"));
        let journal = match write_path_journal(source, &hold, Some(before), "pending") {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        break (hold, journal);
    };
    if let Err(error) = rename_no_replace(source, &hold) {
        let _ = std::fs::remove_file(&journal);
        let _ = sync_directory(parent);
        return Err(error);
    }
    after_move(source, &hold);
    let moved_identity = path_identity(&hold).ok();
    let source_reappeared = std::fs::symlink_metadata(source).is_ok();
    if moved_identity != Some(before) || source_reappeared {
        return Err(io::Error::other("sidecar changed during guarded hold"));
    }
    let hold_file = open_no_follow(&hold)?;
    let handle_identity = FileIdentity::from_file(&hold_file)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "held sidecar is not a regular file",
        )
    })?;
    if handle_identity != before
        || path_identity(&hold)? != before
        || std::fs::symlink_metadata(source).is_ok()
    {
        return Err(io::Error::other("held sidecar identity changed"));
    }
    if sync_dir {
        sync_directory(parent)?;
    }
    std::fs::remove_file(&hold)?;
    drop(hold_file);
    if sync_dir {
        sync_directory(parent)?;
    }
    std::fs::remove_file(&journal)?;
    if sync_dir {
        sync_directory(parent)?;
    }
    Ok(())
}

fn preserve_untracked_sidecar(state: &VfsState, source: &Path, sync_dir: bool) -> io::Result<()> {
    match std::fs::symlink_metadata(source) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    state.check_identity()?;
    let before = match path_identity(source) {
        Ok(identity) => identity,
        Err(error) => {
            let _ = write_path_journal(source, source, None, "untracked-invalid-source");
            return Err(error);
        }
    };
    let parent = source
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "sidecar has no parent"))?;
    let filename = source
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "sidecar has no filename"))?
        .to_string_lossy();
    for _ in 0..8 {
        let id = Uuid::new_v4();
        let hold = parent.join(format!(".{filename}.gogglelab-sidecar-hold-{id}"));
        let journal = write_path_journal(source, &hold, Some(before), "untracked-preserve")?;
        match rename_no_replace(source, &hold) {
            Ok(()) => {
                if path_identity(&hold).ok() != Some(before)
                    || std::fs::symlink_metadata(source).is_ok()
                {
                    return Err(io::Error::other(
                        "untracked sidecar changed during guarded hold",
                    ));
                }
                let held_file = open_no_follow(&hold)?;
                let held_identity = FileIdentity::from_file(&held_file)?.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid sidecar hold")
                })?;
                if held_identity != before || path_identity(&hold)? != before {
                    return Err(io::Error::other("untracked sidecar hold identity changed"));
                }
                if sync_dir {
                    sync_directory(parent)?;
                }
                drop(held_file);
                let _ = journal;
                return Err(io::Error::other(
                    "untracked sidecar preserved in a journaled hold",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let _ = std::fs::remove_file(journal);
                continue;
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate an untracked sidecar hold",
    ))
}

fn guarded_shm_delete(
    state: &VfsState,
    path: &Path,
    expected: Option<FileIdentity>,
) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    guarded_sidecar_delete(state, path, expected, false)
}

fn shm_sidecar_path(state: &VfsState) -> Option<PathBuf> {
    let parent = state.hold_path.parent()?;
    let filename = state.hold_path.file_name()?.to_str()?;
    Some(parent.join(format!("{filename}-shm")))
}

fn known_sidecar_identity(state: &VfsState, path: &Path) -> Option<FileIdentity> {
    state
        .sidecars
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(path)
        .copied()
}

fn sidecar_is_current(state: &VfsState, path: &Path) -> bool {
    known_sidecar_identity(state, path)
        .is_some_and(|expected| path_identity(path).ok() == Some(expected))
}

impl VfsState {
    fn check_identity(&self) -> io::Result<()> {
        let current = path_identity(&self.database_path)?;
        let alias = path_identity(&self.hold_path)?;
        let pinned = FileIdentity::from_file(&self.pinned_file)?
            .ok_or_else(|| io::Error::other("pinned catalog handle is invalid"))?;
        if current != self.pinned_identity
            || alias != self.pinned_identity
            || pinned != self.pinned_identity
        {
            return Err(io::Error::other("catalog identity changed"));
        }
        Ok(())
    }
}

fn path_identity(path: &Path) -> io::Result<FileIdentity> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || is_reparse_point(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is a symlink, reparse point, or not a regular file",
        ));
    }
    let file = open_no_follow(path)?;
    FileIdentity::from_file(&file)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path is not identifiable"))
}

#[cfg(unix)]
fn is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x0000_0400 != 0
}

#[cfg(not(any(unix, windows)))]
fn is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn create_catalog_hold(path: &Path, expected: FileIdentity) -> io::Result<(PathBuf, PathBuf)> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "catalog path has no parent"))?;
    let filename = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "catalog path has no filename"))?
        .to_string_lossy();
    let identity = identity_token(expected);
    let hold = parent.join(format!(".{filename}.gogglelab-{identity}"));
    let journal = parent.join(format!(".{filename}.gogglelab-{identity}.journal"));
    let source_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "catalog filename is not UTF-8")
        })?;
    let hold_name = hold
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "catalog alias filename is not UTF-8",
            )
        })?;
    let journal_text = format!(
        "format=2\nsource_name={source_name}\nhold_name={hold_name}\nidentity={expected:?}\n",
    );
    match std::fs::symlink_metadata(&journal) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "catalog hold journal is not a regular file",
            ));
        }
        Ok(_) => {
            let existing = std::fs::read_to_string(&journal)?;
            if !catalog_hold_journal_matches(&existing, source_name, hold_name, expected) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "catalog hold journal does not match the pinned file",
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&journal)?;
            file.write_all(journal_text.as_bytes())?;
            file.sync_all()?;
            sync_directory(parent)?;
        }
        Err(error) => return Err(error),
    }
    match std::fs::hard_link(path, &hold) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let held_identity = path_identity(&hold)?;
    if held_identity != expected || path_identity(path)? != expected {
        // Keep both artifacts and the journal. An unexpected path change must
        // not cause us to unlink the only surviving catalog copy.
        return Err(io::Error::other(
            "catalog changed while creating its recoverable hold",
        ));
    }
    sync_directory(parent)?;
    Ok((hold, journal))
}

fn catalog_hold_journal_matches(
    contents: &str,
    source_name: &str,
    hold_name: &str,
    expected: FileIdentity,
) -> bool {
    let field = |key: &str| {
        contents.lines().find_map(|line| {
            line.strip_prefix(key)
                .and_then(|value| value.strip_prefix('='))
        })
    };
    let expected_text = format!("{expected:?}");
    if field("identity") != Some(expected_text.as_str()) {
        return false;
    }
    match field("format") {
        Some("2") => {
            field("source_name") == Some(source_name) && field("hold_name") == Some(hold_name)
        }
        // Accept the earlier journal format by checking its leaf names and
        // identity. The parent directory is intentionally not part of the
        // durable binding, so restored libraries can move roots safely.
        Some("1") => {
            let legacy_source = field("source").and_then(|path| Path::new(path).file_name());
            let legacy_hold = field("hold").and_then(|path| Path::new(path).file_name());
            legacy_source.and_then(|name| name.to_str()) == Some(source_name)
                && legacy_hold.and_then(|name| name.to_str()) == Some(hold_name)
        }
        _ => false,
    }
}

fn identity_token(identity: FileIdentity) -> String {
    match identity {
        #[cfg(unix)]
        FileIdentity::Unix { device, inode } => format!("{device:x}-{inode:x}"),
        #[cfg(windows)]
        FileIdentity::Windows { volume, index } => format!("{volume:x}-{index:x}"),
        #[cfg(not(any(unix, windows)))]
        FileIdentity::Unsupported => "unsupported".to_owned(),
    }
}

fn open_no_follow(path: &Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    options.open(path)
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0x4000_0000,
                0x0000_0001 | 0x0000_0002 | 0x0000_0004,
                ptr::null_mut(),
                3,
                0x0200_0000,
                ptr::null_mut(),
            )
        };
        if handle as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        let ok = unsafe { FlushFileBuffers(handle) };
        let error = if ok == 0 {
            Some(io::Error::last_os_error())
        } else {
            None
        };
        unsafe { CloseHandle(handle) };
        error.map_or(Ok(()), Err)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "directory sync is unsupported",
        ))
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        filename: *const u16,
        access: u32,
        share_mode: u32,
        security_attributes: *mut std::ffi::c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn FlushFileBuffers(handle: *mut std::ffi::c_void) -> i32;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    fn MoveFileW(source: *const u16, destination: *const u16) -> i32;
}

#[repr(C)]
struct GuardedFile {
    file: ffi::sqlite3_file,
    inner: *mut ffi::sqlite3_file,
    state: *const VfsState,
    inner_layout: Layout,
    role: FileRole,
}

unsafe fn guarded_file<'a>(file: *mut ffi::sqlite3_file) -> &'a mut GuardedFile {
    &mut *file.cast::<GuardedFile>()
}

unsafe fn inner_methods(file: *mut ffi::sqlite3_file) -> Option<&'static ffi::sqlite3_io_methods> {
    let methods = (*file).pMethods;
    (!methods.is_null()).then(|| &*methods)
}

unsafe fn verify_open_file(guarded: &GuardedFile) -> bool {
    if !guarded.role.is_tracked() {
        return true;
    }
    let state = &*guarded.state;
    let Some(expected) = guarded.role.expected_identity(state) else {
        return true;
    };
    let path_matches = match &guarded.role {
        FileRole::Other => true,
        FileRole::Catalog => state.check_identity().is_ok(),
        FileRole::Output { path, identity } => {
            state.check_identity().is_ok() && path_identity(path).ok() == Some(*identity)
        }
        FileRole::Sidecar { path, identity } => path_identity(path).ok() == Some(*identity),
    };
    if !path_matches || !inner_file_still_at_path(guarded.inner, expected) {
        return false;
    }
    true
}

#[cfg(unix)]
unsafe fn inner_file_still_at_path(file: *mut ffi::sqlite3_file, _expected: FileIdentity) -> bool {
    let Some(methods) = inner_methods(file) else {
        return false;
    };
    let Some(file_control) = methods.xFileControl else {
        return false;
    };
    let mut moved = 1;
    file_control(
        file,
        ffi::SQLITE_FCNTL_HAS_MOVED,
        (&mut moved as *mut i32).cast(),
    ) == ffi::SQLITE_OK
        && moved == 0
}

#[cfg(windows)]
unsafe fn inner_file_still_at_path(file: *mut ffi::sqlite3_file, expected: FileIdentity) -> bool {
    let Some(methods) = inner_methods(file) else {
        return false;
    };
    let Some(file_control) = methods.xFileControl else {
        return false;
    };
    let mut handle: *mut std::ffi::c_void = ptr::null_mut();
    if file_control(
        file,
        ffi::SQLITE_FCNTL_WIN32_GET_HANDLE,
        (&mut handle as *mut *mut std::ffi::c_void).cast(),
    ) != ffi::SQLITE_OK
        || handle.is_null()
    {
        return false;
    }
    let mut info = WinHandleInfo::default();
    GetFileInformationByHandle(handle, &mut info) != 0
        && expected
            == (FileIdentity::Windows {
                volume: info.volume_serial,
                index: ((info.index_high as u64) << 32) | info.index_low as u64,
            })
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct WinHandleInfo {
    attributes: u32,
    creation_time_low: u32,
    creation_time_high: u32,
    access_time_low: u32,
    access_time_high: u32,
    write_time_low: u32,
    write_time_high: u32,
    volume_serial: u32,
    file_size_high: u32,
    file_size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut WinHandleInfo) -> i32;
}

unsafe extern "C" fn vfs_open(
    vfs: *mut ffi::sqlite3_vfs,
    name: ffi::sqlite3_filename,
    file: *mut ffi::sqlite3_file,
    flags: i32,
    out_flags: *mut i32,
) -> i32 {
    if file.is_null() {
        return ffi::SQLITE_CANTOPEN;
    }
    (*file).pMethods = ptr::null();
    let key = vfs as usize;
    let state = registrations()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .cloned();
    let Some(state) = state else {
        return ffi::SQLITE_CANTOPEN;
    };
    let incoming_path = sqlite_path(name);
    let open_role = incoming_path
        .as_deref()
        .map(|path| classify_path(&state, path))
        .unwrap_or(OpenRole::Other);
    let is_catalog = matches!(&open_role, OpenRole::Catalog);
    let output_path = match &open_role {
        OpenRole::Output(path) => Some(path.clone()),
        _ => None,
    };
    let output_identity = if let Some(path) = output_path.as_ref() {
        if flags & ffi::SQLITE_OPEN_MAIN_DB == 0
            || flags & (ffi::SQLITE_OPEN_MAIN_JOURNAL | ffi::SQLITE_OPEN_WAL) != 0
            || state.check_identity().is_err()
        {
            return ffi::SQLITE_CANTOPEN;
        }
        let existing = state
            .output_files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(path)
            .copied();
        if let Some(identity) = existing {
            if path_identity(path).ok() != Some(identity) {
                return ffi::SQLITE_CANTOPEN;
            }
        } else {
            let allowed = state
                .allowed_outputs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains(path);
            if !allowed
                || !matches!(
                    std::fs::symlink_metadata(path),
                    Err(ref error) if error.kind() == io::ErrorKind::NotFound
                )
            {
                return ffi::SQLITE_CANTOPEN;
            }
        }
        existing
    } else {
        None
    };
    let protected_open_flags =
        ffi::SQLITE_OPEN_MAIN_DB | ffi::SQLITE_OPEN_MAIN_JOURNAL | ffi::SQLITE_OPEN_WAL;
    if matches!(&open_role, OpenRole::Other) && flags & protected_open_flags != 0 {
        return ffi::SQLITE_CANTOPEN;
    }
    if let OpenRole::Sidecar(kind, _) = &open_role {
        let required_flag = match kind {
            SidecarKind::Journal => ffi::SQLITE_OPEN_MAIN_JOURNAL,
            SidecarKind::Wal => ffi::SQLITE_OPEN_WAL,
            SidecarKind::Shm => 0,
        };
        if required_flag != 0 && flags & required_flag == 0 {
            return ffi::SQLITE_CANTOPEN;
        }
    }
    if is_catalog && state.check_identity().is_err() {
        return ffi::SQLITE_CANTOPEN;
    }
    if matches!(&open_role, OpenRole::Sidecar(_, _)) && state.check_identity().is_err() {
        return ffi::SQLITE_CANTOPEN;
    }
    if let OpenRole::Sidecar(kind, path) = &open_role {
        if sidecar_path_for(&state.hold_path, *kind).as_deref() == Some(path.as_path())
            && adopt_legacy_sidecar(&state, *kind).is_err()
        {
            return ffi::SQLITE_CANTOPEN;
        }
    }
    let sidecar_before = match &open_role {
        OpenRole::Sidecar(_, path) => match path_identity(path) {
            Ok(identity) => Some(identity),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => return ffi::SQLITE_CANTOPEN,
        },
        _ => None,
    };

    let default_vfs = state.default_vfs as *mut ffi::sqlite3_vfs;
    let Some(open) = (*default_vfs).xOpen else {
        return ffi::SQLITE_CANTOPEN;
    };
    let inner_size = (*default_vfs).szOsFile as usize;
    let Ok(layout) = Layout::from_size_align(inner_size, 16) else {
        return ffi::SQLITE_CANTOPEN;
    };
    let inner = alloc(layout).cast::<ffi::sqlite3_file>();
    if inner.is_null() {
        return ffi::SQLITE_NOMEM;
    }
    ptr::write_bytes(inner.cast::<u8>(), 0, inner_size);
    // Enforce no-follow at the platform VFS boundary even when SQLite omits
    // the incoming flag for an internal reopen (for example, VACUUM INTO).
    let delegated_flags = if is_catalog || output_path.is_some() {
        flags | ffi::SQLITE_OPEN_NOFOLLOW
    } else {
        flags
    };
    let result = open(default_vfs, name, inner, delegated_flags, out_flags);
    if result != ffi::SQLITE_OK {
        if !(*inner).pMethods.is_null() {
            if let Some(close) = (*(*inner).pMethods).xClose {
                close(inner);
            }
        }
        dealloc(inner.cast(), layout);
        return result;
    }
    let role = match open_role {
        OpenRole::Other => FileRole::Other,
        OpenRole::Catalog => FileRole::Catalog,
        OpenRole::Output(path) => {
            let identity = match path_identity(&path) {
                Ok(identity) => identity,
                Err(_) => {
                    if let Some(close) = inner_methods(inner).and_then(|methods| methods.xClose) {
                        close(inner);
                    }
                    dealloc(inner.cast(), layout);
                    return ffi::SQLITE_CANTOPEN;
                }
            };
            if output_identity.is_some_and(|before| before != identity)
                || !inner_file_still_at_path(inner, identity)
                || state.check_identity().is_err()
            {
                if let Some(close) = inner_methods(inner).and_then(|methods| methods.xClose) {
                    close(inner);
                }
                dealloc(inner.cast(), layout);
                return ffi::SQLITE_CANTOPEN;
            }
            FileRole::Output { path, identity }
        }
        OpenRole::Sidecar(_, path) => {
            let identity = match path_identity(&path) {
                Ok(identity) => identity,
                Err(_) => {
                    if let Some(close) = inner_methods(inner).and_then(|methods| methods.xClose) {
                        close(inner);
                    }
                    dealloc(inner.cast(), layout);
                    return ffi::SQLITE_CANTOPEN;
                }
            };
            if sidecar_before.is_some_and(|before| before != identity)
                || !inner_file_still_at_path(inner, identity)
            {
                if let Some(close) = inner_methods(inner).and_then(|methods| methods.xClose) {
                    close(inner);
                }
                dealloc(inner.cast(), layout);
                return ffi::SQLITE_CANTOPEN;
            }
            FileRole::Sidecar { path, identity }
        }
    };
    let sidecar_record = match &role {
        FileRole::Sidecar { path, identity } => Some((path.clone(), *identity)),
        _ => None,
    };
    let output_record = match &role {
        FileRole::Output { path, identity } => Some((path.clone(), *identity)),
        _ => None,
    };
    let provisional = GuardedFile {
        file: ffi::sqlite3_file {
            pMethods: ptr::null(),
        },
        inner,
        state: Arc::into_raw(state.clone()),
        inner_layout: layout,
        role,
    };
    if !verify_open_file(&provisional) {
        if let Some(methods) = inner_methods(inner) {
            if let Some(close) = methods.xClose {
                close(inner);
            }
        }
        dealloc(inner.cast(), layout);
        drop(Arc::from_raw(provisional.state));
        return ffi::SQLITE_CANTOPEN;
    }
    ptr::write(file.cast::<GuardedFile>(), provisional);
    (*file).pMethods = &IO_METHODS;
    if let Some((path, identity)) = sidecar_record {
        state
            .sidecars
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(path, identity);
    }
    if let Some((path, identity)) = output_record {
        state
            .output_files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(path.clone(), identity);
        state
            .allowed_outputs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&path);
    }
    state.open_files.fetch_add(1, Ordering::AcqRel);
    ffi::SQLITE_OK
}

unsafe extern "C" fn vfs_full_pathname(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const std::ffi::c_char,
    output_len: i32,
    output: *mut std::ffi::c_char,
) -> i32 {
    if name.is_null() || output.is_null() || output_len <= 0 {
        return ffi::SQLITE_CANTOPEN_FULLPATH;
    }
    let key = vfs as usize;
    let state = registrations()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .cloned();
    let Some(state) = state else {
        return ffi::SQLITE_CANTOPEN_FULLPATH;
    };
    let Some(path) = sqlite_path(name) else {
        return ffi::SQLITE_CANTOPEN_FULLPATH;
    };
    let role = classify_path(&state, &path);
    let recognized = match &role {
        OpenRole::Catalog | OpenRole::Sidecar(_, _) => state.check_identity().is_ok(),
        OpenRole::Output(path) => {
            let is_admitted = state
                .allowed_outputs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains(path);
            let output_identity = state
                .output_files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(path)
                .copied();
            state.check_identity().is_ok()
                && (is_admitted
                    || output_identity
                        .is_some_and(|identity| path_identity(path).ok() == Some(identity)))
        }
        OpenRole::Other => false,
    };
    if !recognized {
        if matches!(
            &role,
            OpenRole::Catalog | OpenRole::Sidecar(_, _) | OpenRole::Output(_)
        ) {
            return ffi::SQLITE_CANTOPEN_FULLPATH;
        }
        let default_vfs = state.default_vfs as *mut ffi::sqlite3_vfs;
        return (*default_vfs)
            .xFullPathname
            .map_or(ffi::SQLITE_CANTOPEN_FULLPATH, |full_pathname| {
                full_pathname(default_vfs, name, output_len, output)
            });
    }
    let Ok(canonical) = canonical_candidate(&path) else {
        return ffi::SQLITE_CANTOPEN_FULLPATH;
    };
    let Some(bytes) = canonical_path_bytes(&canonical) else {
        return ffi::SQLITE_CANTOPEN_CONVPATH;
    };
    if bytes.len() + 1 > output_len as usize {
        return ffi::SQLITE_CANTOPEN_FULLPATH;
    }
    ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast::<u8>(), bytes.len());
    *output.add(bytes.len()) = 0;
    ffi::SQLITE_OK
}

unsafe extern "C" fn vfs_delete(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const std::ffi::c_char,
    sync_dir: i32,
) -> i32 {
    let key = vfs as usize;
    let state = registrations()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .cloned();
    let Some(state) = state else {
        return ffi::SQLITE_IOERR_DELETE;
    };
    let Some(delete) = (*(state.default_vfs as *mut ffi::sqlite3_vfs)).xDelete else {
        return ffi::SQLITE_IOERR_DELETE;
    };
    let path = sqlite_path(name);
    match path.as_deref().map(|path| classify_path(&state, path)) {
        Some(OpenRole::Catalog) => ffi::SQLITE_IOERR_DELETE,
        Some(OpenRole::Output(path)) => {
            let expected = state
                .output_files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&path)
                .copied();
            match guarded_sidecar_delete(&state, &path, expected, sync_dir != 0) {
                Ok(()) => {
                    state
                        .output_files
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .remove(&path);
                    state
                        .allowed_outputs
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .remove(&path);
                    ffi::SQLITE_OK
                }
                Err(_) => ffi::SQLITE_IOERR_DELETE,
            }
        }
        Some(OpenRole::Sidecar(SidecarKind::Shm, path)) => {
            let expected = known_sidecar_identity(&state, &path);
            if let Some(expected) = expected {
                match existing_identity(&path) {
                    Ok(Some(current)) if current == expected => ffi::SQLITE_OK,
                    Ok(None) => {
                        state
                            .sidecars
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .remove(&path);
                        ffi::SQLITE_OK
                    }
                    _ => {
                        let _ =
                            guarded_sidecar_delete(&state, &path, Some(expected), sync_dir != 0);
                        ffi::SQLITE_IOERR_DELETE
                    }
                }
            } else if guarded_shm_delete(&state, &path, None).is_ok() {
                ffi::SQLITE_OK
            } else {
                ffi::SQLITE_IOERR_DELETE
            }
        }
        Some(OpenRole::Sidecar(_, path)) => {
            let expected = state
                .sidecars
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&path)
                .copied();
            match guarded_sidecar_delete(&state, &path, expected, sync_dir != 0) {
                Ok(()) => {
                    state
                        .sidecars
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .remove(&path);
                    ffi::SQLITE_OK
                }
                Err(_) => ffi::SQLITE_IOERR_DELETE,
            }
        }
        _ => delete(state.default_vfs as *mut ffi::sqlite3_vfs, name, sync_dir),
    }
}

unsafe extern "C" fn file_close(file: *mut ffi::sqlite3_file) -> i32 {
    let guarded = guarded_file(file);
    let result = inner_methods(guarded.inner)
        .and_then(|methods| methods.xClose)
        .map_or(ffi::SQLITE_IOERR, |close| close(guarded.inner));
    dealloc(guarded.inner.cast(), guarded.inner_layout);
    let state = Arc::from_raw(guarded.state);
    ptr::drop_in_place(file.cast::<GuardedFile>());
    let remaining = state.open_files.fetch_sub(1, Ordering::AcqRel) - 1;
    if remaining == 0 && !state.registered.load(Ordering::Acquire) {
        cleanup_unregistered_vfs(&state);
    }
    result
}

unsafe extern "C" fn file_read(
    file: *mut ffi::sqlite3_file,
    buffer: *mut std::ffi::c_void,
    amount: i32,
    offset: ffi::sqlite3_int64,
) -> i32 {
    let guarded = guarded_file(file);
    if !verify_open_file(guarded) {
        return ffi::SQLITE_IOERR_READ;
    }
    let Some(methods) = inner_methods(guarded.inner) else {
        return ffi::SQLITE_IOERR_READ;
    };
    let result = methods.xRead.map_or(ffi::SQLITE_IOERR_READ, |read| {
        read(guarded.inner, buffer, amount, offset)
    });
    if !verify_open_file(guarded) {
        ffi::SQLITE_IOERR_READ
    } else {
        result
    }
}

unsafe extern "C" fn file_write(
    file: *mut ffi::sqlite3_file,
    buffer: *const std::ffi::c_void,
    amount: i32,
    offset: ffi::sqlite3_int64,
) -> i32 {
    let guarded = guarded_file(file);
    if !verify_open_file(guarded) {
        return ffi::SQLITE_IOERR_WRITE;
    }
    let Some(methods) = inner_methods(guarded.inner) else {
        return ffi::SQLITE_IOERR_WRITE;
    };
    let result = methods.xWrite.map_or(ffi::SQLITE_IOERR_WRITE, |write| {
        write(guarded.inner, buffer, amount, offset)
    });
    if !verify_open_file(guarded) {
        ffi::SQLITE_IOERR_WRITE
    } else {
        result
    }
}

unsafe extern "C" fn file_truncate(file: *mut ffi::sqlite3_file, size: ffi::sqlite3_int64) -> i32 {
    let guarded = guarded_file(file);
    if !verify_open_file(guarded) {
        return ffi::SQLITE_IOERR_TRUNCATE;
    }
    let Some(methods) = inner_methods(guarded.inner) else {
        return ffi::SQLITE_IOERR_TRUNCATE;
    };
    let result = methods
        .xTruncate
        .map_or(ffi::SQLITE_IOERR_TRUNCATE, |truncate| {
            truncate(guarded.inner, size)
        });
    if !verify_open_file(guarded) {
        ffi::SQLITE_IOERR_TRUNCATE
    } else {
        result
    }
}

unsafe extern "C" fn file_sync(file: *mut ffi::sqlite3_file, flags: i32) -> i32 {
    let guarded = guarded_file(file);
    if !verify_open_file(guarded) {
        return ffi::SQLITE_IOERR_FSYNC;
    }
    let Some(methods) = inner_methods(guarded.inner) else {
        return ffi::SQLITE_IOERR_FSYNC;
    };
    let result = methods
        .xSync
        .map_or(ffi::SQLITE_IOERR_FSYNC, |sync| sync(guarded.inner, flags));
    if !verify_open_file(guarded) {
        ffi::SQLITE_IOERR_FSYNC
    } else {
        result
    }
}

unsafe extern "C" fn file_size(file: *mut ffi::sqlite3_file, size: *mut ffi::sqlite3_int64) -> i32 {
    let guarded = guarded_file(file);
    if !verify_open_file(guarded) {
        return ffi::SQLITE_IOERR_FSTAT;
    }
    let Some(methods) = inner_methods(guarded.inner) else {
        return ffi::SQLITE_IOERR_FSTAT;
    };
    let result = methods
        .xFileSize
        .map_or(ffi::SQLITE_IOERR_FSTAT, |get_size| {
            get_size(guarded.inner, size)
        });
    if !verify_open_file(guarded) {
        ffi::SQLITE_IOERR_FSTAT
    } else {
        result
    }
}

unsafe extern "C" fn file_lock(file: *mut ffi::sqlite3_file, level: i32) -> i32 {
    let guarded = guarded_file(file);
    if !verify_open_file(guarded) {
        return ffi::SQLITE_IOERR_LOCK;
    }
    let Some(methods) = inner_methods(guarded.inner) else {
        return ffi::SQLITE_IOERR_LOCK;
    };
    methods
        .xLock
        .map_or(ffi::SQLITE_IOERR_LOCK, |lock| lock(guarded.inner, level))
}

unsafe extern "C" fn file_unlock(file: *mut ffi::sqlite3_file, level: i32) -> i32 {
    let guarded = guarded_file(file);
    let Some(methods) = inner_methods(guarded.inner) else {
        return ffi::SQLITE_IOERR_UNLOCK;
    };
    methods.xUnlock.map_or(ffi::SQLITE_IOERR_UNLOCK, |unlock| {
        unlock(guarded.inner, level)
    })
}

unsafe extern "C" fn file_check_reserved(file: *mut ffi::sqlite3_file, result: *mut i32) -> i32 {
    let guarded = guarded_file(file);
    if !verify_open_file(guarded) {
        return ffi::SQLITE_IOERR_CHECKRESERVEDLOCK;
    }
    let Some(methods) = inner_methods(guarded.inner) else {
        return ffi::SQLITE_IOERR_CHECKRESERVEDLOCK;
    };
    methods
        .xCheckReservedLock
        .map_or(ffi::SQLITE_IOERR_CHECKRESERVEDLOCK, |check| {
            check(guarded.inner, result)
        })
}

unsafe extern "C" fn file_control(
    file: *mut ffi::sqlite3_file,
    op: i32,
    arg: *mut std::ffi::c_void,
) -> i32 {
    let guarded = guarded_file(file);
    if op == ffi::SQLITE_FCNTL_HAS_MOVED && guarded.role.is_tracked() {
        if arg.is_null() {
            return ffi::SQLITE_MISUSE;
        }
        *arg.cast::<i32>() = i32::from(!verify_open_file(guarded));
        return ffi::SQLITE_OK;
    }
    let Some(methods) = inner_methods(guarded.inner) else {
        return ffi::SQLITE_NOTFOUND;
    };
    methods
        .xFileControl
        .map_or(ffi::SQLITE_NOTFOUND, |control| {
            control(guarded.inner, op, arg)
        })
}

unsafe extern "C" fn file_sector_size(file: *mut ffi::sqlite3_file) -> i32 {
    let guarded = guarded_file(file);
    inner_methods(guarded.inner)
        .and_then(|methods| methods.xSectorSize)
        .map_or(0, |size| size(guarded.inner))
}

unsafe extern "C" fn file_device_characteristics(file: *mut ffi::sqlite3_file) -> i32 {
    let guarded = guarded_file(file);
    inner_methods(guarded.inner)
        .and_then(|methods| methods.xDeviceCharacteristics)
        .map_or(0, |get| get(guarded.inner))
}

unsafe extern "C" fn file_shm_map(
    file: *mut ffi::sqlite3_file,
    page: i32,
    page_size: i32,
    extend: i32,
    out: *mut *mut std::ffi::c_void,
) -> i32 {
    let guarded = guarded_file(file);
    if !verify_open_file(guarded) {
        return ffi::SQLITE_IOERR_SHMMAP;
    }
    let state = &*guarded.state;
    let Some(shm_path) = shm_sidecar_path(state) else {
        return ffi::SQLITE_IOERR_SHMMAP;
    };
    if adopt_legacy_sidecar(state, SidecarKind::Shm).is_err() {
        return ffi::SQLITE_IOERR_SHMMAP;
    }
    let before = match path_identity(&shm_path) {
        Ok(identity) => Some(identity),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(_) => return ffi::SQLITE_IOERR_SHMMAP,
    };
    if known_sidecar_identity(state, &shm_path).is_some_and(|known| before != Some(known)) {
        return ffi::SQLITE_IOERR_SHMMAP;
    }
    let Some(methods) = inner_methods(guarded.inner) else {
        return ffi::SQLITE_IOERR_SHMMAP;
    };
    let result = methods.xShmMap.map_or(ffi::SQLITE_IOERR_SHMMAP, |map| {
        map(guarded.inner, page, page_size, extend, out)
    });
    if result != ffi::SQLITE_OK {
        return result;
    }
    let mapped = !out.is_null() && !(*out).is_null();
    let after = match path_identity(&shm_path) {
        Ok(identity) => Some(identity),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(_) => return ffi::SQLITE_IOERR_SHMMAP,
    };
    if extend == 0 && before.is_none() && after.is_none() && !mapped {
        return if verify_open_file(guarded) {
            ffi::SQLITE_OK
        } else {
            ffi::SQLITE_IOERR_SHMMAP
        };
    }
    let Some(after) = after else {
        return ffi::SQLITE_IOERR_SHMMAP;
    };
    if before.is_some_and(|before| before != after)
        || known_sidecar_identity(state, &shm_path).is_some_and(|known| known != after)
        || !verify_open_file(guarded)
    {
        return ffi::SQLITE_IOERR_SHMMAP;
    }
    state
        .sidecars
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(shm_path, after);
    result
}

unsafe extern "C" fn file_shm_lock(
    file: *mut ffi::sqlite3_file,
    offset: i32,
    count: i32,
    flags: i32,
) -> i32 {
    let guarded = guarded_file(file);
    if !verify_open_file(guarded) {
        return ffi::SQLITE_IOERR_SHMLOCK;
    }
    let state = &*guarded.state;
    let Some(shm_path) = shm_sidecar_path(state) else {
        return ffi::SQLITE_IOERR_SHMLOCK;
    };
    if !sidecar_is_current(state, &shm_path) {
        return ffi::SQLITE_IOERR_SHMLOCK;
    }
    let Some(methods) = inner_methods(guarded.inner) else {
        return ffi::SQLITE_IOERR_SHMLOCK;
    };
    let result = methods.xShmLock.map_or(ffi::SQLITE_IOERR_SHMLOCK, |lock| {
        lock(guarded.inner, offset, count, flags)
    });
    if !verify_open_file(guarded) || !sidecar_is_current(state, &shm_path) {
        ffi::SQLITE_IOERR_SHMLOCK
    } else {
        result
    }
}

unsafe extern "C" fn file_shm_barrier(file: *mut ffi::sqlite3_file) {
    let guarded = guarded_file(file);
    if let Some(barrier) = inner_methods(guarded.inner).and_then(|methods| methods.xShmBarrier) {
        barrier(guarded.inner);
    }
}

unsafe extern "C" fn file_shm_unmap(file: *mut ffi::sqlite3_file, delete_flag: i32) -> i32 {
    let guarded = guarded_file(file);
    let state = &*guarded.state;
    let Some(shm_path) = shm_sidecar_path(state) else {
        return ffi::SQLITE_IOERR_SHMOPEN;
    };
    let expected = known_sidecar_identity(state, &shm_path);
    let Some(methods) = inner_methods(guarded.inner) else {
        return ffi::SQLITE_IOERR_SHMOPEN;
    };
    // Keep SQLite's unmap behavior but suppress its pathname-based deletion.
    let result = methods
        .xShmUnmap
        .map_or(ffi::SQLITE_IOERR_SHMOPEN, |unmap| unmap(guarded.inner, 0));
    if result != ffi::SQLITE_OK {
        return result;
    }
    if delete_flag != 0 {
        if let Some(expected) = expected {
            match existing_identity(&shm_path) {
                Ok(Some(current)) if current == expected => {
                    // The platform VFS owns the process-wide SHM mapping
                    // reference count. Keep this name so another live
                    // connection cannot map a second inode.
                }
                Ok(None) => {
                    state
                        .sidecars
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .remove(&shm_path);
                }
                _ => {
                    let _ = guarded_sidecar_delete(state, &shm_path, Some(expected), false);
                    return ffi::SQLITE_IOERR_DELETE;
                }
            }
        } else if guarded_shm_delete(state, &shm_path, None).is_err() {
            return ffi::SQLITE_IOERR_DELETE;
        }
    }
    if !verify_open_file(guarded) {
        ffi::SQLITE_IOERR_SHMOPEN
    } else {
        ffi::SQLITE_OK
    }
}

unsafe extern "C" fn file_fetch(
    file: *mut ffi::sqlite3_file,
    offset: ffi::sqlite3_int64,
    amount: i32,
    out: *mut *mut std::ffi::c_void,
) -> i32 {
    let guarded = guarded_file(file);
    if !verify_open_file(guarded) {
        return ffi::SQLITE_IOERR_READ;
    }
    if out.is_null() {
        return ffi::SQLITE_MISUSE;
    }
    let Some(methods) = inner_methods(guarded.inner) else {
        *out = ptr::null_mut();
        return ffi::SQLITE_OK;
    };
    let result = methods.xFetch.map_or_else(
        || {
            *out = ptr::null_mut();
            ffi::SQLITE_OK
        },
        |fetch| fetch(guarded.inner, offset, amount, out),
    );
    if !verify_open_file(guarded) {
        if !out.is_null() {
            let mapping = *out;
            if !mapping.is_null() {
                if let Some(unfetch) = methods.xUnfetch {
                    let _ = unfetch(guarded.inner, offset, mapping);
                }
                *out = ptr::null_mut();
            }
        }
        ffi::SQLITE_IOERR_READ
    } else {
        result
    }
}

unsafe extern "C" fn file_unfetch(
    file: *mut ffi::sqlite3_file,
    offset: ffi::sqlite3_int64,
    pointer: *mut std::ffi::c_void,
) -> i32 {
    let guarded = guarded_file(file);
    let Some(methods) = inner_methods(guarded.inner) else {
        return ffi::SQLITE_OK;
    };
    methods.xUnfetch.map_or(ffi::SQLITE_OK, |unfetch| {
        unfetch(guarded.inner, offset, pointer)
    })
}

static IO_METHODS: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 3,
    xClose: Some(file_close),
    xRead: Some(file_read),
    xWrite: Some(file_write),
    xTruncate: Some(file_truncate),
    xSync: Some(file_sync),
    xFileSize: Some(file_size),
    xLock: Some(file_lock),
    xUnlock: Some(file_unlock),
    xCheckReservedLock: Some(file_check_reserved),
    xFileControl: Some(file_control),
    xSectorSize: Some(file_sector_size),
    xDeviceCharacteristics: Some(file_device_characteristics),
    xShmMap: Some(file_shm_map),
    xShmLock: Some(file_shm_lock),
    xShmBarrier: Some(file_shm_barrier),
    xShmUnmap: Some(file_shm_unmap),
    xFetch: Some(file_fetch),
    xUnfetch: Some(file_unfetch),
};

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::path::PathBuf;

    use rusqlite::{Connection, OpenFlags};
    use uuid::Uuid;

    use super::{
        guarded_sidecar_delete, guarded_sidecar_delete_after_move, open_pinned_file, path_identity,
        same_file_identity, CatalogVfs,
    };

    fn test_catalog_path() -> PathBuf {
        std::env::temp_dir()
            .join(format!("gogglelab-catalog-vfs-{}", Uuid::new_v4()))
            .join("library.sqlite3")
    }

    fn make_catalog() -> (PathBuf, PathBuf, std::fs::File, CatalogVfs) {
        let uncanonical = test_catalog_path();
        let uncanonical_parent = uncanonical.parent().unwrap();
        fs::create_dir_all(uncanonical_parent).unwrap();
        let parent = uncanonical_parent.canonicalize().unwrap();
        let path = parent.join("library.sqlite3");
        {
            let initial = Connection::open(&path).unwrap();
            initial
                .execute_batch("CREATE TABLE items (value TEXT);")
                .unwrap();
        }
        let pinned = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let vfs = CatalogVfs::new(&path, &pinned).unwrap();
        (parent, path, pinned, vfs)
    }

    #[test]
    fn pinned_file_helpers_compare_stable_handle_identities() {
        let parent = test_catalog_path().parent().unwrap().to_path_buf();
        fs::create_dir_all(&parent).unwrap();
        let original = parent.join("original.stl");
        let copy = parent.join("copy.stl");
        fs::write(&original, b"synthetic model bytes").unwrap();
        fs::write(&copy, b"synthetic model bytes").unwrap();

        let original_file = open_pinned_file(&original).unwrap();
        let original_clone = original_file.try_clone().unwrap();
        let copy_file = open_pinned_file(&copy).unwrap();
        assert!(same_file_identity(&original_file, &original_clone).unwrap());
        assert!(!same_file_identity(&original_file, &copy_file).unwrap());

        drop(copy_file);
        drop(original_clone);
        drop(original_file);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn main_database_replacement_after_open_fails_closed_for_writes() {
        let (parent, path, pinned, vfs) = make_catalog();
        let connection = Connection::open_with_flags_and_vfs(
            vfs.sqlite_path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            vfs.vfs_name(),
        )
        .unwrap();
        connection
            .execute("INSERT INTO items(value) VALUES ('before')", [])
            .unwrap();

        let displaced = parent.join("library.sqlite3.displaced");
        fs::rename(&path, &displaced).unwrap();
        let mut replacement = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        replacement.write_all(b"replacement catalog").unwrap();
        replacement.sync_all().unwrap();

        assert!(connection
            .execute("INSERT INTO items(value) VALUES ('after')", [])
            .is_err());
        assert!(vfs.check_identity().is_err());
        assert!(vfs.check_connection(&connection).is_err());

        drop(connection);
        drop(vfs);
        drop(pinned);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn catalog_hold_journal_survives_parent_directory_rename() {
        let (parent, path, pinned, vfs) = make_catalog();
        let original_identity = path_identity(&path).unwrap();
        assert!(vfs.hold_journal_path().exists());
        drop(vfs);
        drop(pinned);

        let moved_parent = parent.with_file_name(format!("moved-library-{}", Uuid::new_v4()));
        fs::rename(&parent, &moved_parent).unwrap();
        let moved_path = moved_parent.join("library.sqlite3");
        let moved_pinned = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&moved_path)
            .unwrap();
        assert_eq!(path_identity(&moved_path).unwrap(), original_identity);
        let reopened = CatalogVfs::new(&moved_path, &moved_pinned);
        assert!(
            reopened.is_ok(),
            "the durable hold journal must bind to the file identity and relative names, not its old parent path"
        );
        let reopened = reopened.unwrap();
        assert!(reopened.check_identity().is_ok());

        drop(reopened);
        drop(moved_pinned);
        let _ = fs::remove_dir_all(moved_parent);
    }

    #[test]
    fn guarded_sidecar_delete_removes_only_the_verified_file() {
        let (parent, _path, pinned, vfs) = make_catalog();
        let sidecar = PathBuf::from(format!("{}-journal", vfs.sqlite_path().display()));
        fs::write(&sidecar, b"rollback journal fixture").unwrap();
        let identity = path_identity(&sidecar).unwrap();
        vfs.state
            .sidecars
            .lock()
            .unwrap()
            .insert(sidecar.clone(), identity);

        guarded_sidecar_delete(&vfs.state, &sidecar, Some(identity), true).unwrap();
        assert!(!sidecar.exists());
        assert!(fs::read_dir(&parent)
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .contains("gogglelab-sidecar-hold")));

        drop(vfs);
        drop(pinned);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn guarded_sidecar_delete_journals_and_preserves_an_unexpected_hold_replacement() {
        let (parent, _path, pinned, vfs) = make_catalog();
        let sidecar = PathBuf::from(format!("{}-wal", vfs.sqlite_path().display()));
        fs::write(&sidecar, b"expected WAL bytes").unwrap();
        let original = OpenOptions::new().read(true).open(&sidecar).unwrap();
        let identity = path_identity(&sidecar).unwrap();
        let mut moved_hold = None;

        let result = guarded_sidecar_delete_after_move(
            &vfs.state,
            &sidecar,
            Some(identity),
            false,
            |_, hold| {
                moved_hold = Some(hold.to_path_buf());
                fs::remove_file(hold).unwrap();
                fs::write(hold, b"unexpected replacement").unwrap();
            },
        );
        assert!(result.is_err());
        let held = moved_hold.unwrap();
        assert_eq!(fs::read(&held).unwrap(), b"unexpected replacement");
        assert!(!sidecar.exists());
        let journals: Vec<_> = fs::read_dir(&parent)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.to_string_lossy().contains("gogglelab-operation"))
            .collect();
        assert_eq!(journals.len(), 1);
        assert!(fs::read_to_string(&journals[0])
            .unwrap()
            .contains("status=pending"));

        drop(original);
        drop(vfs);
        drop(pinned);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn wal_shared_memory_uses_guarded_sidecar_callbacks() {
        let (parent, _path, pinned, vfs) = make_catalog();
        let connection = Connection::open_with_flags_and_vfs(
            vfs.sqlite_path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            vfs.vfs_name(),
        )
        .unwrap();
        let journal_mode: String = connection
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        connection
            .execute("INSERT INTO items(value) VALUES ('wal fixture')", [])
            .unwrap();

        let shm = PathBuf::from(format!("{}-shm", vfs.sqlite_path().display()));
        assert!(shm.exists());
        assert!(vfs.state.sidecars.lock().unwrap().contains_key(&shm));
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        drop(connection);
        assert!(
            shm.exists(),
            "the wrapper must retain SHM until the platform VFS can prove no mappings remain"
        );

        drop(vfs);
        drop(pinned);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn shm_unmap_does_not_split_two_live_connections_onto_different_inodes() {
        let (parent, _path, pinned, vfs) = make_catalog();
        let open = || {
            Connection::open_with_flags_and_vfs(
                vfs.sqlite_path(),
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
                vfs.vfs_name(),
            )
            .unwrap()
        };
        let first = open();
        let journal_mode: String = first
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        first
            .execute("INSERT INTO items(value) VALUES ('first')", [])
            .unwrap();
        let second = open();
        let count: i64 = second
            .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let shm = PathBuf::from(format!("{}-shm", vfs.sqlite_path().display()));
        let identity = path_identity(&shm).unwrap();
        drop(first);
        assert_eq!(path_identity(&shm).unwrap(), identity);
        second
            .execute("INSERT INTO items(value) VALUES ('second')", [])
            .unwrap();

        let third = open();
        let count: i64 = third
            .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(path_identity(&shm).unwrap(), identity);

        drop(second);
        assert_eq!(path_identity(&shm).unwrap(), identity);
        drop(third);
        assert!(shm.exists());
        drop(vfs);
        drop(pinned);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn public_path_wal_sidecars_are_adopted_by_the_inode_alias() {
        let (parent, path, pinned, vfs) = make_catalog();
        let legacy_writer = Connection::open(&path).unwrap();
        let mode: String = legacy_writer
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        legacy_writer
            .execute("INSERT INTO items(value) VALUES ('legacy WAL row')", [])
            .unwrap();

        let public_wal = PathBuf::from(format!("{}-wal", path.display()));
        let public_shm = PathBuf::from(format!("{}-shm", path.display()));
        let alias_wal = PathBuf::from(format!("{}-wal", vfs.sqlite_path().display()));
        let alias_shm = PathBuf::from(format!("{}-shm", vfs.sqlite_path().display()));
        assert!(public_wal.exists());
        assert!(public_shm.exists());

        let reader = Connection::open_with_flags_and_vfs(
            vfs.sqlite_path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            vfs.vfs_name(),
        )
        .unwrap();
        let count: i64 = reader
            .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            path_identity(&alias_wal).unwrap(),
            path_identity(&public_wal).unwrap()
        );
        assert_eq!(
            path_identity(&alias_shm).unwrap(),
            path_identity(&public_shm).unwrap()
        );

        drop(reader);
        drop(legacy_writer);
        drop(vfs);
        drop(pinned);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn vacuum_into_requires_and_consumes_an_exact_output_admission() {
        let (parent, _path, pinned, vfs) = make_catalog();
        let connection = Connection::open_with_flags_and_vfs(
            vfs.sqlite_path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
            vfs.vfs_name(),
        )
        .unwrap();
        let output = parent.join("vacuum-snapshot.sqlite3");
        vfs.allow_output_path(&output).unwrap();
        connection
            .execute_batch(&format!("VACUUM INTO '{}';", output.display()))
            .unwrap();
        let copied = Connection::open(&output).unwrap();
        let table_exists: i64 = copied
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='items'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_exists, 1);
        assert!(vfs.allow_output_path(&output).is_err());

        drop(copied);
        drop(connection);
        drop(vfs);
        drop(pinned);
        let _ = fs::remove_dir_all(parent);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn full_pathname_canonicalizes_admitted_output_with_var_spelling() {
        let (parent, _path, pinned, vfs) = make_catalog();
        let output = parent.join("vacuum-var-spelling.sqlite3");
        vfs.allow_output_path(&output).unwrap();
        let canonical_text = output.to_str().unwrap();
        let prefix = "/private/var/";
        assert!(canonical_text.starts_with(prefix));
        let noncanonical = PathBuf::from(format!("/var/{}", &canonical_text[prefix.len()..]));
        let name = std::ffi::CString::new(noncanonical.to_str().unwrap()).unwrap();
        let mut buffer = vec![0i8; 4096];
        let rc = unsafe {
            super::vfs_full_pathname(
                vfs.state.vfs.load(std::sync::atomic::Ordering::Acquire)
                    as *mut rusqlite::ffi::sqlite3_vfs,
                name.as_ptr(),
                buffer.len() as i32,
                buffer.as_mut_ptr(),
            )
        };
        assert_eq!(rc, rusqlite::ffi::SQLITE_OK);
        let returned = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
        assert_eq!(returned.to_str().unwrap(), canonical_text);

        drop(vfs);
        drop(pinned);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn unclassified_main_database_open_is_rejected() {
        let (parent, _path, pinned, vfs) = make_catalog();
        let unapproved = parent.join("unapproved.sqlite3");
        let result = Connection::open_with_flags_and_vfs(
            &unapproved,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            vfs.vfs_name(),
        );
        assert!(result.is_err());
        assert!(!unapproved.exists());

        drop(vfs);
        drop(pinned);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn vfs_delete_holds_an_existing_untracked_sidecar_and_returns_error() {
        let (parent, _path, pinned, vfs) = make_catalog();
        let sidecar = PathBuf::from(format!("{}-journal", vfs.sqlite_path().display()));
        fs::write(&sidecar, b"untracked rollback journal").unwrap();
        let name = std::ffi::CString::new(sidecar.to_str().unwrap()).unwrap();
        let rc = unsafe {
            super::vfs_delete(
                vfs.state.vfs.load(std::sync::atomic::Ordering::Acquire)
                    as *mut rusqlite::ffi::sqlite3_vfs,
                name.as_ptr(),
                1,
            )
        };
        assert_eq!(rc, rusqlite::ffi::SQLITE_IOERR_DELETE);
        assert!(!sidecar.exists());
        let held: Vec<_> = fs::read_dir(&parent)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.to_string_lossy().contains("gogglelab-sidecar-hold"))
            .collect();
        assert_eq!(held.len(), 1);
        assert_eq!(fs::read(&held[0]).unwrap(), b"untracked rollback journal");
        assert!(fs::read_dir(&parent)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .contains("gogglelab-operation")));

        drop(vfs);
        drop(pinned);
        let _ = fs::remove_dir_all(parent);
    }

    unsafe extern "C" fn mock_shm_map_without_mapping(
        _file: *mut rusqlite::ffi::sqlite3_file,
        _page: i32,
        _page_size: i32,
        extend: i32,
        out: *mut *mut std::ffi::c_void,
    ) -> i32 {
        if extend != 0 || out.is_null() {
            return rusqlite::ffi::SQLITE_MISUSE;
        }
        *out = std::ptr::null_mut();
        rusqlite::ffi::SQLITE_OK
    }

    unsafe extern "C" fn mock_shm_unmap(
        _file: *mut rusqlite::ffi::sqlite3_file,
        delete_flag: i32,
    ) -> i32 {
        if delete_flag != 0 {
            return rusqlite::ffi::SQLITE_MISUSE;
        }
        rusqlite::ffi::SQLITE_OK
    }

    unsafe extern "C" fn mock_file_control(
        _file: *mut rusqlite::ffi::sqlite3_file,
        op: i32,
        arg: *mut std::ffi::c_void,
    ) -> i32 {
        if op == rusqlite::ffi::SQLITE_FCNTL_HAS_MOVED && !arg.is_null() {
            *arg.cast::<i32>() = 0;
            rusqlite::ffi::SQLITE_OK
        } else {
            rusqlite::ffi::SQLITE_NOTFOUND
        }
    }

    static MOCK_SHM_METHODS: rusqlite::ffi::sqlite3_io_methods =
        rusqlite::ffi::sqlite3_io_methods {
            iVersion: 3,
            xClose: None,
            xRead: None,
            xWrite: None,
            xTruncate: None,
            xSync: None,
            xFileSize: None,
            xLock: None,
            xUnlock: None,
            xCheckReservedLock: None,
            xFileControl: Some(mock_file_control),
            xSectorSize: None,
            xDeviceCharacteristics: None,
            xShmMap: Some(mock_shm_map_without_mapping),
            xShmLock: None,
            xShmBarrier: None,
            xShmUnmap: Some(mock_shm_unmap),
            xFetch: None,
            xUnfetch: None,
        };

    fn with_mock_catalog_file<T>(
        vfs: &CatalogVfs,
        operation: impl FnOnce(*mut rusqlite::ffi::sqlite3_file) -> T,
    ) -> T {
        let mut inner = rusqlite::ffi::sqlite3_file {
            pMethods: &MOCK_SHM_METHODS,
        };
        let state = std::sync::Arc::into_raw(vfs.state.clone());
        let mut guarded = super::GuardedFile {
            file: rusqlite::ffi::sqlite3_file {
                pMethods: std::ptr::null(),
            },
            inner: &mut inner,
            state,
            inner_layout: std::alloc::Layout::new::<rusqlite::ffi::sqlite3_file>(),
            role: super::FileRole::Catalog,
        };
        let result = operation(&mut guarded.file);
        unsafe { drop(std::sync::Arc::from_raw(state)) };
        result
    }

    #[test]
    fn shm_map_without_extension_accepts_missing_file_and_no_mapping() {
        let (parent, _path, pinned, vfs) = make_catalog();
        let shm = PathBuf::from(format!("{}-shm", vfs.sqlite_path().display()));
        assert!(!shm.exists());
        let rc = with_mock_catalog_file(&vfs, |file| {
            let mut mapping = std::ptr::null_mut();
            unsafe { super::file_shm_map(file, 0, 4096, 0, &mut mapping) }
        });
        assert_eq!(rc, rusqlite::ffi::SQLITE_OK);
        assert!(!shm.exists());

        drop(vfs);
        drop(pinned);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn shm_unmap_ignores_absent_unknown_sidecar_but_holds_existing_one() {
        let (parent, _path, pinned, vfs) = make_catalog();
        let shm = PathBuf::from(format!("{}-shm", vfs.sqlite_path().display()));
        let missing_rc =
            with_mock_catalog_file(&vfs, |file| unsafe { super::file_shm_unmap(file, 1) });
        assert_eq!(missing_rc, rusqlite::ffi::SQLITE_OK);

        fs::write(&shm, b"unknown shared memory sidecar").unwrap();
        let existing_rc =
            with_mock_catalog_file(&vfs, |file| unsafe { super::file_shm_unmap(file, 1) });
        assert_eq!(existing_rc, rusqlite::ffi::SQLITE_IOERR_DELETE);
        assert!(!shm.exists());
        let held: Vec<_> = fs::read_dir(&parent)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.to_string_lossy().contains("gogglelab-sidecar-hold"))
            .collect();
        assert_eq!(held.len(), 1);
        assert_eq!(
            fs::read(&held[0]).unwrap(),
            b"unknown shared memory sidecar"
        );

        drop(vfs);
        drop(pinned);
        let _ = fs::remove_dir_all(parent);
    }
}
