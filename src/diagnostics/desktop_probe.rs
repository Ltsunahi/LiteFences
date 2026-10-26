//! Read-only shell inventory and same-process hidden-window recovery harness.
//! Diagnostic binary only: COM runs on its STA; the runner bounds process lifetime.
use crate::host_recovery::{HostIdentity, Recovery, RecoveryAction};
use std::{
    io,
    mem::size_of,
    ptr::{null, null_mut},
};
use windows::{
    Win32::{
        System::{Com::*, Variant::VARIANT},
        UI::Shell::*,
    },
    core::Interface,
};
use windows_sys::Win32::{
    Foundation::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{HiDpi::*, WindowsAndMessaging::*},
};

struct Entry {
    hwnd: usize,
    parent: usize,
    pid: u32,
    class: &'static str,
    visible: bool,
    dpi: u32,
    rect: RECT,
}

fn allowed_class(hwnd: HWND) -> Option<&'static str> {
    let mut name = [0_u16; 128];
    // SAFETY: name is a valid writable buffer. No cross-process pointers/messages.
    let length = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
    if length <= 0 {
        return None;
    }
    match String::from_utf16_lossy(&name[..length as usize]).as_str() {
        "Progman" => Some("Progman"),
        "WorkerW" => Some("WorkerW"),
        "Shell_TrayWnd" => Some("Shell_TrayWnd"),
        "SHELLDLL_DefView" => Some("SHELLDLL_DefView"),
        "SysListView32" => Some("SysListView32"),
        _ => None,
    }
}

fn entry(hwnd: HWND, class: &'static str) -> Option<Entry> {
    let mut pid = 0;
    let mut rect = RECT::default();
    // SAFETY: read-only queries; HWND is borrowed and may disappear at any point.
    unsafe {
        if GetWindowThreadProcessId(hwnd, &mut pid) == 0 || GetWindowRect(hwnd, &mut rect) == 0 {
            return None;
        }
        Some(Entry {
            hwnd: hwnd as usize,
            parent: GetAncestor(hwnd, GA_PARENT) as usize,
            pid,
            class,
            visible: IsWindowVisible(hwnd) != 0,
            dpi: GetDpiForWindow(hwnd),
            rect,
        })
    }
}

unsafe extern "system" fn child(hwnd: HWND, context: LPARAM) -> windows_sys::core::BOOL {
    // SAFETY: synchronous EnumChildWindows holds exclusive access to this live Vec.
    unsafe {
        if let Some(class @ ("SHELLDLL_DefView" | "SysListView32")) = allowed_class(hwnd)
            && let Some(item) = entry(hwnd, class)
        {
            (&mut *(context as *mut Vec<Entry>)).push(item);
        }
    }
    1
}

unsafe extern "system" fn top(hwnd: HWND, context: LPARAM) -> windows_sys::core::BOOL {
    if let Some(class @ ("Progman" | "WorkerW" | "Shell_TrayWnd")) = allowed_class(hwnd) {
        // SAFETY: enumeration is synchronous. End the Vec borrow before child callbacks.
        unsafe {
            if let Some(item) = entry(hwnd, class) {
                (&mut *(context as *mut Vec<Entry>)).push(item);
            }
            EnumChildWindows(hwnd, Some(child), context);
        }
    }
    1
}

struct ComApartment;
impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: constructed only after successful initialization, on this same thread.
        unsafe {
            CoUninitialize();
        }
    }
}

fn shell_view() -> windows::core::Result<(i32, bool)> {
    // SAFETY: this short-lived diagnostic uses one STA. All COM references are dropped
    // before the apartment guard; no raw interfaces escape or cross threads.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _apartment = ComApartment;
        let shell: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
        let location = VARIANT::default();
        let mut hwnd = 0;
        let dispatch = shell.FindWindowSW(
            &location,
            &location,
            SWC_DESKTOP,
            &mut hwnd,
            SWFO_NEEDDISPATCH,
        )?;
        let provider: IServiceProvider = dispatch.cast()?;
        let browser: IShellBrowser = provider.QueryService(&SID_STopLevelBrowser)?;
        let view: IFolderView = browser.QueryActiveShellView()?.cast()?;
        let count = view.ItemCount(SVGIO_ALLVIEW)?;
        // Preserve S_FALSE, which the convenience Result<()> wrapper discards.
        let auto_arrange = (view.vtable().GetAutoArrange)(view.as_raw());
        auto_arrange.ok()?;
        Ok((count, auto_arrange.0 == 0))
    }
}

pub fn snapshot() -> io::Result<String> {
    let mut entries: Vec<Entry> = Vec::new();
    // SAFETY: process is freshly started and owns no windows. Read-only enumeration
    // borrows entries synchronously; callbacks do not store the pointer or mutate HWNDs.
    let shell = unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == 0 {
            return Err(io::Error::last_os_error());
        }
        if EnumWindows(Some(top), (&mut entries as *mut Vec<Entry>) as LPARAM) == 0 {
            return Err(io::Error::last_os_error());
        }
        GetShellWindow() as usize
    };
    let inventory = entries.iter().map(|e| format!(
        "{{\"hwnd\":{},\"parent\":{},\"pid\":{},\"class\":\"{}\",\"visible\":{},\"dpi\":{},\"rect\":[{},{},{},{}]}}",
        e.hwnd,e.parent,e.pid,e.class,e.visible,e.dpi,e.rect.left,e.rect.top,e.rect.right,e.rect.bottom
    )).collect::<Vec<_>>().join(",");
    let view = match shell_view() {
        Ok((count, auto)) => {
            format!("{{\"status\":\"ok\",\"item_count\":{count},\"auto_arrange\":{auto}}}")
        }
        Err(error) => format!(
            "{{\"status\":\"unavailable\",\"hresult\":{}}}",
            error.code().0
        ),
    };
    Ok(format!(
        "{{\"schema_version\":1,\"mode\":\"read-only\",\"shell_hwnd\":{shell},\"windows\":[{inventory}],\"folder_view\":{view}}}"
    ))
}

