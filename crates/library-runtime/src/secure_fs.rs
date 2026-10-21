//! Handle-relative filesystem operations for guarded management actions.
//!
//! Paths are used only to acquire directory handles. Once acquired, child
//! files are opened, created, moved, and held relative to those handles so a
//! parent-path swap cannot redirect the operation through a symlink. Guarded
//! original source cleanup preserves bytes in recoverable holds. Generated
//! cache cleanup may use hash-verified unlinking after explicit authorization.

#[cfg(not(unix))]
use std::path::Path;

/// PNG ownership is restricted to the fixed thumbnails child of an admitted Library.
#[allow(dead_code)]
pub(crate) struct PngNamespace {
    inner: std::sync::Arc<PngNamespaceInner>,
}
#[allow(dead_code)]
struct PngNamespaceInner {
    root: Directory,
    root_path: std::path::PathBuf,
    root_identity: (u64, u64, u32),
    directory: Directory,
    directory_identity: (u64, u64, u32),
}
#[derive(Debug)]
enum PngLocation {
    Public(std::ffi::OsString),
    Claim(std::ffi::OsString),
    Unlinked,
}
#[allow(dead_code)]
pub(crate) struct PngFileReceipt {
    file: std::fs::File,
    namespace: std::sync::Arc<PngNamespaceInner>,
    identity: Option<(u64, u64, u32)>,
    rendered: std::sync::Arc<crate::thumbnails::RasterGeneratedPng>,
    public_name: std::ffi::OsString,
    location: PngLocation,
    partial: bool,
}
impl std::fmt::Debug for PngFileReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PngFileReceipt")
            .field("location", &self.location)
            .field("partial", &self.partial)
            .finish_non_exhaustive()
    }
}
#[derive(Debug)]
pub(crate) struct CreatedPngStage {
    receipt: PngFileReceipt,
}
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct PngStageFailure {
    pub source: std::io::Error,
    pub stage: Option<CreatedPngStage>,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PngRetirementOutcome {
    pub unlinked_now: bool,
    pub removed_bytes: u64,
}
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct PngRetirementFailure {
    pub source: std::io::Error,
    pub outcome: PngRetirementOutcome,
}
#[allow(dead_code)]
impl PngFileReceipt {
    pub(crate) fn current_name(&self) -> Option<&std::ffi::OsStr> {
        match &self.location {
            PngLocation::Public(n) | PngLocation::Claim(n) => Some(n),
            PngLocation::Unlinked => None,
        }
    }
}
#[allow(dead_code)]
impl CreatedPngStage {
    pub(crate) fn current_name(&self) -> Option<&std::ffi::OsStr> {
        self.receipt.current_name()
    }
}
const MAX_RECEIPT_PNG_BYTES: u64 = 4 * 1024 * 1024;

/// Geometry-only output. The original exclusively-created descriptor is already
/// anonymous; no payload bytes may be written until construction succeeds.
pub(crate) struct GeometryFileWriter {
    file: std::fs::File,
    identity: (u64, u64, u32),
}
/// Immutable generated packet, never admitted from a pathname or a source FD.
pub(crate) struct SealedGeometryFile {
    file: std::fs::File,
    identity: (u64, u64, u32),
    bytes: u64,
    sha256: String,
}
pub(crate) const MAX_GEOMETRY_PACKET_BYTES: u64 = 128 * 1024 * 1024;
pub(crate) const MAX_GEOMETRY_SETUP_ENTRIES: usize = 256;

// Covers only the dedicated geometry namespace's named-empty setup window.
// Inventory must share this gate so it cannot inspect a cooperating writer's
// temporary mode-000 entry. Never hold TempResourceState while acquiring it.
static GEOMETRY_CREATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub(crate) fn geometry_inventory_guard() -> std::io::Result<std::sync::MutexGuard<'static, ()>> {
    GEOMETRY_CREATION
        .lock()
        .map_err(|_| std::io::Error::other("geometry creation lock is unavailable"))
}
impl SealedGeometryFile {
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }
    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// Narrow namespace for generated purge/restore transaction metadata only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GeneratedControlKind {
    Purge,
    Restore,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GeneratedControlRole {
    Main,
    Temporary,
    Commit,
    PurgeMarker,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedControlNamespace {
    root: std::path::PathBuf,
    kind: GeneratedControlKind,
    journal_name: std::ffi::OsString,
}
impl GeneratedControlNamespace {
    pub fn new(root: &std::path::Path, kind: GeneratedControlKind) -> std::io::Result<Self> {
        use sha2::{Digest, Sha256};
        if !root.is_absolute()
            || root.file_name().is_none()
            || root.to_str().is_none()
            || root.components().any(|part| {
                !matches!(
                    part,
                    std::path::Component::RootDir
                        | std::path::Component::Normal(_)
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "generated control root must be an absolute normalized Unicode path",
            ));
        }
        let hash = Sha256::digest(root.as_os_str().as_encoded_bytes());
        let suffix = hash[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let kind_text = match kind {
            GeneratedControlKind::Purge => "purge",
            GeneratedControlKind::Restore => "restore",
        };
        Ok(Self {
            root: root.to_path_buf(),
            kind,
            journal_name: format!(".gogglelab-{kind_text}-{suffix}.journal").into(),
        })
    }
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }
    pub fn public_name(&self, role: GeneratedControlRole) -> std::io::Result<std::ffi::OsString> {
        Ok(match role {
            GeneratedControlRole::Main => self.journal_name.clone(),
            GeneratedControlRole::Temporary => {
                format!("{}.tmp", self.journal_name.to_string_lossy()).into()
            }
            GeneratedControlRole::Commit => {
                format!("{}.commit", self.journal_name.to_string_lossy()).into()
            }
            GeneratedControlRole::PurgeMarker if self.kind == GeneratedControlKind::Purge => {
                ".gogglelab-purge-stage".into()
            }
            GeneratedControlRole::PurgeMarker => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "restore transactions have no purge marker",
                ))
            }
        })
    }
    fn claim_prefix(&self) -> String {
        format!("{}.cleanup-", self.journal_name.to_string_lossy())
    }
}
#[derive(Debug, Eq, PartialEq)]
pub enum GeneratedFileLocation {
    Public(std::ffi::OsString),
    Claim(std::ffi::OsString),
    Unlinked,
}
/// Ownership is retained from the original created or strictly recovered descriptor.
/// This receipt cannot be cloned, serialized, or built from a fresh fingerprint.
pub struct GeneratedFileReceipt {
    file: std::fs::File,
    parent_identity: (u64, u64, u32),
    file_identity: (u64, u64, u32),
    namespace: GeneratedControlNamespace,
    nonce: String,
    role: GeneratedControlRole,
    bytes: Vec<u8>,
    location: GeneratedFileLocation,
}
impl GeneratedFileReceipt {
    pub fn current_name(&self) -> Option<&std::ffi::OsStr> {
        match &self.location {
            GeneratedFileLocation::Public(name) | GeneratedFileLocation::Claim(name) => Some(name),
            GeneratedFileLocation::Unlinked => None,
        }
    }
    pub fn location(&self) -> &GeneratedFileLocation {
        &self.location
    }
    pub fn nonce(&self) -> &str {
        &self.nonce
    }
    pub fn role(&self) -> GeneratedControlRole {
        self.role
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeldEntry {
    pub original_name: std::ffi::OsString,
    pub hold_name: std::ffi::OsString,
    pub journal_name: std::ffi::OsString,
    pub expected: Option<(u64, String)>,
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MovedEntry {
    pub destination_name: std::ffi::OsString,
    pub journal_name: std::ffi::OsString,
    pub expected: Option<(u64, String)>,
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
}

#[cfg(unix)]
mod imp {
    use std::collections::BTreeMap;
    use std::ffi::{CStr, CString, OsString};
    use std::fs::File;
    use std::io::{self, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::{Component, Path};

    use super::{HeldEntry, MovedEntry};
    use sha2::{Digest, Sha256};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct EntryIdentity {
        device: u64,
        inode: u64,
        mode: u32,
    }

    impl EntryIdentity {
        fn from_file(file: &File) -> io::Result<Self> {
            use std::os::unix::fs::MetadataExt;
            let metadata = file.metadata()?;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
                mode: metadata.mode(),
            })
        }

        fn at(directory: &Directory, name: &std::ffi::OsStr) -> io::Result<Self> {
            let name = c_name(name)?;
            let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
            let result = unsafe {
                libc::fstatat(
                    directory.0.as_raw_fd(),
                    name.as_ptr(),
                    &mut stat,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if result < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                device: stat.st_dev as u64,
                inode: stat.st_ino as u64,
                mode: stat.st_mode as u32,
            })
        }
    }

    pub struct Directory(File);

    impl Directory {
        pub fn open(path: &Path) -> io::Result<Self> {
            if !path.is_absolute() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "directory path must be absolute",
                ));
            }
            let root = open_at(
                libc::AT_FDCWD,
                b"/",
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )?;
            let mut current = Self(root);
            for component in path.components() {
                match component {
                    Component::RootDir => {}
                    Component::Normal(name) => current = current.open_child(name)?,
                    Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "directory path is not normalized",
                        ));
                    }
                }
            }
            Ok(current)
        }

        pub fn open_child(&self, name: &std::ffi::OsStr) -> io::Result<Self> {
            let name = c_name(name)?;
            let fd = unsafe {
                libc::openat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(unsafe { File::from_raw_fd(fd) }))
        }

        pub fn open_relative(&self, relative: &Path) -> io::Result<Self> {
            if relative.as_os_str().is_empty() {
                return Ok(Self(self.0.try_clone()?));
            }
            let mut current = Self(self.0.try_clone()?);
            for component in relative.components() {
                match component {
                    Component::Normal(name) => current = current.open_child(name)?,
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "relative directory path contains an unsafe component",
                        ));
                    }
                }
            }
            Ok(current)
        }

        pub fn open_file(&self, name: &std::ffi::OsStr) -> io::Result<File> {
            let name = c_name(name)?;
            let fd = unsafe {
                libc::openat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let file = unsafe { File::from_raw_fd(fd) };
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "entry is not a regular file",
                ));
            }
            Ok(file)
        }

        pub fn open_relative_file(&self, relative: &Path) -> io::Result<File> {
            if relative.as_os_str().is_empty()
                || relative
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "relative file path contains an unsafe component",
                ));
            }
            let name = relative
                .file_name()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file name missing"))?;
            let parent = relative.parent().unwrap_or_else(|| Path::new(""));
            self.open_relative(parent)?.open_file(name)
        }

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        pub fn read_entry_names(&self, maximum: usize) -> io::Result<Vec<OsString>> {
            let current_directory = CString::new(".").expect("constant contains no NUL");
            let descriptor = unsafe {
                libc::openat(
                    self.0.as_raw_fd(),
                    current_directory.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if descriptor < 0 {
                return Err(io::Error::last_os_error());
            }
            let stream = unsafe { libc::fdopendir(descriptor) };
            if stream.is_null() {
                let error = io::Error::last_os_error();
                unsafe { libc::close(descriptor) };
                return Err(error);
            }

            let result = (|| {
                let mut names = Vec::new();
                loop {
                    reset_errno();
                    let entry = unsafe { libc::readdir(stream) };
                    if entry.is_null() {
                        let error = current_errno();
                        if error != 0 {
                            return Err(io::Error::from_raw_os_error(error));
                        }
                        break;
                    }
                    let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
                    if name.to_bytes() != b"." && name.to_bytes() != b".." {
                        if names.len() >= maximum {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "directory entry limit exceeded",
                            ));
                        }
                        names.push(OsString::from_vec(name.to_bytes().to_vec()));
                    }
                }
                Ok(names)
            })();
            unsafe { libc::closedir(stream) };
            result
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        pub fn read_entry_names(&self, _maximum: usize) -> io::Result<Vec<OsString>> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "handle-relative directory enumeration is unavailable on this Unix host",
            ))
        }

        /// Creates only a new, empty geometry output, then makes it anonymous.
        /// No original/source descriptor or arbitrary filename is accepted.
        pub(crate) fn create_anonymous_geometry_writer(
            &self,
        ) -> io::Result<super::GeometryFileWriter> {
            self.create_anonymous_geometry_writer_with_hook(|_, _, _| Ok(()))
        }
        pub(crate) fn create_anonymous_geometry_writer_with_hook(
            &self,
            mut hook: impl FnMut(
                &'static str,
                &std::ffi::OsStr,
                Option<&std::ffi::OsStr>,
            ) -> io::Result<()>,
        ) -> io::Result<super::GeometryFileWriter> {
            if !cfg!(any(target_os = "macos", target_os = "linux")) {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "anonymous geometry output is unsupported on this host",
                ));
            }
            // Cooperating runtimes hold CatalogLock across processes. Serialize
            // only the small named-empty birth window inside this process.
            let _creation = super::geometry_inventory_guard()?;
            if self
                .read_entry_names(super::MAX_GEOMETRY_SETUP_ENTRIES)?
                .len()
                >= super::MAX_GEOMETRY_SETUP_ENTRIES
            {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "geometry setup namespace entry limit reached",
                ));
            }
            let id = uuid::Uuid::new_v4().simple().to_string();
            let public = OsString::from(format!("geometry-new-{id}.empty"));
            let capture = OsString::from(format!(
                ".geometry-empty-{id}.capture-{}",
                uuid::Uuid::new_v4().simple()
            ));
            let name = c_name(&public)?;
            let fd = unsafe {
                libc::openat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDWR
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    0o000,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let file = unsafe { File::from_raw_fd(fd) };
            let identity = generated_identity(&file)?;
            hook("after_empty_create", &public, None)?;
            verify_empty_geometry(self, &public, &file, identity)?;
            hook("before_capture", &public, Some(&capture))?;
            self.rename_child_exclusive_raw(&public, &capture)?;
            hook("after_capture", &public, Some(&capture))?;
            verify_empty_geometry(self, &capture, &file, identity)?;
            hook("before_private_unlink", &public, Some(&capture))?;
            // Only this private claim is unlinked. Concurrent public-name
            // replacements survive. Non-cooperating same-UID mutation of this
            // private claim/inode is outside the internal-file ownership boundary.
            let capture_name = c_name(&capture)?;
            if unsafe { libc::unlinkat(self.0.as_raw_fd(), capture_name.as_ptr(), 0) } < 0 {
                return Err(io::Error::last_os_error());
            }
            hook("after_unlink", &public, Some(&capture))?;
            use std::os::unix::fs::MetadataExt;
            if file.metadata()?.nlink() != 0 {
                return Err(generated_invalid(
                    "geometry output retained an unexpected named link",
                ));
            }
            self.sync()?;
            hook("after_parent_sync", &public, None)?;
            if self.entry_exists(&public)? {
                return Err(generated_invalid("geometry setup public entry reappeared"));
            }
            Ok(super::GeometryFileWriter { file, identity })
        }

        pub fn admit_created_generated(
            &self,
            namespace: &super::GeneratedControlNamespace,
            nonce: &str,
            role: super::GeneratedControlRole,
            file: File,
            bytes: &[u8],
        ) -> io::Result<super::GeneratedFileReceipt> {
            generated_supported()?;
            validate_generated_nonce(nonce)?;
            validate_generated_bytes(role, nonce, bytes, 8192)?;
            let name = namespace.public_name(role)?;
            let receipt = super::GeneratedFileReceipt {
                parent_identity: generated_identity(&self.0)?,
                file_identity: generated_identity(&file)?,
                file,
                namespace: namespace.clone(),
                nonce: nonce.to_owned(),
                role,
                bytes: bytes.to_vec(),
                location: super::GeneratedFileLocation::Public(name),
            };
            self.verify_generated_receipt(&receipt)?;
            Ok(receipt)
        }

        pub fn recover_generated_file<T>(
            &self,
            namespace: &super::GeneratedControlNamespace,
            role: super::GeneratedControlRole,
            maximum: u64,
            validate: impl FnOnce(&[u8], Option<&str>) -> io::Result<(String, T)>,
        ) -> io::Result<Option<(super::GeneratedFileReceipt, T)>> {
            generated_supported()?;
            if maximum == 0
                || maximum > 8192
                || (role == super::GeneratedControlRole::PurgeMarker && maximum > 64)
            {
                return Err(generated_invalid(
                    "generated recovery byte limit is outside its bounds",
                ));
            }
            let public_name = namespace.public_name(role)?;
            let mut candidates = Vec::new();
            if self.entry_exists(&public_name)? {
                candidates.push((public_name.clone(), None));
            }
            let mut claim_count = 0;
            let mut transaction_nonce: Option<String> = None;
            for name in self.read_entry_names(4096)? {
                if let Some(claim) = parse_generated_claim(namespace, &name)? {
                    claim_count += 1;
                    if claim_count > 16 {
                        return Err(generated_invalid("generated control claim limit exceeded"));
                    }
                    if transaction_nonce
                        .as_ref()
                        .is_some_and(|nonce| nonce != &claim.nonce)
                    {
                        return Err(generated_invalid(
                            "multiple generated control transaction nonces are ambiguous",
                        ));
                    }
                    transaction_nonce = Some(claim.nonce.clone());
                    if claim.role == role {
                        candidates.push((name, Some(claim.nonce)));
                    }
                }
            }
            if candidates.len() > 1 {
                return Err(generated_invalid(
                    "public and claimed generated control files are ambiguous",
                ));
            }
            let Some((name, claimed_nonce)) = candidates.pop() else {
                return Ok(None);
            };
            let file = self.open_file(&name)?;
            let file_identity = generated_identity(&file)?;
            let bytes = read_generated_bytes(&file, maximum)?;
            let (nonce, value) = validate(&bytes, claimed_nonce.as_deref())?;
            validate_generated_nonce(&nonce)?;
            if claimed_nonce
                .as_ref()
                .is_some_and(|expected| expected != &nonce)
                || transaction_nonce
                    .as_ref()
                    .is_some_and(|expected| expected != &nonce)
            {
                return Err(generated_invalid(
                    "generated control nonce does not match its recovery namespace",
                ));
            }
            validate_generated_bytes(role, &nonce, &bytes, maximum)?;
            let location = if claimed_nonce.is_some() {
                super::GeneratedFileLocation::Claim(name)
            } else {
                super::GeneratedFileLocation::Public(name)
            };
            let receipt = super::GeneratedFileReceipt {
                file,
                parent_identity: generated_identity(&self.0)?,
                file_identity,
                namespace: namespace.clone(),
                nonce,
                role,
                bytes,
                location,
            };
            self.verify_generated_receipt(&receipt)?;
            Ok(Some((receipt, value)))
        }

        fn verify_generated_receipt(
            &self,
            receipt: &super::GeneratedFileReceipt,
        ) -> io::Result<()> {
            generated_supported()?;
            if generated_identity(&self.0)? != receipt.parent_identity
                || generated_identity(&receipt.file)? != receipt.file_identity
            {
                return Err(generated_invalid(
                    "generated control descriptor identity changed",
                ));
            }
            let name = receipt.current_name().ok_or_else(|| {
                generated_invalid("generated control file has already been unlinked")
            })?;
            match &receipt.location {
                super::GeneratedFileLocation::Public(_)
                    if receipt.namespace.public_name(receipt.role)? != name =>
                {
                    return Err(generated_invalid(
                        "generated control public binding is invalid",
                    ))
                }
                super::GeneratedFileLocation::Claim(_) => {
                    let claim =
                        parse_generated_claim(&receipt.namespace, name)?.ok_or_else(|| {
                            generated_invalid("generated control claim binding is invalid")
                        })?;
                    if claim.nonce != receipt.nonce || claim.role != receipt.role {
                        return Err(generated_invalid(
                            "generated control claim role or nonce changed",
                        ));
                    }
                }
                _ => {}
            }
            let entry = EntryIdentity::at(self, name)?;
            if (entry.device, entry.inode, entry.mode) != receipt.file_identity {
                return Err(generated_invalid(
                    "generated control entry no longer names its admitted descriptor",
                ));
            }
            let bytes = read_generated_bytes(&receipt.file, 8192)?;
            if bytes != receipt.bytes {
                return Err(generated_invalid(
                    "generated control bytes no longer match their admission",
                ));
            }
            let entry = EntryIdentity::at(self, name)?;
            if (entry.device, entry.inode, entry.mode) != receipt.file_identity
                || generated_identity(&receipt.file)? != receipt.file_identity
            {
                return Err(generated_invalid(
                    "generated control entry changed while checking its bytes",
                ));
            }
            Ok(())
        }

        pub fn publish_generated_file(
            &self,
            receipt: &mut super::GeneratedFileReceipt,
            destination: super::GeneratedControlRole,
        ) -> io::Result<()> {
            self.publish_generated_file_with_hook(receipt, destination, |_, _| Ok(()))
        }
        pub(super) fn publish_generated_file_with_hook(
            &self,
            receipt: &mut super::GeneratedFileReceipt,
            destination: super::GeneratedControlRole,
            mut hook: impl FnMut(&'static str, &super::GeneratedFileReceipt) -> io::Result<()>,
        ) -> io::Result<()> {
            if receipt.role != super::GeneratedControlRole::Temporary
                || destination != super::GeneratedControlRole::Main
                || !matches!(receipt.location, super::GeneratedFileLocation::Public(_))
            {
                return Err(generated_invalid(
                    "generated publication requires temporary to main",
                ));
            }
            self.verify_generated_receipt(receipt)?;
            let source = receipt.current_name().unwrap().to_owned();
            let target = receipt.namespace.public_name(destination)?;
            hook("before_publish", receipt)?;
            self.rename_child_exclusive_raw(&source, &target)?;
            receipt.role = destination;
            receipt.location = super::GeneratedFileLocation::Public(target);
            hook("after_publish", receipt)?;
            self.verify_generated_receipt(receipt)?;
            self.sync()
        }

        pub fn exchange_generated_files(
            &self,
            new: &mut super::GeneratedFileReceipt,
            old: &mut super::GeneratedFileReceipt,
        ) -> io::Result<()> {
            self.exchange_generated_files_with_hook(new, old, |_, _, _| Ok(()))
        }
        pub(super) fn exchange_generated_files_with_hook(
            &self,
            new: &mut super::GeneratedFileReceipt,
            old: &mut super::GeneratedFileReceipt,
            mut hook: impl FnMut(
                &'static str,
                &super::GeneratedFileReceipt,
                &super::GeneratedFileReceipt,
            ) -> io::Result<()>,
        ) -> io::Result<()> {
            if new.role != super::GeneratedControlRole::Commit
                || old.role != super::GeneratedControlRole::Main
                || new.namespace != old.namespace
                || new.nonce != old.nonce
                || !matches!(new.location, super::GeneratedFileLocation::Public(_))
                || !matches!(old.location, super::GeneratedFileLocation::Public(_))
            {
                return Err(generated_invalid(
                    "generated exchange requires matching commit and main receipts",
                ));
            }
            self.verify_generated_receipt(new)?;
            self.verify_generated_receipt(old)?;
            let new_name = new.current_name().unwrap().to_owned();
            let old_name = old.current_name().unwrap().to_owned();
            hook("before_exchange", new, old)?;
            self.exchange_children_atomically_raw(&new_name, &old_name)?;
            new.location = super::GeneratedFileLocation::Public(old_name);
            new.role = super::GeneratedControlRole::Main;
            old.location = super::GeneratedFileLocation::Public(new_name);
            old.role = super::GeneratedControlRole::Commit;
            hook("after_exchange", new, old)?;
            self.verify_generated_receipt(new)?;
            self.verify_generated_receipt(old)?;
            self.sync()
        }

        /// Retire internal nonce-bound transaction metadata under the caller's
        /// process/maintenance lock and single or cooperating-writer ownership.
        /// Atomic capture protects replacements at the public name. An existing
        /// writable descriptor can still mutate the captured inode after the
        /// final hash without learning the private claim; that non-cooperating
        /// writer is outside this narrow boundary. Never use this primitive for
        /// sources, configuration, custom uploads, or arbitrary multiwriter files.
        pub fn delete_generated_file(
            &self,
            receipt: &mut super::GeneratedFileReceipt,
        ) -> io::Result<()> {
            self.delete_generated_file_with_hook(receipt, |_, _| Ok(()))
        }
        /// Atomic capture closes the ordinary public-name replacement window.
        /// A same-user program actively replacing the private claim/inode, or
        /// writing through a previously opened descriptor, remains outside this
        /// cooperating-writer boundary; no absolute content-CAS unlink exists.
        pub(super) fn delete_generated_file_with_hook(
            &self,
            receipt: &mut super::GeneratedFileReceipt,
            mut hook: impl FnMut(&'static str, &super::GeneratedFileReceipt) -> io::Result<()>,
        ) -> io::Result<()> {
            self.verify_generated_receipt(receipt)?;
            let public_name = receipt.namespace.public_name(receipt.role)?;
            if matches!(receipt.location, super::GeneratedFileLocation::Public(_)) {
                let source = receipt.current_name().unwrap().to_owned();
                let claim = std::ffi::OsString::from(format!(
                    "{}{}-{}-{}",
                    receipt.namespace.claim_prefix(),
                    receipt.nonce,
                    generated_role_name(receipt.role),
                    uuid::Uuid::new_v4().simple()
                ));
                // Last public identity/content check precedes this hook. Even a
                // replacement here is captured and rejected, never unlinked.
                hook("before_capture", receipt)?;
                self.rename_child_exclusive_raw(&source, &claim)?;
                receipt.location = super::GeneratedFileLocation::Claim(claim);
                hook("after_capture", receipt)?;
                self.sync()?;
            }
            self.verify_generated_receipt(receipt)?;
            hook("after_cleanup_hash", receipt)?;
            self.verify_generated_receipt(receipt)?;
            hook("before_private_unlink", receipt)?;
            let claim = receipt.current_name().unwrap().to_owned();
            let name = c_name(&claim)?;
            let result = unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), 0) };
            if result < 0 {
                return Err(io::Error::last_os_error());
            }
            receipt.location = super::GeneratedFileLocation::Unlinked;
            hook("after_unlink", receipt)?;
            self.sync()?;
            // Never treat a concurrently recreated public entry as the removed
            // metadata, even if it happens to contain the same bytes.
            if self.entry_exists(&public_name)? {
                return Err(generated_invalid(
                    "public generated control entry reappeared during cleanup",
                ));
            }
            Ok(())
        }

        pub fn create_new_file(&self, name: &std::ffi::OsStr) -> io::Result<File> {
            let name = c_name(name)?;
            let fd = unsafe {
                libc::openat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    0o666,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(unsafe { File::from_raw_fd(fd) })
        }

        pub fn create_new_file_read_write(&self, name: &std::ffi::OsStr) -> io::Result<File> {
            let name = c_name(name)?;
            let fd = unsafe {
                libc::openat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDWR
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    0o600,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(unsafe { File::from_raw_fd(fd) })
        }

        pub fn create_new_relative_file(&self, relative: &Path) -> io::Result<File> {
            if relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "relative file path contains an unsafe component",
                ));
            }
            let name = relative
                .file_name()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file name missing"))?
                .to_os_string();
            let parent_path = relative.parent().unwrap_or_else(|| Path::new(""));
            let mut parent = self.try_clone()?;
            for component in parent_path.components() {
                let Component::Normal(name) = component else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "relative directory path contains an unsafe component",
                    ));
                };
                parent = match parent.open_child(name) {
                    Ok(child) => child,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        parent.create_child_directory(name)?;
                        parent.sync()?;
                        parent.open_child(name)?
                    }
                    Err(error) => return Err(error),
                };
            }
            parent.create_new_file(&name)
        }

        pub fn create_child_directory(&self, name: &std::ffi::OsStr) -> io::Result<()> {
            let name = c_name(name)?;
            let result = unsafe { libc::mkdirat(self.0.as_raw_fd(), name.as_ptr(), 0o700) };
            if result < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        pub fn rename_child_exclusive(
            &self,
            source: &std::ffi::OsStr,
            destination: &std::ffi::OsStr,
        ) -> io::Result<()> {
            self.rename_child_exclusive_raw(source, destination)?;
            self.sync()
        }

        fn rename_child_exclusive_raw(
            &self,
            source: &std::ffi::OsStr,
            destination: &std::ffi::OsStr,
        ) -> io::Result<()> {
            self.rename_child_exclusive_to_raw(source, self, destination)
        }

        pub fn rename_child_exclusive_to(
            &self,
            source: &std::ffi::OsStr,
            destination_directory: &Directory,
            destination: &std::ffi::OsStr,
        ) -> io::Result<()> {
            self.rename_child_exclusive_to_raw(source, destination_directory, destination)?;
            self.sync()?;
            if !std::ptr::eq(self, destination_directory) {
                destination_directory.sync()?;
            }
            Ok(())
        }

        // Success means only that the rename syscall completed. Callers that
        // retain ownership state can update it before fallible synchronization.
        pub(super) fn rename_child_exclusive_to_raw(
            &self,
            source: &std::ffi::OsStr,
            destination_directory: &Directory,
            destination: &std::ffi::OsStr,
        ) -> io::Result<()> {
            let source = c_name(source)?;
            let destination = c_name(destination)?;
            #[cfg(target_os = "macos")]
            let result = unsafe {
                libc::renameatx_np(
                    self.0.as_raw_fd(),
                    source.as_ptr(),
                    destination_directory.0.as_raw_fd(),
                    destination.as_ptr(),
                    libc::RENAME_EXCL,
                )
            };
            #[cfg(target_os = "linux")]
            let result = unsafe {
                libc::renameat2(
                    self.0.as_raw_fd(),
                    source.as_ptr(),
                    destination_directory.0.as_raw_fd(),
                    destination.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            let result = return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "exclusive directory rename is unavailable on this Unix host",
            ));
            if result < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        pub fn replace_child(
            &self,
            source: &std::ffi::OsStr,
            destination: &std::ffi::OsStr,
        ) -> io::Result<()> {
            self.replace_child_with_hook(source, destination, || {})
        }

        pub(super) fn replace_child_with_hook<F>(
            &self,
            source: &std::ffi::OsStr,
            destination: &std::ffi::OsStr,
            after_exchange: F,
        ) -> io::Result<()>
        where
            F: FnOnce(),
        {
            let source_name = source.to_os_string();
            let destination_name = destination.to_os_string();
            let _ = c_name(&source_name)?;
            let _ = c_name(&destination_name)?;
            let source_identity = EntryIdentity::at(self, &source_name)?;
            let destination_identity = match EntryIdentity::at(self, &destination_name) {
                Ok(identity) => identity,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return self.rename_child_exclusive(&source_name, &destination_name);
                }
                Err(error) => return Err(error),
            };

            let nonce = uuid::Uuid::new_v4().simple().to_string();
            let journal_name =
                std::ffi::OsString::from(format!(".gogglelab-replace-{nonce}.journal"));
            let journal = format!(
                "version=1\nsource_hex={}\ndestination_hex={}\nsource_device={}\nsource_inode={}\nsource_mode={}\ndestination_device={}\ndestination_inode={}\ndestination_mode={}\n",
                hex_bytes(source_name.as_bytes()),
                hex_bytes(destination_name.as_bytes()),
                source_identity.device,
                source_identity.inode,
                source_identity.mode,
                destination_identity.device,
                destination_identity.inode,
                destination_identity.mode,
            );
            let mut journal_file = self.create_new_file_read_write(&journal_name)?;
            journal_file.write_all(journal.as_bytes())?;
            journal_file.sync_all()?;
            self.sync()?;

            self.exchange_children_atomically(&source_name, &destination_name)?;
            self.write_hold_marker(&journal_name, "moved")?;
            after_exchange();

            if EntryIdentity::at(self, &source_name)? != destination_identity
                || EntryIdentity::at(self, &destination_name)? != source_identity
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "replacement entries changed after atomic exchange; both names and journal {} were preserved",
                        journal_name.to_string_lossy()
                    ),
                ));
            }
            self.hold_entry_with_identity(&source_name, destination_identity, None, || {}, || {})?;
            self.write_hold_marker(&journal_name, "verified")
        }

        /// Replace a file only when the existing destination still matches
        /// the fingerprint observed by the caller. The previous file is kept
        /// in a recoverable hold after a successful replacement.
        pub fn replace_child_if_same(
            &self,
            source: &std::ffi::OsStr,
            destination: &std::ffi::OsStr,
            expected_old: &(u64, String),
            max_bytes: u64,
        ) -> io::Result<()> {
            self.replace_child_if_same_internal(source, destination, expected_old, max_bytes, || {})
        }

        #[cfg(test)]
        pub(crate) fn replace_child_if_same_with_hook<F>(
            &self,
            source: &std::ffi::OsStr,
            destination: &std::ffi::OsStr,
            expected_old: &(u64, String),
            max_bytes: u64,
            after_preflight: F,
        ) -> io::Result<()>
        where
            F: FnOnce(),
        {
            self.replace_child_if_same_internal(
                source,
                destination,
                expected_old,
                max_bytes,
                after_preflight,
            )
        }

        fn replace_child_if_same_internal<F>(
            &self,
            source: &std::ffi::OsStr,
            destination: &std::ffi::OsStr,
            expected_old: &(u64, String),
            max_bytes: u64,
            after_preflight: F,
        ) -> io::Result<()>
        where
            F: FnOnce(),
        {
            let source_name = source.to_os_string();
            let destination_name = destination.to_os_string();
            let mut source_file = self.open_file(&source_name)?;
            let mut destination_file = self.open_file(&destination_name)?;
            let source_identity = EntryIdentity::from_file(&source_file)?;
            let destination_identity = EntryIdentity::from_file(&destination_file)?;
            let source_hash = hash_file(&mut source_file, max_bytes)?;
            let current_old_hash = hash_file(&mut destination_file, max_bytes)?;
            if current_old_hash != *expected_old {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "replacement destination changed since preflight",
                ));
            }
            after_preflight();

            let nonce = uuid::Uuid::new_v4().simple().to_string();
            let journal_name =
                std::ffi::OsString::from(format!(".gogglelab-replace-{nonce}.journal"));
            let journal = format!(
                "version=1\nsource_hex={}\ndestination_hex={}\nsource_device={}\nsource_inode={}\nsource_mode={}\ndestination_device={}\ndestination_inode={}\ndestination_mode={}\nsource_bytes={}\nsource_sha256={}\nexpected_old_bytes={}\nexpected_old_sha256={}\n",
                hex_bytes(source_name.as_bytes()),
                hex_bytes(destination_name.as_bytes()),
                source_identity.device,
                source_identity.inode,
                source_identity.mode,
                destination_identity.device,
                destination_identity.inode,
                destination_identity.mode,
                source_hash.0,
                source_hash.1,
                expected_old.0,
                expected_old.1,
            );
            let mut journal_file = self.create_new_file_read_write(&journal_name)?;
            journal_file.write_all(journal.as_bytes())?;
            journal_file.sync_all()?;
            self.sync()?;

            self.exchange_children_atomically(&source_name, &destination_name)?;
            self.write_hold_marker(&journal_name, "moved")?;
            let post_exchange_matches = EntryIdentity::at(self, &source_name)?
                == destination_identity
                && EntryIdentity::at(self, &destination_name)? == source_identity
                && self.hash_file(&source_name, max_bytes)? == *expected_old
                && self.hash_file(&destination_name, max_bytes)? == source_hash;
            if !post_exchange_matches {
                let rollback = self.exchange_children_atomically(&source_name, &destination_name);
                let rollback_detail = match rollback {
                    Ok(()) => {
                        let _ = self.write_hold_marker(&journal_name, "restored");
                        "atomic replacement was rolled back after the destination changed"
                    }
                    Err(error) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "replacement entries remain under both names with journal {}; no-clobber exchange rollback failed: {error}",
                                journal_name.to_string_lossy()
                            ),
                        ));
                    }
                };
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{rollback_detail}; journal {} retained",
                        journal_name.to_string_lossy()
                    ),
                ));
            }
            if let Err(error) = self.hold_entry_with_identity(
                &source_name,
                destination_identity,
                Some(expected_old),
                || {},
                || {},
            ) {
                let rollback = self.exchange_children_atomically(&source_name, &destination_name);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "replacement hold failed: {error}; atomic exchange rollback: {rollback:?}; journal {} retained",
                        journal_name.to_string_lossy()
                    ),
                ));
            }
            self.write_hold_marker(&journal_name, "verified")
        }

        fn exchange_children_atomically(
            &self,
            first: &std::ffi::OsStr,
            second: &std::ffi::OsStr,
        ) -> io::Result<()> {
            self.exchange_children_atomically_raw(first, second)?;
            self.sync()
        }

        fn exchange_children_atomically_raw(
            &self,
            first: &std::ffi::OsStr,
            second: &std::ffi::OsStr,
        ) -> io::Result<()> {
            let first = c_name(first)?;
            let second = c_name(second)?;
            #[cfg(target_os = "macos")]
            let result = unsafe {
                libc::renameatx_np(
                    self.0.as_raw_fd(),
                    first.as_ptr(),
                    self.0.as_raw_fd(),
                    second.as_ptr(),
                    2, // RENAME_SWAP
                )
            };
            #[cfg(target_os = "linux")]
            let result = unsafe {
                libc::renameat2(
                    self.0.as_raw_fd(),
                    first.as_ptr(),
                    self.0.as_raw_fd(),
                    second.as_ptr(),
                    2, // RENAME_EXCHANGE
                )
            };
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            let result = return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "atomic entry exchange is unavailable on this Unix host",
            ));
            if result < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        pub fn path(&self) -> io::Result<std::path::PathBuf> {
            #[cfg(target_os = "linux")]
            {
                return std::fs::canonicalize(format!("/proc/self/fd/{}", self.0.as_raw_fd()));
            }
            #[cfg(target_os = "macos")]
            {
                use std::ffi::CStr;
                use std::os::unix::ffi::OsStringExt;

                let mut buffer = [0_i8; libc::PATH_MAX as usize];
                let result = unsafe {
                    libc::fcntl(self.0.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr())
                };
                if result < 0 {
                    return Err(io::Error::last_os_error());
                }
                let bytes = unsafe { CStr::from_ptr(buffer.as_ptr()) }
                    .to_bytes()
                    .to_vec();
                Ok(std::ffi::OsString::from_vec(bytes).into())
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "directory paths are unavailable on this Unix host",
                ))
            }
        }

        pub fn try_clone(&self) -> io::Result<Self> {
            Ok(Self(self.0.try_clone()?))
        }

        pub fn link_child(
            &self,
            source: &std::ffi::OsStr,
            destination: &std::ffi::OsStr,
        ) -> io::Result<()> {
            let source = c_name(source)?;
            let destination = c_name(destination)?;
            let result = unsafe {
                libc::linkat(
                    self.0.as_raw_fd(),
                    source.as_ptr(),
                    self.0.as_raw_fd(),
                    destination.as_ptr(),
                    0,
                )
            };
            if result < 0 {
                Err(io::Error::last_os_error())
            } else {
                self.sync()
            }
        }

        pub fn entry_exists(&self, name: &std::ffi::OsStr) -> io::Result<bool> {
            let name = c_name(name)?;
            let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
            let result = unsafe {
                libc::fstatat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    &mut metadata,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if result == 0 {
                Ok(true)
            } else {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::NotFound {
                    Ok(false)
                } else {
                    Err(error)
                }
            }
        }

        pub fn child_path(&self, name: &std::ffi::OsStr) -> io::Result<std::path::PathBuf> {
            let _ = c_name(name)?;
            Ok(self.path()?.join(name))
        }

        /// Remove a directory entry from its active name by moving the entire
        /// entry to a recoverable hold. Contents are not recursively unlinked.
        pub fn remove_child_tree(&self, name: &std::ffi::OsStr) -> io::Result<()> {
            self.hold_entry(name).map(|_| ())
        }

        /// Compatibility-named cleanup that moves the entry into a durable
        /// hold. It never unlinks file bytes.
        pub fn unlink_file(&self, name: &std::ffi::OsStr) -> io::Result<()> {
            self.hold_entry(name).map(|_| ())
        }

        pub fn unlink_file_if_same(
            &self,
            name: &std::ffi::OsStr,
            expected_file: &File,
        ) -> io::Result<HeldEntry> {
            let expected = EntryIdentity::from_file(expected_file)?;
            let current = EntryIdentity::at(self, name)?;
            if expected.device != current.device || expected.inode != current.inode {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "entry no longer refers to the created file",
                ));
            }
            self.hold_entry_with_identity(name, expected, None, || {}, || {})
        }

        /// Permanently delete a generated file only when its bytes and entry
        /// identity still match the admitted snapshot. Original model sources
        /// must use a recoverable hold instead.
        pub fn delete_verified_file(
            &self,
            name: &std::ffi::OsStr,
            expected: &(u64, String),
            max_bytes: u64,
        ) -> io::Result<()> {
            self.delete_verified_file_with_hook(name, expected, max_bytes, || {})
        }

        pub(super) fn delete_verified_file_with_hook<F>(
            &self,
            name: &std::ffi::OsStr,
            expected: &(u64, String),
            max_bytes: u64,
            after_hash: F,
        ) -> io::Result<()>
        where
            F: FnOnce(),
        {
            let mut file = self.open_file(name)?;
            let actual = hash_file(&mut file, max_bytes)?;
            if &actual != expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "generated file bytes changed before cleanup",
                ));
            }
            let expected_identity = EntryIdentity::from_file(&file)?;
            after_hash();
            if EntryIdentity::at(self, name)? != expected_identity {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "generated file was replaced before cleanup",
                ));
            }
            let name = c_name(name)?;
            let result = unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), 0) };
            if result < 0 {
                return Err(io::Error::last_os_error());
            }
            self.sync()
        }

        /// Move an entry out of its active name into a unique same-directory
        /// hold. A durable journal is written before the exclusive rename. The
        /// held entry is never unlinked by this crate.
        pub fn hold_entry(&self, name: &std::ffi::OsStr) -> io::Result<HeldEntry> {
            let identity = EntryIdentity::at(self, name)?;
            self.hold_entry_with_identity(name, identity, None, || {}, || {})
        }

        fn hold_entry_with_identity<F, G>(
            &self,
            name: &std::ffi::OsStr,
            identity: EntryIdentity,
            expected_hash: Option<&(u64, String)>,
            after_journal: F,
            after_rename: G,
        ) -> io::Result<HeldEntry>
        where
            F: FnOnce(),
            G: FnOnce(),
        {
            self.hold_entry_with_hook(name, identity, expected_hash, after_journal, after_rename)
        }

        fn hold_entry_with_hook<F, G>(
            &self,
            name: &std::ffi::OsStr,
            identity: EntryIdentity,
            expected_hash: Option<&(u64, String)>,
            after_journal: F,
            after_rename: G,
        ) -> io::Result<HeldEntry>
        where
            F: FnOnce(),
            G: FnOnce(),
        {
            let source_name = name.to_os_string();
            let _ = c_name(&source_name)?;
            let nonce = uuid::Uuid::new_v4().simple().to_string();
            let hold_name = std::ffi::OsString::from(format!(".gogglelab-held-{nonce}.data"));
            let journal_name = std::ffi::OsString::from(format!(".gogglelab-held-{nonce}.journal"));
            let journal = format!(
                "version=1\noriginal_hex={}\nhold_name={}\ndevice={}\ninode={}\nmode={}\nexpected_bytes={}\nexpected_sha256={}\n",
                hex_bytes(source_name.as_bytes()),
                hold_name.to_string_lossy(),
                identity.device,
                identity.inode,
                identity.mode,
                expected_hash.map_or_else(|| "unknown".to_owned(), |hash| hash.0.to_string()),
                expected_hash.map_or("unknown", |hash| hash.1.as_str()),
            );
            let mut journal_file = self.create_new_file_read_write(&journal_name)?;
            journal_file.write_all(journal.as_bytes())?;
            journal_file.sync_all()?;
            self.sync()?;
            after_journal();

            if let Err(error) = self.rename_child_exclusive(&source_name, &hold_name) {
                // The prepared journal is intentionally retained: after an
                // interrupted or failed operation it is evidence, not debris.
                return Err(error);
            }
            self.write_hold_marker(&journal_name, "moved")?;
            after_rename();

            let actual_identity = match EntryIdentity::at(self, &hold_name) {
                Ok(identity) => identity,
                Err(error) => return Err(error),
            };
            if actual_identity.device != identity.device || actual_identity.inode != identity.inode
            {
                return self.restore_held_entry_no_clobber(
                    &source_name,
                    &hold_name,
                    &journal_name,
                    "entry identity changed during hold",
                );
            }
            if let Some(expected) = expected_hash {
                match self.hash_file(&hold_name, expected.0.max(1)) {
                    Ok(actual) if actual == *expected => {}
                    Ok(_) => {
                        return self.restore_held_entry_no_clobber(
                            &source_name,
                            &hold_name,
                            &journal_name,
                            "entry hash changed during hold",
                        );
                    }
                    Err(error) => {
                        let _ = self.restore_held_entry_no_clobber(
                            &source_name,
                            &hold_name,
                            &journal_name,
                            "entry could not be verified after hold",
                        );
                        return Err(error);
                    }
                }
            }
            if self.entry_exists(&source_name)? {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "source path was replaced; original retained as {} with journal {}",
                        hold_name.to_string_lossy(),
                        journal_name.to_string_lossy()
                    ),
                ));
            }
            self.write_hold_marker(&journal_name, "verified")?;
            Ok(HeldEntry {
                original_name: source_name,
                hold_name,
                journal_name,
                expected: expected_hash.cloned(),
                device: identity.device,
                inode: identity.inode,
                mode: identity.mode,
            })
        }

        fn restore_held_entry_no_clobber(
            &self,
            original_name: &std::ffi::OsStr,
            hold_name: &std::ffi::OsStr,
            journal_name: &std::ffi::OsStr,
            reason: &str,
        ) -> io::Result<HeldEntry> {
            let restoration = self.rename_child_exclusive(hold_name, original_name);
            if restoration.is_ok() {
                self.write_hold_marker(journal_name, "restored")?;
                return Err(io::Error::new(io::ErrorKind::InvalidData, reason));
            }
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{reason}; original preserved as {} with journal {} because no-clobber restoration failed: {restoration:?}",
                    hold_name.to_string_lossy(),
                    journal_name.to_string_lossy()
                ),
            ))
        }

        fn write_hold_marker(&self, journal_name: &std::ffi::OsStr, phase: &str) -> io::Result<()> {
            let marker_name =
                std::ffi::OsString::from(format!("{}.{}", journal_name.to_string_lossy(), phase));
            let mut marker = self.create_new_file_read_write(&marker_name)?;
            marker.write_all(phase.as_bytes())?;
            marker.sync_all()?;
            self.sync()
        }

        pub fn restore_held_entry(
            &self,
            held: &HeldEntry,
            original_name: &std::ffi::OsStr,
        ) -> io::Result<()> {
            let held_identity = EntryIdentity::at(self, &held.hold_name)?;
            if held_identity.device != held.device
                || held_identity.inode != held.inode
                || held_identity.mode != held.mode
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "held entry identity changed; bytes were preserved",
                ));
            }
            if let Some(expected) = &held.expected {
                if self.hash_file(&held.hold_name, expected.0.max(1))? != *expected {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "held file hash changed; bytes were preserved",
                    ));
                }
            }
            self.rename_child_exclusive(&held.hold_name, original_name)?;
            let restored_identity = EntryIdentity::at(self, original_name)?;
            if restored_identity.device != held.device || restored_identity.inode != held.inode {
                let _ = self.rename_child_exclusive(original_name, &held.hold_name);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "restored entry identity changed; recoverable bytes were retained",
                ));
            }
            self.write_hold_marker(&held.journal_name, "restored")
        }

        /// Reconstruct and verify a hold receipt from its durable journal.
        /// This supports manual crash recovery without trusting a path string
        /// copied from the interrupted process.
        pub fn recover_held_entry(&self, journal_name: &std::ffi::OsStr) -> io::Result<HeldEntry> {
            let mut journal_file = self.open_file(journal_name)?;
            if journal_file.metadata()?.len() > 8 * 1024 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "hold journal exceeds its byte limit",
                ));
            }
            let mut contents = String::new();
            journal_file.read_to_string(&mut contents)?;
            let fields = parse_journal_fields(&contents)?;
            if fields.get("version").map(String::as_str) != Some("1") {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unsupported hold journal version",
                ));
            }
            let original_bytes = decode_hex(required_journal_field(&fields, "original_hex")?)?;
            let original_name = OsString::from_vec(original_bytes);
            let _ = c_name(&original_name)?;
            let hold_name = OsString::from(required_journal_field(&fields, "hold_name")?);
            let _ = c_name(&hold_name)?;
            if !hold_name.to_string_lossy().starts_with(".gogglelab-held-")
                || !hold_name.to_string_lossy().ends_with(".data")
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "hold journal contains an invalid held name",
                ));
            }
            let identity = EntryIdentity {
                device: parse_journal_number(&fields, "device")?,
                inode: parse_journal_number(&fields, "inode")?,
                mode: u32::try_from(parse_journal_number(&fields, "mode")?).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "hold mode is invalid")
                })?,
            };
            if EntryIdentity::at(self, &hold_name)? != identity {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "held entry identity does not match its journal",
                ));
            }
            let expected_bytes = required_journal_field(&fields, "expected_bytes")?;
            let expected_hash = required_journal_field(&fields, "expected_sha256")?;
            let expected = if expected_bytes == "unknown" && expected_hash == "unknown" {
                None
            } else {
                let bytes = expected_bytes.parse::<u64>().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "hold byte count is invalid")
                })?;
                if expected_hash.len() != 64
                    || !expected_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "hold hash is invalid",
                    ));
                }
                let expected = (bytes, expected_hash.to_ascii_lowercase());
                if self.hash_file(&hold_name, bytes.max(1))? != expected {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "held bytes do not match their journal",
                    ));
                }
                Some(expected)
            };
            Ok(HeldEntry {
                original_name,
                hold_name,
                journal_name: journal_name.to_os_string(),
                expected,
                device: identity.device,
                inode: identity.inode,
                mode: identity.mode,
            })
        }

        pub fn hold_verified_file(
            &self,
            name: &std::ffi::OsStr,
            expected: &(u64, String),
            max_bytes: u64,
        ) -> io::Result<HeldEntry> {
            self.hold_verified_file_with_hooks(name, expected, max_bytes, || {}, || {})
        }

        /// Hold a verified file under caller-selected names. This supports
        /// deterministic recovery across higher-level metadata commits. Names
        /// are exclusive and restricted to the hold namespace.
        pub fn hold_verified_file_as(
            &self,
            source_name: &std::ffi::OsStr,
            expected: &(u64, String),
            max_bytes: u64,
            hold_name: &std::ffi::OsStr,
            journal_name: &std::ffi::OsStr,
        ) -> io::Result<HeldEntry> {
            self.hold_verified_file_as_internal(
                source_name,
                expected,
                max_bytes,
                hold_name,
                journal_name,
                || {},
            )
        }

        #[cfg(test)]
        pub(super) fn hold_verified_file_as_with_hook<F>(
            &self,
            source_name: &std::ffi::OsStr,
            expected: &(u64, String),
            max_bytes: u64,
            hold_name: &std::ffi::OsStr,
            journal_name: &std::ffi::OsStr,
            after_rename: F,
        ) -> io::Result<HeldEntry>
        where
            F: FnOnce(),
        {
            self.hold_verified_file_as_internal(
                source_name,
                expected,
                max_bytes,
                hold_name,
                journal_name,
                after_rename,
            )
        }

        fn hold_verified_file_as_internal<F>(
            &self,
            source_name: &std::ffi::OsStr,
            expected: &(u64, String),
            max_bytes: u64,
            hold_name: &std::ffi::OsStr,
            journal_name: &std::ffi::OsStr,
            after_rename: F,
        ) -> io::Result<HeldEntry>
        where
            F: FnOnce(),
        {
            if expected.0 > max_bytes {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "expected file exceeds its byte limit",
                ));
            }
            let source_name = source_name.to_os_string();
            let hold_name = hold_name.to_os_string();
            let journal_name = journal_name.to_os_string();
            let _ = c_name(&source_name)?;
            if !valid_hold_component(&hold_name, ".data")
                || !valid_hold_component(&journal_name, ".journal")
                || source_name == hold_name
                || source_name == journal_name
                || hold_name == journal_name
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "deterministic hold and journal names are invalid",
                ));
            }
            if self.entry_exists(&hold_name)? || self.entry_exists(&journal_name)? {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "deterministic hold or journal name is already occupied",
                ));
            }

            let mut source_file = self.open_file(&source_name)?;
            let identity = EntryIdentity::from_file(&source_file)?;
            if hash_file(&mut source_file, max_bytes)? != *expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "file hash changed before hold",
                ));
            }
            let journal = format!(
                "version=1\noriginal_hex={}\nhold_name={}\ndevice={}\ninode={}\nmode={}\nexpected_bytes={}\nexpected_sha256={}\n",
                hex_bytes(source_name.as_bytes()),
                hold_name.to_string_lossy(),
                identity.device,
                identity.inode,
                identity.mode,
                expected.0,
                expected.1,
            );
            let mut journal_file = self.create_new_file_read_write(&journal_name)?;
            journal_file.write_all(journal.as_bytes())?;
            journal_file.sync_all()?;
            self.sync()?;

            if let Err(error) = self.rename_child_exclusive(&source_name, &hold_name) {
                let moved = self.entry_exists(&hold_name).unwrap_or(false)
                    && !self.entry_exists(&source_name).unwrap_or(true);
                return if moved {
                    Err(io::Error::new(
                        error.kind(),
                        format!(
                            "source moved to deterministic hold {} with journal {} but directory sync failed: {error}",
                            hold_name.to_string_lossy(),
                            journal_name.to_string_lossy()
                        ),
                    ))
                } else {
                    Err(error)
                };
            }
            if let Err(error) = self.write_hold_marker(&journal_name, "moved") {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "source retained as deterministic hold {} with journal {} after phase marker failure: {error}",
                        hold_name.to_string_lossy(),
                        journal_name.to_string_lossy()
                    ),
                ));
            }
            after_rename();

            let actual_identity = match EntryIdentity::at(self, &hold_name) {
                Ok(identity) => identity,
                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!(
                            "source may be retained as deterministic hold {} with journal {} but could not be inspected: {error}",
                            hold_name.to_string_lossy(),
                            journal_name.to_string_lossy()
                        ),
                    ));
                }
            };
            if actual_identity != identity {
                return self.restore_held_entry_no_clobber(
                    &source_name,
                    &hold_name,
                    &journal_name,
                    "deterministic held entry identity changed",
                );
            }
            match self.hash_file(&hold_name, expected.0.max(1)) {
                Ok(actual) if actual == *expected => {}
                Ok(_) => {
                    return self.restore_held_entry_no_clobber(
                        &source_name,
                        &hold_name,
                        &journal_name,
                        "deterministic held file hash changed",
                    );
                }
                Err(error) => {
                    let restore = self.restore_held_entry_no_clobber(
                        &source_name,
                        &hold_name,
                        &journal_name,
                        "deterministic held file could not be verified",
                    );
                    return Err(io::Error::new(
                        error.kind(),
                        format!(
                            "deterministic hold {} with journal {} could not be verified: {error}; restoration: {restore:?}",
                            hold_name.to_string_lossy(),
                            journal_name.to_string_lossy()
                        ),
                    ));
                }
            }
            if self.entry_exists(&source_name)? {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "source path was replaced; original retained as deterministic hold {} with journal {}",
                        hold_name.to_string_lossy(),
                        journal_name.to_string_lossy()
                    ),
                ));
            }
            if let Err(error) = self.write_hold_marker(&journal_name, "verified") {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "source retained as deterministic hold {} with journal {} after verification marker failure: {error}",
                        hold_name.to_string_lossy(),
                        journal_name.to_string_lossy()
                    ),
                ));
            }
            Ok(HeldEntry {
                original_name: source_name,
                hold_name,
                journal_name,
                expected: Some(expected.clone()),
                device: identity.device,
                inode: identity.inode,
                mode: identity.mode,
            })
        }

        pub(super) fn hold_verified_file_with_hooks<F, G>(
            &self,
            name: &std::ffi::OsStr,
            expected: &(u64, String),
            max_bytes: u64,
            after_hash: F,
            after_rename: G,
        ) -> io::Result<HeldEntry>
        where
            F: FnOnce(),
            G: FnOnce(),
        {
            if expected.0 > max_bytes {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "expected file exceeds its byte limit",
                ));
            }
            let mut verified_file = self.open_file(name)?;
            let identity = EntryIdentity::from_file(&verified_file)?;
            if hash_file(&mut verified_file, max_bytes)? != *expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "file hash changed",
                ));
            }
            after_hash();
            self.hold_entry_with_identity(name, identity, Some(expected), || {}, after_rename)
        }

        /// Atomically move a verified file to another directory on the same
        /// filesystem. Cross-device moves fail closed. The durable journal is
        /// retained next to the original path for crash recovery.
        pub fn move_verified_file_to(
            &self,
            source_name: &std::ffi::OsStr,
            destination_directory: &Directory,
            destination_name: &std::ffi::OsStr,
            expected: &(u64, String),
            max_bytes: u64,
        ) -> io::Result<MovedEntry> {
            self.move_verified_file_to_with_hooks(
                source_name,
                destination_directory,
                destination_name,
                expected,
                max_bytes,
                || {},
                || {},
            )
        }

        pub(super) fn move_verified_file_to_with_hooks<F, G>(
            &self,
            source_name: &std::ffi::OsStr,
            destination_directory: &Directory,
            destination_name: &std::ffi::OsStr,
            expected: &(u64, String),
            max_bytes: u64,
            after_journal: F,
            after_rename: G,
        ) -> io::Result<MovedEntry>
        where
            F: FnOnce(),
            G: FnOnce(),
        {
            let source_name = source_name.to_os_string();
            let destination_name = destination_name.to_os_string();
            let source = self.open_file(&source_name)?;
            let identity = EntryIdentity::from_file(&source)?;
            let mut source_for_hash = source.try_clone()?;
            if hash_file(&mut source_for_hash, max_bytes)? != *expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "file hash changed before move",
                ));
            }
            let nonce = uuid::Uuid::new_v4().simple().to_string();
            let journal_name = std::ffi::OsString::from(format!(".gogglelab-move-{nonce}.journal"));
            let journal = format!(
                "version=1\nsource_hex={}\ndestination_hex={}\ndevice={}\ninode={}\nmode={}\nexpected_bytes={}\nexpected_sha256={}\n",
                hex_bytes(source_name.as_bytes()),
                hex_bytes(destination_name.as_bytes()),
                identity.device,
                identity.inode,
                identity.mode,
                expected.0,
                expected.1,
            );
            let mut journal_file = self.create_new_file_read_write(&journal_name)?;
            journal_file.write_all(journal.as_bytes())?;
            journal_file.sync_all()?;
            self.sync()?;
            after_journal();

            self.rename_child_exclusive_to(&source_name, destination_directory, &destination_name)?;
            self.write_hold_marker(&journal_name, "moved")?;
            after_rename();

            let moved_identity = EntryIdentity::at(destination_directory, &destination_name)?;
            let mut moved_file = destination_directory.open_file(&destination_name)?;
            let moved_hash = hash_file(&mut moved_file, max_bytes)?;
            if moved_identity.device != identity.device
                || moved_identity.inode != identity.inode
                || moved_hash != *expected
            {
                let restoration = destination_directory.rename_child_exclusive_to(
                    &destination_name,
                    self,
                    &source_name,
                );
                let detail = if restoration.is_ok() {
                    self.write_hold_marker(&journal_name, "restored")?;
                    "moved entry changed; original path restored"
                } else {
                    "moved entry changed; bytes remain at destination because no-clobber restoration failed"
                };
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{detail}; move journal {} retained",
                        journal_name.to_string_lossy()
                    ),
                ));
            }
            if self.entry_exists(&source_name)? {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "source path was replaced; moved bytes remain at destination with journal {}",
                        journal_name.to_string_lossy()
                    ),
                ));
            }
            self.write_hold_marker(&journal_name, "verified")?;
            Ok(MovedEntry {
                destination_name,
                journal_name,
                expected: Some(expected.clone()),
                device: identity.device,
                inode: identity.inode,
                mode: identity.mode,
            })
        }

        /// Move a file, directory, or other entry without replacing an
        /// existing destination. This identity-only form supports visible
        /// folders whose contents are not represented by a catalog hash.
        pub fn move_entry_to(
            &self,
            source_name: &std::ffi::OsStr,
            destination_directory: &Directory,
            destination_name: &std::ffi::OsStr,
        ) -> io::Result<MovedEntry> {
            self.move_entry_to_with_hooks(
                source_name,
                destination_directory,
                destination_name,
                || {},
            )
        }

        pub(super) fn move_entry_to_with_hooks<F>(
            &self,
            source_name: &std::ffi::OsStr,
            destination_directory: &Directory,
            destination_name: &std::ffi::OsStr,
            after_rename: F,
        ) -> io::Result<MovedEntry>
        where
            F: FnOnce(),
        {
            let source_name = source_name.to_os_string();
            let destination_name = destination_name.to_os_string();
            let identity = EntryIdentity::at(self, &source_name)?;
            let nonce = uuid::Uuid::new_v4().simple().to_string();
            let journal_name = std::ffi::OsString::from(format!(".gogglelab-move-{nonce}.journal"));
            let journal = format!(
                "version=1\nsource_hex={}\ndestination_hex={}\ndevice={}\ninode={}\nmode={}\nexpected_bytes=unknown\nexpected_sha256=unknown\n",
                hex_bytes(source_name.as_bytes()),
                hex_bytes(destination_name.as_bytes()),
                identity.device,
                identity.inode,
                identity.mode,
            );
            let mut journal_file = self.create_new_file_read_write(&journal_name)?;
            journal_file.write_all(journal.as_bytes())?;
            journal_file.sync_all()?;
            self.sync()?;
            self.rename_child_exclusive_to(&source_name, destination_directory, &destination_name)?;
            self.write_hold_marker(&journal_name, "moved")?;
            after_rename();

            let moved_identity = EntryIdentity::at(destination_directory, &destination_name)?;
            if moved_identity != identity {
                let restoration = destination_directory.rename_child_exclusive_to(
                    &destination_name,
                    self,
                    &source_name,
                );
                let detail = if restoration.is_ok() {
                    self.write_hold_marker(&journal_name, "restored")?;
                    "moved entry identity changed; no-clobber restoration completed"
                } else {
                    "moved entry identity changed; entry remains recoverable because no-clobber restoration failed"
                };
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{detail}; journal {} retained",
                        journal_name.to_string_lossy()
                    ),
                ));
            }
            if self.entry_exists(&source_name)? {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "source path was replaced; moved entry remains at destination with journal {}",
                        journal_name.to_string_lossy()
                    ),
                ));
            }
            self.write_hold_marker(&journal_name, "verified")?;
            Ok(MovedEntry {
                destination_name,
                journal_name,
                expected: None,
                device: identity.device,
                inode: identity.inode,
                mode: identity.mode,
            })
        }

        /// Move an empty directory to a recoverable hold instead of unlinking
        /// it; a concurrent population remains preserved in that hold.
        pub fn remove_empty_directory(&self, name: &std::ffi::OsStr) -> io::Result<()> {
            let child = self.open_child(name)?;
            if !child.read_entry_names(1)?.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::DirectoryNotEmpty,
                    "directory is not empty",
                ));
            }
            drop(child);
            self.hold_entry(name).map(|_| ())
        }

        pub fn remove_empty_relative_directory(&self, relative: &Path) -> io::Result<()> {
            let name = relative.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "directory name missing")
            })?;
            let parent_path = relative.parent().unwrap_or_else(|| Path::new(""));
            let parent = self.open_relative(parent_path)?;
            parent.remove_empty_directory(name)?;
            parent.sync()
        }

        pub fn sync(&self) -> io::Result<()> {
            self.0.sync_all()
        }

        pub fn sync_tree(&self) -> io::Result<()> {
            for entry in std::fs::read_dir(self.path()?)? {
                let entry = entry?;
                let name = entry.file_name();
                match self.open_child(&name) {
                    Ok(child) => child.sync_tree()?,
                    Err(error)
                        if error.kind() == io::ErrorKind::NotADirectory
                            || error.raw_os_error() == Some(libc::ELOOP) =>
                    {
                        self.open_file(&name)?.sync_all()?;
                    }
                    Err(error) => return Err(error),
                }
            }
            self.sync()
        }

        pub fn matches_path(&self, path: &Path) -> io::Result<bool> {
            let other = Self::open(path)?;
            let left = self.0.metadata()?;
            let right = other.0.metadata()?;
            use std::os::unix::fs::MetadataExt;
            Ok(left.dev() == right.dev() && left.ino() == right.ino())
        }
    }

    pub fn hash_file(file: &mut File, max_bytes: u64) -> io::Result<(u64, String)> {
        use std::io::Read;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file is not regular or exceeds its byte limit",
            ));
        }
        let mut digest = Sha256::new();
        let mut bytes = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes = bytes
                .checked_add(count as u64)
                .ok_or_else(|| io::Error::other("file size overflow"))?;
            if bytes > metadata.len() || bytes > max_bytes {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "file changed"));
            }
            digest.update(&buffer[..count]);
        }
        if bytes != metadata.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "file changed"));
        }
        Ok((bytes, format!("{:x}", digest.finalize())))
    }

    #[cfg(test)]
    pub fn copy_verified_file_with<F>(
        source_root: &Path,
        source_relative: &Path,
        destination_root: &Path,
        destination_name: &std::ffi::OsStr,
        expected: &(u64, String),
        max_bytes: u64,
        after_open: F,
    ) -> io::Result<std::fs::Metadata>
    where
        F: FnOnce(),
    {
        let (source_parent, source_name) = open_parent_of(source_root, source_relative)?;
        let destination_parent = Directory::open(destination_root)?;
        after_open();
        copy_verified_into_directory(
            &source_parent,
            &source_name,
            &destination_parent,
            destination_name,
            expected,
            max_bytes,
        )
    }

    pub fn copy_verified_into_directory(
        source_parent: &Directory,
        source_name: &std::ffi::OsStr,
        destination_parent: &Directory,
        destination_name: &std::ffi::OsStr,
        expected: &(u64, String),
        max_bytes: u64,
    ) -> io::Result<std::fs::Metadata> {
        let mut source = source_parent.open_file(source_name)?;
        let metadata = source.metadata()?;
        if metadata.len() != expected.0 || metadata.len() > max_bytes {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "source changed"));
        }
        let mut destination = destination_parent.create_new_file(destination_name)?;
        let mut digest = Sha256::new();
        let mut bytes = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        let result = (|| {
            loop {
                let count = source.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                bytes = bytes.checked_add(count as u64).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "source size overflow")
                })?;
                if bytes > expected.0 || bytes > max_bytes {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "source changed"));
                }
                digest.update(&buffer[..count]);
                destination.write_all(&buffer[..count])?;
            }
            if bytes != expected.0 || format!("{:x}", digest.finalize()) != expected.1 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "source hash changed",
                ));
            }
            destination.sync_all()?;
            destination_parent.sync()?;
            Ok(())
        })();
        drop(destination);
        if let Err(error) = result {
            let _ = destination_parent.unlink_file(destination_name);
            return Err(error);
        }
        destination_parent.open_file(destination_name)?.metadata()
    }

    fn copy_tree_contents(
        source: &Directory,
        destination: &Directory,
        max_file_bytes: u64,
    ) -> io::Result<()> {
        for entry in std::fs::read_dir(source.path()?)? {
            let entry = entry?;
            let name = entry.file_name();
            match source.open_child(&name) {
                Ok(source_child) => {
                    destination.create_child_directory(&name)?;
                    let destination_child = destination.open_child(&name)?;
                    copy_tree_contents(&source_child, &destination_child, max_file_bytes)?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotADirectory => {
                    let mut input = source.open_file(&name)?;
                    let expected = hash_file(&mut input, max_file_bytes)?;
                    copy_verified_into_directory(
                        source,
                        &name,
                        destination,
                        &name,
                        &expected,
                        max_file_bytes,
                    )?;
                }
                Err(error) => return Err(error),
            }
        }
        destination.sync()
    }

    impl Directory {
        pub fn hash_file(
            &self,
            name: &std::ffi::OsStr,
            max_bytes: u64,
        ) -> io::Result<(u64, String)> {
            hash_file(&mut self.open_file(name)?, max_bytes)
        }

        pub fn copy_tree_from_path(
            &self,
            source_root: &Path,
            max_file_bytes: u64,
        ) -> io::Result<()> {
            let source = Directory::open(source_root)?;
            copy_tree_contents(&source, self, max_file_bytes)
        }

        pub fn copy_tree_from_directory(
            &self,
            source: &Directory,
            max_file_bytes: u64,
        ) -> io::Result<()> {
            copy_tree_contents(source, self, max_file_bytes)
        }

        /// Compatibility-named verified cleanup. The checked inode moves into
        /// a journaled hold; permanent unlink is deliberately not claimed.
        pub fn unlink_verified_file(
            &self,
            name: &std::ffi::OsStr,
            expected: &(u64, String),
            max_bytes: u64,
        ) -> io::Result<HeldEntry> {
            self.unlink_verified_file_after_hash(name, expected, max_bytes, || {})
        }

        pub(super) fn unlink_verified_file_after_hash<F>(
            &self,
            name: &std::ffi::OsStr,
            expected: &(u64, String),
            max_bytes: u64,
            after_hash: F,
        ) -> io::Result<HeldEntry>
        where
            F: FnOnce(),
        {
            self.hold_verified_file_with_hooks(name, expected, max_bytes, after_hash, || {})
        }

        pub fn hash_relative_file(
            &self,
            relative: &Path,
            max_bytes: u64,
        ) -> io::Result<(u64, String)> {
            let (parent, name) = open_parent_from_directory(self, relative)?;
            hash_file(&mut parent.open_file(&name)?, max_bytes)
        }

        pub fn relative_file_exists(&self, relative: &Path) -> io::Result<bool> {
            let (parent, name) = open_parent_from_directory(self, relative)?;
            parent.entry_exists(&name)
        }

        pub fn sync_relative_parent(&self, relative: &Path) -> io::Result<()> {
            let (parent, _) = open_parent_from_directory(self, relative)?;
            parent.sync()
        }

        /// Move a hash-verified relative file into a recoverable hold.
        pub fn remove_verified_relative_file(
            &self,
            relative: &Path,
            expected: &(u64, String),
            max_bytes: u64,
        ) -> io::Result<HeldEntry> {
            let (parent, name) = open_parent_from_directory(self, relative)?;
            parent.unlink_verified_file(&name, expected, max_bytes)
        }

        pub fn delete_verified_relative_file(
            &self,
            relative: &Path,
            expected: &(u64, String),
            max_bytes: u64,
        ) -> io::Result<()> {
            let (parent, name) = open_parent_from_directory(self, relative)?;
            parent.delete_verified_file(&name, expected, max_bytes)
        }
    }

    #[cfg(test)]
    pub fn remove_verified_file_with<F>(
        root: &Path,
        relative: &Path,
        expected: &(u64, String),
        max_bytes: u64,
        before_unlink: F,
    ) -> io::Result<HeldEntry>
    where
        F: FnOnce(),
    {
        let (parent, name) = open_parent_of(root, relative)?;
        parent.hold_verified_file_with_hooks(&name, expected, max_bytes, before_unlink, || {})
    }

    #[cfg(test)]
    fn open_parent_of(root: &Path, relative: &Path) -> io::Result<(Directory, std::ffi::OsString)> {
        open_parent_from_directory(&Directory::open(root)?, relative)
    }

    fn open_parent_from_directory(
        root: &Directory,
        relative: &Path,
    ) -> io::Result<(Directory, std::ffi::OsString)> {
        let name = relative
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file name missing"))?
            .to_os_string();
        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "relative file path contains an unsafe component",
            ));
        }
        Ok((root.open_relative(parent)?, name))
    }

    fn hex_bytes(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
        encoded
    }

    fn valid_hold_component(name: &std::ffi::OsStr, suffix: &str) -> bool {
        let Some(value) = name.to_str() else {
            return false;
        };
        value.len() <= 200
            && value.starts_with(".gogglelab-held-")
            && value.ends_with(suffix)
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    }

    fn decode_hex(encoded: &str) -> io::Result<Vec<u8>> {
        if encoded.len() % 2 != 0 || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "journal contains invalid hexadecimal data",
            ));
        }
        encoded
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let high = (pair[0] as char).to_digit(16).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "journal hex is invalid")
                })?;
                let low = (pair[1] as char).to_digit(16).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "journal hex is invalid")
                })?;
                Ok(((high << 4) | low) as u8)
            })
            .collect()
    }

    fn parse_journal_fields(contents: &str) -> io::Result<BTreeMap<String, String>> {
        let mut fields = BTreeMap::new();
        for line in contents.lines() {
            if line.is_empty() {
                continue;
            }
            let (key, value) = line.split_once('=').ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "hold journal is malformed")
            })?;
            if key.is_empty() || fields.insert(key.to_owned(), value.to_owned()).is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "hold journal contains duplicate or empty fields",
                ));
            }
        }
        Ok(fields)
    }

    fn required_journal_field<'a>(
        fields: &'a BTreeMap<String, String>,
        key: &str,
    ) -> io::Result<&'a str> {
        fields
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "hold journal is incomplete"))
    }

    fn parse_journal_number(fields: &BTreeMap<String, String>, key: &str) -> io::Result<u64> {
        required_journal_field(fields, key)?.parse().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "hold journal number is invalid")
        })
    }

    fn c_name(name: &std::ffi::OsStr) -> io::Result<CString> {
        if name.is_empty() || name == "." || name == ".." || name.as_bytes().contains(&b'/') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "entry name must be one normal path component",
            ));
        }
        CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "entry name contains NUL"))
    }

    #[cfg(target_os = "linux")]
    fn reset_errno() {
        unsafe { *libc::__errno_location() = 0 };
    }

    #[cfg(target_os = "linux")]
    fn current_errno() -> i32 {
        unsafe { *libc::__errno_location() }
    }

    #[cfg(target_os = "macos")]
    fn reset_errno() {
        unsafe { *libc::__error() = 0 };
    }

    #[cfg(target_os = "macos")]
    fn current_errno() -> i32 {
        unsafe { *libc::__error() }
    }

    fn verify_empty_geometry(
        directory: &Directory,
        name: &std::ffi::OsStr,
        file: &File,
        identity: (u64, u64, u32),
    ) -> io::Result<()> {
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() != 0 || generated_identity(file)? != identity {
            return Err(generated_invalid(
                "new geometry output is not its original empty descriptor",
            ));
        }
        let entry = EntryIdentity::at(directory, name)?;
        if (entry.device, entry.inode, entry.mode) != identity {
            return Err(generated_invalid("empty geometry output was replaced"));
        }
        Ok(())
    }
    fn geometry_shape(packet: &[u8]) -> io::Result<()> {
        if packet.len() < 16
            || packet.len() as u64 > super::MAX_GEOMETRY_PACKET_BYTES
            || &packet[..4] != b"STL2"
            || u32::from_le_bytes(packet[4..8].try_into().unwrap()) != 2
            || packet[12..16] != [0; 4]
        {
            return Err(generated_invalid(
                "generated geometry packet header or size is invalid",
            ));
        }
        let triangles = u32::from_le_bytes(packet[8..12].try_into().unwrap()) as u64;
        if triangles
            .checked_mul(36)
            .and_then(|value| value.checked_add(16))
            != Some(packet.len() as u64)
            || packet[16..]
                .chunks_exact(4)
                .any(|value| !f32::from_le_bytes(value.try_into().unwrap()).is_finite())
        {
            return Err(generated_invalid(
                "generated geometry packet positions are invalid",
            ));
        }
        Ok(())
    }
    fn geometry_hash(file: &File, bytes: u64) -> io::Result<String> {
        use std::os::unix::fs::{FileExt, MetadataExt};
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 0
            || metadata.len() != bytes
            || bytes > super::MAX_GEOMETRY_PACKET_BYTES
        {
            return Err(generated_invalid(
                "anonymous geometry size or identity changed",
            ));
        }
        let mut digest = Sha256::new();
        let mut position = 0;
        let mut buffer = [0u8; 64 * 1024];
        while position < bytes {
            let maximum = (bytes - position).min(buffer.len() as u64) as usize;
            let read = file.read_at(&mut buffer[..maximum], position)?;
            if read == 0 {
                return Err(generated_invalid("anonymous geometry was truncated"));
            }
            digest.update(&buffer[..read]);
            position += read as u64;
        }
        let after = file.metadata()?;
        if after.len() != bytes || after.nlink() != 0 {
            return Err(generated_invalid(
                "anonymous geometry changed while reading",
            ));
        }
        Ok(format!("{:x}", digest.finalize()))
    }
    impl super::GeometryFileWriter {
        pub(crate) fn write_and_seal(self, packet: &[u8]) -> io::Result<super::SealedGeometryFile> {
            self.write_and_seal_with_hook(packet, |_, _| Ok(()))
        }
        pub(super) fn write_and_seal_with_hook(
            mut self,
            packet: &[u8],
            mut hook: impl FnMut(&'static str, &File) -> io::Result<()>,
        ) -> io::Result<super::SealedGeometryFile> {
            geometry_shape(packet)?;
            use std::os::unix::fs::MetadataExt;
            if self.file.metadata()?.nlink() != 0
                || self.file.metadata()?.len() != 0
                || generated_identity(&self.file)? != self.identity
            {
                return Err(generated_invalid(
                    "geometry writer no longer owns its original anonymous empty file",
                ));
            }
            let expected = format!("{:x}", Sha256::digest(packet));
            self.file.write_all(packet)?;
            hook("after_payload_write", &self.file)?;
            self.file.sync_all()?;
            hook("after_payload_sync", &self.file)?;
            let sealed = super::SealedGeometryFile {
                file: self.file,
                identity: self.identity,
                bytes: packet.len() as u64,
                sha256: expected,
            };
            sealed.verify()?;
            Ok(sealed)
        }
    }
    impl super::SealedGeometryFile {
        pub(crate) fn verify(&self) -> io::Result<()> {
            if generated_identity(&self.file)? != self.identity
                || geometry_hash(&self.file, self.bytes)? != self.sha256
            {
                return Err(generated_invalid(
                    "sealed generated geometry no longer matches its admission",
                ));
            }
            Ok(())
        }
        pub(crate) fn stream_to(&self, sink: &mut dyn Write) -> io::Result<()> {
            use std::os::unix::fs::FileExt;
            self.verify()?;
            let mut position = 0;
            let mut buffer = [0u8; 64 * 1024];
            let mut digest = Sha256::new();
            while position < self.bytes {
                let maximum = (self.bytes - position).min(buffer.len() as u64) as usize;
                let read = self.file.read_at(&mut buffer[..maximum], position)?;
                if read == 0 {
                    return Err(generated_invalid("sealed geometry was truncated"));
                }
                digest.update(&buffer[..read]);
                sink.write_all(&buffer[..read])?;
                position += read as u64;
            }
            use std::os::unix::fs::MetadataExt;
            let after = self.file.metadata()?;
            if format!("{:x}", digest.finalize()) != self.sha256
                || generated_identity(&self.file)? != self.identity
                || after.len() != self.bytes
                || after.nlink() != 0
            {
                return Err(generated_invalid(
                    "sealed geometry changed during streaming",
                ));
            }
            Ok(())
        }
    }

    #[allow(dead_code)]
    impl super::PngNamespace {
        pub(crate) fn open(expected_library_root: &Path, root: &Directory) -> io::Result<Self> {
            generated_supported()?;
            if !expected_library_root.is_absolute() || !root.matches_path(expected_library_root)? {
                return Err(generated_invalid(
                    "PNG admitted root path does not match descriptor",
                ));
            }
            let root_path = expected_library_root.to_path_buf();
            let directory = root.open_child(std::ffi::OsStr::new("thumbnails"))?;
            let value = Self {
                inner: std::sync::Arc::new(super::PngNamespaceInner {
                    root: root.try_clone()?,
                    root_path,
                    root_identity: generated_identity(&root.0)?,
                    directory_identity: generated_identity(&directory.0)?,
                    directory,
                }),
            };
            value.verify_namespace()?;
            Ok(value)
        }
        fn verify_namespace(&self) -> io::Result<()> {
            let n = &self.inner;
            if generated_identity(&n.root.0)? != n.root_identity
                || !n.root.matches_path(&n.root_path)?
                || generated_identity(&n.directory.0)? != n.directory_identity
                || generated_identity(&n.root.open_child(std::ffi::OsStr::new("thumbnails"))?.0)?
                    != n.directory_identity
            {
                return Err(generated_invalid("PNG namespace binding changed"));
            }
            Ok(())
        }
        fn verify_receipt(&self, r: &mut super::PngFileReceipt) -> io::Result<u64> {
            use std::os::unix::fs::FileExt;
            self.verify_namespace()?;
            if self.inner.root_identity != r.namespace.root_identity
                || self.inner.directory_identity != r.namespace.directory_identity
                || self.inner.root_path != r.namespace.root_path
            {
                return Err(generated_invalid(
                    "PNG receipt belongs to another namespace",
                ));
            }
            let identity = generated_identity(&r.file)?;
            if let Some(original) = r.identity {
                if identity != original {
                    return Err(generated_invalid("PNG descriptor identity changed"));
                }
            } else {
                r.identity = Some(identity);
            }
            let name = r
                .current_name()
                .ok_or_else(|| generated_invalid("PNG receipt is unlinked"))?;
            let named = EntryIdentity::at(&self.inner.directory, name)?;
            if (named.device, named.inode, named.mode) != identity {
                return Err(generated_invalid("PNG directory entry was replaced"));
            }
            let length = r.file.metadata()?.len();
            let intended = r.rendered.bytes();
            if length > super::MAX_RECEIPT_PNG_BYTES
                || length > intended.len() as u64
                || (!r.partial && length != intended.len() as u64)
            {
                return Err(generated_invalid("PNG receipt length mismatch"));
            }
            let mut actual = vec![0; length as usize];
            r.file.read_exact_at(&mut actual, 0)?;
            if actual != intended[..length as usize] || r.file.metadata()?.len() != length {
                return Err(generated_invalid("PNG receipt bytes changed"));
            }
            let named = EntryIdentity::at(&self.inner.directory, name)?;
            if (named.device, named.inode, named.mode) != identity {
                return Err(generated_invalid(
                    "PNG directory entry changed during verification",
                ));
            }
            self.verify_namespace()?;
            Ok(length)
        }
        pub(crate) fn create_png_stage(
            &self,
            rendered: std::sync::Arc<crate::thumbnails::RasterGeneratedPng>,
        ) -> Result<super::PngFileReceipt, super::PngStageFailure> {
            self.create_png_stage_with(rendered, |_, _, _| Ok(()))
        }
        pub(super) fn create_png_stage_with(
            &self,
            rendered: std::sync::Arc<crate::thumbnails::RasterGeneratedPng>,
            mut hook: impl FnMut(&str, &File, &std::ffi::OsStr) -> io::Result<()>,
        ) -> Result<super::PngFileReceipt, super::PngStageFailure> {
            let setup = (|| {
                self.verify_namespace()?;
                if rendered.bytes().is_empty()
                    || rendered.bytes().len() as u64 > super::MAX_RECEIPT_PNG_BYTES
                {
                    return Err(generated_invalid("PNG proof exceeds output bound"));
                }
                let name: OsString = format!(
                    ".gogglelab-png-stage-{}.part",
                    uuid::Uuid::new_v4().simple()
                )
                .into();
                let cname = c_name(&name)?;
                let fd = unsafe {
                    libc::openat(
                        self.inner.directory.0.as_raw_fd(),
                        cname.as_ptr(),
                        libc::O_RDWR
                            | libc::O_CREAT
                            | libc::O_EXCL
                            | libc::O_CLOEXEC
                            | libc::O_NOFOLLOW,
                        0o600,
                    )
                };
                if fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok((unsafe { File::from_raw_fd(fd) }, name))
            })()
            .map_err(|source| super::PngStageFailure {
                source,
                stage: None,
            })?;
            let (file, name) = setup;
            let mut stage = super::CreatedPngStage {
                receipt: super::PngFileReceipt {
                    file,
                    namespace: self.inner.clone(),
                    identity: None,
                    rendered,
                    public_name: name.clone(),
                    location: super::PngLocation::Public(name.clone()),
                    partial: true,
                },
            };
            let result = (|| {
                stage.receipt.identity = Some(generated_identity(&stage.receipt.file)?);
                hook("before_payload_write", &stage.receipt.file, &name)?;
                (&stage.receipt.file).write_all(stage.receipt.rendered.bytes())?;
                hook("before_file_sync", &stage.receipt.file, &name)?;
                stage.receipt.file.sync_all()?;
                self.inner.directory.sync()?;
                stage.receipt.partial = false;
                self.verify_receipt(&mut stage.receipt)?;
                Ok(())
            })();
            match result {
                Ok(()) => Ok(stage.receipt),
                Err(source) => Err(super::PngStageFailure {
                    source,
                    stage: Some(stage),
                }),
            }
        }
        pub(crate) fn admit_reproduced_png(
            &self,
            name: &std::ffi::OsStr,
            rendered: std::sync::Arc<crate::thumbnails::RasterGeneratedPng>,
        ) -> io::Result<super::PngFileReceipt> {
            self.verify_namespace()?;
            if !rendered.matches_filename(name)
                || rendered.bytes().is_empty()
                || rendered.bytes().len() as u64 > super::MAX_RECEIPT_PNG_BYTES
            {
                return Err(generated_invalid(
                    "PNG candidate does not match raster recipe",
                ));
            }
            let file = self.inner.directory.open_file(name)?;
            let identity = Some(generated_identity(&file)?);
            let mut receipt = super::PngFileReceipt {
                file,
                identity,
                namespace: self.inner.clone(),
                rendered,
                public_name: name.into(),
                location: super::PngLocation::Public(name.into()),
                partial: false,
            };
            self.verify_receipt(&mut receipt)?;
            Ok(receipt)
        }
        pub(crate) fn retire_png(
            &self,
            r: &mut super::PngFileReceipt,
        ) -> Result<super::PngRetirementOutcome, super::PngRetirementFailure> {
            self.retire_png_with(r, |_, _| Ok(()))
        }
        pub(crate) fn retire_created_png_stage(
            &self,
            r: &mut super::CreatedPngStage,
        ) -> Result<super::PngRetirementOutcome, super::PngRetirementFailure> {
            self.retire_png(&mut r.receipt)
        }
        pub(super) fn retire_png_with(
            &self,
            r: &mut super::PngFileReceipt,
            mut hook: impl FnMut(&str, &super::PngFileReceipt) -> io::Result<()>,
        ) -> Result<super::PngRetirementOutcome, super::PngRetirementFailure> {
            let mut outcome = super::PngRetirementOutcome::default();
            let result = (|| {
                self.verify_namespace()?;
                if matches!(r.location, super::PngLocation::Unlinked) {
                    if self.inner.root_identity != r.namespace.root_identity
                        || self.inner.directory_identity != r.namespace.directory_identity
                        || self.inner.root_path != r.namespace.root_path
                    {
                        return Err(generated_invalid(
                            "PNG receipt belongs to another namespace",
                        ));
                    }
                } else {
                    self.verify_receipt(r)?;
                    if let super::PngLocation::Public(name) = &r.location {
                        let name = name.clone();
                        let claim: OsString =
                            format!(".gogglelab-png-claim-{}", uuid::Uuid::new_v4().simple())
                                .into();
                        hook("before_capture", r)?;
                        self.verify_namespace()?;
                        self.inner
                            .directory
                            .rename_child_exclusive_raw(&name, &claim)?;
                        r.location = super::PngLocation::Claim(claim);
                        hook("after_capture", r)?;
                        self.inner.directory.sync()?;
                    }
                    self.verify_receipt(r)?;
                    hook("before_private_unlink", r)?;
                    let length = self.verify_receipt(r)?;
                    let name = c_name(r.current_name().unwrap())?;
                    if unsafe {
                        libc::unlinkat(self.inner.directory.0.as_raw_fd(), name.as_ptr(), 0)
                    } < 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                    r.location = super::PngLocation::Unlinked;
                    outcome = super::PngRetirementOutcome {
                        unlinked_now: true,
                        removed_bytes: length,
                    };
                    hook("after_unlink", r)?;
                }
                self.inner.directory.sync()?;
                self.verify_namespace()?;
                if self.inner.directory.entry_exists(&r.public_name)? {
                    return Err(generated_invalid(
                        "PNG public name reappeared; replacement preserved",
                    ));
                }
                Ok(())
            })();
            result
                .map(|()| outcome)
                .map_err(|source| super::PngRetirementFailure { source, outcome })
        }
    }

    fn generated_supported() -> io::Result<()> {
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "atomic generated control operations are unavailable on this platform",
            ))
        }
    }
    fn generated_invalid(message: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, message)
    }
    fn generated_identity(file: &File) -> io::Result<(u64, u64, u32)> {
        let identity = EntryIdentity::from_file(file)?;
        Ok((identity.device, identity.inode, identity.mode))
    }
    fn validate_generated_nonce(nonce: &str) -> io::Result<()> {
        if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            Err(generated_invalid(
                "generated control nonce must contain exactly 32 hexadecimal characters",
            ))
        } else {
            Ok(())
        }
    }
    fn validate_generated_bytes(
        role: super::GeneratedControlRole,
        nonce: &str,
        bytes: &[u8],
        maximum: u64,
    ) -> io::Result<()> {
        if bytes.is_empty() || bytes.len() as u64 > maximum || maximum > 8192 {
            return Err(generated_invalid(
                "generated control bytes exceed their admission bounds",
            ));
        }
        if role == super::GeneratedControlRole::PurgeMarker && bytes != nonce.as_bytes() {
            return Err(generated_invalid(
                "purge stage marker does not match its admitted nonce",
            ));
        }
        Ok(())
    }
    fn read_generated_bytes(file: &File, maximum: u64) -> io::Result<Vec<u8>> {
        use std::os::unix::fs::FileExt;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > maximum {
            return Err(generated_invalid(
                "generated control file is not regular or exceeds its byte bound",
            ));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        let mut buffer = [0u8; 1024];
        loop {
            let remaining = (maximum + 1).saturating_sub(bytes.len() as u64) as usize;
            if remaining == 0 {
                return Err(generated_invalid(
                    "generated control file grew while reading",
                ));
            }
            let take = remaining.min(buffer.len());
            let read = file.read_at(&mut buffer[..take], bytes.len() as u64)?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            if bytes.len() as u64 > maximum {
                return Err(generated_invalid(
                    "generated control file grew while reading",
                ));
            }
        }
        if bytes.len() as u64 != metadata.len() || file.metadata()?.len() != metadata.len() {
            return Err(generated_invalid(
                "generated control size changed while reading",
            ));
        }
        Ok(bytes)
    }
    fn generated_role_name(role: super::GeneratedControlRole) -> &'static str {
        match role {
            super::GeneratedControlRole::Main => "main",
            super::GeneratedControlRole::Temporary => "tmp",
            super::GeneratedControlRole::Commit => "commit",
            super::GeneratedControlRole::PurgeMarker => "marker",
        }
    }
    struct ParsedGeneratedClaim {
        nonce: String,
        role: super::GeneratedControlRole,
    }
    fn parse_generated_claim(
        namespace: &super::GeneratedControlNamespace,
        name: &std::ffi::OsStr,
    ) -> io::Result<Option<ParsedGeneratedClaim>> {
        let prefix = namespace.claim_prefix();
        if !name.as_bytes().starts_with(prefix.as_bytes()) {
            return Ok(None);
        }
        let text = name
            .to_str()
            .ok_or_else(|| generated_invalid("generated control claim name is not Unicode"))?;
        let mut parts = text[prefix.len()..].split('-');
        let nonce = parts.next().unwrap_or_default();
        let role_text = parts.next().unwrap_or_default();
        let capture = parts.next().unwrap_or_default();
        validate_generated_nonce(nonce)?;
        if parts.next().is_some()
            || capture.len() != 32
            || !capture
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(generated_invalid(
                "generated control capture name is malformed",
            ));
        }
        let role = match role_text {
            "main" => super::GeneratedControlRole::Main,
            "tmp" => super::GeneratedControlRole::Temporary,
            "commit" => super::GeneratedControlRole::Commit,
            "marker" => super::GeneratedControlRole::PurgeMarker,
            _ => {
                return Err(generated_invalid(
                    "generated control capture role is invalid",
                ))
            }
        };
        namespace.public_name(role)?;
        Ok(Some(ParsedGeneratedClaim {
            nonce: nonce.to_owned(),
            role,
        }))
    }

    fn open_at(directory: libc::c_int, name: &[u8], flags: libc::c_int) -> io::Result<File> {
        let name = CString::new(name)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
        let fd = unsafe { libc::openat(directory, name.as_ptr(), flags) };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }
}

