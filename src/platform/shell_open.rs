//! Shell activation on the existing STA worker, never on the UI thread.
use super::open_policy::{
    OpenAction, OpenFailure, failure_from_hresult, failure_from_win32, validate_kind,
};
use litefences::core::model::FileReference;
use std::{
    io,
    mem::size_of,
    sync::atomic::{AtomicBool, Ordering},
};
use windows_sys::Win32::{
    Foundation::GetLastError,
    UI::{
        Shell::{
            Common::ITEMIDLIST, ILCreateFromPathW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC,
            SHELLEXECUTEINFOW, SHOpenFolderAndSelectItems, ShellExecuteExW,
        },
        WindowsAndMessaging::SW_SHOWNORMAL,
    },
};

struct OwnedPidl(*mut ITEMIDLIST);
impl Drop for OwnedPidl {
    fn drop(&mut self) {
        // SAFETY: ILCreateFromPathW gives sole ownership of a COM task-allocated
        // PIDL. The guard frees it once on this STA, including stopped/errors.
        unsafe {
            windows::Win32::System::Com::CoTaskMemFree(Some(self.0.cast()));
        }
    }
}

pub(super) fn open(
    reference: &FileReference,
    action: OpenAction,
    stopped: &AtomicBool,
) -> Result<(), OpenFailure> {
    if stopped.load(Ordering::Acquire) {
        return Err(OpenFailure::ShellUnavailable);
    }
    // The initial string-based Shell activation supports ordinary local paths.
    // Do not truncate a long path or substitute another target.
    if reference.path.as_utf16().len() >= 260 {
        return Err(OpenFailure::InvalidPath);
    }
    let native = std::path::PathBuf::from(reference.path.to_os_string());
    if action == OpenAction::Reveal && native.parent().is_none() {
        return Err(OpenFailure::NoParent);
    }
    let metadata = std::fs::metadata(&native).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => OpenFailure::Missing,
        io::ErrorKind::PermissionDenied => OpenFailure::AccessDenied,
        _ => OpenFailure::Failed,
    })?;
    validate_kind(reference.kind, metadata.is_file(), metadata.is_dir())?;
    let mut file = reference.path.as_utf16().to_vec();
    file.push(0);
    if action == OpenAction::Reveal {
        // SAFETY: file is a checked absolute UTF-16 path with a live terminator.
        // PIDL parsing and selection occur on the existing initialized STA.
        let pidl = OwnedPidl(unsafe { ILCreateFromPathW(file.as_ptr()) });
        if pidl.0.is_null() {
            return Err(OpenFailure::Failed);
        }
        if stopped.load(Ordering::Acquire) {
            return Err(OpenFailure::ShellUnavailable);
        }
        // SAFETY: with zero children the absolute PIDL denotes the item itself;
        // Windows opens its parent and selects it. No edit/desktop flags, link
        // resolution, arguments, launch, file mutation, or shell injection.
        let result = unsafe { SHOpenFolderAndSelectItems(pidl.0, 0, std::ptr::null(), 0) };
        return if result >= 0 {
            Ok(())
        } else {
            Err(failure_from_hresult(result))
        };
    }
    use std::os::windows::ffi::OsStrExt;
    let directory = native.parent().map(|parent| {
        parent
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    });
    if stopped.load(Ordering::Acquire) {
        return Err(OpenFailure::ShellUnavailable);
    }
    // SAFETY: this POD structure permits a zero initialization. All pointed-to
    // UTF-16 buffers remain alive through the synchronous call on the STA.
    // There are no arguments, environment substitution, runas, or zone bypass.
    // NOCLOSEPROCESS is not requested, so no returned process handle is owned.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
    info.lpVerb = windows_sys::core::w!("open");
    info.lpFile = file.as_ptr();
    info.lpDirectory = directory
        .as_ref()
        .map_or(std::ptr::null(), |value| value.as_ptr());
    info.nShow = SW_SHOWNORMAL;
    // Security prompts remain controlled by Windows; only ordinary error
    // dialogs are replaced with the app's own visible failure status.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        Err(failure_from_win32(unsafe { GetLastError() }))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use litefences::core::model::{NativePath, ReferenceKind};

    #[test]
    fn missing_and_type_changed_references_fail_before_any_shell_activation() {
        use std::os::windows::ffi::OsStrExt;
        let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let missing = project.join("artifacts").join(format!(
            "missing-open-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let missing = FileReference {
            kind: ReferenceKind::File,
            path: NativePath::from_utf16(missing.as_os_str().encode_wide().collect()).unwrap(),
        };
        let mismatch = FileReference {
            kind: ReferenceKind::Directory,
            path: NativePath::from_utf16(
                project
                    .join("Cargo.toml")
                    .as_os_str()
                    .encode_wide()
                    .collect(),
            )
            .unwrap(),
        };
        for action in [OpenAction::Open, OpenAction::Reveal] {
            assert_eq!(
                open(&missing, action, &AtomicBool::new(false)),
                Err(OpenFailure::Missing)
            );
            assert_eq!(
                open(&mismatch, action, &AtomicBool::new(false)),
                Err(OpenFailure::TypeChanged)
            );
        }
    }

    #[test]
    fn stopped_worker_does_not_activate_even_an_existing_target() {
        let reference = FileReference {
            kind: ReferenceKind::Directory,
            path: NativePath::from_utf16(r"C:\".encode_utf16().collect()).unwrap(),
        };
        assert_eq!(
            open(&reference, OpenAction::Open, &AtomicBool::new(true)),
            Err(OpenFailure::ShellUnavailable)
        );
    }

    #[test]
    fn long_path_is_rejected_without_truncation_or_shell_activation() {
        let reference = FileReference {
            kind: ReferenceKind::File,
            path: NativePath::from_utf16(
                format!("C:\\{}", "x".repeat(260)).encode_utf16().collect(),
            )
            .unwrap(),
        };
        assert_eq!(
            open(&reference, OpenAction::Open, &AtomicBool::new(false)),
            Err(OpenFailure::InvalidPath)
        );
    }
    #[test]
    fn reveal_root_and_stopped_requests_never_activate_explorer() {
        let root = FileReference {
            kind: ReferenceKind::Directory,
            path: NativePath::from_utf16(r"C:\".encode_utf16().collect()).unwrap(),
        };
        assert_eq!(
            open(&root, OpenAction::Reveal, &AtomicBool::new(true)),
            Err(OpenFailure::ShellUnavailable)
        );
        assert_eq!(
            open(&root, OpenAction::Reveal, &AtomicBool::new(false)),
            Err(OpenFailure::NoParent)
        );
        let long = FileReference {
            kind: ReferenceKind::File,
            path: NativePath::from_utf16(
                format!("C:\\{}", "x".repeat(260)).encode_utf16().collect(),
            )
            .unwrap(),
        };
        assert_eq!(
            open(&long, OpenAction::Reveal, &AtomicBool::new(false)),
            Err(OpenFailure::InvalidPath)
        );
    }
}
