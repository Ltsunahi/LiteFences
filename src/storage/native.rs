//! Narrow Windows filesystem boundary. No Shell or desktop operations.
use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
    ptr::null,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_READ, FILE_SHARE_WRITE, MoveFileExW, ReplaceFileW,
};

pub(super) fn directory(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid configuration directory",
        ));
    }
    Ok(file)
}
pub(super) fn lock(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    regular(&file)?;
    Ok(file)
}
pub(super) fn read(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    regular(&file)?;
    Ok(file)
}
fn regular(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Not a regular configuration file",
        ));
    }
    Ok(())
}
pub(super) fn create(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(path)
}
pub(super) fn diagnostic(path: &Path) -> io::Result<File> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    regular(&file)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the live owned file handle and writable output structure span the call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.nNumberOfLinks != 1 {
        return Err(io::Error::other("Linked diagnostic file rejected"));
    }
    Ok(file)
}
fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut units: Vec<_> = path.as_os_str().encode_wide().collect();
    if units.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in configuration path",
        ));
    }
    units.push(0);
    Ok(units)
}
pub(super) fn commit(pending: &Path, destination: &Path, exists: bool) -> io::Result<()> {
    let pending = wide(pending)?;
    let destination = wide(destination)?;
    // SAFETY: owned, NUL-terminated buffers live throughout the synchronous call.
    // Paths are same-directory store-owned names. The caller holds our writer lock,
    // has flushed and closed pending, and has preserved the old committed bytes.
    // No IGNORE_ACL flags or unsupported WRITE_THROUGH flag are used. Failure may
    // leave partial rename state: caller poisons writes and retains artifacts.
    let ok = unsafe {
        if exists {
            ReplaceFileW(
                destination.as_ptr(),
                pending.as_ptr(),
                null(),
                0,
                null(),
                null(),
            )
        } else {
            MoveFileExW(pending.as_ptr(), destination.as_ptr(), 0)
        }
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