#[cfg(unix)]
pub(crate) use imp::{copy_verified_into_directory, hash_file, Directory};

#[cfg(not(unix))]
pub(crate) struct Directory;

#[cfg(not(unix))]
#[allow(dead_code)]
impl PngNamespace {
    pub(crate) fn open(
        _expected_library_root: &std::path::Path,
        _root: &Directory,
    ) -> std::io::Result<Self> {
        Err(unsupported())
    }
    pub(crate) fn create_png_stage(
        &self,
        _rendered: std::sync::Arc<crate::thumbnails::RasterGeneratedPng>,
    ) -> Result<PngFileReceipt, PngStageFailure> {
        Err(PngStageFailure {
            source: unsupported(),
            stage: None,
        })
    }
    pub(crate) fn admit_reproduced_png(
        &self,
        _name: &std::ffi::OsStr,
        _rendered: std::sync::Arc<crate::thumbnails::RasterGeneratedPng>,
    ) -> std::io::Result<PngFileReceipt> {
        Err(unsupported())
    }
    pub(crate) fn retire_png(
        &self,
        _receipt: &mut PngFileReceipt,
    ) -> Result<PngRetirementOutcome, PngRetirementFailure> {
        Err(PngRetirementFailure {
            source: unsupported(),
            outcome: PngRetirementOutcome::default(),
        })
    }
    pub(crate) fn retire_created_png_stage(
        &self,
        _stage: &mut CreatedPngStage,
    ) -> Result<PngRetirementOutcome, PngRetirementFailure> {
        Err(PngRetirementFailure {
            source: unsupported(),
            outcome: PngRetirementOutcome::default(),
        })
    }
}