const CLASS: windows_sys::core::PCWSTR = windows_sys::core::w!("LiteFences.P0.MockHost");
struct Class(HINSTANCE);
impl Drop for Class {
    fn drop(&mut self) {
        // SAFETY: every owned HWND is destroyed before the class guard.
        unsafe {
            UnregisterClassW(CLASS, self.0);
        }
    }
}
struct Window(HWND);
impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: only this UI thread creates/destroys these hidden test HWNDs. Never
        // wrap a borrowed shell window. Clear child handles after parent destruction.
        unsafe {
            if !self.0.is_null() {
                DestroyWindow(self.0);
            }
        }
    }
}
fn create(instance: HINSTANCE, parent: HWND) -> io::Result<Window> {
    // SAFETY: registered test class and optional same-thread test parent are live.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            CLASS,
            windows_sys::core::w!("mock"),
            if parent.is_null() {
                WS_OVERLAPPED
            } else {
                WS_CHILD
            },
            0,
            0,
            100,
            100,
            parent,
            null_mut(),
            instance,
            null(),
        )
    };
    if hwnd.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(Window(hwnd))
    }
}
fn identity(hwnd: HWND) -> Option<HostIdentity> {
    let mut pid = 0;
    // SAFETY: borrowed HWND is only queried. This is not an atomic liveness guarantee;
    // the production adapter must revalidate at use and invalidate each generation.
    unsafe {
        if IsWindow(hwnd) == 0 || GetWindowThreadProcessId(hwnd, &mut pid) == 0 {
            None
        } else {
            Some(HostIdentity {
                hwnd: hwnd as usize,
                process_id: pid,
            })
        }
    }
}
fn generation(action: RecoveryAction) -> io::Result<u64> {
    match action {
        RecoveryAction::Probe { generation, .. } => Ok(generation),
        _ => Err(io::Error::other("Expected probe")),
    }
}
fn check(value: bool, message: &str) -> io::Result<()> {
    if value {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}

pub fn self_test() -> io::Result<String> {
    // SAFETY: isolated test process/thread; DefWindowProc owns no user pointers.
    let instance = unsafe {
        let instance = GetModuleHandleW(null());
        if instance.is_null() {
            return Err(io::Error::last_os_error());
        }
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(DefWindowProcW),
            hInstance: instance,
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(io::Error::last_os_error());
        }
        instance
    };
    let _class = Class(instance);
    let mut recovery = Recovery::new(3);
    let mut host = create(instance, null_mut())?;
    let first = generation(recovery.on_host_lost())?;
    check(
        matches!(
            recovery.on_probe_result(first, identity(host.0)),
            RecoveryAction::Attached(_)
        ),
        "Initial host not found",
    )?;
    for _ in 0..20 {
        let mut panel = create(instance, host.0)?;
        let old = recovery.current();
        // SAFETY: destroy only our own hidden mock parent, not Explorer. Win32 also
        // destroys its child. Clear guards immediately so handle reuse cannot double-drop.
        unsafe {
            check(GetParent(panel.0) == host.0, "Child parent mismatch")?;
            check(DestroyWindow(host.0) != 0, "Mock destruction failed")?;
            let old_host = host.0;
            host.0 = null_mut();
            let old_panel = panel.0;
            panel.0 = null_mut();
            check(
                IsWindow(old_host) == 0 && IsWindow(old_panel) == 0,
                "Mock windows survived destruction",
            )?;
        }
        let stale = generation(recovery.on_host_lost())?;
        check(recovery.current().is_none(), "Old host identity retained")?;
        check(
            recovery.on_probe_result(stale, None) == RecoveryAction::Waiting,
            "Missing host must wait",
        )?;
        let retry = generation(recovery.on_retry_timer())?;
        check(
            recovery.on_probe_result(retry, None) == RecoveryAction::Waiting,
            "Second miss must wait",
        )?;
        let last = generation(recovery.on_retry_timer())?;
        check(
            recovery.on_probe_result(last, None) == RecoveryAction::Exhausted,
            "Retries must be bounded",
        )?;
        check(
            recovery.on_retry_timer() == RecoveryAction::Detached,
            "Exhausted timer must do nothing",
        )?;
        host = create(instance, null_mut())?;
        let fresh = generation(recovery.on_shell_hint())?;
        check(
            recovery.on_probe_result(stale, old) == RecoveryAction::Detached,
            "Stale result accepted",
        )?;
        let new_id = identity(host.0);
        check(
            matches!(
                recovery.on_probe_result(fresh, new_id),
                RecoveryAction::Attached(_)
            ) && recovery.current() == new_id,
            "Replacement identity not accepted",
        )?;
    }
    Ok("{\"schema_version\":1,\"mode\":\"same-process-mock\",\"cycles\":20,\"passed\":true,\"explorer_restarts\":0,\"desktop_mount_tested\":false}".into())
}
