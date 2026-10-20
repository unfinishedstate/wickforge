//! Persistence-only OS operations. Windows inherits normal per-user profile
//! ACLs; it does not emulate Unix permission bits or directory fsync.

use std::fs::File;
use std::io;
use std::path::Path;

pub(super) fn sync_file(file: &File) -> io::Result<()> {
    file.sync_all()?;
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // fsync alone may leave data in the drive cache on macOS.
        // SAFETY: this is an owned, open file descriptor; F_FULLFSYNC takes no
        // additional argument and does not transfer descriptor ownership.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn replace(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
pub(super) fn replace(source: &Path, destination: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = native_path(source)?;
    let destination = native_path(destination)?;
    // Both files are siblings. No COPY_ALLOWED: replacement must be a native
    // same-volume rename, never a copy/delete or delete-destination fallback.
    // WRITE_THROUGH supplies the durable namespace operation where Windows
    // offers no unprivileged Unix-style directory fsync.
    // SAFETY: both buffers are live, NUL-terminated UTF-16 paths.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
fn native_path(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;

    // Canonicalize the existing parent, not the destination (which may not yet
    // exist). This also obtains a verbatim path for Unicode/long-path support.
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing path parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing file name"))?;
    let absolute = std::fs::canonicalize(parent)?.join(name);
    let mut encoded: Vec<u16> = absolute.as_os_str().encode_wide().collect();
    if encoded.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in workspace path",
        ));
    }
    encoded.push(0);
    Ok(encoded)
}

#[cfg(windows)]
fn known_folder(id: &windows_sys::core::GUID) -> Option<std::path::PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;

    let mut raw = std::ptr::null_mut();
    // SAFETY: the GUID and output pointer are valid; a null token means the
    // current user. The shell owns allocation; CoTaskMemFree releases it below.
    let status = unsafe { SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut raw) };
    let result = if status >= 0 && !raw.is_null() {
        let mut length = 0;
        // SAFETY: a successful call returns a NUL-terminated UTF-16 allocation.
        unsafe {
            while *raw.add(length) != 0 {
                length += 1;
            }
            let path = std::path::PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(
                raw, length,
            )));
            path.is_absolute().then_some(path)
        }
    } else {
        None
    };
    if !raw.is_null() {
        // SAFETY: this allocation came from SHGetKnownFolderPath and is no
        // longer referenced, including on a failed shell call.
        unsafe { CoTaskMemFree(raw.cast()) };
    }
    result
}

#[cfg(windows)]
pub(super) fn local_app_data() -> Option<std::path::PathBuf> {
    known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_LocalAppData)
}

#[cfg(windows)]
pub(super) fn profile_home() -> Option<std::path::PathBuf> {
    known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Profile)
}