#[cfg(not(unix))]
impl Directory {
    pub(crate) fn read_entry_names(
        &self,
        _maximum: usize,
    ) -> std::io::Result<Vec<std::ffi::OsString>> {
        Err(unsupported())
    }

    pub(crate) fn create_anonymous_geometry_writer(&self) -> std::io::Result<GeometryFileWriter> {
        Err(unsupported())
    }

    pub fn admit_created_generated(
        &self,
        _namespace: &GeneratedControlNamespace,
        _nonce: &str,
        _role: GeneratedControlRole,
        _file: std::fs::File,
        _bytes: &[u8],
    ) -> std::io::Result<GeneratedFileReceipt> {
        Err(unsupported())
    }
    pub fn recover_generated_file<T>(
        &self,
        _namespace: &GeneratedControlNamespace,
        _role: GeneratedControlRole,
        _maximum: u64,
        _validate: impl FnOnce(&[u8], Option<&str>) -> std::io::Result<(String, T)>,
    ) -> std::io::Result<Option<(GeneratedFileReceipt, T)>> {
        Err(unsupported())
    }
    pub fn publish_generated_file(
        &self,
        _receipt: &mut GeneratedFileReceipt,
        _destination: GeneratedControlRole,
    ) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub fn exchange_generated_files(
        &self,
        _new: &mut GeneratedFileReceipt,
        _old: &mut GeneratedFileReceipt,
    ) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub fn delete_generated_file(
        &self,
        _receipt: &mut GeneratedFileReceipt,
    ) -> std::io::Result<()> {
        Err(unsupported())
    }

