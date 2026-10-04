//! Platform filesystem operations with atomic no-replace semantics.
use std::fs::File;
use std::io;
use std::path::Path;

pub(crate) fn open_directory(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Deny deletion/renaming while held; never follow a final reparse point.
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .custom_flags(0x02000000 | 0x00200000)
            .open(path)
    }
    #[cfg(not(windows))]
    {
        File::open(path)
    }
}

#[cfg(windows)]
pub(crate) fn windows_identity(file: &File) -> io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct Info {
        attributes: u32,
        creation: [u32; 2],
        access: [u32; 2],
        write: [u32; 2],
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut Info) -> i32;
    }
    let mut info: Info = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.attributes & 0x400 != 0 || info.attributes & 0x10 == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Selected folder is not a regular directory",
        ));
    }
    Ok((
        u64::from(info.volume),
        (u64::from(info.index_high) << 32) | u64::from(info.index_low),
    ))
}

#[cfg(windows)]
pub(crate) fn lock_ancestors(root: &Path, target: &Path) -> io::Result<Vec<File>> {
    let relative = target.strip_prefix(root).map_err(io::Error::other)?;
    let mut current = root.to_path_buf();
    let mut held = vec![open_directory(root)?];
    windows_identity(&held[0])?;
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid relative path",
            ));
        };
        current.push(name);
        let directory = open_directory(&current)?;
        windows_identity(&directory)?;
        held.push(directory);
    }
    Ok(held)
}

#[cfg(windows)]
pub(crate) fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileW(from: *const u16, to: *const u16) -> i32;
    }
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileW(from.as_ptr(), to.as_ptr()) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn linux_parent(directory: &File, relative: &Path) -> io::Result<File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn openat(fd: i32, path: *const std::ffi::c_char, flags: i32, mode: u32) -> i32;
    }
    let mut current = directory.try_clone()?;
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid relative path",
            ));
        };
        let name = CString::new(name.as_bytes()).map_err(io::Error::other)?;
        // O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC, read only.
        let fd = unsafe {
            openat(
                current.as_raw_fd(),
                name.as_ptr(),
                0x10000 | 0x20000 | 0x80000,
                0,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        current = unsafe { File::from_raw_fd(fd) };
    }
    Ok(current)
}

#[cfg(target_os = "linux")]
pub(crate) fn linux_rename_at(
    from_directory: &File,
    from: &Path,
    to_directory: &File,
    to: &Path,
) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn renameat2(
            from_fd: i32,
            from: *const std::ffi::c_char,
            to_fd: i32,
            to: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    let from = CString::new(from.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(io::Error::other)?;
    if unsafe {
        renameat2(
            from_directory.as_raw_fd(),
            from.as_ptr(),
            to_directory.as_raw_fd(),
            to.as_ptr(),
            1,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
