//! Handle-relative filesystem operations for guarded management actions.
//!
//! Paths are used only to acquire directory handles. Once acquired, child
//! files are opened, created, moved, and held relative to those handles so a
//! parent-path swap cannot redirect the operation through a symlink. Guarded
//! original source cleanup preserves bytes in recoverable holds. Generated
//! cache cleanup may use hash-verified unlinking after explicit authorization.

#[cfg(not(unix))]
use std::path::Path;

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
            let source = c_name(source)?;
            let destination = c_name(destination)?;
            #[cfg(target_os = "macos")]
            let result = unsafe {
                libc::renameatx_np(
                    self.0.as_raw_fd(),
                    source.as_ptr(),
                    self.0.as_raw_fd(),
                    destination.as_ptr(),
                    libc::RENAME_EXCL,
                )
            };
            #[cfg(target_os = "linux")]
            let result = unsafe {
                libc::renameat2(
                    self.0.as_raw_fd(),
                    source.as_ptr(),
                    self.0.as_raw_fd(),
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
                self.sync()
            }
        }

        pub fn rename_child_exclusive_to(
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
                self.sync()?;
                if !std::ptr::eq(self, destination_directory) {
                    destination_directory.sync()?;
                }
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
                self.sync()
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
impl Directory {
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