    pub(crate) fn open(_path: &Path) -> std::io::Result<Self> {
        Err(unsupported())
    }
    pub(crate) fn open_child(&self, _name: &std::ffi::OsStr) -> std::io::Result<Self> {
        Err(unsupported())
    }
    pub(crate) fn open_relative(&self, _relative: &Path) -> std::io::Result<Self> {
        Err(unsupported())
    }
    pub(crate) fn matches_path(&self, _path: &Path) -> std::io::Result<bool> {
        Err(unsupported())
    }
    pub(crate) fn open_file(&self, _name: &std::ffi::OsStr) -> std::io::Result<std::fs::File> {
        Err(unsupported())
    }
    pub(crate) fn create_new_file(
        &self,
        _name: &std::ffi::OsStr,
    ) -> std::io::Result<std::fs::File> {
        Err(unsupported())
    }
    pub(crate) fn create_new_file_read_write(
        &self,
        _name: &std::ffi::OsStr,
    ) -> std::io::Result<std::fs::File> {
        Err(unsupported())
    }
    pub(crate) fn hold_entry(&self, _name: &std::ffi::OsStr) -> std::io::Result<HeldEntry> {
        Err(unsupported())
    }
    pub(crate) fn hold_verified_file(
        &self,
        _name: &std::ffi::OsStr,
        _expected: &(u64, String),
        _max_bytes: u64,
    ) -> std::io::Result<HeldEntry> {
        Err(unsupported())
    }
    pub(crate) fn hold_verified_file_as(
        &self,
        _source_name: &std::ffi::OsStr,
        _expected: &(u64, String),
        _max_bytes: u64,
        _hold_name: &std::ffi::OsStr,
        _journal_name: &std::ffi::OsStr,
    ) -> std::io::Result<HeldEntry> {
        Err(unsupported())
    }
    pub(crate) fn restore_held_entry(
        &self,
        _held: &HeldEntry,
        _original_name: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn recover_held_entry(
        &self,
        _journal_name: &std::ffi::OsStr,
    ) -> std::io::Result<HeldEntry> {
        Err(unsupported())
    }
    pub(crate) fn move_verified_file_to(
        &self,
        _source_name: &std::ffi::OsStr,
        _destination_directory: &Directory,
        _destination_name: &std::ffi::OsStr,
        _expected: &(u64, String),
        _max_bytes: u64,
    ) -> std::io::Result<MovedEntry> {
        Err(unsupported())
    }
    pub(crate) fn move_entry_to(
        &self,
        _source_name: &std::ffi::OsStr,
        _destination_directory: &Directory,
        _destination_name: &std::ffi::OsStr,
    ) -> std::io::Result<MovedEntry> {
        Err(unsupported())
    }
    pub(crate) fn rename_child_exclusive_to(
        &self,
        _source: &std::ffi::OsStr,
        _destination_directory: &Directory,
        _destination: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn unlink_verified_file(
        &self,
        _name: &std::ffi::OsStr,
        _expected: &(u64, String),
        _max_bytes: u64,
    ) -> std::io::Result<HeldEntry> {
        Err(unsupported())
    }
    pub(crate) fn delete_verified_file(
        &self,
        _name: &std::ffi::OsStr,
        _expected: &(u64, String),
        _max_bytes: u64,
    ) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn hash_file(
        &self,
        _name: &std::ffi::OsStr,
        _max_bytes: u64,
    ) -> std::io::Result<(u64, String)> {
        Err(unsupported())
    }
    pub(crate) fn remove_empty_relative_directory(&self, _relative: &Path) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn remove_empty_directory(&self, _name: &std::ffi::OsStr) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn hash_relative_file(
        &self,
        _relative: &Path,
        _max_bytes: u64,
    ) -> std::io::Result<(u64, String)> {
        Err(unsupported())
    }
    pub(crate) fn relative_file_exists(&self, _relative: &Path) -> std::io::Result<bool> {
        Err(unsupported())
    }
    pub(crate) fn sync_relative_parent(&self, _relative: &Path) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn remove_verified_relative_file(
        &self,
        _relative: &Path,
        _expected: &(u64, String),
        _max_bytes: u64,
    ) -> std::io::Result<HeldEntry> {
        Err(unsupported())
    }
    pub(crate) fn delete_verified_relative_file(
        &self,
        _relative: &Path,
        _expected: &(u64, String),
        _max_bytes: u64,
    ) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn copy_tree_from_path(
        &self,
        _source_root: &Path,
        _max_file_bytes: u64,
    ) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn create_new_relative_file(
        &self,
        _relative: &Path,
    ) -> std::io::Result<std::fs::File> {
        Err(unsupported())
    }
    pub(crate) fn copy_tree_from_directory(
        &self,
        _source: &Directory,
        _max_file_bytes: u64,
    ) -> std::io::Result<()> {
        Err(unsupported())
    }

    pub(crate) fn path(&self) -> std::io::Result<std::path::PathBuf> {
        Err(unsupported())
    }
    pub(crate) fn sync_tree(&self) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn sync(&self) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn try_clone(&self) -> std::io::Result<Self> {
        Err(unsupported())
    }
    pub(crate) fn entry_exists(&self, _name: &std::ffi::OsStr) -> std::io::Result<bool> {
        Err(unsupported())
    }
    pub(crate) fn child_path(
        &self,
        _name: &std::ffi::OsStr,
    ) -> std::io::Result<std::path::PathBuf> {
        Err(unsupported())
    }
    pub(crate) fn create_child_directory(&self, _name: &std::ffi::OsStr) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn rename_child_exclusive(
        &self,
        _source: &std::ffi::OsStr,
        _destination: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn replace_child(
        &self,
        _source: &std::ffi::OsStr,
        _destination: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn replace_child_if_same(
        &self,
        _source: &std::ffi::OsStr,
        _destination: &std::ffi::OsStr,
        _expected_old: &(u64, String),
        _max_bytes: u64,
    ) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn link_child(
        &self,
        _source: &std::ffi::OsStr,
        _destination: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn remove_child_tree(&self, _name: &std::ffi::OsStr) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn unlink_file(&self, _name: &std::ffi::OsStr) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn unlink_file_if_same(
        &self,
        _name: &std::ffi::OsStr,
        _expected_file: &std::fs::File,
    ) -> std::io::Result<HeldEntry> {
        Err(unsupported())
    }
}

#[cfg(not(unix))]
impl GeometryFileWriter {
    pub(crate) fn write_and_seal(self, _packet: &[u8]) -> std::io::Result<SealedGeometryFile> {
        Err(unsupported())
    }
}
#[cfg(not(unix))]
impl SealedGeometryFile {
    pub(crate) fn verify(&self) -> std::io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn stream_to(&self, _sink: &mut dyn std::io::Write) -> std::io::Result<()> {
        Err(unsupported())
    }
}

#[cfg(not(unix))]
pub(crate) fn copy_verified_into_directory(
    _source_parent: &Directory,
    _source_name: &std::ffi::OsStr,
    _destination_parent: &Directory,
    _destination_name: &std::ffi::OsStr,
    _expected: &(u64, String),
    _max_bytes: u64,
) -> std::io::Result<std::fs::Metadata> {
    Err(unsupported())
}

#[cfg(not(unix))]
pub(crate) fn hash_file(
    _file: &mut std::fs::File,
    _max_bytes: u64,
) -> std::io::Result<(u64, String)> {
    Err(unsupported())
}

#[cfg(not(unix))]
fn unsupported() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "handle-relative management operations are unavailable on this platform",
    )
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Barrier};
    use std::thread;

    use sha2::{Digest, Sha256};

    use super::imp::{copy_verified_file_with, remove_verified_file_with, Directory};

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn raw_cross_directory_rename_moves_without_replacing_existing_bytes() {
        use std::ffi::OsStr;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        let source_directory = Directory::open(&source).unwrap();
        let destination_directory = Directory::open(&destination).unwrap();
        fs::write(source.join("stage"), b"created bytes").unwrap();
        source_directory
            .rename_child_exclusive_to_raw(
                OsStr::new("stage"),
                &destination_directory,
                OsStr::new("published"),
            )
            .unwrap();
        assert!(!source.join("stage").exists());
        assert_eq!(
            fs::read(destination.join("published")).unwrap(),
            b"created bytes"
        );

        fs::write(source.join("stage"), b"another owner").unwrap();
        assert!(source_directory
            .rename_child_exclusive_to_raw(
                OsStr::new("stage"),
                &destination_directory,
                OsStr::new("published"),
            )
            .is_err());
        assert_eq!(fs::read(source.join("stage")).unwrap(), b"another owner");
        assert_eq!(
            fs::read(destination.join("published")).unwrap(),
            b"created bytes"
        );
        assert_eq!(fs::read_dir(&source).unwrap().count(), 1);
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn raw_cross_directory_rename_rejects_traversal_without_moving_files() {
        use std::ffi::OsStr;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(source.join("stage"), b"preserve stage").unwrap();
        fs::write(root.join("outside"), b"preserve outside").unwrap();
        let source_directory = Directory::open(&source).unwrap();
        let destination_directory = Directory::open(&destination).unwrap();
        for (from, to) in [("../outside", "published"), ("stage", "../outside")] {
            assert!(source_directory
                .rename_child_exclusive_to_raw(
                    OsStr::new(from),
                    &destination_directory,
                    OsStr::new(to),
                )
                .is_err());
        }
        assert_eq!(fs::read(source.join("stage")).unwrap(), b"preserve stage");
        assert_eq!(fs::read(root.join("outside")).unwrap(), b"preserve outside");
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn raw_cross_directory_rename_uses_pinned_directory_handles() {
        use std::ffi::OsStr;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        let destination = root.join("destination");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(source.join("stage"), b"admitted bytes").unwrap();
        let source_directory = Directory::open(&source).unwrap();
        let destination_directory = Directory::open(&destination).unwrap();
        let admitted_source = root.join("admitted-source");
        let admitted_destination = root.join("admitted-destination");
        fs::rename(&source, &admitted_source).unwrap();
        fs::rename(&destination, &admitted_destination).unwrap();
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(source.join("stage"), b"replacement source").unwrap();
        fs::write(destination.join("published"), b"replacement destination").unwrap();

        source_directory
            .rename_child_exclusive_to_raw(
                OsStr::new("stage"),
                &destination_directory,
                OsStr::new("published"),
            )
            .unwrap();
        assert!(!admitted_source.join("stage").exists());
        assert_eq!(
            fs::read(admitted_destination.join("published")).unwrap(),
            b"admitted bytes"
        );
        assert_eq!(
            fs::read(source.join("stage")).unwrap(),
            b"replacement source"
        );
        assert_eq!(
            fs::read(destination.join("published")).unwrap(),
            b"replacement destination"
        );
    }

    fn png_receipt_fixture() -> (
        tempfile::TempDir,
        super::PngNamespace,
        Arc<crate::thumbnails::RasterGeneratedPng>,
    ) {
        use crate::thumbnails::{reproduce_thumbnail, ThumbnailRecipe};
        use sha2::{Digest, Sha256};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Library");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("thumbnails")).unwrap();
        let directory = Directory::open(&root.canonicalize().unwrap()).unwrap();
        let namespace =
            super::PngNamespace::open(&root.canonicalize().unwrap(), &directory).unwrap();
        let mut source = vec![0u8; 84];
        source[80..84].copy_from_slice(&1u32.to_le_bytes());
        for value in [0f32, 0., 1., 0., 0., 0., 1., 0., 0., 0., 1., 0.] {
            source.extend_from_slice(&value.to_le_bytes());
        }
        source.extend_from_slice(&0u16.to_le_bytes());
        let source_hash = format!("{:x}", Sha256::digest(&source));
        let name = format!("{source_hash}-isometric-v1-128-12171f.png");
        let recipe = ThumbnailRecipe::parse_filename(std::ffi::OsStr::new(&name)).unwrap();
        let proof = Arc::new(
            reproduce_thumbnail(&recipe, &source, source.len() as u64)
                .unwrap()
                .unwrap(),
        );
        (temp, namespace, proof)
    }

    #[test]
    fn png_receipt_created_stage_retirement_is_private_and_idempotent() {
        let (temp, namespace, proof) = png_receipt_fixture();
        let mut receipt = namespace.create_png_stage(proof.clone()).unwrap();
        let directory = temp.path().join("Library/thumbnails");
        assert_eq!(
            fs::read(directory.join(receipt.current_name().unwrap())).unwrap(),
            proof.bytes()
        );
        let retired = namespace.retire_png(&mut receipt).unwrap();
        assert!(retired.unlinked_now);
        assert_eq!(retired.removed_bytes, proof.bytes().len() as u64);
        let retry = namespace.retire_png(&mut receipt).unwrap();
        assert!(!retry.unlinked_now);
        assert_eq!(retry.removed_bytes, 0);
        assert_eq!(fs::read_dir(directory).unwrap().count(), 0);
    }

    #[test]
    fn png_receipt_partial_stage_errors_retain_exact_owner() {
        use std::io::Write;
        for length in [0, 17] {
            let (temp, ns, proof) = png_receipt_fixture();
            let failure = ns
                .create_png_stage_with(proof.clone(), |phase, file, _| {
                    if phase == "before_payload_write" {
                        (&*file).write_all(&proof.bytes()[..length])?;
                        return Err(std::io::Error::other("injected write failure"));
                    }
                    Ok(())
                })
                .unwrap_err();
            let mut stage = failure.stage.unwrap();
            let result = ns.retire_created_png_stage(&mut stage).unwrap();
            assert!(result.unlinked_now);
            assert_eq!(result.removed_bytes, length as u64);
            assert_eq!(
                fs::read_dir(temp.path().join("Library/thumbnails"))
                    .unwrap()
                    .count(),
                0
            );
        }
    }
    #[test]
    fn png_receipt_candidate_requires_full_recipe_and_bytes() {
        let (temp, ns, proof) = png_receipt_fixture();
        let dir = temp.path().join("Library/thumbnails");
        let name = format!(
            "{}-isometric-v1-128-12171f.png",
            proof.recipe().source_hash()
        );
        fs::write(dir.join(&name), proof.bytes()).unwrap();
        let mut receipt = ns
            .admit_reproduced_png(std::ffi::OsStr::new(&name), proof.clone())
            .unwrap();
        assert!(ns
            .admit_reproduced_png(std::ffi::OsStr::new("custom.png"), proof.clone())
            .is_err());
        fs::rename(dir.join(&name), dir.join("original")).unwrap();
        fs::write(dir.join(&name), proof.bytes()).unwrap();
        assert!(ns.retire_png(&mut receipt).is_err());
        assert_eq!(fs::read(dir.join(&name)).unwrap(), proof.bytes());
        fs::write(dir.join(&name), &proof.bytes()[..17]).unwrap();
        assert!(ns
            .admit_reproduced_png(std::ffi::OsStr::new(&name), proof.clone())
            .is_err());
        fs::write(
            dir.join(&name),
            vec![0u8; super::MAX_RECEIPT_PNG_BYTES as usize + 1],
        )
        .unwrap();
        assert!(ns
            .admit_reproduced_png(std::ffi::OsStr::new(&name), proof)
            .is_err());
    }
    #[test]
    fn png_receipt_sync_failure_reports_unlinked_once_and_public_recreation() {
        let (temp, ns, proof) = png_receipt_fixture();
        let mut r = ns.create_png_stage(proof.clone()).unwrap();
        let name = r.current_name().unwrap().to_owned();
        let error = ns
            .retire_png_with(&mut r, |phase, _| {
                if phase == "after_unlink" {
                    Err(std::io::Error::other("injected sync boundary"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(error.outcome.unlinked_now);
        assert_eq!(error.outcome.removed_bytes, proof.bytes().len() as u64);
        assert_eq!(
            ns.retire_png(&mut r).unwrap(),
            super::PngRetirementOutcome::default()
        );
        fs::write(
            temp.path().join("Library/thumbnails").join(name),
            b"replacement",
        )
        .unwrap();
        let error = ns.retire_png(&mut r).unwrap_err();
        assert_eq!(error.outcome, super::PngRetirementOutcome::default());
    }
    #[test]
    fn png_receipt_capture_and_private_replacements_are_preserved() {
        for phase in ["before_capture", "before_private_unlink"] {
            let (temp, ns, proof) = png_receipt_fixture();
            let dir = temp.path().join("Library/thumbnails");
            let mut r = ns.create_png_stage(proof).unwrap();
            let error = ns
                .retire_png_with(&mut r, |at, r| {
                    if at == phase {
                        let name = r.current_name().unwrap();
                        fs::rename(dir.join(name), dir.join("displaced"))?;
                        fs::write(dir.join(name), b"unknown")?;
                    }
                    Ok(())
                })
                .unwrap_err();
            assert!(!error.outcome.unlinked_now);
            assert_eq!(
                fs::read(dir.join(r.current_name().unwrap())).unwrap(),
                b"unknown"
            );
            assert!(dir.join("displaced").exists());
        }
    }
    #[test]
    fn png_receipt_namespace_swaps_fail_closed() {
        for phase in ["before_capture", "before_private_unlink"] {
            let (temp, ns, proof) = png_receipt_fixture();
            let root = temp.path().join("Library");
            let mut r = ns.create_png_stage(proof).unwrap();
            let error = ns
                .retire_png_with(&mut r, |at, _| {
                    if at == phase {
                        fs::rename(root.join("thumbnails"), root.join("old-thumbnails"))?;
                        fs::create_dir(root.join("thumbnails"))?;
                    }
                    Ok(())
                })
                .unwrap_err();
            assert!(!error.outcome.unlinked_now);
            assert_eq!(
                fs::read_dir(root.join("old-thumbnails")).unwrap().count(),
                1
            );
        }
        let (temp, _ns, _proof) = png_receipt_fixture();
        let root = temp.path().join("Library");
        let fd = Directory::open(&root.canonicalize().unwrap()).unwrap();
        fs::rename(&root, temp.path().join("previousLibrary")).unwrap();
        fs::create_dir(&root).unwrap();
        assert!(super::PngNamespace::open(&root, &fd).is_err());
    }
    #[test]
    fn png_receipt_partial_mutation_and_capture_retry() {
        use std::io::Write;
        let (temp, ns, proof) = png_receipt_fixture();
        let mut failure = ns
            .create_png_stage_with(proof, |_, _, _| Err(std::io::Error::other("injected")))
            .unwrap_err();
        let mut stage = failure.stage.take().unwrap();
        (&stage.receipt.file).write_all(b"not png prefix").unwrap();
        assert!(ns.retire_created_png_stage(&mut stage).is_err());
        assert!(temp
            .path()
            .join("Library/thumbnails")
            .join(stage.current_name().unwrap())
            .exists());
        let (_temp, ns, proof) = png_receipt_fixture();
        let mut r = ns.create_png_stage(proof).unwrap();
        let error = ns
            .retire_png_with(&mut r, |at, _| {
                if at == "after_capture" {
                    Err(std::io::Error::other("capture sync failure"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(!error.outcome.unlinked_now);
        assert!(matches!(r.location, super::PngLocation::Claim(_)));
        assert!(ns.retire_png(&mut r).unwrap().unlinked_now);
    }

    #[test]
    fn png_receipt_root_swap_cross_namespace_and_public_reappearance() {
        for boundary in ["before_capture", "before_private_unlink"] {
            let (temp, ns, proof) = png_receipt_fixture();
            let mut r = ns.create_png_stage(proof.clone()).unwrap();
            let root = temp.path().join("Library");
            let error = ns
                .retire_png_with(&mut r, |phase, _| {
                    if phase == boundary {
                        fs::rename(&root, temp.path().join("previousLibrary"))?;
                        fs::create_dir(&root)?;
                        fs::create_dir(root.join("thumbnails"))?;
                    }
                    Ok(())
                })
                .unwrap_err();
            assert!(!error.outcome.unlinked_now);
            let held = temp.path().join("previousLibrary/thumbnails");
            assert_eq!(fs::read_dir(&held).unwrap().count(), 1);
            assert_eq!(
                fs::read(held.join(r.current_name().unwrap())).unwrap(),
                proof.bytes()
            );
            assert!(matches!(
                (&r.location, boundary),
                (super::PngLocation::Public(_), "before_capture")
                    | (super::PngLocation::Claim(_), "before_private_unlink")
            ));
        }
        let (_temp, ns, proof) = png_receipt_fixture();
        let (_other_temp, other, _) = png_receipt_fixture();
        let mut r = ns.create_png_stage(proof).unwrap();
        assert!(other.retire_png(&mut r).is_err());
        assert!(r.current_name().is_some());
        let (temp, ns, proof) = png_receipt_fixture();
        let mut r = ns.create_png_stage(proof.clone()).unwrap();
        let public = r.current_name().unwrap().to_owned();
        let path = temp.path().join("Library/thumbnails").join(public);
        let error = ns
            .retire_png_with(&mut r, |phase, _| {
                if phase == "before_private_unlink" {
                    fs::write(&path, b"new public bytes")?;
                }
                Ok(())
            })
            .unwrap_err();
        assert!(error.outcome.unlinked_now);
        assert_eq!(error.outcome.removed_bytes, proof.bytes().len() as u64);
        assert_eq!(fs::read(&path).unwrap(), b"new public bytes");
        assert_eq!(
            ns.retire_png(&mut r).unwrap_err().outcome,
            super::PngRetirementOutcome::default()
        );
    }
    #[test]
    fn png_receipt_full_write_sync_failure_retains_owner() {
        let (_temp, ns, proof) = png_receipt_fixture();
        let failure = ns
            .create_png_stage_with(proof.clone(), |phase, _, _| {
                if phase == "before_file_sync" {
                    Err(std::io::Error::other("injected file sync"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(failure.source.to_string().contains("injected"));
        let mut stage = failure.stage.unwrap();
        let result = ns.retire_created_png_stage(&mut stage).unwrap();
        assert_eq!(result.removed_bytes, proof.bytes().len() as u64);
    }

    #[test]
    fn png_receipt_content_addressed_candidate_matches_exact_output() {
        let (temp, ns, proof) = png_receipt_fixture();
        let dir = temp.path().join("Library/thumbnails");
        let name = format!(
            "{}-isometric-v1-128-12171f-{}.png",
            proof.recipe().source_hash(),
            proof.png_hash()
        );
        fs::write(dir.join(&name), proof.bytes()).unwrap();
        let mut receipt = ns
            .admit_reproduced_png(std::ffi::OsStr::new(&name), proof.clone())
            .unwrap();
        let wrong = format!(
            "{}-isometric-v1-128-12171f-{}.png",
            proof.recipe().source_hash(),
            "0".repeat(64)
        );
        fs::write(dir.join(&wrong), proof.bytes()).unwrap();
        assert!(ns
            .admit_reproduced_png(std::ffi::OsStr::new(&wrong), proof.clone())
            .is_err());
        for rejected in [
            format!("{}-isometric-v1-128-12171f.png", "0".repeat(64)),
            format!(
                "{}-isometric-v1-256-12171f.png",
                proof.recipe().source_hash()
            ),
            format!(
                "{}-isometric-v1-128-000000.png",
                proof.recipe().source_hash()
            ),
            format!("../{name}"),
        ] {
            if !rejected.starts_with("../") {
                fs::write(dir.join(&rejected), proof.bytes()).unwrap();
            }
            assert!(ns
                .admit_reproduced_png(std::ffi::OsStr::new(&rejected), proof.clone())
                .is_err());
        }
        assert!(ns.retire_png(&mut receipt).unwrap().unlinked_now);
        assert!(dir.join(wrong).exists());
    }

    fn geometry_test_packet() -> Vec<u8> {
        let mut packet = b"STL2".to_vec();
        packet.extend_from_slice(&2u32.to_le_bytes());
        packet.extend_from_slice(&1u32.to_le_bytes());
        packet.extend_from_slice(&0u32.to_le_bytes());
        for value in [0f32, 0., 0., 1., 0., 0., 0., 1., 0.] {
            packet.extend_from_slice(&value.to_le_bytes());
        }
        packet
    }
    #[test]
    fn geometry_inventory_waits_for_the_named_empty_birth_window() {
        use super::{geometry_inventory_guard, GEOMETRY_CREATION, MAX_GEOMETRY_SETUP_ENTRIES};
        use std::sync::mpsc;
        use std::time::Duration;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap();
        let directory = Directory::open(&path).unwrap();
        let (attempt_tx, attempt_rx) = mpsc::channel();
        let (snapshot_tx, snapshot_rx) = mpsc::channel();
        let mut inventory = None;
        let writer = directory
            .create_anonymous_geometry_writer_with_hook(|stage, _, _| {
                if stage == "after_empty_create" {
                    assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
                    let path = path.clone();
                    let attempt_tx = attempt_tx.clone();
                    let snapshot_tx = snapshot_tx.clone();
                    inventory = Some(thread::spawn(move || {
                        attempt_tx.send(()).unwrap();
                        let _gate = geometry_inventory_guard().unwrap();
                        let names = Directory::open(&path)
                            .unwrap()
                            .read_entry_names(MAX_GEOMETRY_SETUP_ENTRIES)
                            .unwrap();
                        snapshot_tx.send(names).unwrap();
                    }));
                    attempt_rx.recv().unwrap();
                    assert!(matches!(
                        GEOMETRY_CREATION.try_lock(),
                        Err(std::sync::TryLockError::WouldBlock)
                    ));
                    assert!(snapshot_rx.recv_timeout(Duration::from_millis(20)).is_err());
                }
                Ok(())
            })
            .unwrap();
        assert!(snapshot_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .is_empty());
        inventory.unwrap().join().unwrap();
        drop(writer);
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
    }
    #[test]
    fn anonymous_geometry_payload_and_parallel_readers_never_have_a_named_backing() {
        use std::os::unix::fs::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        let writer = directory.create_anonymous_geometry_writer().unwrap();
        assert_eq!(writer.file.metadata().unwrap().nlink(), 0);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
        let packet = geometry_test_packet();
        let sealed = Arc::new(writer.write_and_seal(&packet).unwrap());
        assert_eq!(sealed.file.metadata().unwrap().nlink(), 0);
        assert_eq!(sealed.bytes(), packet.len() as u64);
        assert_eq!(sealed.sha256(), format!("{:x}", Sha256::digest(&packet)));
        let readers = (0..4)
            .map(|_| {
                let sealed = sealed.clone();
                thread::spawn(move || {
                    let mut bytes = Vec::new();
                    sealed.stream_to(&mut bytes).unwrap();
                    bytes
                })
            })
            .collect::<Vec<_>>();
        for reader in readers {
            assert_eq!(reader.join().unwrap(), packet);
        }
        drop(sealed);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
    #[test]
    fn anonymous_geometry_capture_preserves_replacement_after_final_public_check() {
        let temp = tempfile::tempdir().unwrap();
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        let displaced = temp.path().join("original-empty-file");
        let mut capture = None;
        let result =
            directory.create_anonymous_geometry_writer_with_hook(|stage, public, claim| {
                if stage == "before_capture" {
                    fs::rename(temp.path().join(public), &displaced).unwrap();
                    fs::write(temp.path().join(public), b"unknown replacement bytes").unwrap();
                    capture = claim.map(|name| name.to_owned());
                }
                Ok(())
            });
        assert!(result.is_err());
        assert_eq!(
            fs::read(temp.path().join(capture.unwrap())).unwrap(),
            b"unknown replacement bytes"
        );
        assert_eq!(fs::metadata(displaced).unwrap().len(), 0);
    }
    #[test]
    fn anonymous_geometry_preserves_recreated_public_name_before_private_unlink() {
        let temp = tempfile::tempdir().unwrap();
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        let mut public_name = None;
        let result = directory.create_anonymous_geometry_writer_with_hook(|stage, public, _| {
            if stage == "before_private_unlink" {
                fs::write(temp.path().join(public), b"foreign public bytes").unwrap();
                public_name = Some(public.to_owned());
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read(temp.path().join(public_name.unwrap())).unwrap(),
            b"foreign public bytes"
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }
    #[test]
    fn anonymous_geometry_setup_failure_never_writes_packet_bytes_or_creates_holds() {
        for fault in [
            "after_empty_create",
            "after_capture",
            "after_unlink",
            "after_parent_sync",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
            assert!(directory
                .create_anonymous_geometry_writer_with_hook(|stage, _, _| if stage == fault {
                    Err(std::io::Error::other(
                        "injected setup sync/cancellation failure",
                    ))
                } else {
                    Ok(())
                })
                .is_err());
            for entry in fs::read_dir(temp.path()).unwrap() {
                let entry = entry.unwrap();
                assert_eq!(entry.metadata().unwrap().len(), 0);
                assert!(!entry
                    .file_name()
                    .to_string_lossy()
                    .contains("gogglelab-held"));
            }
        }
    }
    #[test]
    fn anonymous_geometry_write_and_sync_failures_close_unlinked_generated_output() {
        use std::os::unix::fs::MetadataExt;
        for fault in ["after_payload_write", "after_payload_sync"] {
            let temp = tempfile::tempdir().unwrap();
            let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
            let writer = directory.create_anonymous_geometry_writer().unwrap();
            assert!(writer
                .write_and_seal_with_hook(&geometry_test_packet(), |stage, file| {
                    assert_eq!(file.metadata().unwrap().nlink(), 0);
                    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
                    if stage == fault {
                        Err(std::io::Error::other("injected write/sync cancellation"))
                    } else {
                        Ok(())
                    }
                })
                .is_err());
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
        }
    }
    #[test]
    fn anonymous_geometry_empty_birth_is_pinned_when_its_parent_path_is_swapped() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        let held = temp.path().join("held");
        let outside = temp.path().join("outside");
        fs::create_dir(&parent).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("source.stl"), b"preserve original").unwrap();
        let directory = Directory::open(&parent.canonicalize().unwrap()).unwrap();
        let writer = directory
            .create_anonymous_geometry_writer_with_hook(|stage, _, _| {
                if stage == "after_empty_create" {
                    fs::rename(&parent, &held).unwrap();
                    std::os::unix::fs::symlink(&outside, &parent).unwrap();
                }
                Ok(())
            })
            .unwrap();
        let sealed = writer.write_and_seal(&geometry_test_packet()).unwrap();
        sealed.verify().unwrap();
        drop(sealed);
        assert_eq!(
            fs::read(outside.join("source.stl")).unwrap(),
            b"preserve original"
        );
        assert_eq!(fs::read_dir(held).unwrap().count(), 0);
        assert_eq!(fs::read_dir(outside).unwrap().count(), 1);
    }
    #[test]
    fn anonymous_geometry_refuses_excess_residual_entries_without_deleting_unknowns() {
        let temp = tempfile::tempdir().unwrap();
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        for index in 0..super::MAX_GEOMETRY_SETUP_ENTRIES {
            fs::write(temp.path().join(format!("unknown-{index}")), b"").unwrap();
        }
        assert!(directory.create_anonymous_geometry_writer().is_err());
        assert_eq!(
            fs::read_dir(temp.path()).unwrap().count(),
            super::MAX_GEOMETRY_SETUP_ENTRIES
        );
    }
    #[test]
    fn anonymous_geometry_rejects_malformed_packets_and_detects_descriptor_mutation() {
        use std::io::Write;
        for malformed in [
            b"unknown user bytes".to_vec(),
            {
                let mut bytes = geometry_test_packet();
                bytes[4] = 3;
                bytes
            },
            {
                let mut bytes = geometry_test_packet();
                bytes[16..20].copy_from_slice(&f32::NAN.to_le_bytes());
                bytes
            },
        ] {
            let temp = tempfile::tempdir().unwrap();
            let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
            assert!(directory
                .create_anonymous_geometry_writer()
                .unwrap()
                .write_and_seal(&malformed)
                .is_err());
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
        }
        let temp = tempfile::tempdir().unwrap();
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        let sealed = directory
            .create_anonymous_geometry_writer()
            .unwrap()
            .write_and_seal(&geometry_test_packet())
            .unwrap();
        let mut noncooperating = &sealed.file;
        noncooperating.write_all(b"mutated").unwrap();
        assert!(sealed.verify().is_err());
        let mut sink = Vec::new();
        assert!(sealed.stream_to(&mut sink).is_err());
        assert!(sink.is_empty());
    }
    #[test]
    fn generated_control_publication_exchange_and_retirement_leave_no_holds() {
        use super::{
            GeneratedControlKind, GeneratedControlNamespace, GeneratedControlRole as Role,
            GeneratedFileLocation,
        };
        use std::io::Write;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Library");
        let namespace = GeneratedControlNamespace::new(&root, GeneratedControlKind::Purge).unwrap();
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        let nonce = "a".repeat(32);
        let temporary = namespace.public_name(Role::Temporary).unwrap();
        let mut file = directory.create_new_file_read_write(&temporary).unwrap();
        file.write_all(b"prepared metadata").unwrap();
        file.sync_all().unwrap();
        let mut prepared = directory
            .admit_created_generated(
                &namespace,
                &nonce,
                Role::Temporary,
                file,
                b"prepared metadata",
            )
            .unwrap();
        directory
            .publish_generated_file(&mut prepared, Role::Main)
            .unwrap();
        let commit = namespace.public_name(Role::Commit).unwrap();
        let mut file = directory.create_new_file_read_write(&commit).unwrap();
        file.write_all(b"committed metadata").unwrap();
        file.sync_all().unwrap();
        let mut committed = directory
            .admit_created_generated(
                &namespace,
                &nonce,
                Role::Commit,
                file,
                b"committed metadata",
            )
            .unwrap();
        directory
            .exchange_generated_files(&mut committed, &mut prepared)
            .unwrap();
        assert_eq!(prepared.role(), Role::Commit);
        assert_eq!(committed.role(), Role::Main);
        directory.delete_generated_file(&mut prepared).unwrap();
        assert_eq!(prepared.location(), &GeneratedFileLocation::Unlinked);
        directory.delete_generated_file(&mut committed).unwrap();
        assert_eq!(committed.location(), &GeneratedFileLocation::Unlinked);
        assert!(std::fs::read_dir(temp.path()).unwrap().next().is_none());
    }

    fn generated_control_file(
        directory: &Directory,
        namespace: &super::GeneratedControlNamespace,
        nonce: &str,
        role: super::GeneratedControlRole,
        bytes: &[u8],
    ) -> super::GeneratedFileReceipt {
        use std::io::Write;
        let name = namespace.public_name(role).unwrap();
        let mut file = directory.create_new_file_read_write(&name).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        directory
            .admit_created_generated(namespace, nonce, role, file, bytes)
            .unwrap()
    }
    fn generated_control_main(
        directory: &Directory,
        namespace: &super::GeneratedControlNamespace,
        nonce: &str,
        bytes: &[u8],
    ) -> super::GeneratedFileReceipt {
        let mut receipt = generated_control_file(
            directory,
            namespace,
            nonce,
            super::GeneratedControlRole::Temporary,
            bytes,
        );
        directory
            .publish_generated_file(&mut receipt, super::GeneratedControlRole::Main)
            .unwrap();
        receipt
    }
    fn generated_control_namespace(root: &Path) -> super::GeneratedControlNamespace {
        super::GeneratedControlNamespace::new(root, super::GeneratedControlKind::Purge).unwrap()
    }
    fn generated_test_error() -> std::io::Error {
        std::io::Error::other("injected generated control interruption")
    }

    #[test]
    fn generated_control_capture_preserves_public_replacement_after_last_identity_check() {
        use super::{GeneratedControlRole as Role, GeneratedFileLocation};
        let temp = tempfile::tempdir().unwrap();
        let namespace = generated_control_namespace(&temp.path().join("Library"));
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        let nonce = "a".repeat(32);
        let mut receipt =
            generated_control_main(&directory, &namespace, &nonce, b"admitted metadata");
        let public = temp.path().join(namespace.public_name(Role::Main).unwrap());
        let displaced = temp.path().join("admitted.displaced");
        let result = directory.delete_generated_file_with_hook(&mut receipt, |stage, _| {
            if stage == "before_capture" {
                fs::rename(&public, &displaced).unwrap();
                fs::write(&public, b"unknown replacement").unwrap();
            }
            Ok(())
        });
        assert!(result.is_err());
        assert!(matches!(
            receipt.location(),
            GeneratedFileLocation::Claim(_)
        ));
        assert_eq!(
            fs::read(temp.path().join(receipt.current_name().unwrap())).unwrap(),
            b"unknown replacement"
        );
        assert_eq!(fs::read(displaced).unwrap(), b"admitted metadata");
        assert!(!public.exists());
    }

    #[test]
    fn generated_control_unlinks_only_private_claim_when_public_name_reappears_after_hash() {
        use super::{GeneratedControlRole as Role, GeneratedFileLocation};
        let temp = tempfile::tempdir().unwrap();
        let namespace = generated_control_namespace(&temp.path().join("Library"));
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        let nonce = "b".repeat(32);
        let mut receipt =
            generated_control_main(&directory, &namespace, &nonce, b"known generated metadata");
        let public = temp.path().join(namespace.public_name(Role::Main).unwrap());
        let result = directory.delete_generated_file_with_hook(&mut receipt, |stage, _| {
            if stage == "before_private_unlink" {
                fs::write(&public, b"unknown new public bytes").unwrap();
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(receipt.location(), &GeneratedFileLocation::Unlinked);
        assert!(receipt.current_name().is_none());
        assert_eq!(fs::read(public).unwrap(), b"unknown new public bytes");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn generated_control_receipt_does_not_readmit_same_bytes_from_another_inode() {
        use super::GeneratedControlRole as Role;
        let temp = tempfile::tempdir().unwrap();
        let namespace = generated_control_namespace(&temp.path().join("Library"));
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        let nonce = "c".repeat(32);
        let mut receipt =
            generated_control_main(&directory, &namespace, &nonce, b"identical bytes");
        let public = temp.path().join(namespace.public_name(Role::Main).unwrap());
        let displaced = temp.path().join("original-descriptor");
        fs::rename(&public, &displaced).unwrap();
        fs::write(&public, b"identical bytes").unwrap();
        assert!(directory.delete_generated_file(&mut receipt).is_err());
        assert_eq!(fs::read(public).unwrap(), b"identical bytes");
        assert_eq!(fs::read(displaced).unwrap(), b"identical bytes");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[test]
    fn generated_control_exchange_preserves_unknown_entries_before_and_after_swap() {
        use super::GeneratedControlRole as Role;
        for fault in ["before_exchange", "after_exchange"] {
            let temp = tempfile::tempdir().unwrap();
            let namespace = generated_control_namespace(&temp.path().join("Library"));
            let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
            let nonce = "d".repeat(32);
            let mut old = generated_control_main(&directory, &namespace, &nonce, b"prepared");
            let mut new =
                generated_control_file(&directory, &namespace, &nonce, Role::Commit, b"committed");
            let displaced = temp.path().join("preserved-old-descriptor");
            let result = directory.exchange_generated_files_with_hook(
                &mut new,
                &mut old,
                |stage, _, old| {
                    if stage == fault {
                        let path = temp.path().join(old.current_name().unwrap());
                        fs::rename(&path, &displaced).unwrap();
                        fs::write(path, b"unknown exchange replacement").unwrap();
                    }
                    Ok(())
                },
            );
            assert!(result.is_err());
            assert_eq!(old.role(), Role::Commit);
            assert_eq!(new.role(), Role::Main);
            assert_eq!(
                fs::read(
                    temp.path()
                        .join(namespace.public_name(Role::Commit).unwrap())
                )
                .unwrap(),
                b"unknown exchange replacement"
            );
            assert_eq!(fs::read(displaced).unwrap(), b"prepared");
            assert_eq!(
                fs::read(temp.path().join(namespace.public_name(Role::Main).unwrap())).unwrap(),
                b"committed"
            );
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 3);
        }
    }

    #[test]
    fn generated_control_exclusive_publication_preserves_an_occupied_main() {
        use super::GeneratedControlRole as Role;
        let temp = tempfile::tempdir().unwrap();
        let namespace = generated_control_namespace(&temp.path().join("Library"));
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        let nonce = "e".repeat(32);
        let mut receipt =
            generated_control_file(&directory, &namespace, &nonce, Role::Temporary, b"prepared");
        let public = temp.path().join(namespace.public_name(Role::Main).unwrap());
        fs::write(&public, b"unknown occupied entry").unwrap();
        assert!(directory
            .publish_generated_file(&mut receipt, Role::Main)
            .is_err());
        assert_eq!(receipt.role(), Role::Temporary);
        assert_eq!(fs::read(public).unwrap(), b"unknown occupied entry");
    }

    #[test]
    fn generated_control_strict_recovery_finds_a_claimed_main_without_its_public_name() {
        use super::{GeneratedControlRole as Role, GeneratedFileLocation};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Library");
        let namespace = generated_control_namespace(&root);
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        let nonce = "f".repeat(32);
        let bytes=serde_json::to_vec(&serde_json::json!({"version":1,"rootPath":root.to_str().unwrap(),"nonce":nonce,"committed":true})).unwrap();
        let mut receipt = generated_control_main(&directory, &namespace, &nonce, &bytes);
        assert!(directory
            .delete_generated_file_with_hook(&mut receipt, |stage, _| if stage == "after_capture" {
                Err(generated_test_error())
            } else {
                Ok(())
            })
            .is_err());
        drop(receipt);
        let (mut recovered, committed) = directory
            .recover_generated_file(&namespace, Role::Main, 8192, |bytes, claim_nonce| {
                let value: serde_json::Value =
                    serde_json::from_slice(bytes).map_err(std::io::Error::other)?;
                assert_eq!(value["rootPath"], root.to_str().unwrap());
                assert_eq!(value["version"], 1);
                let parsed = value["nonce"].as_str().unwrap();
                assert_eq!(claim_nonce, Some(parsed));
                Ok((parsed.to_owned(), value["committed"].as_bool().unwrap()))
            })
            .unwrap()
            .unwrap();
        assert!(committed);
        assert!(matches!(
            recovered.location(),
            GeneratedFileLocation::Claim(_)
        ));
        directory.delete_generated_file(&mut recovered).unwrap();
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn generated_control_recovery_rejects_public_claim_ambiguity_and_malformed_names() {
        use super::GeneratedControlRole as Role;
        for mode in ["ambiguous", "malformed", "multiple-nonces"] {
            let temp = tempfile::tempdir().unwrap();
            let namespace = generated_control_namespace(&temp.path().join("Library"));
            let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
            let nonce = "1".repeat(32);
            let base = namespace
                .public_name(Role::Main)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let claim = format!("{base}.cleanup-{nonce}-main-{}", "a".repeat(32));
            fs::write(temp.path().join(&claim), b"known metadata").unwrap();
            if mode == "ambiguous" {
                fs::write(temp.path().join(&base), b"known metadata").unwrap();
            }
            if mode == "malformed" {
                fs::write(
                    temp.path().join(format!("{base}.cleanup-unknown")),
                    b"unknown",
                )
                .unwrap();
            }
            if mode == "multiple-nonces" {
                fs::write(
                    temp.path().join(format!(
                        "{base}.cleanup-{}-tmp-{}",
                        "2".repeat(32),
                        "b".repeat(32)
                    )),
                    b"other operation",
                )
                .unwrap();
            }
            assert!(directory
                .recover_generated_file(&namespace, Role::Main, 8192, |_, _| Ok((
                    nonce.clone(),
                    ()
                )))
                .is_err());
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
            assert_eq!(
                fs::read(temp.path().join(claim)).unwrap(),
                b"known metadata"
            );
        }
    }

    #[test]
    fn generated_control_recovery_never_readmits_a_replacement_during_validation() {
        use super::GeneratedControlRole as Role;
        let temp = tempfile::tempdir().unwrap();
        let namespace = generated_control_namespace(&temp.path().join("Library"));
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        let nonce = "3".repeat(32);
        let receipt =
            generated_control_main(&directory, &namespace, &nonce, b"strictly validated bytes");
        let public = temp.path().join(receipt.current_name().unwrap());
        drop(receipt);
        let displaced = temp.path().join("original-recovery-descriptor");
        assert!(directory
            .recover_generated_file(&namespace, Role::Main, 8192, |bytes, _| {
                assert_eq!(bytes, b"strictly validated bytes");
                fs::rename(&public, &displaced).unwrap();
                fs::write(&public, b"unknown replacement").unwrap();
                Ok((nonce.clone(), ()))
            })
            .is_err());
        assert_eq!(fs::read(public).unwrap(), b"unknown replacement");
        assert_eq!(fs::read(displaced).unwrap(), b"strictly validated bytes");
    }

    #[test]
    fn generated_control_recovery_bounds_claims_entries_and_file_sizes() {
        use super::GeneratedControlRole as Role;
        for mode in ["claims", "entries", "oversized", "symlink", "wrong-nonce"] {
            let temp = tempfile::tempdir().unwrap();
            let namespace = generated_control_namespace(&temp.path().join("Library"));
            let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
            let nonce = "4".repeat(32);
            let public = namespace.public_name(Role::Main).unwrap();
            let base = public.to_string_lossy();
            match mode {
                "claims" => {
                    for index in 0..17 {
                        fs::write(
                            temp.path()
                                .join(format!("{base}.cleanup-{nonce}-main-{index:032x}")),
                            b"metadata",
                        )
                        .unwrap();
                    }
                }
                "entries" => {
                    for index in 0..4097 {
                        fs::write(temp.path().join(format!("entry-{index}")), b"").unwrap();
                    }
                }
                "oversized" => fs::write(temp.path().join(&public), vec![b'a'; 8193]).unwrap(),
                "symlink" => {
                    let target = temp.path().join("unknown-target");
                    fs::write(&target, b"unknown target bytes").unwrap();
                    std::os::unix::fs::symlink(&target, temp.path().join(&public)).unwrap();
                }
                _ => fs::write(
                    temp.path()
                        .join(format!("{base}.cleanup-{nonce}-main-{}", "a".repeat(32))),
                    b"metadata",
                )
                .unwrap(),
            }
            let before = fs::read_dir(temp.path()).unwrap().count();
            assert!(
                directory
                    .recover_generated_file(&namespace, Role::Main, 8192, |_, _| Ok((
                        "5".repeat(32),
                        ()
                    )))
                    .is_err(),
                "{mode}"
            );
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), before);
        }
    }

    #[test]
    fn generated_control_sync_failures_retain_precise_receipt_locations_and_library_data() {
        use super::GeneratedFileLocation;
        for fault in ["after_capture", "before_private_unlink", "after_unlink"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("Library");
            let previous = temp.path().join("Library.previous");
            fs::create_dir(&root).unwrap();
            fs::create_dir(&previous).unwrap();
            fs::write(root.join("model.stl"), b"current original").unwrap();
            fs::write(previous.join("model.stl"), b"previous original").unwrap();
            let namespace = generated_control_namespace(&root);
            let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
            let nonce = "6".repeat(32);
            let mut receipt =
                generated_control_main(&directory, &namespace, &nonce, b"committed metadata");
            assert!(directory
                .delete_generated_file_with_hook(&mut receipt, |stage, _| if stage == fault {
                    Err(generated_test_error())
                } else {
                    Ok(())
                })
                .is_err());
            if fault == "after_unlink" {
                assert_eq!(receipt.location(), &GeneratedFileLocation::Unlinked);
                assert!(receipt.current_name().is_none());
            } else {
                assert!(matches!(
                    receipt.location(),
                    GeneratedFileLocation::Claim(_)
                ));
                assert!(temp.path().join(receipt.current_name().unwrap()).is_file());
            }
            assert_eq!(
                fs::read(root.join("model.stl")).unwrap(),
                b"current original"
            );
            assert_eq!(
                fs::read(previous.join("model.stl")).unwrap(),
                b"previous original"
            );
        }
    }

    #[test]
    fn generated_control_scratch_cleanup_failure_leaves_the_authoritative_main() {
        use super::GeneratedControlRole as Role;
        let temp = tempfile::tempdir().unwrap();
        let namespace = generated_control_namespace(&temp.path().join("Library"));
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        let nonce = "7".repeat(32);
        let main = generated_control_main(
            &directory,
            &namespace,
            &nonce,
            b"authoritative committed metadata",
        );
        let mut scratch = generated_control_file(
            &directory,
            &namespace,
            &nonce,
            Role::Commit,
            b"displaced prepared metadata",
        );
        assert!(directory
            .delete_generated_file_with_hook(&mut scratch, |stage, _| if stage == "after_capture" {
                Err(generated_test_error())
            } else {
                Ok(())
            })
            .is_err());
        assert_eq!(
            fs::read(temp.path().join(main.current_name().unwrap())).unwrap(),
            b"authoritative committed metadata"
        );
    }

    #[test]
    fn generated_control_parent_handle_swap_never_redirects_cleanup_into_the_new_path() {
        use super::GeneratedControlRole as Role;
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        let outside = temp.path().join("outside");
        let held = temp.path().join("held-parent");
        fs::create_dir(&parent).unwrap();
        fs::create_dir(&outside).unwrap();
        let namespace = generated_control_namespace(&parent.join("Library"));
        let directory = Directory::open(&parent.canonicalize().unwrap()).unwrap();
        let nonce = "8".repeat(32);
        let mut receipt =
            generated_control_main(&directory, &namespace, &nonce, b"owned generated metadata");
        fs::write(
            outside.join(namespace.public_name(Role::Main).unwrap()),
            b"unknown outside metadata",
        )
        .unwrap();
        fs::rename(&parent, &held).unwrap();
        std::os::unix::fs::symlink(&outside, &parent).unwrap();
        directory.delete_generated_file(&mut receipt).unwrap();
        assert_eq!(
            fs::read(outside.join(namespace.public_name(Role::Main).unwrap())).unwrap(),
            b"unknown outside metadata"
        );
        assert_eq!(fs::read_dir(held).unwrap().count(), 0);
    }

    #[test]
    fn generated_control_purge_marker_requires_exact_known_nonce_and_retains_original_descriptor() {
        use super::GeneratedControlRole as Role;
        let temp = tempfile::tempdir().unwrap();
        let namespace = generated_control_namespace(&temp.path().join("Library"));
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        let nonce = "9".repeat(32);
        let mut marker = generated_control_file(
            &directory,
            &namespace,
            &nonce,
            Role::PurgeMarker,
            nonce.as_bytes(),
        );
        assert_eq!(marker.bytes().len(), 32);
        directory.delete_generated_file(&mut marker).unwrap();
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
    #[test]
    fn generated_file_delete_preserves_a_replacement_after_hashing() {
        let temp = tempfile::tempdir().unwrap();
        let thumbnails = temp.path().join("thumbnails");
        fs::create_dir(&thumbnails).unwrap();
        let path = thumbnails.join("generated.png");
        let displaced = thumbnails.join("generated.displaced.png");
        fs::write(&path, b"generated source bytes").unwrap();
        let expected = (
            b"generated source bytes".len() as u64,
            format!("{:x}", Sha256::digest(b"generated source bytes")),
        );
        let directory = Directory::open(&thumbnails.canonicalize().unwrap()).unwrap();
        let result = directory.delete_verified_file_with_hook(
            std::ffi::OsStr::new("generated.png"),
            &expected,
            1024,
            || {
                fs::rename(&path, &displaced).unwrap();
                fs::write(&path, b"replacement bytes").unwrap();
            },
        );

        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&path).unwrap(), b"replacement bytes");
        assert_eq!(fs::read(&displaced).unwrap(), b"generated source bytes");
    }

    fn swap_path_for_outside(path: &Path, outside: &Path) -> PathBuf {
        let displaced = path.with_extension("displaced");
        fs::rename(path, &displaced).unwrap();
        std::os::unix::fs::symlink(outside, path).unwrap();
        displaced
    }

    #[test]
    fn destination_write_stays_in_the_admitted_directory_after_root_path_swap() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let destination = temp_root.join("destination");
        let outside = temp_root.join("outside");
        fs::create_dir(&destination).unwrap();
        fs::create_dir(&outside).unwrap();
        let ready = Arc::new(Barrier::new(2));
        let proceed = Arc::new(Barrier::new(2));
        let worker_ready = ready.clone();
        let worker_proceed = proceed.clone();
        let source_root = temp_root.join("source");
        fs::create_dir(&source_root).unwrap();
        fs::write(source_root.join("part.stl"), b"synthetic model").unwrap();
        let source_relative = PathBuf::from("part.stl");
        let expected = (
            b"synthetic model".len() as u64,
            format!("{:x}", Sha256::digest(b"synthetic model")),
        );
        let source_root_thread = source_root.clone();
        let destination_thread = destination.clone();
        let name = std::ffi::OsString::from("part-copy.stl");
        let handle = thread::spawn(move || {
            let expected = expected.clone();
            let ready = worker_ready.clone();
            let proceed = worker_proceed.clone();
            copy_verified_file_with(
                &source_root_thread,
                &source_relative,
                &destination_thread,
                &name,
                &expected,
                1024,
                move || {
                    ready.wait();
                    proceed.wait();
                },
            )?;
            Ok::<_, std::io::Error>(())
        });
        ready.wait();
        let displaced = swap_path_for_outside(&destination, &outside);
        proceed.wait();
        handle.join().unwrap().unwrap();
        assert!(!outside.join("part-copy.stl").exists());
        assert_eq!(
            fs::read(displaced.join("part-copy.stl")).unwrap(),
            b"synthetic model"
        );
    }

    #[test]
    fn orphan_cleanup_stays_in_the_pinned_hash_directory_after_swap() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let cache = temp_root.join("indexed-cache");
        let hash = "a".repeat(64);
        let orphan_dir = cache.join(&hash);
        let outside = temp_root.join("outside");
        fs::create_dir_all(&orphan_dir).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(orphan_dir.join("orphan.stl"), b"synthetic orphan").unwrap();
        fs::write(outside.join("orphan.stl"), b"preserve me").unwrap();
        let ready = Arc::new(Barrier::new(2));
        let proceed = Arc::new(Barrier::new(2));
        let worker_ready = ready.clone();
        let worker_proceed = proceed.clone();
        let root = temp_root;
        let relative = PathBuf::from(format!("indexed-cache/{hash}/orphan.stl"));
        let expected = (
            b"synthetic orphan".len() as u64,
            format!("{:x}", Sha256::digest(b"synthetic orphan")),
        );
        let handle = thread::spawn(move || {
            remove_verified_file_with(&root, &relative, &expected, 1024, || {
                worker_ready.wait();
                worker_proceed.wait();
            })
        });
        ready.wait();
        let displaced = swap_path_for_outside(&orphan_dir, &outside);
        proceed.wait();
        handle.join().unwrap().unwrap();
        assert_eq!(
            fs::read(outside.join("orphan.stl")).unwrap(),
            b"preserve me"
        );
        assert!(!displaced.join("orphan.stl").exists());
    }

    #[test]
    fn source_unlink_stays_inside_the_pinned_source_directory_after_swap() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let source_root = temp_root.join("admitted-source");
        let outside = temp_root.join("outside-source");
        fs::create_dir(&source_root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(source_root.join("part.stl"), b"synthetic source").unwrap();
        fs::write(outside.join("part.stl"), b"preserve external file").unwrap();
        let source_directory = Directory::open(&source_root).unwrap();
        let expected = (
            b"synthetic source".len() as u64,
            format!("{:x}", Sha256::digest(b"synthetic source")),
        );
        let ready = Arc::new(Barrier::new(2));
        let proceed = Arc::new(Barrier::new(2));
        let worker_ready = ready.clone();
        let worker_proceed = proceed.clone();
        let handle = thread::spawn(move || {
            worker_ready.wait();
            worker_proceed.wait();
            source_directory.unlink_verified_file(std::ffi::OsStr::new("part.stl"), &expected, 1024)
        });
        ready.wait();
        let displaced = swap_path_for_outside(&source_root, &outside);
        proceed.wait();
        handle.join().unwrap().unwrap();
        assert_eq!(
            fs::read(outside.join("part.stl")).unwrap(),
            b"preserve external file"
        );
        assert!(!displaced.join("part.stl").exists());
    }

    #[test]
    fn verified_unlink_refuses_a_replacement_swapped_after_hashing() {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        fs::create_dir(&source_root).unwrap();
        let source_path = source_root.join("part.stl");
        let displaced_path = source_root.join("part.original");
        fs::write(&source_path, b"verified source").unwrap();
        let source_directory = Directory::open(&source_root.canonicalize().unwrap()).unwrap();
        let expected = (
            b"verified source".len() as u64,
            format!("{:x}", Sha256::digest(b"verified source")),
        );

        let result = source_directory.unlink_verified_file_after_hash(
            std::ffi::OsStr::new("part.stl"),
            &expected,
            1024,
            || {
                fs::rename(&source_path, &displaced_path).unwrap();
                fs::write(&source_path, b"replacement source").unwrap();
            },
        );

        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&source_path).unwrap(), b"replacement source");
        assert_eq!(fs::read(&displaced_path).unwrap(), b"verified source");
    }

    #[test]
    fn verified_hold_preserves_replacement_and_original_after_post_rename_race() {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        fs::create_dir(&source_root).unwrap();
        let source_path = source_root.join("part.stl");
        fs::write(&source_path, b"verified source").unwrap();
        let source_directory = Directory::open(&source_root.canonicalize().unwrap()).unwrap();
        let expected = (
            b"verified source".len() as u64,
            format!("{:x}", Sha256::digest(b"verified source")),
        );

        let result = source_directory.hold_verified_file_with_hooks(
            std::ffi::OsStr::new("part.stl"),
            &expected,
            1024,
            || {},
            || fs::write(&source_path, b"replacement after rename").unwrap(),
        );

        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&source_path).unwrap(), b"replacement after rename");
        let held_name = fs::read_dir(&source_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .find(|name| {
                name.to_string_lossy().starts_with(".gogglelab-held-")
                    && name.to_string_lossy().ends_with(".data")
            })
            .expect("original bytes remain under a held name");
        assert_eq!(
            fs::read(source_root.join(&held_name)).unwrap(),
            b"verified source"
        );
        let journal_name = std::ffi::OsString::from(format!(
            "{}.journal",
            held_name.to_string_lossy().trim_end_matches(".data")
        ));
        assert!(source_root.join(&journal_name).exists());
        assert!(source_root
            .join(format!("{}.moved", journal_name.to_string_lossy()))
            .exists());
    }

    #[test]
    fn crash_after_hold_rename_leaves_a_durable_journal_and_recoverable_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        fs::create_dir(&source_root).unwrap();
        let source_path = source_root.join("part.stl");
        fs::write(&source_path, b"crash-safe source").unwrap();
        let source_directory = Directory::open(&source_root.canonicalize().unwrap()).unwrap();
        let expected = (
            b"crash-safe source".len() as u64,
            format!("{:x}", Sha256::digest(b"crash-safe source")),
        );

        let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = source_directory.hold_verified_file_with_hooks(
                std::ffi::OsStr::new("part.stl"),
                &expected,
                1024,
                || {},
                || panic!("simulated process interruption after rename"),
            );
        }));

        assert!(crash.is_err());
        assert!(!source_path.exists());
        let names = fs::read_dir(&source_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        let hold_name = names
            .iter()
            .find(|name| name.to_string_lossy().ends_with(".data"))
            .expect("held file must survive simulated crash");
        let journal_name = names
            .iter()
            .find(|name| name.to_string_lossy().ends_with(".journal"))
            .expect("prepared journal must survive simulated crash");
        assert_eq!(
            fs::read(source_root.join(hold_name)).unwrap(),
            b"crash-safe source"
        );
        let journal = fs::read_to_string(source_root.join(journal_name)).unwrap();
        assert!(journal.contains("original_hex=706172742e73746c"));
        assert!(source_root
            .join(format!("{}.moved", journal_name.to_string_lossy()))
            .exists());
        let restarted_directory = Directory::open(&source_root.canonicalize().unwrap()).unwrap();
        let receipt = restarted_directory
            .recover_held_entry(journal_name)
            .unwrap();
        assert_eq!(receipt.original_name, std::ffi::OsStr::new("part.stl"));
        restarted_directory
            .restore_held_entry(&receipt, &receipt.original_name)
            .unwrap();
        assert_eq!(fs::read(source_path).unwrap(), b"crash-safe source");
    }

    #[test]
    fn deterministic_hold_names_recover_and_restore_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        fs::create_dir(&source_root).unwrap();
        let source_path = source_root.join("part.stl");
        fs::write(&source_path, b"deterministic source").unwrap();
        let source_directory = Directory::open(&source_root.canonicalize().unwrap()).unwrap();
        let expected = (
            b"deterministic source".len() as u64,
            format!("{:x}", Sha256::digest(b"deterministic source")),
        );
        let hold_name = std::ffi::OsStr::new(".gogglelab-held-removal_abc123-model_1.data");
        let journal_name = std::ffi::OsStr::new(".gogglelab-held-removal_abc123-model_1.journal");

        let held = source_directory
            .hold_verified_file_as(
                std::ffi::OsStr::new("part.stl"),
                &expected,
                1024,
                hold_name,
                journal_name,
            )
            .unwrap();

        assert_eq!(held.hold_name, hold_name);
        assert_eq!(held.journal_name, journal_name);
        assert!(!source_path.exists());
        let restarted_directory = Directory::open(&source_root.canonicalize().unwrap()).unwrap();
        let recovered = restarted_directory
            .recover_held_entry(journal_name)
            .unwrap();
        assert_eq!(recovered.original_name, std::ffi::OsStr::new("part.stl"));
        restarted_directory
            .restore_held_entry(&recovered, &recovered.original_name)
            .unwrap();
        assert_eq!(fs::read(source_path).unwrap(), b"deterministic source");
    }

    #[test]
    fn deterministic_hold_rejects_existing_hold_or_journal_names_without_moving_source() {
        for (occupied, hold_leaf, journal_leaf) in [
            (
                ".gogglelab-held-removal_collision-model.data",
                ".gogglelab-held-removal_collision-model.data",
                ".gogglelab-held-removal_collision-model.journal",
            ),
            (
                ".gogglelab-held-removal_collision-model.journal",
                ".gogglelab-held-removal_collision-model.data",
                ".gogglelab-held-removal_collision-model.journal",
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let source_root = temp.path().join("source");
            fs::create_dir(&source_root).unwrap();
            let source_path = source_root.join("part.stl");
            fs::write(&source_path, b"source stays put").unwrap();
            fs::write(source_root.join(occupied), b"occupied sentinel").unwrap();
            let directory = Directory::open(&source_root.canonicalize().unwrap()).unwrap();
            let expected = (
                b"source stays put".len() as u64,
                format!("{:x}", Sha256::digest(b"source stays put")),
            );

            let error = directory
                .hold_verified_file_as(
                    std::ffi::OsStr::new("part.stl"),
                    &expected,
                    1024,
                    std::ffi::OsStr::new(hold_leaf),
                    std::ffi::OsStr::new(journal_leaf),
                )
                .unwrap_err();

            assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(&source_path).unwrap(), b"source stays put");
            assert_eq!(
                fs::read(source_root.join(occupied)).unwrap(),
                b"occupied sentinel"
            );
        }
    }

    #[test]
    fn deterministic_hold_keeps_journaled_bytes_when_source_name_is_recreated() {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        fs::create_dir(&source_root).unwrap();
        let source_path = source_root.join("part.stl");
        fs::write(&source_path, b"verified source").unwrap();
        let directory = Directory::open(&source_root.canonicalize().unwrap()).unwrap();
        let expected = (
            b"verified source".len() as u64,
            format!("{:x}", Sha256::digest(b"verified source")),
        );
        let hold_name = std::ffi::OsStr::new(".gogglelab-held-removal_crash-model_2.data");
        let journal_name = std::ffi::OsStr::new(".gogglelab-held-removal_crash-model_2.journal");

        let error = directory
            .hold_verified_file_as_with_hook(
                std::ffi::OsStr::new("part.stl"),
                &expected,
                1024,
                hold_name,
                journal_name,
                || fs::write(&source_path, b"concurrent replacement").unwrap(),
            )
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        let message = error.to_string();
        assert!(message.contains(&hold_name.to_string_lossy().to_string()));
        assert!(message.contains(&journal_name.to_string_lossy().to_string()));
        assert_eq!(fs::read(&source_path).unwrap(), b"concurrent replacement");
        assert_eq!(
            fs::read(source_root.join(hold_name)).unwrap(),
            b"verified source"
        );
        let recovered = directory.recover_held_entry(journal_name).unwrap();
        assert!(directory
            .restore_held_entry(&recovered, &recovered.original_name)
            .is_err());
        assert_eq!(fs::read(&source_path).unwrap(), b"concurrent replacement");
        assert_eq!(
            fs::read(source_root.join(hold_name)).unwrap(),
            b"verified source"
        );
    }

    #[test]
    fn move_preserves_source_and_destination_replacements_without_clobbering() {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        let destination_root = temp.path().join("destination");
        fs::create_dir(&source_root).unwrap();
        fs::create_dir(&destination_root).unwrap();
        let source_path = source_root.join("part.stl");
        let destination_path = destination_root.join("part.stl");
        let expected_path = destination_root.join("original-after-race.stl");
        fs::write(&source_path, b"expected model bytes").unwrap();
        let source_directory = Directory::open(&source_root.canonicalize().unwrap()).unwrap();
        let destination_directory =
            Directory::open(&destination_root.canonicalize().unwrap()).unwrap();
        let expected = (
            b"expected model bytes".len() as u64,
            format!("{:x}", Sha256::digest(b"expected model bytes")),
        );

        let result = source_directory.move_verified_file_to_with_hooks(
            std::ffi::OsStr::new("part.stl"),
            &destination_directory,
            std::ffi::OsStr::new("part.stl"),
            &expected,
            1024,
            || {},
            || {
                fs::rename(&destination_path, &expected_path).unwrap();
                fs::write(&destination_path, b"destination replacement").unwrap();
                fs::write(&source_path, b"source replacement").unwrap();
            },
        );

        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&source_path).unwrap(), b"source replacement");
        assert_eq!(
            fs::read(&destination_path).unwrap(),
            b"destination replacement"
        );
        assert_eq!(fs::read(&expected_path).unwrap(), b"expected model bytes");
        assert!(fs::read_dir(&source_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .any(|name| name.to_string_lossy().ends_with(".journal")));
    }

    #[test]
    fn restore_held_entry_never_overwrites_a_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        fs::create_dir(&source_root).unwrap();
        let source_path = source_root.join("part.stl");
        fs::write(&source_path, b"held original").unwrap();
        let source_directory = Directory::open(&source_root.canonicalize().unwrap()).unwrap();
        let expected = (
            b"held original".len() as u64,
            format!("{:x}", Sha256::digest(b"held original")),
        );
        let held = source_directory
            .hold_verified_file(std::ffi::OsStr::new("part.stl"), &expected, 1024)
            .unwrap();
        fs::write(&source_path, b"new source bytes").unwrap();

        let error = source_directory
            .restore_held_entry(&held, std::ffi::OsStr::new("part.stl"))
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&source_path).unwrap(), b"new source bytes");
        assert_eq!(
            fs::read(source_root.join(&held.hold_name)).unwrap(),
            b"held original"
        );
    }

    #[test]
    fn recovery_refuses_a_held_file_whose_bytes_no_longer_match_its_journal() {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        fs::create_dir(&source_root).unwrap();
        fs::write(source_root.join("part.stl"), b"verified bytes").unwrap();
        let source_directory = Directory::open(&source_root.canonicalize().unwrap()).unwrap();
        let expected = (
            b"verified bytes".len() as u64,
            format!("{:x}", Sha256::digest(b"verified bytes")),
        );
        let held = source_directory
            .hold_verified_file(std::ffi::OsStr::new("part.stl"), &expected, 1024)
            .unwrap();
        fs::write(source_root.join(&held.hold_name), b"changed after hold").unwrap();

        let error = source_directory
            .recover_held_entry(&held.journal_name)
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(!source_root.join("part.stl").exists());
        assert_eq!(
            fs::read(source_root.join(&held.hold_name)).unwrap(),
            b"changed after hold"
        );
        assert!(source_root.join(&held.journal_name).exists());
    }

    #[test]
    fn identity_only_move_preserves_directory_contents_and_writes_a_journal() {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        let destination_root = temp.path().join("destination");
        fs::create_dir(&source_root).unwrap();
        fs::create_dir(&destination_root).unwrap();
        let source_folder = source_root.join("models");
        fs::create_dir(&source_folder).unwrap();
        fs::write(source_folder.join("part.stl"), b"visible folder model").unwrap();
        let source_directory = Directory::open(&source_root.canonicalize().unwrap()).unwrap();
        let destination_directory =
            Directory::open(&destination_root.canonicalize().unwrap()).unwrap();

        let moved = source_directory
            .move_entry_to(
                std::ffi::OsStr::new("models"),
                &destination_directory,
                std::ffi::OsStr::new("models"),
            )
            .unwrap();

        assert!(moved.expected.is_none());
        assert!(!source_folder.exists());
        assert_eq!(
            fs::read(destination_root.join("models/part.stl")).unwrap(),
            b"visible folder model"
        );
        assert!(source_root.join(&moved.journal_name).exists());
        assert!(source_root
            .join(format!("{}.verified", moved.journal_name.to_string_lossy()))
            .exists());
    }

    #[test]
    fn identity_only_directory_move_preserves_entries_on_source_and_destination_collision() {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        let destination_root = temp.path().join("destination");
        fs::create_dir(&source_root).unwrap();
        fs::create_dir(&destination_root).unwrap();
        let source_folder = source_root.join("models");
        let destination_folder = destination_root.join("models");
        let preserved_folder = destination_root.join("expected-after-race");
        fs::create_dir(&source_folder).unwrap();
        fs::write(source_folder.join("part.stl"), b"expected tree bytes").unwrap();
        let source_directory = Directory::open(&source_root.canonicalize().unwrap()).unwrap();
        let destination_directory =
            Directory::open(&destination_root.canonicalize().unwrap()).unwrap();

        let result = source_directory.move_entry_to_with_hooks(
            std::ffi::OsStr::new("models"),
            &destination_directory,
            std::ffi::OsStr::new("models"),
            || {
                fs::rename(&destination_folder, &preserved_folder).unwrap();
                fs::create_dir(&destination_folder).unwrap();
                fs::write(
                    destination_folder.join("part.stl"),
                    b"destination replacement",
                )
                .unwrap();
                fs::create_dir(&source_folder).unwrap();
                fs::write(source_folder.join("part.stl"), b"source replacement").unwrap();
            },
        );

        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read(preserved_folder.join("part.stl")).unwrap(),
            b"expected tree bytes"
        );
        assert_eq!(
            fs::read(destination_folder.join("part.stl")).unwrap(),
            b"destination replacement"
        );
        assert_eq!(
            fs::read(source_folder.join("part.stl")).unwrap(),
            b"source replacement"
        );
        assert!(fs::read_dir(&source_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .any(|name| name.to_string_lossy().ends_with(".journal")));
    }

    #[test]
    fn crash_after_identity_only_move_rename_keeps_destination_and_source_journal() {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        let destination_root = temp.path().join("destination");
        fs::create_dir(&source_root).unwrap();
        fs::create_dir(&destination_root).unwrap();
        let source_folder = source_root.join("models");
        fs::create_dir(&source_folder).unwrap();
        fs::write(source_folder.join("part.stl"), b"crash-safe moved tree").unwrap();
        let source_directory = Directory::open(&source_root.canonicalize().unwrap()).unwrap();
        let destination_directory =
            Directory::open(&destination_root.canonicalize().unwrap()).unwrap();

        let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = source_directory.move_entry_to_with_hooks(
                std::ffi::OsStr::new("models"),
                &destination_directory,
                std::ffi::OsStr::new("models"),
                || panic!("simulated process interruption after directory move"),
            );
        }));

        assert!(crash.is_err());
        assert!(!source_folder.exists());
        assert_eq!(
            fs::read(destination_root.join("models/part.stl")).unwrap(),
            b"crash-safe moved tree"
        );
        let journal = fs::read_dir(&source_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .find(|name| name.to_string_lossy().ends_with(".journal"))
            .expect("move journal survives the simulated crash");
        let contents = fs::read_to_string(source_root.join(&journal)).unwrap();
        assert!(contents.contains("source_hex=6d6f64656c73"));
        assert!(source_root
            .join(format!("{}.moved", journal.to_string_lossy()))
            .exists());
    }

    #[test]
    fn replacement_atomically_publishes_new_bytes_and_holds_previous_destination() {
        let temp = tempfile::tempdir().unwrap();
        let parent_path = temp.path().canonicalize().unwrap();
        let parent = Directory::open(&parent_path).unwrap();
        fs::write(parent_path.join("journal.tmp"), b"new journal bytes").unwrap();
        fs::write(parent_path.join("journal"), b"previous journal bytes").unwrap();

        parent
            .replace_child(
                std::ffi::OsStr::new("journal.tmp"),
                std::ffi::OsStr::new("journal"),
            )
            .unwrap();

        assert_eq!(
            fs::read(parent_path.join("journal")).unwrap(),
            b"new journal bytes"
        );
        assert!(!parent_path.join("journal.tmp").exists());
        let names = fs::read_dir(&parent_path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        let held_name = names
            .iter()
            .find(|name| name.to_string_lossy().ends_with(".data"))
            .expect("previous journal bytes should remain held");
        assert_eq!(
            fs::read(parent_path.join(held_name)).unwrap(),
            b"previous journal bytes"
        );
        assert!(names.iter().any(|name| {
            name.to_string_lossy().contains("gogglelab-replace-")
                && name.to_string_lossy().ends_with(".journal.verified")
        }));
    }

    #[test]
    fn crash_after_atomic_replacement_keeps_both_entries_and_the_operation_journal() {
        let temp = tempfile::tempdir().unwrap();
        let parent_path = temp.path().canonicalize().unwrap();
        let parent = Directory::open(&parent_path).unwrap();
        fs::write(parent_path.join("journal.tmp"), b"new journal bytes").unwrap();
        fs::write(parent_path.join("journal"), b"previous journal bytes").unwrap();

        let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = parent.replace_child_with_hook(
                std::ffi::OsStr::new("journal.tmp"),
                std::ffi::OsStr::new("journal"),
                || panic!("simulated process interruption after atomic exchange"),
            );
        }));

        assert!(crash.is_err());
        assert_eq!(
            fs::read(parent_path.join("journal")).unwrap(),
            b"new journal bytes"
        );
        assert_eq!(
            fs::read(parent_path.join("journal.tmp")).unwrap(),
            b"previous journal bytes"
        );
        assert!(fs::read_dir(&parent_path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .any(|name| name.to_string_lossy().contains("gogglelab-replace-")
                && name.to_string_lossy().ends_with(".journal.moved")));
    }

    #[test]
    fn replacement_preserves_a_source_name_changed_after_atomic_exchange() {
        let temp = tempfile::tempdir().unwrap();
        let parent_path = temp.path().canonicalize().unwrap();
        let parent = Directory::open(&parent_path).unwrap();
        let source = parent_path.join("journal.tmp");
        let destination = parent_path.join("journal");
        let displaced = parent_path.join("previous-journal-after-race");
        fs::write(&source, b"new journal bytes").unwrap();
        fs::write(&destination, b"previous journal bytes").unwrap();

        let result = parent.replace_child_with_hook(
            std::ffi::OsStr::new("journal.tmp"),
            std::ffi::OsStr::new("journal"),
            || {
                fs::rename(&source, &displaced).unwrap();
                fs::write(&source, b"replacement temp bytes").unwrap();
            },
        );

        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&destination).unwrap(), b"new journal bytes");
        assert_eq!(fs::read(&source).unwrap(), b"replacement temp bytes");
        assert_eq!(fs::read(&displaced).unwrap(), b"previous journal bytes");
        assert!(fs::read_dir(&parent_path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .any(|name| name.to_string_lossy().contains("gogglelab-replace-")
                && name.to_string_lossy().ends_with(".journal")));
    }

    #[test]
    fn replace_child_if_same_holds_the_expected_old_bytes_after_atomic_exchange() {
        let temp = tempfile::tempdir().unwrap();
        let parent_path = temp.path().canonicalize().unwrap();
        let parent = Directory::open(&parent_path).unwrap();
        fs::write(parent_path.join("settings.tmp"), b"new settings").unwrap();
        fs::write(parent_path.join("settings.json"), b"old settings").unwrap();
        let expected_old = (
            b"old settings".len() as u64,
            format!("{:x}", Sha256::digest(b"old settings")),
        );

        parent
            .replace_child_if_same(
                std::ffi::OsStr::new("settings.tmp"),
                std::ffi::OsStr::new("settings.json"),
                &expected_old,
                1024,
            )
            .unwrap();

        assert_eq!(
            fs::read(parent_path.join("settings.json")).unwrap(),
            b"new settings"
        );
        assert!(!parent_path.join("settings.tmp").exists());
        let held_name = fs::read_dir(&parent_path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .find(|name| name.to_string_lossy().ends_with(".data"))
            .expect("previous settings should remain recoverable");
        assert_eq!(
            fs::read(parent_path.join(held_name)).unwrap(),
            b"old settings"
        );
    }

    #[test]
    fn replace_child_if_same_rolls_back_when_destination_changes_after_preflight() {
        let temp = tempfile::tempdir().unwrap();
        let parent_path = temp.path().canonicalize().unwrap();
        let parent = Directory::open(&parent_path).unwrap();
        let source = parent_path.join("settings.tmp");
        let destination = parent_path.join("settings.json");
        let displaced = parent_path.join("previous-settings-after-race");
        fs::write(&source, b"new settings").unwrap();
        fs::write(&destination, b"old settings").unwrap();
        let expected_old = (
            b"old settings".len() as u64,
            format!("{:x}", Sha256::digest(b"old settings")),
        );

        let result = parent.replace_child_if_same_with_hook(
            std::ffi::OsStr::new("settings.tmp"),
            std::ffi::OsStr::new("settings.json"),
            &expected_old,
            1024,
            || {
                fs::rename(&destination, &displaced).unwrap();
                fs::write(&destination, b"concurrent settings edit").unwrap();
            },
        );

        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&source).unwrap(), b"new settings");
        assert_eq!(fs::read(&destination).unwrap(), b"concurrent settings edit");
        assert_eq!(fs::read(&displaced).unwrap(), b"old settings");
    }

    #[test]
    fn restore_tree_copy_stays_in_the_pinned_stage_after_parent_path_swap() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let source = temp_root.join("restored-backup");
        let stage = temp_root.join("stage");
        let outside = temp_root.join("outside");
        fs::create_dir_all(source.join("originals")).unwrap();
        fs::create_dir(&stage).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(source.join("library.sqlite3"), b"synthetic database").unwrap();
        fs::write(source.join("originals/part.stl"), b"synthetic model").unwrap();
        fs::write(outside.join("library.sqlite3"), b"preserve database").unwrap();
        let stage_directory = Directory::open(&stage).unwrap();
        let ready = Arc::new(Barrier::new(2));
        let proceed = Arc::new(Barrier::new(2));
        let worker_ready = ready.clone();
        let worker_proceed = proceed.clone();
        let source_thread = source.clone();
        let handle = thread::spawn(move || {
            worker_ready.wait();
            worker_proceed.wait();
            stage_directory.copy_tree_from_path(&source_thread, 1024)
        });
        ready.wait();
        let displaced = swap_path_for_outside(&stage, &outside);
        proceed.wait();
        handle.join().unwrap().unwrap();
        assert_eq!(
            fs::read(outside.join("library.sqlite3")).unwrap(),
            b"preserve database"
        );
        assert_eq!(
            fs::read(displaced.join("originals/part.stl")).unwrap(),
            b"synthetic model"
        );
    }

    #[test]
    fn backup_restore_writes_through_the_pinned_runtime_target_after_path_swap() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let source_library = temp_root.join("source-library");
        crate::LibraryDb::open(&source_library).unwrap();
        let backup = temp_root.join("backup.zip");
        library_backup::create_backup(&source_library, &backup).unwrap();

        let staging = temp_root.join("staging");
        let outside = temp_root.join("outside");
        fs::create_dir(&staging).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("preserve.txt"), b"external sentinel").unwrap();
        let staging_directory = Directory::open(&staging).unwrap();
        let ready = Arc::new(Barrier::new(2));
        let proceed = Arc::new(Barrier::new(2));
        let worker_ready = ready.clone();
        let worker_proceed = proceed.clone();
        let backup_thread = backup.clone();
        let handle = thread::spawn(move || {
            let mut target = crate::GuardedDirectory(staging_directory);
            worker_ready.wait();
            worker_proceed.wait();
            let result = library_backup::restore_backup_to_target(&backup_thread, &mut target);
            (result, target)
        });

        ready.wait();
        let displaced = swap_path_for_outside(&staging, &outside);
        proceed.wait();
        let (result, target) = handle.join().unwrap();

        result.unwrap();
        assert!(target.matches_path(&displaced).unwrap());
        assert!(!target.matches_path(&staging).unwrap_or(false));
        assert_eq!(
            fs::read(outside.join("preserve.txt")).unwrap(),
            b"external sentinel"
        );
        assert!(!outside.join("library.sqlite3").exists());
        assert!(displaced.join("library.sqlite3").is_file());
        assert!(crate::LibraryDb::open(&displaced).is_ok());
    }

    #[test]
    fn restore_swap_stays_in_pinned_parent_after_parent_path_swap() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let parent_path = temp_root.join("library-parent");
        let outside = temp_root.join("outside");
        fs::create_dir(&parent_path).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::create_dir(parent_path.join("library")).unwrap();
        fs::write(parent_path.join("library/marker"), b"original library").unwrap();
        fs::create_dir(parent_path.join("staged")).unwrap();
        fs::write(parent_path.join("staged/marker"), b"restored library").unwrap();
        fs::write(outside.join("sentinel"), b"preserve outside").unwrap();

        let root_name = std::ffi::OsString::from("library");
        let staged_name = std::ffi::OsString::from("staged");
        let previous_name = std::ffi::OsString::from("library.previous");
        let failed_name = std::ffi::OsString::from("library.failed");
        let ready = Arc::new(Barrier::new(2));
        let proceed = Arc::new(Barrier::new(2));
        let worker_ready = ready.clone();
        let worker_proceed = proceed.clone();
        let parent_thread = parent_path.clone();
        let root_name_thread = root_name.clone();
        let staged_name_thread = staged_name.clone();
        let previous_name_thread = previous_name.clone();
        let failed_name_thread = failed_name.clone();
        let handle = thread::spawn(move || {
            let parent = Directory::open(&parent_thread).unwrap();
            let original = parent.open_child(&root_name_thread).unwrap();
            let staged = parent.open_child(&staged_name_thread).unwrap();
            let result = crate::swap_restored_library_with(
                &parent,
                &original,
                &staged,
                &root_name_thread,
                &staged_name_thread,
                &previous_name_thread,
                &failed_name_thread,
                || {
                    worker_ready.wait();
                    worker_proceed.wait();
                },
                |installed| {
                    if fs::read(installed.join("marker")).map_err(|error| error.to_string())?
                        != b"restored library"
                    {
                        return Err("restored marker mismatch".into());
                    }
                    Ok(())
                },
            );
            (result, parent, original, staged)
        });

        ready.wait();
        let displaced = parent_path.with_extension("displaced");
        fs::rename(&parent_path, &displaced).unwrap();
        std::os::unix::fs::symlink(&outside, &parent_path).unwrap();
        proceed.wait();
        let (result, parent, original, staged) = handle.join().unwrap();

        result.unwrap();
        assert_eq!(
            fs::read(displaced.join("library/marker")).unwrap(),
            b"restored library"
        );
        assert_eq!(
            fs::read(displaced.join("library.previous/marker")).unwrap(),
            b"original library"
        );
        assert_eq!(
            fs::read(outside.join("sentinel")).unwrap(),
            b"preserve outside"
        );
        assert!(!outside.join("library").exists());
        assert!(original
            .matches_path(&parent.child_path(&previous_name).unwrap())
            .unwrap());
        assert!(staged
            .matches_path(&parent.child_path(&root_name).unwrap())
            .unwrap());
    }

    #[test]
    fn restore_swap_rolls_back_when_the_pinned_stage_name_changes() {
        let temp = tempfile::tempdir().unwrap();
        let parent_path = temp.path().join("library-parent");
        fs::create_dir(&parent_path).unwrap();
        let parent_path = parent_path.canonicalize().unwrap();
        fs::create_dir(parent_path.join("library")).unwrap();
        fs::write(parent_path.join("library/marker"), b"original library").unwrap();
        fs::create_dir(parent_path.join("staged")).unwrap();
        fs::write(parent_path.join("staged/marker"), b"restored library").unwrap();

        let parent = Directory::open(&parent_path).unwrap();
        let original = parent.open_child(std::ffi::OsStr::new("library")).unwrap();
        let staged = parent.open_child(std::ffi::OsStr::new("staged")).unwrap();
        let result = crate::swap_restored_library_with(
            &parent,
            &original,
            &staged,
            std::ffi::OsStr::new("library"),
            std::ffi::OsStr::new("staged"),
            std::ffi::OsStr::new("library.previous"),
            std::ffi::OsStr::new("library.failed"),
            || {
                fs::rename(
                    parent_path.join("staged"),
                    parent_path.join("displaced-staging"),
                )
                .unwrap();
            },
            |_| Ok(()),
        )
        .unwrap_err();

        assert!(result.recovered, "{result:?}");
        assert!(result.message.contains("staging directory changed"));
        assert_eq!(
            fs::read(parent_path.join("library/marker")).unwrap(),
            b"original library"
        );
        assert_eq!(
            fs::read(parent_path.join("displaced-staging/marker")).unwrap(),
            b"restored library"
        );
        assert!(!parent_path.join("library.previous").exists());
    }

    #[test]
    fn restore_swap_rolls_back_validation_failure_and_preserves_failed_stage() {
        let temp = tempfile::tempdir().unwrap();
        let parent_path = temp.path().join("library-parent");
        fs::create_dir(&parent_path).unwrap();
        let parent_path = parent_path.canonicalize().unwrap();
        fs::create_dir(parent_path.join("library")).unwrap();
        fs::write(parent_path.join("library/marker"), b"original library").unwrap();
        fs::create_dir(parent_path.join("staged")).unwrap();
        fs::write(parent_path.join("staged/marker"), b"restored library").unwrap();

        let parent = Directory::open(&parent_path).unwrap();
        let original = parent.open_child(std::ffi::OsStr::new("library")).unwrap();
        let staged = parent.open_child(std::ffi::OsStr::new("staged")).unwrap();
        let failure = crate::swap_restored_library_with(
            &parent,
            &original,
            &staged,
            std::ffi::OsStr::new("library"),
            std::ffi::OsStr::new("staged"),
            std::ffi::OsStr::new("library.previous"),
            std::ffi::OsStr::new("library.failed"),
            || {},
            |_| Err("injected catalog validation failure".into()),
        )
        .unwrap_err();

        assert!(failure.recovered, "{failure:?}");
        assert!(failure
            .message
            .contains("injected catalog validation failure"));
        assert_eq!(
            fs::read(parent_path.join("library/marker")).unwrap(),
            b"original library"
        );
        assert_eq!(
            fs::read(parent_path.join("library.failed/marker")).unwrap(),
            b"restored library"
        );
        assert!(!parent_path.join("library.previous").exists());
    }

    #[test]
    fn purge_sibling_swap_stays_in_the_pinned_parent_after_parent_path_swap() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = temp.path().canonicalize().unwrap();
        let parent_path = temp_root.join("catalog-parent");
        let outside = temp_root.join("outside");
        fs::create_dir(&parent_path).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::create_dir(parent_path.join("library")).unwrap();
        fs::write(parent_path.join("library/sentinel"), b"old library").unwrap();
        fs::create_dir(parent_path.join("staged")).unwrap();
        fs::write(parent_path.join("staged/sentinel"), b"empty library").unwrap();
        fs::write(outside.join("library"), b"preserve external entry").unwrap();
        let parent = crate::GuardedDirectoryParent::open(&parent_path).unwrap();
        let ready = Arc::new(Barrier::new(2));
        let proceed = Arc::new(Barrier::new(2));
        let worker_ready = ready.clone();
        let worker_proceed = proceed.clone();
        let handle = thread::spawn(move || {
            worker_ready.wait();
            worker_proceed.wait();
            parent.swap_library_children(
                std::ffi::OsStr::new("library"),
                std::ffi::OsStr::new("staged"),
                std::ffi::OsStr::new("library.previous"),
            )
        });
        ready.wait();
        let displaced = parent_path.with_extension("displaced");
        fs::rename(&parent_path, &displaced).unwrap();
        std::os::unix::fs::symlink(&outside, &parent_path).unwrap();
        proceed.wait();
        handle.join().unwrap().unwrap();
        assert_eq!(
            fs::read(outside.join("library")).unwrap(),
            b"preserve external entry"
        );
        assert_eq!(
            fs::read(displaced.join("library/sentinel")).unwrap(),
            b"empty library"
        );
        assert_eq!(
            fs::read(displaced.join("library.previous/sentinel")).unwrap(),
            b"old library"
        );
    }

    #[test]
    fn empty_library_staging_database_opens_through_its_directory_handle() {
        let temp = tempfile::tempdir().unwrap();
        let parent_path = temp.path().canonicalize().unwrap();
        let parent = crate::GuardedDirectoryParent::open(&parent_path).unwrap();
        let stage = parent
            .create_empty_library_child(std::ffi::OsStr::new("stage"))
            .unwrap();
        let db = library_core::LibraryDb::open(stage.io_path().unwrap()).unwrap();
        assert!(db.list_models(None, None).unwrap().is_empty());
        assert!(db.list_index_roots().unwrap().is_empty());
    }
}
