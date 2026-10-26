//! P0-01: one ordinary top-level window. No desktop parenting or Shell mutation.
//! All window state and GDI objects are owned by the UI thread.

use crate::app_options::Options;
use std::{
    cell::Cell,
    io,
    mem::size_of,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{HiDpi::*, WindowsAndMessaging::*},
};
#[cfg(feature = "config-store")]
mod config_ui;
#[cfg(feature = "config-store")]
mod drop_policy;
#[cfg(feature = "config-store")]
mod external_drop;
#[cfg(feature = "config-store")]
mod folder_picker;
#[cfg(feature = "config-store")]
mod icon_worker;
#[cfg(feature = "config-store")]
mod item_drag;
#[cfg(feature = "ui-acceptance")]
mod loop_diagnostics;
#[cfg(feature = "ui-acceptance")]
#[allow(dead_code)] // Only loop-diagnostic reads this test-only context module.
pub mod loop_environment;
#[cfg(feature = "config-store")]
mod open_policy;
#[cfg(feature = "config-store")]
mod shell_open;
#[cfg(feature = "ui-acceptance")]
pub use loop_diagnostics::Snapshot as LoopSnapshot;
#[cfg(feature = "ui-acceptance")]
use loop_diagnostics::{Diagnostics, Event as LoopEvent};
#[cfg(feature = "config-store")]
mod picker_paths;
#[cfg(feature = "config-store")]
mod preview;
#[cfg(feature = "config-store")]
mod save_focus;
#[cfg(feature = "config-store")]
use {
    config_ui::{GroupPreview, ItemPreview, Session},
    icon_worker::{IconView, IconWorker, OpenQueueError},
    litefences::core::{
        layout::RectDip,
        model::{FileReference, GroupId, ItemId, NativePath, ReferenceKind},
    },
    litefences::services::config_worker::DataRoot,
    preview::{PreviewLayout, RectPx, item_content, item_slots},
    std::cell::RefCell,
    windows_sys::Win32::UI::{
        Controls::Dialogs::{
            CommDlgExtendedError, FNERR_BUFFERTOOSMALL, GetOpenFileNameW, OFN_ALLOWMULTISELECT,
            OFN_DONTADDTORECENT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NODEREFERENCELINKS,
            OPENFILENAMEW,
        },
        Input::KeyboardAndMouse::{
            EnableWindow, GetCapture, GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus,
            VK_CONTROL,
        },
    },
};
#[cfg(feature = "config-store")]
const SAVE_BUTTON: usize = 1001;
#[cfg(feature = "config-store")]
const CREATE_GROUP_BUTTON: usize = 1002;
#[cfg(feature = "config-store")]
const GROUP_LIST: usize = 1003;
#[cfg(feature = "config-store")]
const GROUP_NAME: usize = 1004;
#[cfg(feature = "config-store")]
const RENAME_GROUP_BUTTON: usize = 1005;
#[cfg(feature = "config-store")]
const GROUP_LIST_LABEL: usize = 1006;
#[cfg(feature = "config-store")]
const GROUP_NAME_LABEL: usize = 1007;
#[cfg(feature = "config-store")]
const GROUP_X_LABEL: usize = 1008;
#[cfg(feature = "config-store")]
const GROUP_Y_LABEL: usize = 1009;
#[cfg(feature = "config-store")]
const GROUP_X: usize = 1010;
#[cfg(feature = "config-store")]
const GROUP_Y: usize = 1011;
#[cfg(feature = "config-store")]
const MOVE_GROUP_BUTTON: usize = 1012;
#[cfg(feature = "config-store")]
const GROUP_WIDTH_LABEL: usize = 1013;
#[cfg(feature = "config-store")]
const GROUP_HEIGHT_LABEL: usize = 1014;
#[cfg(feature = "config-store")]
const GROUP_WIDTH: usize = 1015;
#[cfg(feature = "config-store")]
const GROUP_HEIGHT: usize = 1016;
#[cfg(feature = "config-store")]
const RESIZE_GROUP_BUTTON: usize = 1017;
#[cfg(feature = "config-store")]
const DELETE_GROUP_BUTTON: usize = 1018;
#[cfg(feature = "config-store")]
const GROUP_COLOR_LABEL: usize = 1019;
#[cfg(feature = "config-store")]
const GROUP_COLOR: usize = 1020;
#[cfg(feature = "config-store")]
const COLOR_GROUP_BUTTON: usize = 1021;
#[cfg(feature = "config-store")]
const FOLD_GROUP_BUTTON: usize = 1022;
#[cfg(feature = "config-store")]
const ADD_FILES_BUTTON: usize = 1023;
#[cfg(feature = "config-store")]
const ADD_FOLDER_BUTTON: usize = 1024;
#[cfg(feature = "config-store")]
const DROP_REPORT: usize = 1025;
#[cfg(feature = "config-store")]
const REMOVE_ITEM_BUTTON: usize = 1026;
#[cfg(feature = "config-store")]
const UNDO_REMOVAL_BUTTON: usize = 1027;
#[cfg(feature = "config-store")]
const SORT_EARLIER_BUTTON: usize = 1028;
#[cfg(feature = "config-store")]
const SORT_LATER_BUTTON: usize = 1029;
#[cfg(feature = "config-store")]
const REVEAL_ITEM_BUTTON: usize = 1030;
#[cfg(feature = "config-store")]
const TRANSFER_GROUP_COMBO: usize = 1031;
#[cfg(feature = "config-store")]
const TRANSFER_ITEM_BUTTON: usize = 1032;
#[cfg(feature = "config-store")]
const EM_SETLIMITTEXT_MESSAGE: u32 = 0x00C5;

const CLASS_NAME: windows_sys::core::PCWSTR = windows_sys::core::w!("LiteFences.P0.Window");

#[derive(Default)]
struct WindowState {
    #[cfg(feature = "ui-acceptance")]
    diagnostics: Diagnostics,
    #[cfg(feature = "ui-acceptance")]
    initial_visibility: Cell<Option<(bool, bool)>>,
    #[cfg(feature = "ui-acceptance")]
    closing_visibility: Cell<Option<(bool, bool)>>,
    #[cfg(feature = "ui-acceptance")]
    diagnostic_timeout_fired: Cell<bool>,
    #[cfg(feature = "ui-acceptance")]
    undo_fixture_target: Cell<Option<(GroupId, ItemId)>>,
    #[cfg(feature = "ui-acceptance")]
    undo_fixture_history_ready: Cell<bool>,
    #[cfg(feature = "ui-acceptance")]
    undo_fixture_restored: Cell<bool>,
    paints: Cell<u32>,
    resizes: Cell<u32>,
    destroyed: Cell<bool>,
    #[cfg(feature = "config-store")]
    session: RefCell<Option<Session>>,
    #[cfg(feature = "config-store")]
    icons: RefCell<Option<IconWorker>>,
    #[cfg(feature = "config-store")]
    open_allowed: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_save_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_group_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_rename_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_move_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_resize_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_delete_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    armed_delete: Cell<Option<GroupId>>,
    #[cfg(feature = "config-store")]
    selected_item: Cell<Option<(GroupId, ItemId)>>,
    #[cfg(feature = "config-store")]
    armed_remove: Cell<Option<(GroupId, ItemId)>>,
    #[cfg(feature = "config-store")]
    restore_remove_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_undo_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_sort_earlier_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_sort_later_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_reveal_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_transfer_combo_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_transfer_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    transfer_target: Cell<Option<GroupId>>,
    #[cfg(feature = "config-store")]
    rendered_transfer_titles: RefCell<Vec<String>>,
    #[cfg(feature = "config-store")]
    restore_color_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_fold_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_add_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    restore_folder_focus: Cell<bool>,
    #[cfg(feature = "config-store")]
    picker_override: RefCell<Option<Vec<NativePath>>>,
    #[cfg(feature = "config-store")]
    folder_override: RefCell<Option<NativePath>>,
    #[cfg(feature = "config-store")]
    rendered_group_titles: RefCell<Vec<String>>,
    #[cfg(feature = "config-store")]
    rendered_selection_rect: Cell<Option<(GroupId, RectDip)>>,
    #[cfg(feature = "config-store")]
    rendered_selection_color: Cell<Option<(GroupId, [u8; 3])>>,
    #[cfg(feature = "config-store")]
    rendered_selection_fold: Cell<Option<(GroupId, bool)>>,
    #[cfg(feature = "config-store")]
    drag: Cell<Option<DragState>>,
    #[cfg(feature = "config-store")]
    item_drag: Cell<Option<item_drag::ItemDrag>>,
    #[cfg(feature = "config-store")]
    item_drag_hover: Cell<Option<GroupId>>,
    #[cfg(feature = "config-store")]
    drop_hover: Cell<Option<GroupId>>,
    #[cfg(feature = "config-store")]
    drop_registered: Cell<bool>,
    #[cfg(feature = "config-store")]
    drop_unavailable: Cell<bool>,
}

#[cfg(feature = "config-store")]
#[derive(Clone, Copy)]
struct DragState {
    id: GroupId,
    kind: DragKind,
    start: POINT,
    last: POINT,
    original: RectDip,
    layout: PreviewLayout,
}

#[cfg(feature = "config-store")]
#[derive(Clone, Copy, Eq, PartialEq)]
enum DragKind {
    Move,
    Resize,
}

struct Window(HWND);
impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: UI-thread-owned HWND; WM_NCDESTROY has cleared user data on normal exit.
        unsafe {
            if IsWindow(self.0) != 0 {
                DestroyWindow(self.0);
            }
        }
    }
}

struct WindowClass(HINSTANCE);
impl Drop for WindowClass {
    fn drop(&mut self) {
        // SAFETY: the window guard is destroyed before this class guard.
        unsafe {
            UnregisterClassW(CLASS_NAME, self.0);
        }
    }
}

struct GdiObject(HGDIOBJ);
impl Drop for GdiObject {
    fn drop(&mut self) {
        // SAFETY: only owned brushes/fonts enter this wrapper; selection is restored first.
        unsafe {
            DeleteObject(self.0);
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn px(value: i32, dpi: u32) -> i32 {
    ((i64::from(value) * i64::from(dpi) + 48) / 96) as i32
}
fn color(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

#[cfg(feature = "config-store")]
unsafe fn choose_files(hwnd: HWND) -> Result<Option<Vec<NativePath>>, String> {
    // The dialog is invoked only after a writable group is selected. Its owned
    // buffer remains alive across the modal call; no Session borrow is held.
    unsafe {
        let mut buffer = vec![0_u16; 65_536];
        let filter: Vec<u16> = "所有文件\0*.*\0\0".encode_utf16().collect();
        let title = wide("选择要添加为引用的文件（源文件不会移动）");
        let mut options = OPENFILENAMEW {
            lStructSize: size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: hwnd,
            lpstrFilter: filter.as_ptr(),
            lpstrFile: buffer.as_mut_ptr(),
            nMaxFile: buffer.len() as u32,
            lpstrTitle: title.as_ptr(),
            Flags: OFN_EXPLORER
                | OFN_ALLOWMULTISELECT
                | OFN_FILEMUSTEXIST
                | OFN_NODEREFERENCELINKS
                | OFN_DONTADDTORECENT,
            ..Default::default()
        };
        if GetOpenFileNameW(&mut options) == 0 {
            let error = CommDlgExtendedError();
            return if error == 0 {
                Ok(None)
            } else if error == FNERR_BUFFERTOOSMALL {
                Err("选择的文件过多或路径过长，请分批添加。".into())
            } else {
                Err(format!("文件选择器出错（代码 {error}）。"))
            };
        }
        picker_paths::parse_picker_paths(&buffer)
            .map(Some)
            .map_err(|error| format!("所选路径不符合本地引用范围：{error:?}。"))
    }
}

#[cfg(feature = "config-store")]
fn reference_kind(path: &NativePath) -> ReferenceKind {
    let units = path.as_utf16();
    if units.len() >= 4
        && units[units.len() - 4..]
            .iter()
            .copied()
            .map(|unit| match unit {
                97..=122 => unit - 32,
                _ => unit,
            })
            .eq([46, 76, 78, 75])
    {
        ReferenceKind::Shortcut
    } else {
        ReferenceKind::File
    }
}

#[cfg(all(test, feature = "config-store"))]
mod picker_kind_tests {
    use super::*;

    #[test]
    fn shortcut_extension_is_retained_case_insensitively() {
        for value in [r"C:\links\name.lnk", r"C:\links\NAME.LNK"] {
            let path = NativePath::from_utf16(value.encode_utf16().collect()).unwrap();
            assert_eq!(reference_kind(&path), ReferenceKind::Shortcut);
        }
        let path = NativePath::from_utf16(r"C:\links\name.txt".encode_utf16().collect()).unwrap();
        assert_eq!(reference_kind(&path), ReferenceKind::File);
    }
}

#[cfg(feature = "ui-acceptance")]
#[allow(dead_code)] // Fields are read by the dedicated diagnostic binary.
pub struct LoopReport {
    pub counters: LoopSnapshot,
    pub initial_visibility: Option<(bool, bool)>,
    pub closing_visibility: Option<(bool, bool)>,
    pub timeout_fired: bool,
    pub message_histogram: Vec<(u32, u64)>,
    pub high_message_ids: u64,
    pub undo_fixture_seeded: bool,
    pub undo_fixture_restored: bool,
}
#[cfg(feature = "ui-acceptance")]
type RunReport = LoopReport;
#[cfg(not(feature = "ui-acceptance"))]
type RunReport = ();

pub fn run(options: Options) -> io::Result<()> {
    run_impl(options, None).map(|_| ())
}

/// Test-only timed run: requires an explicit data root, never the user directory.
#[cfg(feature = "ui-acceptance")]
#[allow(dead_code)] // Only the dedicated diagnostic binary calls this entry point.
pub fn run_diagnostic(options: Options, seconds: u32, hidden: bool) -> io::Result<LoopReport> {
    if !(1..=60).contains(&seconds)
        || options.data_dir.is_none()
        || options.smoke
        || options.storage_smoke.is_some()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid diagnostic run",
        ));
    }
    run_impl(options, Some((seconds, hidden, UndoFixtureMode::None)))
}

#[cfg(feature = "ui-acceptance")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum UndoFixtureMode {
    None,
    #[cfg(feature = "ui-acceptance")]
    Reference,
    #[cfg(feature = "ui-acceptance")]
    Group,
}
#[cfg(not(feature = "ui-acceptance"))]
type UndoFixtureMode = ();

/// Seeds a removal history in a new project-local fixture, never in user data.
/// Restoration must be performed by real keyboard/button input, not this helper.
#[cfg(all(test, feature = "ui-acceptance"))]
fn undo_fixture_name_matches(name: &str, process_id: u32) -> bool {
    undo_fixture_name_matches_mode(name, process_id, UndoFixtureMode::Reference)
}

#[cfg(feature = "ui-acceptance")]
fn undo_fixture_name_matches_mode(name: &str, process_id: u32, mode: UndoFixtureMode) -> bool {
    let prefix = match mode {
        UndoFixtureMode::Reference => "undo-visible-",
        UndoFixtureMode::Group => "undo-group-visible-",
        UndoFixtureMode::None => return false,
    };
    name.strip_prefix(prefix)
        .and_then(|suffix| suffix.split_once('-'))
        .is_some_and(|(pid, stamp)| {
            pid == process_id.to_string()
                && !stamp.is_empty()
                && stamp.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[cfg(feature = "ui-acceptance")]
fn undo_fixture_observation(
    history_was_ready: bool,
    can_undo: bool,
    can_save: bool,
    reference_present: bool,
) -> (bool, bool) {
    let history_ready = history_was_ready || can_undo;
    let restored = history_ready && can_save && !can_undo && reference_present;
    (history_ready, restored)
}

#[cfg(feature = "ui-acceptance")]
#[allow(dead_code)]
pub fn run_undo_acceptance(options: Options, seconds: u32) -> io::Result<LoopReport> {
    run_undo_fixture(options, seconds, UndoFixtureMode::Reference)
}

#[cfg(feature = "ui-acceptance")]
#[allow(dead_code)]
pub fn run_undo_group_acceptance(options: Options, seconds: u32) -> io::Result<LoopReport> {
    run_undo_fixture(options, seconds, UndoFixtureMode::Group)
}

#[cfg(feature = "ui-acceptance")]
fn run_undo_fixture(
    options: Options,
    seconds: u32,
    mode: UndoFixtureMode,
) -> io::Result<LoopReport> {
    let artifacts =
        std::fs::canonicalize(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("artifacts"))?;
    let root = options
        .data_dir
        .as_ref()
        .ok_or_else(|| io::Error::other("missing undo fixture"))?;
    let canonical_root = std::fs::canonicalize(root)?;
    let valid_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| undo_fixture_name_matches_mode(name, std::process::id(), mode));
    if !(1..=60).contains(&seconds)
        || options.smoke
        || options.storage_smoke.is_some()
        || canonical_root.parent() != Some(artifacts.as_path())
        || !valid_name
    {
        return Err(io::Error::other("invalid undo fixture"));
    }
    run_impl(options, Some((seconds, false, mode)))
}

#[cfg(all(test, feature = "ui-acceptance"))]
mod undo_fixture_tests {
    use super::*;

    #[test]
    fn failed_removal_or_unconfirmed_restoration_cannot_pass() {
        // Failure rollback leaves the original item present, but no history.
        assert_eq!(
            undo_fixture_observation(false, false, true, true),
            (false, false)
        );
        // A durably removed item creates the actual undo record.
        assert_eq!(
            undo_fixture_observation(false, true, true, false),
            (true, false)
        );
        // In-flight save has not durably confirmed restoration.
        assert_eq!(
            undo_fixture_observation(true, false, false, true),
            (true, false)
        );
        // Failed restoration retains history and leaves the item absent.
        assert_eq!(
            undo_fixture_observation(true, true, true, false),
            (true, false)
        );
        // Merely adding the target while an undo record remains is not undo.
        assert_eq!(
            undo_fixture_observation(true, true, true, true),
            (true, false)
        );
        // Only committed restoration with a consumed record can pass.
        assert_eq!(
            undo_fixture_observation(true, false, true, true),
            (true, true)
        );
    }

    #[test]
    fn fixture_name_is_current_process_only_and_cannot_contain_path_components() {
        assert!(undo_fixture_name_matches(
            "undo-visible-123-1791191455192865000",
            123
        ));
        for name in [
            "undo-visible-124-1",
            "undo-visible-0123-1",
            "undo-visible-123-",
            "undo-visible-123-1-0",
            "undo-visible-123-../config",
            "undo-visible-123-1\\config",
            "ui-sky-123-1",
            "undo-visible-123-１２３",
        ] {
            assert!(!undo_fixture_name_matches(name, 123), "{name}");
        }
    }

    #[test]
    fn group_fixture_name_cannot_be_used_as_a_reference_fixture_or_foreign_pid() {
        assert!(undo_fixture_name_matches_mode(
            "undo-group-visible-123-1",
            123,
            UndoFixtureMode::Group
        ));
        assert!(!undo_fixture_name_matches_mode(
            "undo-group-visible-123-1",
            123,
            UndoFixtureMode::Reference
        ));
        assert!(!undo_fixture_name_matches_mode(
            "undo-visible-123-1",
            123,
            UndoFixtureMode::Group
        ));
        for name in [
            "undo-group-visible-124-1",
            "undo-group-visible-123-",
            "undo-group-visible-123-1-0",
            "undo-group-visible-123-1/other",
        ] {
            assert!(!undo_fixture_name_matches_mode(
                name,
                123,
                UndoFixtureMode::Group
            ));
        }
        assert!(!undo_fixture_name_matches_mode(
            "undo-group-visible-123-1",
            123,
            UndoFixtureMode::None
        ));
    }

    #[test]
    fn ordinary_project_directory_is_rejected_before_native_window_calls() {
        assert!(
            run_undo_acceptance(
                Options {
                    smoke: false,
                    data_dir: Some(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
                    storage_smoke: None,
                },
                1
            )
            .is_err()
        );
    }
}

fn run_impl(
    options: Options,
    diagnostic: Option<(u32, bool, UndoFixtureMode)>,
) -> io::Result<RunReport> {
    let hidden = options.hidden() || diagnostic.is_some_and(|(_, hidden, _)| hidden);
    let smoke_test = options.smoke;
    // SAFETY: DPI context is established before any HWND is created in this process.
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == 0 {
            return Err(io::Error::last_os_error());
        }
        let instance = GetModuleHandleW(null());
        if instance.is_null() {
            return Err(io::Error::last_os_error());
        }
        let cursor = LoadCursorW(null_mut(), IDC_ARROW);
        if cursor.is_null() {
            return Err(io::Error::last_os_error());
        }
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: cursor,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(io::Error::last_os_error());
        }
        let _class_guard = WindowClass(instance);
        // Stable heap address, alive until after Window is dropped; callbacks borrow it only.
        let state = Box::<WindowState>::default();
        #[cfg(feature = "config-store")]
        state.open_allowed.set(!hidden);
        #[cfg(feature = "ui-acceptance")]
        if diagnostic.is_some() {
            state.diagnostics.enable();
        }
        let dpi = GetDpiForSystem();
        let hwnd = CreateWindowExW(
            0,
            CLASS_NAME,
            windows_sys::core::w!("LiteFences · 轻量桌面分组"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            px(1120, dpi),
            px(930, dpi),
            null_mut(),
            null_mut(),
            instance,
            (&*state as *const WindowState).cast(),
        );
        if hwnd.is_null() {
            return Err(io::Error::last_os_error());
        }
        let _window_guard = Window(hwnd);
        #[cfg(feature = "config-store")]
        let _drop_registration = if !smoke_test {
            match external_drop::Registration::new(hwnd, &state) {
                Ok(registration) => {
                    state.drop_registered.set(true);
                    Some(registration)
                }
                Err(_) => {
                    state.drop_unavailable.set(true);
                    None
                }
            }
        } else {
            None
        };
        #[cfg(feature = "config-store")]
        if !smoke_test {
            let root = options
                .data_dir
                .clone()
                .map_or(DataRoot::User, DataRoot::Existing);
            let session = Session::start(root, options.storage_smoke)?;
            *state.icons.borrow_mut() = IconWorker::start(session.wake.clone()).ok();
            *state.session.borrow_mut() = Some(session);
            let button = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                windows_sys::core::w!("保存配置 (Ctrl+S)"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                px(48, dpi),
                px(310, dpi),
                px(220, dpi),
                px(34, dpi),
                hwnd,
                SAVE_BUTTON as HMENU,
                instance,
                null(),
            );
            if button.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(button, 0);
            let create = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                windows_sys::core::w!("新建分组 (Ctrl+N)"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                px(285, dpi),
                px(310, dpi),
                px(220, dpi),
                px(34, dpi),
                hwnd,
                CREATE_GROUP_BUTTON as HMENU,
                instance,
                null(),
            );
            if create.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(create, 0);
            let undo = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                windows_sys::core::w!("撤销最近移除/删除 (Ctrl+Z)"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                px(550, dpi),
                px(290, dpi),
                px(285, dpi),
                px(34, dpi),
                hwnd,
                UNDO_REMOVAL_BUTTON as HMENU,
                instance,
                null(),
            );
            if undo.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(undo, 0);
            for (id, label, x) in [
                (SORT_EARLIER_BUTTON, "引用前移", 550),
                (SORT_LATER_BUTTON, "引用后移", 695),
            ] {
                let sort = CreateWindowExW(
                    0,
                    windows_sys::core::w!("BUTTON"),
                    wide(label).as_ptr(),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                    px(x, dpi),
                    px(330, dpi),
                    px(140, dpi),
                    px(34, dpi),
                    hwnd,
                    id as HMENU,
                    instance,
                    null(),
                );
                if sort.is_null() {
                    return Err(io::Error::last_os_error());
                }
                EnableWindow(sort, 0);
            }
            let reveal = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                windows_sys::core::w!("打开所在位置（选中引用本身）"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                px(550, dpi),
                px(370, dpi),
                px(285, dpi),
                px(34, dpi),
                hwnd,
                REVEAL_ITEM_BUTTON as HMENU,
                instance,
                null(),
            );
            if reveal.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(reveal, 0);
            let transfer_combo = CreateWindowExW(
                0,
                windows_sys::core::w!("COMBOBOX"),
                null(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
                px(550, dpi),
                px(410, dpi),
                px(140, dpi),
                px(200, dpi),
                hwnd,
                TRANSFER_GROUP_COMBO as HMENU,
                instance,
                null(),
            );
            if transfer_combo.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(transfer_combo, 0);
            let transfer_button = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                windows_sys::core::w!("移至目标分组"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                px(695, dpi),
                px(410, dpi),
                px(140, dpi),
                px(34, dpi),
                hwnd,
                TRANSFER_ITEM_BUTTON as HMENU,
                instance,
                null(),
            );
            if transfer_button.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(transfer_button, 0);
            let group_label = CreateWindowExW(
                0,
                windows_sys::core::w!("STATIC"),
                windows_sys::core::w!("选择分组"),
                WS_CHILD | WS_VISIBLE,
                px(48, dpi),
                px(350, dpi),
                px(220, dpi),
                px(22, dpi),
                hwnd,
                GROUP_LIST_LABEL as HMENU,
                instance,
                null(),
            );
            if group_label.is_null() {
                return Err(io::Error::last_os_error());
            }
            let name_label = CreateWindowExW(
                0,
                windows_sys::core::w!("STATIC"),
                windows_sys::core::w!("新名称"),
                WS_CHILD | WS_VISIBLE,
                px(285, dpi),
                px(350, dpi),
                px(220, dpi),
                px(22, dpi),
                hwnd,
                GROUP_NAME_LABEL as HMENU,
                instance,
                null(),
            );
            if name_label.is_null() {
                return Err(io::Error::last_os_error());
            }
            let list = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                windows_sys::core::w!("LISTBOX"),
                null(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_VSCROLL | LBS_NOTIFY as u32,
                px(48, dpi),
                px(375, dpi),
                px(220, dpi),
                px(140, dpi),
                hwnd,
                GROUP_LIST as HMENU,
                instance,
                null(),
            );
            if list.is_null() {
                return Err(io::Error::last_os_error());
            }
            let name = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                windows_sys::core::w!("EDIT"),
                null(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
                px(285, dpi),
                px(375, dpi),
                px(230, dpi),
                px(30, dpi),
                hwnd,
                GROUP_NAME as HMENU,
                instance,
                null(),
            );
            if name.is_null() {
                return Err(io::Error::last_os_error());
            }
            SendMessageW(name, EM_SETLIMITTEXT_MESSAGE, 256, 0);
            let rename = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                windows_sys::core::w!("重命名所选分组"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                px(285, dpi),
                px(420, dpi),
                px(230, dpi),
                px(34, dpi),
                hwnd,
                RENAME_GROUP_BUTTON as HMENU,
                instance,
                null(),
            );
            if rename.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(rename, 0);
            for (id, caption, x) in [
                (GROUP_X_LABEL, "X (DIP)", 285),
                (GROUP_Y_LABEL, "Y (DIP)", 405),
            ] {
                if CreateWindowExW(
                    0,
                    windows_sys::core::w!("STATIC"),
                    wide(caption).as_ptr(),
                    WS_CHILD | WS_VISIBLE,
                    px(x, dpi),
                    px(460, dpi),
                    px(110, dpi),
                    px(22, dpi),
                    hwnd,
                    id as HMENU,
                    instance,
                    null(),
                )
                .is_null()
                {
                    return Err(io::Error::last_os_error());
                }
            }
            for (id, x) in [(GROUP_X, 285), (GROUP_Y, 405)] {
                let edit = CreateWindowExW(
                    WS_EX_CLIENTEDGE,
                    windows_sys::core::w!("EDIT"),
                    null(),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
                    px(x, dpi),
                    px(485, dpi),
                    px(110, dpi),
                    px(30, dpi),
                    hwnd,
                    id as HMENU,
                    instance,
                    null(),
                );
                if edit.is_null() {
                    return Err(io::Error::last_os_error());
                }
                SendMessageW(edit, EM_SETLIMITTEXT_MESSAGE, 12, 0);
            }
            let move_button = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                windows_sys::core::w!("应用位置"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                px(285, dpi),
                px(530, dpi),
                px(230, dpi),
                px(34, dpi),
                hwnd,
                MOVE_GROUP_BUTTON as HMENU,
                instance,
                null(),
            );
            if move_button.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(move_button, 0);
            for (id, caption, x) in [
                (GROUP_WIDTH_LABEL, "宽度 (DIP)", 285),
                (GROUP_HEIGHT_LABEL, "高度 (DIP)", 405),
            ] {
                if CreateWindowExW(
                    0,
                    windows_sys::core::w!("STATIC"),
                    wide(caption).as_ptr(),
                    WS_CHILD | WS_VISIBLE,
                    px(x, dpi),
                    px(575, dpi),
                    px(110, dpi),
                    px(22, dpi),
                    hwnd,
                    id as HMENU,
                    instance,
                    null(),
                )
                .is_null()
                {
                    return Err(io::Error::last_os_error());
                }
            }
            for (id, x) in [(GROUP_WIDTH, 285), (GROUP_HEIGHT, 405)] {
                let edit = CreateWindowExW(
                    WS_EX_CLIENTEDGE,
                    windows_sys::core::w!("EDIT"),
                    null(),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
                    px(x, dpi),
                    px(600, dpi),
                    px(110, dpi),
                    px(30, dpi),
                    hwnd,
                    id as HMENU,
                    instance,
                    null(),
                );
                if edit.is_null() {
                    return Err(io::Error::last_os_error());
                }
                SendMessageW(edit, EM_SETLIMITTEXT_MESSAGE, 10, 0);
            }
            let resize_button = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                windows_sys::core::w!("应用尺寸"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                px(285, dpi),
                px(645, dpi),
                px(230, dpi),
                px(34, dpi),
                hwnd,
                RESIZE_GROUP_BUTTON as HMENU,
                instance,
                null(),
            );
            if resize_button.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(resize_button, 0);
            let delete_button = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                windows_sys::core::w!("删除所选分组（仅配置）"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                px(48, dpi),
                px(535, dpi),
                px(220, dpi),
                px(34, dpi),
                hwnd,
                DELETE_GROUP_BUTTON as HMENU,
                instance,
                null(),
            );
            if delete_button.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(delete_button, 0);
            let color_label = CreateWindowExW(
                0,
                windows_sys::core::w!("STATIC"),
                windows_sys::core::w!("分组颜色 #RRGGBB"),
                WS_CHILD | WS_VISIBLE,
                px(48, dpi),
                px(590, dpi),
                px(220, dpi),
                px(22, dpi),
                hwnd,
                GROUP_COLOR_LABEL as HMENU,
                instance,
                null(),
            );
            if color_label.is_null() {
                return Err(io::Error::last_os_error());
            }
            let color_edit = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                windows_sys::core::w!("EDIT"),
                null(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
                px(48, dpi),
                px(615, dpi),
                px(220, dpi),
                px(30, dpi),
                hwnd,
                GROUP_COLOR as HMENU,
                instance,
                null(),
            );
            if color_edit.is_null() {
                return Err(io::Error::last_os_error());
            }
            SendMessageW(color_edit, EM_SETLIMITTEXT_MESSAGE, 7, 0);
            let color_button = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                windows_sys::core::w!("应用颜色"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                px(48, dpi),
                px(660, dpi),
                px(220, dpi),
                px(34, dpi),
                hwnd,
                COLOR_GROUP_BUTTON as HMENU,
                instance,
                null(),
            );
            if color_button.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(color_button, 0);
            let fold_button = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                windows_sys::core::w!("折叠所选分组"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                px(48, dpi),
                px(710, dpi),
                px(220, dpi),
                px(34, dpi),
                hwnd,
                FOLD_GROUP_BUTTON as HMENU,
                instance,
                null(),
            );
            if fold_button.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(fold_button, 0);
            let remove_item = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                windows_sys::core::w!("移除选中引用（仅配置）"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                px(285, dpi),
                px(710, dpi),
                px(230, dpi),
                px(34, dpi),
                hwnd,
                REMOVE_ITEM_BUTTON as HMENU,
                instance,
                null(),
            );
            if remove_item.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(remove_item, 0);
            let add_files = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                windows_sys::core::w!("添加文件引用…"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                px(48, dpi),
                px(765, dpi),
                px(220, dpi),
                px(34, dpi),
                hwnd,
                ADD_FILES_BUTTON as HMENU,
                instance,
                null(),
            );
            if add_files.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(add_files, 0);
            let add_folder = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                windows_sys::core::w!("添加目录引用…"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                px(285, dpi),
                px(765, dpi),
                px(230, dpi),
                px(34, dpi),
                hwnd,
                ADD_FOLDER_BUTTON as HMENU,
                instance,
                null(),
            );
            if add_folder.is_null() {
                return Err(io::Error::last_os_error());
            }
            EnableWindow(add_folder, 0);
            let drop_report = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                windows_sys::core::w!("EDIT"),
                windows_sys::core::w!("拖入结果会逐项显示在这里。"),
                WS_CHILD | WS_VISIBLE | WS_VSCROLL | ES_MULTILINE as u32 | ES_READONLY as u32,
                px(48, dpi),
                px(805, dpi),
                px(467, dpi),
                px(38, dpi),
                hwnd,
                DROP_REPORT as HMENU,
                instance,
                null(),
            );
            if drop_report.is_null() {
                return Err(io::Error::last_os_error());
            }
            SendMessageW(drop_report, EM_SETLIMITTEXT_MESSAGE, 65_535, 0);
            if options.storage_smoke.is_some() && SetTimer(hwnd, 1, 10_000, None) == 0 {
                return Err(io::Error::last_os_error());
            }
            if options.storage_smoke.is_none() && !hidden && SetTimer(hwnd, 2, 30_000, None) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        #[cfg(feature = "ui-acceptance")]
        if let Some((seconds, _, _)) = diagnostic
            && SetTimer(hwnd, 3, seconds * 1000, None) == 0
        {
            return Err(io::Error::last_os_error());
        }
        if smoke_test {
            // Exercise real HWND creation, resize, WM_PAINT and clean message-loop shutdown,
            // with no visible window or simulated keyboard/global desktop actions.
            if SetWindowPos(
                hwnd,
                null_mut(),
                0,
                0,
                px(900, dpi),
                px(560, dpi),
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let mut rect = RECT::default();
            if GetClientRect(hwnd, &mut rect) == 0 || rect.right <= 0 || rect.bottom <= 0 {
                return Err(io::Error::other("Invalid client rectangle"));
            }
            InvalidateRect(hwnd, null(), 0);
            SendMessageW(hwnd, WM_PAINT, 0, 0);
            if PostMessageW(hwnd, WM_CLOSE, 0, 0) == 0 {
                return Err(io::Error::last_os_error());
            }
        } else if !hidden {
            ShowWindow(hwnd, SW_SHOWNORMAL);
            UpdateWindow(hwnd);
        }
        #[cfg(feature = "ui-acceptance")]
        if diagnostic.is_some() {
            state
                .initial_visibility
                .set(Some((IsWindowVisible(hwnd) != 0, IsIconic(hwnd) != 0)));
        }
        let mut message = MSG::default();
        #[cfg(feature = "config-store")]
        let mut ui_rename_started = false;
        #[cfg(feature = "config-store")]
        let mut ui_move_started = false;
        #[cfg(feature = "config-store")]
        let mut ui_drag_started = false;
        #[cfg(feature = "config-store")]
        let mut ui_resize_started = false;
        #[cfg(feature = "config-store")]
        let mut ui_resize_drag_started = false;
        #[cfg(feature = "config-store")]
        let mut ui_delete_started = false;
        #[cfg(feature = "config-store")]
        let mut ui_color_started = false;
        #[cfg(feature = "config-store")]
        let mut ui_fold_started = false;
        #[cfg(feature = "config-store")]
        let mut add_started = false;
        #[cfg(feature = "config-store")]
        let mut ui_add_started = false;
        #[cfg(feature = "config-store")]
        let mut ui_folder_started = false;
        #[cfg(feature = "config-store")]
        let mut ui_drop_started = false;
        #[cfg(feature = "config-store")]
        let mut ui_remove_started = false;
        #[cfg(feature = "config-store")]
        let mut ui_transfer_started = false;
        #[cfg(feature = "config-store")]
        let mut ui_undo_started = false;
        'messages: loop {
            #[cfg(feature = "config-store")]
            let wake = state.session.borrow().as_ref().map(|s| s.wake.clone());
            #[cfg(feature = "config-store")]
            if let Some(wake) = wake {
                let handle = wake.handle();
                let result = MsgWaitForMultipleObjectsEx(
                    1,
                    &handle,
                    u32::MAX,
                    QS_ALLINPUT,
                    MWMO_INPUTAVAILABLE,
                );
                if result == WAIT_FAILED {
                    return Err(io::Error::last_os_error());
                }
                #[cfg(feature = "ui-acceptance")]
                state.diagnostics.record(match result {
                    WAIT_OBJECT_0 => LoopEvent::WaitWorker,
                    value if value == WAIT_OBJECT_0 + 1 => LoopEvent::WaitInput,
                    _ => LoopEvent::WaitOther,
                });
                // Drain on every wake, including input; bounded queue, never waits for I/O.
                // Release RefCell before any native calls that could re-enter window_proc.
                let changed = state.session.borrow_mut().as_mut().unwrap().drain();
                let icons_changed = state
                    .icons
                    .borrow_mut()
                    .as_mut()
                    .is_some_and(IconWorker::drain);
                #[cfg(feature = "ui-acceptance")]
                if diagnostic.is_some_and(|(_, _, mode)| mode != UndoFixtureMode::None)
                    && state.undo_fixture_target.get().is_none()
                    && state
                        .session
                        .borrow()
                        .as_ref()
                        .is_some_and(Session::can_save)
                {
                    let mut borrowed = state.session.borrow_mut();
                    let session = borrowed.as_mut().unwrap();
                    let groups = session.group_previews();
                    if groups.len() != 1 {
                        return Err(io::Error::other("undo fixture requires one group"));
                    }
                    let (count, items) = session.preview_items(groups[0].id, 1);
                    if count != 1 || items.len() != 1 {
                        return Err(io::Error::other("undo fixture requires one reference"));
                    }
                    let target = (groups[0].id, items[0].id);
                    match diagnostic.unwrap().2 {
                        UndoFixtureMode::Reference => {
                            if !session.remove_reference(target.0, target.1) {
                                return Err(io::Error::other("cannot seed reference undo history"));
                            }
                        }
                        UndoFixtureMode::Group => {
                            session.delete_group(target.0);
                            if session.contains_reference_id(target.0, target.1) {
                                return Err(io::Error::other("cannot seed group undo history"));
                            }
                        }
                        UndoFixtureMode::None => unreachable!("seed mode was checked"),
                    }
                    state.undo_fixture_target.set(Some(target));
                    drop(borrowed);
                    let title = if diagnostic.unwrap().2 == UndoFixtureMode::Group {
                        "LiteFences · 分组撤销验收（仅测试配置）"
                    } else {
                        "LiteFences · 撤销验收（仅测试引用）"
                    };
                    SetWindowTextW(hwnd, wide(title).as_ptr());
                    refresh_session(hwnd, &state);
                }
                #[cfg(feature = "ui-acceptance")]
                if let Some((group, item)) = state.undo_fixture_target.get() {
                    let borrowed = state.session.borrow();
                    let session = borrowed.as_ref().unwrap();
                    let (ready, restored) = undo_fixture_observation(
                        state.undo_fixture_history_ready.get(),
                        session.can_undo_removal(),
                        session.can_save(),
                        session.contains_reference_id(group, item),
                    );
                    state.undo_fixture_history_ready.set(ready);
                    if restored {
                        state.undo_fixture_restored.set(true);
                    }
                }
                let open_outcome = state
                    .icons
                    .borrow_mut()
                    .as_mut()
                    .and_then(IconWorker::take_open_outcome);
                if let Some((action, outcome)) = open_outcome {
                    if let Some(session) = state.session.borrow_mut().as_mut() {
                        session.status = match (action, outcome) {
                            (open_policy::OpenAction::Open, Ok(())) => {
                                "打开请求已交给系统处理；引用配置和源文件未改动。".into()
                            }
                            (open_policy::OpenAction::Reveal, Ok(())) => {
                                "定位请求已交给 Explorer；引用本身将被选中，配置和源文件未改动。"
                                    .into()
                            }
                            (open_policy::OpenAction::Open, Err(failure)) => format!(
                                "无法打开：{} 引用仍保留。",
                                open_policy::failure_hint(failure)
                            ),
                            (open_policy::OpenAction::Reveal, Err(failure)) => format!(
                                "无法定位：{} 引用仍保留。",
                                open_policy::failure_hint(failure)
                            ),
                        };
                    }
                    refresh_session(hwnd, &state);
                }
                if icons_changed {
                    #[cfg(feature = "ui-acceptance")]
                    state.diagnostics.record(LoopEvent::IconsChanged);
                    InvalidateRect(hwnd, null(), 0);
                }
                if changed {
                    #[cfg(feature = "ui-acceptance")]
                    state.diagnostics.record(LoopEvent::SessionChanged);
                    refresh_session(hwnd, &state);
                    if options.storage_smoke == Some(crate::app_options::StorageSmoke::UiRename)
                        && !ui_rename_started
                    {
                        if !state
                            .session
                            .borrow()
                            .as_ref()
                            .is_some_and(Session::can_save)
                        {
                            return Err(io::Error::other("UI rename fixture is not writable"));
                        }
                        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
                        let edit = GetDlgItem(hwnd, GROUP_NAME as i32);
                        if list.is_null()
                            || edit.is_null()
                            || SendMessageW(list, LB_GETCOUNT, 0, 0) != 2
                            || SendMessageW(list, LB_SETCURSEL, 1, 0) != 1
                        {
                            return Err(io::Error::other("UI rename fixture needs two groups"));
                        }
                        SendMessageW(
                            hwnd,
                            WM_COMMAND,
                            GROUP_LIST | ((LBN_SELCHANGE as usize) << 16),
                            list as isize,
                        );
                        let expected = state
                            .session
                            .borrow()
                            .as_ref()
                            .and_then(|session| session.group_title_at(1))
                            .ok_or_else(|| io::Error::other("UI rename fixture title missing"))?;
                        let mut units = vec![0_u16; expected.encode_utf16().count() + 2];
                        let copied = GetWindowTextW(edit, units.as_mut_ptr(), units.len() as i32);
                        if GetWindowTextLengthW(edit) as usize != expected.encode_utf16().count()
                            || String::from_utf16(&units[..copied as usize])
                                .ok()
                                .as_deref()
                                != Some(expected.as_str())
                        {
                            return Err(io::Error::other("UI rename input contains a list prefix"));
                        }
                        SetWindowTextW(edit, wide("第二组已改名").as_ptr());
                        SendMessageW(hwnd, WM_COMMAND, RENAME_GROUP_BUTTON, 0);
                        state.session.borrow_mut().as_mut().unwrap().close();
                        ui_rename_started = true;
                    }
                    if options.storage_smoke == Some(crate::app_options::StorageSmoke::UiMove)
                        && !ui_move_started
                    {
                        if !state
                            .session
                            .borrow()
                            .as_ref()
                            .is_some_and(Session::can_save)
                        {
                            return Err(io::Error::other("UI move fixture is not writable"));
                        }
                        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
                        let x = GetDlgItem(hwnd, GROUP_X as i32);
                        let y = GetDlgItem(hwnd, GROUP_Y as i32);
                        if list.is_null()
                            || x.is_null()
                            || y.is_null()
                            || SendMessageW(list, LB_GETCOUNT, 0, 0) != 2
                            || SendMessageW(list, LB_SETCURSEL, 1, 0) != 1
                        {
                            return Err(io::Error::other("UI move fixture needs two groups"));
                        }
                        SendMessageW(
                            hwnd,
                            WM_COMMAND,
                            GROUP_LIST | ((LBN_SELCHANGE as usize) << 16),
                            list as isize,
                        );
                        SetWindowTextW(x, wide("-240").as_ptr());
                        SetWindowTextW(y, wide("96").as_ptr());
                        SendMessageW(hwnd, WM_COMMAND, MOVE_GROUP_BUTTON, 0);
                        state.session.borrow_mut().as_mut().unwrap().close();
                        ui_move_started = true;
                    }
                    if options.storage_smoke == Some(crate::app_options::StorageSmoke::UiDrag)
                        && !ui_drag_started
                    {
                        if !state
                            .session
                            .borrow()
                            .as_ref()
                            .is_some_and(Session::can_save)
                        {
                            return Err(io::Error::other("UI drag fixture is not writable"));
                        }
                        let groups = state.session.borrow().as_ref().unwrap().group_previews();
                        if groups.len() != 2 {
                            return Err(io::Error::other("UI drag fixture needs two groups"));
                        }
                        let layout =
                            preview_layout(hwnd, GetDpiForWindow(hwnd).max(96), &groups)
                                .ok_or_else(|| io::Error::other("UI drag preview unavailable"))?;
                        let box_rect = layout.group(groups[1].rect);
                        let start = POINT {
                            x: box_rect.left + 12,
                            y: box_rect.top + 12,
                        };
                        let (dx, dy) = layout.pixel_delta(64, 32);
                        let end = POINT {
                            x: start.x + dx,
                            y: start.y + dy,
                        };
                        // First cancel a gesture with Esc: it must not save or close the window.
                        SendMessageW(hwnd, WM_LBUTTONDOWN, 1, point_lparam(start));
                        SendMessageW(hwnd, WM_MOUSEMOVE, 1, point_lparam(end));
                        SendMessageW(hwnd, WM_KEYDOWN, 0x1B, 0);
                        if state.drag.get().is_some()
                            || !state
                                .session
                                .borrow()
                                .as_ref()
                                .is_some_and(Session::can_save)
                            || state
                                .session
                                .borrow()
                                .as_ref()
                                .and_then(|s| s.group_rect_at(1))
                                != Some(groups[1].rect)
                        {
                            return Err(io::Error::other("UI drag Esc did not cancel cleanly"));
                        }
                        SendMessageW(hwnd, WM_LBUTTONDOWN, 1, point_lparam(start));
                        if !state.drag.get().is_some_and(|drag| drag.id == groups[1].id) {
                            return Err(io::Error::other("UI drag did not capture second group"));
                        }
                        SendMessageW(hwnd, WM_MOUSEMOVE, 1, point_lparam(end));
                        SendMessageW(hwnd, WM_LBUTTONUP, 0, point_lparam(end));
                        state.session.borrow_mut().as_mut().unwrap().close();
                        ui_drag_started = true;
                    }
                    if options.storage_smoke == Some(crate::app_options::StorageSmoke::UiResize)
                        && !ui_resize_started
                    {
                        if !state
                            .session
                            .borrow()
                            .as_ref()
                            .is_some_and(Session::can_save)
                        {
                            return Err(io::Error::other("UI resize fixture is not writable"));
                        }
                        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
                        let width = GetDlgItem(hwnd, GROUP_WIDTH as i32);
                        let height = GetDlgItem(hwnd, GROUP_HEIGHT as i32);
                        if list.is_null()
                            || width.is_null()
                            || height.is_null()
                            || SendMessageW(list, LB_GETCOUNT, 0, 0) != 2
                            || SendMessageW(list, LB_SETCURSEL, 1, 0) != 1
                        {
                            return Err(io::Error::other("UI resize fixture needs two groups"));
                        }
                        SendMessageW(
                            hwnd,
                            WM_COMMAND,
                            GROUP_LIST | ((LBN_SELCHANGE as usize) << 16),
                            list as isize,
                        );
                        SetWindowTextW(width, wide("500").as_ptr());
                        SetWindowTextW(height, wide("300").as_ptr());
                        SendMessageW(hwnd, WM_COMMAND, RESIZE_GROUP_BUTTON, 0);
                        state.session.borrow_mut().as_mut().unwrap().close();
                        ui_resize_started = true;
                    }
                    if options.storage_smoke == Some(crate::app_options::StorageSmoke::UiResizeDrag)
                        && !ui_resize_drag_started
                    {
                        if !state
                            .session
                            .borrow()
                            .as_ref()
                            .is_some_and(Session::can_save)
                        {
                            return Err(io::Error::other("UI resize-drag fixture is not writable"));
                        }
                        let groups = state.session.borrow().as_ref().unwrap().group_previews();
                        if groups.len() != 2 {
                            return Err(io::Error::other(
                                "UI resize-drag fixture needs two groups",
                            ));
                        }
                        let layout = preview_layout(hwnd, GetDpiForWindow(hwnd).max(96), &groups)
                            .ok_or_else(|| {
                            io::Error::other("UI resize-drag preview unavailable")
                        })?;
                        let box_rect = layout.group(groups[1].rect);
                        let start = POINT {
                            x: box_rect.right - 6,
                            y: box_rect.bottom - 6,
                        };
                        let (dx, dy) = layout.pixel_delta(64, 32);
                        let end = POINT {
                            x: start.x + dx,
                            y: start.y + dy,
                        };
                        SendMessageW(hwnd, WM_LBUTTONDOWN, 1, point_lparam(start));
                        if !state.drag.get().is_some_and(|drag| {
                            drag.id == groups[1].id && drag.kind == DragKind::Resize
                        }) {
                            return Err(io::Error::other("UI resize-drag did not hit handle"));
                        }
                        SendMessageW(hwnd, WM_MOUSEMOVE, 1, point_lparam(end));
                        SendMessageW(hwnd, WM_KEYDOWN, 0x1B, 0);
                        if state.drag.get().is_some()
                            || !state
                                .session
                                .borrow()
                                .as_ref()
                                .is_some_and(Session::can_save)
                            || state
                                .session
                                .borrow()
                                .as_ref()
                                .and_then(|s| s.group_rect_at(1))
                                != Some(groups[1].rect)
                        {
                            return Err(io::Error::other(
                                "UI resize-drag Esc did not cancel cleanly",
                            ));
                        }
                        SendMessageW(hwnd, WM_LBUTTONDOWN, 1, point_lparam(start));
                        SendMessageW(hwnd, WM_MOUSEMOVE, 1, point_lparam(end));
                        SendMessageW(hwnd, WM_LBUTTONUP, 0, point_lparam(end));
                        state.session.borrow_mut().as_mut().unwrap().close();
                        ui_resize_drag_started = true;
                    }
                    if matches!(
                        options.storage_smoke,
                        Some(
                            crate::app_options::StorageSmoke::UiDelete
                                | crate::app_options::StorageSmoke::UiUndoDelete
                        )
                    ) && !ui_delete_started
                    {
                        if !state
                            .session
                            .borrow()
                            .as_ref()
                            .is_some_and(Session::can_save)
                        {
                            return Err(io::Error::other("UI delete fixture is not writable"));
                        }
                        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
                        let button = GetDlgItem(hwnd, DELETE_GROUP_BUTTON as i32);
                        if list.is_null()
                            || button.is_null()
                            || SendMessageW(list, LB_GETCOUNT, 0, 0) != 2
                            || SendMessageW(list, LB_SETCURSEL, 1, 0) != 1
                        {
                            return Err(io::Error::other("UI delete fixture needs two groups"));
                        }
                        SendMessageW(
                            hwnd,
                            WM_COMMAND,
                            GROUP_LIST | ((LBN_SELCHANGE as usize) << 16),
                            list as isize,
                        );
                        SendMessageW(hwnd, WM_COMMAND, DELETE_GROUP_BUTTON, button as isize);
                        if state
                            .session
                            .borrow()
                            .as_ref()
                            .unwrap()
                            .group_titles(3)
                            .len()
                            != 2
                            || state.armed_delete.get()
                                != state.session.borrow().as_ref().unwrap().group_id_at(1)
                        {
                            return Err(io::Error::other("UI delete first click did not only arm"));
                        }
                        SendMessageW(hwnd, WM_COMMAND, DELETE_GROUP_BUTTON, button as isize);
                        if options.storage_smoke == Some(crate::app_options::StorageSmoke::UiDelete)
                        {
                            state.session.borrow_mut().as_mut().unwrap().close();
                        }
                        ui_delete_started = true;
                    }
                    if options.storage_smoke == Some(crate::app_options::StorageSmoke::UiColor)
                        && !ui_color_started
                    {
                        if !state
                            .session
                            .borrow()
                            .as_ref()
                            .is_some_and(Session::can_save)
                        {
                            return Err(io::Error::other("UI color fixture is not writable"));
                        }
                        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
                        let edit = GetDlgItem(hwnd, GROUP_COLOR as i32);
                        if list.is_null()
                            || edit.is_null()
                            || SendMessageW(list, LB_GETCOUNT, 0, 0) != 2
                            || SendMessageW(list, LB_SETCURSEL, 1, 0) != 1
                        {
                            return Err(io::Error::other("UI color fixture needs two groups"));
                        }
                        SendMessageW(
                            hwnd,
                            WM_COMMAND,
                            GROUP_LIST | ((LBN_SELCHANGE as usize) << 16),
                            list as isize,
                        );
                        SetWindowTextW(edit, wide("#E85D75").as_ptr());
                        SendMessageW(hwnd, WM_COMMAND, COLOR_GROUP_BUTTON, 0);
                        state.session.borrow_mut().as_mut().unwrap().close();
                        ui_color_started = true;
                    }
                    if options.storage_smoke == Some(crate::app_options::StorageSmoke::UiFold)
                        && !ui_fold_started
                    {
                        if !state
                            .session
                            .borrow()
                            .as_ref()
                            .is_some_and(Session::can_save)
                        {
                            return Err(io::Error::other("UI fold fixture is not writable"));
                        }
                        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
                        let button = GetDlgItem(hwnd, FOLD_GROUP_BUTTON as i32);
                        if list.is_null()
                            || button.is_null()
                            || SendMessageW(list, LB_GETCOUNT, 0, 0) != 2
                            || SendMessageW(list, LB_SETCURSEL, 1, 0) != 1
                        {
                            return Err(io::Error::other("UI fold fixture needs two groups"));
                        }
                        SendMessageW(
                            hwnd,
                            WM_COMMAND,
                            GROUP_LIST | ((LBN_SELCHANGE as usize) << 16),
                            list as isize,
                        );
                        SendMessageW(hwnd, WM_COMMAND, FOLD_GROUP_BUTTON, button as isize);
                        state.session.borrow_mut().as_mut().unwrap().close();
                        ui_fold_started = true;
                    }
                    if matches!(
                        options.storage_smoke,
                        Some(
                            crate::app_options::StorageSmoke::UiRemove
                                | crate::app_options::StorageSmoke::UiUndoRemove
                        )
                    ) && !ui_remove_started
                    {
                        let (groups, item_id) = {
                            let borrowed = state.session.borrow();
                            let session = borrowed.as_ref().unwrap();
                            let groups = session.group_previews();
                            if !session.can_save() || groups.len() != 2 {
                                return Err(io::Error::other(
                                    "UI remove fixture needs two writable groups",
                                ));
                            }
                            let (count, items) = session.preview_items(groups[1].id, 24);
                            if count != 1 || items.len() != 1 {
                                return Err(io::Error::other(
                                    "UI remove fixture needs one visible reference",
                                ));
                            }
                            (groups, items[0].id)
                        };
                        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
                        let button = GetDlgItem(hwnd, REMOVE_ITEM_BUTTON as i32);
                        if list.is_null()
                            || button.is_null()
                            || SendMessageW(list, LB_SETCURSEL, 1, 0) != 1
                        {
                            return Err(io::Error::other("UI remove controls unavailable"));
                        }
                        SendMessageW(
                            hwnd,
                            WM_COMMAND,
                            GROUP_LIST | ((LBN_SELCHANGE as usize) << 16),
                            list as isize,
                        );
                        let dpi = GetDpiForWindow(hwnd).max(96);
                        let layout = preview_layout(hwnd, dpi, &groups)
                            .ok_or_else(|| io::Error::other("UI remove preview unavailable"))?;
                        let box_rect = preview_group_rect(layout, &groups[1], dpi);
                        let content = item_content(box_rect, dpi)
                            .ok_or_else(|| io::Error::other("UI remove content unavailable"))?;
                        let slot = item_slots(content, dpi, 1)
                            .into_iter()
                            .next()
                            .ok_or_else(|| io::Error::other("UI remove tile unavailable"))?;
                        let point = POINT {
                            x: (slot.left + slot.right) / 2,
                            y: (slot.top + slot.bottom) / 2,
                        };
                        SendMessageW(hwnd, WM_LBUTTONDOWN, 1, point_lparam(point));
                        SendMessageW(hwnd, WM_LBUTTONUP, 0, point_lparam(point));
                        if state.selected_item.get() != Some((groups[1].id, item_id)) {
                            return Err(io::Error::other("UI remove tile was not selected"));
                        }
                        SendMessageW(hwnd, WM_COMMAND, REMOVE_ITEM_BUTTON, button as isize);
                        if state.armed_remove.get() != Some((groups[1].id, item_id))
                            || state
                                .session
                                .borrow()
                                .as_ref()
                                .unwrap()
                                .preview_items(groups[1].id, 1)
                                .0
                                != 1
                        {
                            return Err(io::Error::other("UI remove first click did not only arm"));
                        }
                        SendMessageW(hwnd, WM_KEYDOWN, 0x1B, 0);
                        if state.armed_remove.get().is_some()
                            || state.selected_item.get() != Some((groups[1].id, item_id))
                            || !state.session.borrow().as_ref().unwrap().can_save()
                            || state
                                .session
                                .borrow()
                                .as_ref()
                                .unwrap()
                                .preview_items(groups[1].id, 1)
                                .0
                                != 1
                            || !state
                                .session
                                .borrow()
                                .as_ref()
                                .unwrap()
                                .status
                                .contains("已取消")
                        {
                            return Err(io::Error::other("UI remove Escape did not cancel safely"));
                        }
                        SendMessageW(hwnd, WM_COMMAND, REMOVE_ITEM_BUTTON, button as isize);
                        if state.armed_remove.get() != Some((groups[1].id, item_id)) {
                            return Err(io::Error::other("UI remove did not re-arm after Escape"));
                        }
                        SendMessageW(hwnd, WM_COMMAND, REMOVE_ITEM_BUTTON, button as isize);
                        if state.armed_remove.get().is_some()
                            || state.selected_item.get().is_some()
                            || state
                                .session
                                .borrow()
                                .as_ref()
                                .unwrap()
                                .preview_items(groups[1].id, 1)
                                .0
                                != 0
                        {
                            return Err(io::Error::other(
                                "UI remove did not remove only the reference",
                            ));
                        }
                        if options.storage_smoke == Some(crate::app_options::StorageSmoke::UiRemove)
                        {
                            state.session.borrow_mut().as_mut().unwrap().close();
                        }
                        ui_remove_started = true;
                    }
                    if matches!(
                        options.storage_smoke,
                        Some(
                            crate::app_options::StorageSmoke::UiUndoRemove
                                | crate::app_options::StorageSmoke::UiUndoDelete
                        )
                    ) && (ui_remove_started || ui_delete_started)
                        && !ui_undo_started
                        && state
                            .session
                            .borrow()
                            .as_ref()
                            .is_some_and(Session::can_undo_removal)
                    {
                        let button = GetDlgItem(hwnd, UNDO_REMOVAL_BUTTON as i32);
                        if button.is_null() {
                            return Err(io::Error::other("UI undo button unavailable"));
                        }
                        SendMessageW(hwnd, WM_COMMAND, UNDO_REMOVAL_BUTTON, button as isize);
                        let borrowed = state.session.borrow();
                        let session = borrowed.as_ref().unwrap();
                        if session.can_save()
                            || session.can_undo_removal()
                            || session.group_previews().len() != 2
                            || session.preview_items(session.group_id_at(1).unwrap(), 1).0 != 1
                        {
                            return Err(io::Error::other(
                                "UI undo failed to restore group/reference and begin save",
                            ));
                        }
                        drop(borrowed);
                        state.session.borrow_mut().as_mut().unwrap().close();
                        ui_undo_started = true;
                    }
                    if options.storage_smoke == Some(crate::app_options::StorageSmoke::Add)
                        && !add_started
                    {
                        let group_id = {
                            let session = state.session.borrow();
                            let session = session.as_ref().unwrap();
                            if !session.can_save() {
                                return Err(io::Error::other("add fixture is not writable"));
                            }
                            let groups = session.group_previews();
                            if groups.len() != 2 {
                                return Err(io::Error::other("add fixture needs two groups"));
                            }
                            groups[0].id
                        };
                        let source = options
                            .data_dir
                            .as_ref()
                            .expect("storage smoke requires data dir")
                            .join("source.txt");
                        let raw = source.to_string_lossy();
                        let path = raw
                            .strip_prefix(r"\\?\")
                            .unwrap_or(raw.as_ref())
                            .replace('/', "\\");
                        let path = NativePath::from_utf16(path.encode_utf16().collect())
                            .map_err(|_| io::Error::other("invalid add fixture path"))?;
                        state.session.borrow_mut().as_mut().unwrap().add_references(
                            group_id,
                            vec![FileReference {
                                kind: ReferenceKind::File,
                                path,
                            }],
                        );
                        state.session.borrow_mut().as_mut().unwrap().close();
                        add_started = true;
                    }
                    if matches!(
                        options.storage_smoke,
                        Some(
                            crate::app_options::StorageSmoke::UiAdd
                                | crate::app_options::StorageSmoke::UiShortcut
                        )
                    ) && !ui_add_started
                    {
                        let ready = state.session.borrow().as_ref().is_some_and(|session| {
                            session.can_save() && session.group_previews().len() == 2
                        });
                        if !ready {
                            return Err(io::Error::other(
                                "UI add fixture needs two writable groups",
                            ));
                        }
                        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
                        if list.is_null() || SendMessageW(list, LB_SETCURSEL, 1, 0) != 1 {
                            return Err(io::Error::other("UI add fixture needs a second group"));
                        }
                        SendMessageW(
                            hwnd,
                            WM_COMMAND,
                            GROUP_LIST | ((LBN_SELCHANGE as usize) << 16),
                            list as isize,
                        );
                        let source = options.data_dir.as_ref().unwrap().join(
                            if options.storage_smoke
                                == Some(crate::app_options::StorageSmoke::UiShortcut)
                            {
                                "source-link.lnk"
                            } else {
                                "source.txt"
                            },
                        );
                        let raw = source.to_string_lossy();
                        let path = raw
                            .strip_prefix(r"\\?\")
                            .unwrap_or(raw.as_ref())
                            .replace('/', "\\");
                        let mut buffer: Vec<u16> = path.encode_utf16().collect();
                        buffer.extend([0, 0]);
                        *state.picker_override.borrow_mut() = Some(
                            picker_paths::parse_picker_paths(&buffer)
                                .map_err(|_| io::Error::other("UI add fixture path invalid"))?,
                        );
                        SendMessageW(hwnd, WM_COMMAND, ADD_FILES_BUTTON, 0);
                        state.session.borrow_mut().as_mut().unwrap().close();
                        ui_add_started = true;
                    }
                    if options.storage_smoke == Some(crate::app_options::StorageSmoke::UiFolder)
                        && !ui_folder_started
                    {
                        let ready = state.session.borrow().as_ref().is_some_and(|session| {
                            session.can_save() && session.group_previews().len() == 2
                        });
                        if !ready {
                            return Err(io::Error::other(
                                "UI folder fixture needs two writable groups",
                            ));
                        }
                        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
                        if list.is_null() || SendMessageW(list, LB_SETCURSEL, 1, 0) != 1 {
                            return Err(io::Error::other("UI folder fixture needs a second group"));
                        }
                        SendMessageW(
                            hwnd,
                            WM_COMMAND,
                            GROUP_LIST | ((LBN_SELCHANGE as usize) << 16),
                            list as isize,
                        );
                        let source = options.data_dir.as_ref().unwrap().join("source-folder");
                        let raw = source.to_string_lossy();
                        let path = raw
                            .strip_prefix(r"\\?\")
                            .unwrap_or(raw.as_ref())
                            .replace('/', "\\");
                        *state.folder_override.borrow_mut() = Some(
                            NativePath::from_utf16(path.encode_utf16().collect())
                                .map_err(|_| io::Error::other("UI folder fixture path invalid"))?,
                        );
                        SendMessageW(hwnd, WM_COMMAND, ADD_FOLDER_BUTTON, 0);
                        state.session.borrow_mut().as_mut().unwrap().close();
                        ui_folder_started = true;
                    }
                    if matches!(
                        options.storage_smoke,
                        Some(
                            crate::app_options::StorageSmoke::UiDrop
                                | crate::app_options::StorageSmoke::UiDropFolder
                                | crate::app_options::StorageSmoke::UiDropMixed
                                | crate::app_options::StorageSmoke::UiDropShortcut
                        )
                    ) && !ui_drop_started
                    {
                        let ready = state.session.borrow().as_ref().is_some_and(|session| {
                            session.can_save() && session.group_previews().len() == 2
                        });
                        if !ready {
                            return Err(io::Error::other(
                                "UI drop fixture needs two writable groups",
                            ));
                        }
                        let groups = state.session.borrow().as_ref().unwrap().group_previews();
                        let dpi = GetDpiForWindow(hwnd).max(96);
                        let layout = preview_layout(hwnd, dpi, &groups)
                            .ok_or_else(|| io::Error::other("UI drop preview unavailable"))?;
                        let rect = preview_group_rect(layout, &groups[1], dpi);
                        let mut point = POINT {
                            x: rect.left + (rect.right - rect.left) / 2,
                            y: rect.top + 12,
                        };
                        if ClientToScreen(hwnd, &mut point) == 0 {
                            return Err(io::Error::last_os_error());
                        }
                        let name = match options.storage_smoke {
                            Some(crate::app_options::StorageSmoke::UiDropFolder) => "source-folder",
                            Some(crate::app_options::StorageSmoke::UiDropShortcut) => {
                                "source-link.lnk"
                            }
                            _ => "source.txt",
                        };
                        let source = options.data_dir.as_ref().unwrap().join(name);
                        let registration = _drop_registration
                            .as_ref()
                            .ok_or_else(|| io::Error::other("drop target unavailable"))?;
                        let point = windows::Win32::Foundation::POINTL {
                            x: point.x,
                            y: point.y,
                        };
                        if options.storage_smoke
                            == Some(crate::app_options::StorageSmoke::UiDropMixed)
                        {
                            let folder = options.data_dir.as_ref().unwrap().join("source-folder");
                            registration
                                .exercise_mixed_drop(&source, &folder, point)
                                .map_err(io::Error::other)?;
                        } else {
                            registration
                                .exercise_drop(&source, point)
                                .map_err(io::Error::other)?;
                        }
                        state.session.borrow_mut().as_mut().unwrap().close();
                        ui_drop_started = true;
                    }
                    if matches!(
                        options.storage_smoke,
                        Some(
                            crate::app_options::StorageSmoke::UiTransfer
                                | crate::app_options::StorageSmoke::UiTransferCancel
                                | crate::app_options::StorageSmoke::UiTransferOutside
                                | crate::app_options::StorageSmoke::UiTransferCapture
                        )
                    ) && !ui_transfer_started
                        && state
                            .session
                            .borrow()
                            .as_ref()
                            .is_some_and(Session::can_save)
                    {
                        use crate::app_options::StorageSmoke;
                        let groups = state.session.borrow().as_ref().unwrap().group_previews();
                        if groups.len() != 2 || groups[0].collapsed || groups[1].collapsed {
                            return Err(io::Error::other(
                                "UI transfer requires two expanded groups",
                            ));
                        }
                        let dpi = GetDpiForWindow(hwnd).max(96);
                        let layout = preview_layout(hwnd, dpi, &groups)
                            .ok_or_else(|| io::Error::other("UI transfer layout unavailable"))?;
                        let slots = item_content(preview_group_rect(layout, &groups[0], dpi), dpi)
                            .map_or_else(Vec::new, |area| item_slots(area, dpi, 24));
                        let slot = slots
                            .first()
                            .ok_or_else(|| io::Error::other("No transfer tile"))?;
                        let start = POINT {
                            x: slot.left + 5,
                            y: slot.top + 5,
                        };
                        let target_box = preview_group_rect(layout, &groups[1], dpi);
                        let end = POINT {
                            x: target_box.left + 10,
                            y: target_box.top + 10,
                        };
                        let item = state
                            .session
                            .borrow()
                            .as_ref()
                            .unwrap()
                            .preview_items(groups[0].id, 1)
                            .1
                            .first()
                            .ok_or_else(|| io::Error::other("No transfer reference"))?
                            .id;
                        SendMessageW(hwnd, WM_LBUTTONDOWN, 1, point_lparam(start));
                        if state.selected_item.get() != Some((groups[0].id, item))
                            || state.item_drag.get().is_none()
                            || GetCapture() != hwnd
                        {
                            return Err(io::Error::other(
                                "Transfer pointer-down did not capture exact item",
                            ));
                        }
                        SendMessageW(hwnd, WM_MOUSEMOVE, 1, point_lparam(end));
                        if !state.item_drag.get().is_some_and(|drag| drag.active)
                            || state.item_drag_hover.get() != Some(groups[1].id)
                            || !state
                                .session
                                .borrow()
                                .as_ref()
                                .unwrap()
                                .contains_reference_id(groups[0].id, item)
                            || !state.session.borrow().as_ref().unwrap().can_save()
                        {
                            return Err(io::Error::other(
                                "Drag hover mutated config or chose wrong group",
                            ));
                        }
                        let smoke = options.storage_smoke.unwrap();
                        if smoke == StorageSmoke::UiTransferCancel {
                            SendMessageW(hwnd, WM_KEYDOWN, 0x1B, 0);
                        } else if smoke == StorageSmoke::UiTransferCapture {
                            ReleaseCapture(); // Real capture-loss message for this hidden HWND.
                        }
                        let release = if smoke == StorageSmoke::UiTransferOutside {
                            POINT {
                                x: layout.view.left + 1,
                                y: layout.view.top + 1,
                            }
                        } else {
                            end
                        };
                        SendMessageW(hwnd, WM_LBUTTONUP, 0, point_lparam(release));
                        if state.item_drag.get().is_some()
                            || state.item_drag_hover.get().is_some()
                            || GetCapture() == hwnd
                        {
                            return Err(io::Error::other(
                                "Transfer release retained capture or hover",
                            ));
                        }
                        if smoke != StorageSmoke::UiTransfer {
                            let unchanged =
                                state.session.borrow().as_ref().is_some_and(|session| {
                                    session.can_save()
                                        && session.contains_reference_id(groups[0].id, item)
                                        && !session.contains_reference_id(groups[1].id, item)
                                });
                            if !unchanged {
                                return Err(io::Error::other(
                                    "Cancelled transfer mutated references",
                                ));
                            }
                            state.session.borrow_mut().as_mut().unwrap().smoke_passed = true;
                        }
                        ui_transfer_started = true;
                        state.session.borrow_mut().as_mut().unwrap().close();
                    }
                    if options.storage_smoke.is_some() {
                        // Hidden windows do not receive ordinary visible-region paint dispatch.
                        SendMessageW(hwnd, WM_PAINT, 0, 0);
                    }
                }
                if state
                    .session
                    .borrow()
                    .as_ref()
                    .is_some_and(Session::should_destroy)
                {
                    DestroyWindow(hwnd);
                }
                while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    if message.message == WM_QUIT {
                        break 'messages;
                    }
                    if message.message == WM_KEYDOWN {
                        if message.wParam == 0x0D
                            && GetFocus() == hwnd
                            && state.selected_item.get().is_some()
                        {
                            open_selected_reference(hwnd, &state);
                            continue;
                        }
                        if message.wParam == 0x1B {
                            if state.drag.get().is_some() || state.item_drag.get().is_some() {
                                cancel_drag(hwnd, &state);
                            } else if state.armed_delete.get().is_some()
                                || state.armed_remove.get().is_some()
                            {
                                cancel_confirmation(hwnd, &state);
                            } else {
                                SendMessageW(hwnd, WM_CLOSE, 0, 0);
                            }
                            continue;
                        }
                        if message.wParam == 0x53 && GetKeyState(VK_CONTROL as i32) < 0 {
                            SendMessageW(hwnd, WM_COMMAND, SAVE_BUTTON, 0);
                            continue;
                        }
                        if message.wParam == 0x5A && GetKeyState(VK_CONTROL as i32) < 0 {
                            let focused = GetFocus();
                            // Text editors keep their native Ctrl+Z semantics.
                            let editing = [
                                GROUP_NAME,
                                GROUP_X,
                                GROUP_Y,
                                GROUP_WIDTH,
                                GROUP_HEIGHT,
                                GROUP_COLOR,
                            ]
                            .iter()
                            .any(|id| focused == GetDlgItem(hwnd, *id as i32));
                            if !editing {
                                SendMessageW(hwnd, WM_COMMAND, UNDO_REMOVAL_BUTTON, 0);
                                continue;
                            }
                        }
                        if message.wParam == 0x4e && GetKeyState(VK_CONTROL as i32) < 0 {
                            SendMessageW(hwnd, WM_COMMAND, CREATE_GROUP_BUTTON, 0);
                            continue;
                        }
                    }
                    if IsDialogMessageW(hwnd, &message) == 0 {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                continue;
            }
            let result = GetMessageW(&mut message, null_mut(), 0, 0);
            if result == -1 {
                return Err(io::Error::last_os_error());
            }
            if result == 0 {
                break 'messages;
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        #[cfg(feature = "config-store")]
        if options.storage_smoke.is_some()
            && (!state
                .session
                .borrow()
                .as_ref()
                .is_some_and(|s| s.smoke_passed && s.stopped)
                || state.paints.get() == 0
                || !state.destroyed.get())
        {
            return Err(io::Error::other("Storage window smoke test failed"));
        }
        if smoke_test
            && (state.paints.get() == 0 || state.resizes.get() == 0 || !state.destroyed.get())
        {
            return Err(io::Error::other("Window lifecycle smoke test failed"));
        }
        #[cfg(feature = "ui-acceptance")]
        {
            let (message_histogram, high_message_ids) = state.diagnostics.message_histogram();
            Ok(LoopReport {
                counters: state.diagnostics.snapshot(),
                initial_visibility: state.initial_visibility.get(),
                closing_visibility: state.closing_visibility.get(),
                timeout_fired: state.diagnostic_timeout_fired.get(),
                message_histogram,
                high_message_ids,
                undo_fixture_seeded: state.undo_fixture_history_ready.get(),
                undo_fixture_restored: state.undo_fixture_restored.get(),
            })
        }
        #[cfg(not(feature = "ui-acceptance"))]
        {
            Ok(())
        }
    }
}

#[cfg(feature = "config-store")]
unsafe fn refresh_session(hwnd: HWND, state: &WindowState) {
    let enabled = state
        .session
        .borrow()
        .as_ref()
        .is_some_and(Session::can_save);
    let create_enabled = state
        .session
        .borrow()
        .as_ref()
        .is_some_and(Session::can_create);
    let undo_enabled = state
        .session
        .borrow()
        .as_ref()
        .is_some_and(Session::can_undo_removal);
    let titles = state
        .session
        .borrow()
        .as_ref()
        .map_or_else(Vec::new, |session| session.group_titles(128));
    // SAFETY: only UI thread touches HWND; no state borrows survive native callbacks.
    unsafe {
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        if !list.is_null() {
            let old_len = state.rendered_group_titles.borrow().len();
            if *state.rendered_group_titles.borrow() != titles {
                let previous = SendMessageW(list, LB_GETCURSEL, 0, 0);
                SendMessageW(list, LB_RESETCONTENT, 0, 0);
                for title in &titles {
                    let wide_title = wide(title);
                    SendMessageW(list, LB_ADDSTRING, 0, wide_title.as_ptr() as isize);
                }
                let selection = if titles.is_empty() {
                    None
                } else if titles.len() > old_len {
                    Some(titles.len() - 1)
                } else if previous >= 0 && (previous as usize) < titles.len() {
                    Some(previous as usize)
                } else {
                    Some(0)
                };
                if let Some(index) = selection {
                    SendMessageW(list, LB_SETCURSEL, index, 0);
                }
                *state.rendered_group_titles.borrow_mut() = titles.clone();
                sync_group_name(hwnd, state);
            }
        }
        sync_group_position_if_changed(hwnd, state);
        sync_group_color_if_changed(hwnd, state);
        sync_group_fold_if_changed(hwnd, state);
        let transfer_combo = GetDlgItem(hwnd, TRANSFER_GROUP_COMBO as i32);
        if !transfer_combo.is_null() && *state.rendered_transfer_titles.borrow() != titles {
            // Copy stable IDs before native callbacks; never cast a u64 ID into LPARAM.
            let ids: Vec<_> = state
                .session
                .borrow()
                .as_ref()
                .map_or_else(Vec::new, |session| {
                    (0..titles.len())
                        .filter_map(|i| session.group_id_at(i))
                        .collect()
                });
            let index = ids
                .iter()
                .position(|id| Some(*id) == state.transfer_target.get())
                .or_else(|| (!ids.is_empty()).then_some(0));
            SendMessageW(transfer_combo, CB_RESETCONTENT, 0, 0);
            for title in &titles {
                let label = wide(title);
                SendMessageW(transfer_combo, CB_ADDSTRING, 0, label.as_ptr() as isize);
            }
            if let Some(index) = index {
                SendMessageW(transfer_combo, CB_SETCURSEL, index, 0);
            }
            state
                .transfer_target
                .set(index.and_then(|i| ids.get(i).copied()));
            *state.rendered_transfer_titles.borrow_mut() = titles;
        }
        save_focus::update(
            hwnd,
            GetDlgItem(hwnd, SAVE_BUTTON as i32),
            enabled,
            &state.restore_save_focus,
        );
        save_focus::update(
            hwnd,
            GetDlgItem(hwnd, CREATE_GROUP_BUTTON as i32),
            create_enabled,
            &state.restore_group_focus,
        );
        save_focus::update(
            hwnd,
            GetDlgItem(hwnd, RENAME_GROUP_BUTTON as i32),
            enabled && !list.is_null() && SendMessageW(list, LB_GETCURSEL, 0, 0) >= 0,
            &state.restore_rename_focus,
        );
        save_focus::update(
            hwnd,
            GetDlgItem(hwnd, MOVE_GROUP_BUTTON as i32),
            enabled && !list.is_null() && SendMessageW(list, LB_GETCURSEL, 0, 0) >= 0,
            &state.restore_move_focus,
        );
        save_focus::update(
            hwnd,
            GetDlgItem(hwnd, RESIZE_GROUP_BUTTON as i32),
            enabled && !list.is_null() && SendMessageW(list, LB_GETCURSEL, 0, 0) >= 0,
            &state.restore_resize_focus,
        );
        save_focus::update(
            hwnd,
            GetDlgItem(hwnd, DELETE_GROUP_BUTTON as i32),
            enabled && !list.is_null() && SendMessageW(list, LB_GETCURSEL, 0, 0) >= 0,
            &state.restore_delete_focus,
        );
        save_focus::update(
            hwnd,
            GetDlgItem(hwnd, COLOR_GROUP_BUTTON as i32),
            enabled && !list.is_null() && SendMessageW(list, LB_GETCURSEL, 0, 0) >= 0,
            &state.restore_color_focus,
        );
        save_focus::update(
            hwnd,
            GetDlgItem(hwnd, FOLD_GROUP_BUTTON as i32),
            enabled && !list.is_null() && SendMessageW(list, LB_GETCURSEL, 0, 0) >= 0,
            &state.restore_fold_focus,
        );
        save_focus::update(
            hwnd,
            GetDlgItem(hwnd, ADD_FILES_BUTTON as i32),
            enabled && !list.is_null() && SendMessageW(list, LB_GETCURSEL, 0, 0) >= 0,
            &state.restore_add_focus,
        );
        save_focus::update(
            hwnd,
            GetDlgItem(hwnd, ADD_FOLDER_BUTTON as i32),
            enabled && !list.is_null() && SendMessageW(list, LB_GETCURSEL, 0, 0) >= 0,
            &state.restore_folder_focus,
        );
        let selected_item_valid = state
            .selected_item
            .get()
            .is_some_and(|(group_id, item_id)| {
                let index = if list.is_null() {
                    -1
                } else {
                    SendMessageW(list, LB_GETCURSEL, 0, 0)
                };
                index >= 0
                    && state.session.borrow().as_ref().is_some_and(|session| {
                        session.group_id_at(index as usize) == Some(group_id)
                            && session.contains_reference_id(group_id, item_id)
                    })
            });
        save_focus::update(
            hwnd,
            GetDlgItem(hwnd, REMOVE_ITEM_BUTTON as i32),
            enabled && selected_item_valid,
            &state.restore_remove_focus,
        );
        save_focus::update(
            hwnd,
            GetDlgItem(hwnd, UNDO_REMOVAL_BUTTON as i32),
            undo_enabled,
            &state.restore_undo_focus,
        );
        for (id, step, restore) in [
            (SORT_EARLIER_BUTTON, -1, &state.restore_sort_earlier_focus),
            (SORT_LATER_BUTTON, 1, &state.restore_sort_later_focus),
        ] {
            let can_shift = enabled
                && selected_item_valid
                && state.selected_item.get().is_some_and(|(group, item)| {
                    state
                        .session
                        .borrow()
                        .as_ref()
                        .is_some_and(|s| s.shifted_index(group, item, step).is_some())
                });
            save_focus::update(hwnd, GetDlgItem(hwnd, id as i32), can_shift, restore);
        }
        save_focus::update(
            hwnd,
            GetDlgItem(hwnd, REVEAL_ITEM_BUTTON as i32),
            selected_item_valid && state.open_allowed.get(),
            &state.restore_reveal_focus,
        );
        save_focus::update(
            hwnd,
            transfer_combo,
            enabled && selected_item_valid,
            &state.restore_transfer_combo_focus,
        );
        let can_transfer = enabled
            && selected_item_valid
            && state.selected_item.get().is_some_and(|(source, _)| {
                state
                    .transfer_target
                    .get()
                    .is_some_and(|target| source != target)
            });
        save_focus::update(
            hwnd,
            GetDlgItem(hwnd, TRANSFER_ITEM_BUTTON as i32),
            can_transfer,
            &state.restore_transfer_focus,
        );
        InvalidateRect(hwnd, null(), 0);
        UpdateWindow(hwnd);
    }
}

#[cfg(feature = "config-store")]
unsafe fn sync_group_name(hwnd: HWND, state: &WindowState) {
    // SAFETY: handles belong to this UI-thread window; SetWindowTextW copies its input.
    unsafe {
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        let edit = GetDlgItem(hwnd, GROUP_NAME as i32);
        if list.is_null() || edit.is_null() {
            return;
        }
        let selected = SendMessageW(list, LB_GETCURSEL, 0, 0);
        let title = if selected >= 0 {
            state
                .session
                .borrow()
                .as_ref()
                .and_then(|session| session.group_title_at(selected as usize))
                .unwrap_or_default()
        } else {
            String::new()
        };
        // The list label includes the stable ID; only the actual title is editable.
        // Its owned copy also releases the Session borrow before native callbacks.
        SetWindowTextW(edit, wide(&title).as_ptr());
    }
}

#[cfg(feature = "config-store")]
unsafe fn sync_group_position_if_changed(hwnd: HWND, state: &WindowState) {
    unsafe {
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        let x = GetDlgItem(hwnd, GROUP_X as i32);
        let y = GetDlgItem(hwnd, GROUP_Y as i32);
        let width = GetDlgItem(hwnd, GROUP_WIDTH as i32);
        let height = GetDlgItem(hwnd, GROUP_HEIGHT as i32);
        if list.is_null() || x.is_null() || y.is_null() || width.is_null() || height.is_null() {
            return;
        }
        let index = SendMessageW(list, LB_GETCURSEL, 0, 0);
        let selected = if index >= 0 {
            state.session.borrow().as_ref().and_then(|session| {
                Some((
                    session.group_id_at(index as usize)?,
                    session.group_rect_at(index as usize)?,
                ))
            })
        } else {
            None
        };
        if state.rendered_selection_rect.get() == selected {
            return;
        }
        state.rendered_selection_rect.set(selected);
        let (x_text, y_text) = selected.map_or((String::new(), String::new()), |(_, rect)| {
            (rect.x.to_string(), rect.y.to_string())
        });
        SetWindowTextW(x, wide(&x_text).as_ptr());
        SetWindowTextW(y, wide(&y_text).as_ptr());
        let (width_text, height_text) = selected
            .map_or((String::new(), String::new()), |(_, rect)| {
                (rect.width.to_string(), rect.height.to_string())
            });
        SetWindowTextW(width, wide(&width_text).as_ptr());
        SetWindowTextW(height, wide(&height_text).as_ptr());
    }
}

#[cfg(feature = "config-store")]
unsafe fn sync_group_color_if_changed(hwnd: HWND, state: &WindowState) {
    unsafe {
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        let edit = GetDlgItem(hwnd, GROUP_COLOR as i32);
        if list.is_null() || edit.is_null() {
            return;
        }
        let index = SendMessageW(list, LB_GETCURSEL, 0, 0);
        let selected = if index >= 0 {
            state.session.borrow().as_ref().and_then(|session| {
                Some((
                    session.group_id_at(index as usize)?,
                    session.group_color_at(index as usize)?,
                ))
            })
        } else {
            None
        };
        if state.rendered_selection_color.get() == selected {
            return;
        }
        state.rendered_selection_color.set(selected);
        let value = selected.map_or_else(String::new, |(_, rgb)| {
            format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
        });
        SetWindowTextW(edit, wide(&value).as_ptr());
    }
}

#[cfg(feature = "config-store")]
unsafe fn sync_group_fold_if_changed(hwnd: HWND, state: &WindowState) {
    unsafe {
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        let button = GetDlgItem(hwnd, FOLD_GROUP_BUTTON as i32);
        if list.is_null() || button.is_null() {
            return;
        }
        let index = SendMessageW(list, LB_GETCURSEL, 0, 0);
        let selected = if index >= 0 {
            state.session.borrow().as_ref().and_then(|session| {
                Some((
                    session.group_id_at(index as usize)?,
                    session.group_collapsed_at(index as usize)?,
                ))
            })
        } else {
            None
        };
        if state.rendered_selection_fold.get() == selected {
            return;
        }
        state.rendered_selection_fold.set(selected);
        let label = if selected.is_some_and(|(_, collapsed)| collapsed) {
            windows_sys::core::w!("展开所选分组")
        } else {
            windows_sys::core::w!("折叠所选分组")
        };
        SetWindowTextW(button, label);
    }
}

#[cfg(feature = "config-store")]
unsafe fn position_group_controls(hwnd: HWND, dpi: u32) {
    // SAFETY: fixed child IDs belong to this UI-thread parent; absent controls are skipped.
    unsafe {
        for (id, x, y, width, height) in [
            (GROUP_LIST_LABEL, 48, 350, 220, 22),
            (GROUP_NAME_LABEL, 285, 350, 220, 22),
            (GROUP_LIST, 48, 375, 220, 140),
            (GROUP_NAME, 285, 375, 230, 30),
            (RENAME_GROUP_BUTTON, 285, 420, 230, 34),
            (GROUP_X_LABEL, 285, 460, 110, 22),
            (GROUP_Y_LABEL, 405, 460, 110, 22),
            (GROUP_X, 285, 485, 110, 30),
            (GROUP_Y, 405, 485, 110, 30),
            (MOVE_GROUP_BUTTON, 285, 530, 230, 34),
            (GROUP_WIDTH_LABEL, 285, 575, 110, 22),
            (GROUP_HEIGHT_LABEL, 405, 575, 110, 22),
            (GROUP_WIDTH, 285, 600, 110, 30),
            (GROUP_HEIGHT, 405, 600, 110, 30),
            (RESIZE_GROUP_BUTTON, 285, 645, 230, 34),
            (DELETE_GROUP_BUTTON, 48, 535, 220, 34),
            (GROUP_COLOR_LABEL, 48, 590, 220, 22),
            (GROUP_COLOR, 48, 615, 220, 30),
            (COLOR_GROUP_BUTTON, 48, 660, 220, 34),
            (FOLD_GROUP_BUTTON, 48, 710, 220, 34),
            (REMOVE_ITEM_BUTTON, 285, 710, 230, 34),
            (UNDO_REMOVAL_BUTTON, 550, 290, 285, 34),
            (SORT_EARLIER_BUTTON, 550, 330, 140, 34),
            (SORT_LATER_BUTTON, 695, 330, 140, 34),
            (REVEAL_ITEM_BUTTON, 550, 370, 285, 34),
            (TRANSFER_GROUP_COMBO, 550, 410, 140, 200),
            (TRANSFER_ITEM_BUTTON, 695, 410, 140, 34),
            (ADD_FILES_BUTTON, 48, 765, 220, 34),
            (ADD_FOLDER_BUTTON, 285, 765, 230, 34),
        ] {
            let control = GetDlgItem(hwnd, id as i32);
            if !control.is_null() {
                SetWindowPos(
                    control,
                    null_mut(),
                    px(x, dpi),
                    px(y, dpi),
                    px(width, dpi),
                    px(height, dpi),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
    }
}

#[cfg(feature = "config-store")]
unsafe fn read_coordinate(hwnd: HWND) -> Option<i32> {
    unsafe {
        let length = GetWindowTextLengthW(hwnd);
        if !(1..=12).contains(&length) {
            return None;
        }
        let mut units = vec![0_u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, units.as_mut_ptr(), units.len() as i32);
        String::from_utf16(&units[..copied as usize])
            .ok()?
            .parse()
            .ok()
    }
}

#[cfg(feature = "config-store")]
unsafe fn read_size(hwnd: HWND) -> Option<u32> {
    unsafe {
        let length = GetWindowTextLengthW(hwnd);
        if !(1..=10).contains(&length) {
            return None;
        }
        let mut units = vec![0_u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, units.as_mut_ptr(), units.len() as i32);
        String::from_utf16(&units[..copied as usize])
            .ok()?
            .parse()
            .ok()
    }
}

#[cfg(feature = "config-store")]
unsafe fn submit_selected_add_files(hwnd: HWND, state: &WindowState) {
    // SAFETY: all controls and session state belong to this UI thread. The
    // picker runs without any RefCell borrow or pending configuration save.
    unsafe {
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        if list.is_null() {
            return;
        }
        let index = SendMessageW(list, LB_GETCURSEL, 0, 0);
        if index < 0 {
            return;
        }
        let group_id = {
            let session = state.session.borrow();
            let Some(session) = session.as_ref() else {
                return;
            };
            if !session.can_save() {
                return;
            }
            let Some(id) = session.group_id_at(index as usize) else {
                return;
            };
            id
        };
        let selected = match state.picker_override.borrow_mut().take() {
            Some(paths) => Ok(Some(paths)),
            None => choose_files(hwnd),
        };
        match selected {
            Ok(None) => return,
            Ok(Some(paths)) => {
                let targets = paths
                    .into_iter()
                    .map(|path| FileReference {
                        kind: reference_kind(&path),
                        path,
                    })
                    .collect();
                if let Some(session) = state.session.borrow_mut().as_mut() {
                    session.add_references(group_id, targets);
                }
            }
            Err(error) => {
                if let Some(session) = state.session.borrow_mut().as_mut() {
                    session.status = error;
                }
            }
        }
        refresh_session(hwnd, state);
    }
}

#[cfg(feature = "config-store")]
unsafe fn submit_selected_add_folder(hwnd: HWND, state: &WindowState) {
    // SAFETY: only the UI thread touches these HWNDs. The picker runs with no
    // Session borrow, then stores a reference without modifying the directory.
    unsafe {
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        if list.is_null() {
            return;
        }
        let index = SendMessageW(list, LB_GETCURSEL, 0, 0);
        if index < 0 {
            return;
        }
        let group_id = {
            let session = state.session.borrow();
            let Some(session) = session.as_ref() else {
                return;
            };
            if !session.can_save() {
                return;
            }
            let Some(id) = session.group_id_at(index as usize) else {
                return;
            };
            id
        };
        let selected = match state.folder_override.borrow_mut().take() {
            Some(path) => Ok(Some(path)),
            None => folder_picker::choose_folder(windows::Win32::Foundation::HWND(hwnd)),
        };
        match selected {
            Ok(None) => return,
            Ok(Some(path)) => {
                if let Some(session) = state.session.borrow_mut().as_mut() {
                    session.add_references(
                        group_id,
                        vec![FileReference {
                            kind: ReferenceKind::Directory,
                            path,
                        }],
                    );
                }
            }
            Err(error) => {
                if let Some(session) = state.session.borrow_mut().as_mut() {
                    session.status = error;
                }
            }
        }
        refresh_session(hwnd, state);
    }
}

#[cfg(feature = "config-store")]
unsafe fn submit_selected_resize(hwnd: HWND, state: &WindowState) {
    unsafe {
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        let width_edit = GetDlgItem(hwnd, GROUP_WIDTH as i32);
        let height_edit = GetDlgItem(hwnd, GROUP_HEIGHT as i32);
        if list.is_null() || width_edit.is_null() || height_edit.is_null() {
            return;
        }
        let index = SendMessageW(list, LB_GETCURSEL, 0, 0);
        if index < 0 {
            return;
        }
        let Some(id) = state
            .session
            .borrow()
            .as_ref()
            .and_then(|s| s.group_id_at(index as usize))
        else {
            return;
        };
        match (read_size(width_edit), read_size(height_edit)) {
            (Some(width), Some(height)) => {
                if let Some(session) = state.session.borrow_mut().as_mut() {
                    session.resize_group(id, width, height);
                }
            }
            _ => {
                if let Some(session) = state.session.borrow_mut().as_mut() {
                    session.status = "无法缩放分组：宽度和高度必须是有效非负整数。".into();
                }
            }
        }
        refresh_session(hwnd, state);
    }
}

#[cfg(feature = "config-store")]
unsafe fn submit_selected_move(hwnd: HWND, state: &WindowState) {
    unsafe {
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        let x_edit = GetDlgItem(hwnd, GROUP_X as i32);
        let y_edit = GetDlgItem(hwnd, GROUP_Y as i32);
        if list.is_null() || x_edit.is_null() || y_edit.is_null() {
            return;
        }
        let index = SendMessageW(list, LB_GETCURSEL, 0, 0);
        if index < 0 {
            return;
        }
        let Some(id) = state
            .session
            .borrow()
            .as_ref()
            .and_then(|s| s.group_id_at(index as usize))
        else {
            return;
        };
        match (read_coordinate(x_edit), read_coordinate(y_edit)) {
            (Some(x), Some(y)) => {
                if let Some(session) = state.session.borrow_mut().as_mut() {
                    session.move_group(id, x, y);
                }
            }
            _ => {
                if let Some(session) = state.session.borrow_mut().as_mut() {
                    session.status = "无法移动分组：X/Y 坐标必须是有效整数。".into();
                }
            }
        }
        refresh_session(hwnd, state);
    }
}

#[cfg(feature = "config-store")]
unsafe fn submit_selected_rename(hwnd: HWND, state: &WindowState) {
    // SAFETY: reads only this parent's list/edit on the UI thread. The model borrow is
    // obtained after synchronous Win32 calls, so callbacks cannot re-enter a RefCell borrow.
    unsafe {
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        let edit = GetDlgItem(hwnd, GROUP_NAME as i32);
        if list.is_null() || edit.is_null() {
            return;
        }
        let index = SendMessageW(list, LB_GETCURSEL, 0, 0);
        if index < 0 {
            return;
        }
        let Some(id) = state
            .session
            .borrow()
            .as_ref()
            .and_then(|s| s.group_id_at(index as usize))
        else {
            return;
        };
        let length = GetWindowTextLengthW(edit);
        if !(0..=512).contains(&length) {
            return;
        }
        let mut units = vec![0_u16; length as usize + 1];
        let copied = GetWindowTextW(edit, units.as_mut_ptr(), units.len() as i32);
        let Ok(title) = String::from_utf16(&units[..copied as usize]) else {
            if let Some(session) = state.session.borrow_mut().as_mut() {
                session.status = "无法重命名分组：名称含无效 Unicode 字符。".into();
            }
            refresh_session(hwnd, state);
            return;
        };
        if let Some(session) = state.session.borrow_mut().as_mut() {
            session.rename_group(id, title);
        }
        refresh_session(hwnd, state);
    }
}

#[cfg(feature = "config-store")]
unsafe fn disarm_delete(hwnd: HWND, state: &WindowState) {
    unsafe {
        if state.armed_delete.take().is_some() {
            let button = GetDlgItem(hwnd, DELETE_GROUP_BUTTON as i32);
            if !button.is_null() {
                SetWindowTextW(button, windows_sys::core::w!("删除所选分组（仅配置）"));
            }
        }
        disarm_remove(hwnd, state);
    }
}

#[cfg(feature = "config-store")]
unsafe fn disarm_remove(hwnd: HWND, state: &WindowState) {
    unsafe {
        if state.armed_remove.take().is_some() {
            let button = GetDlgItem(hwnd, REMOVE_ITEM_BUTTON as i32);
            if !button.is_null() {
                SetWindowTextW(button, windows_sys::core::w!("移除选中引用（仅配置）"));
            }
        }
    }
}

#[cfg(feature = "config-store")]
unsafe fn cancel_confirmation(hwnd: HWND, state: &WindowState) {
    unsafe {
        disarm_delete(hwnd, state);
        if let Some(session) = state.session.borrow_mut().as_mut() {
            session.status = "已取消删除/移除确认；配置和源文件均未改变。".into();
        }
        refresh_session(hwnd, state);
    }
}

#[cfg(feature = "config-store")]
unsafe fn submit_selected_color(hwnd: HWND, state: &WindowState) {
    unsafe {
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        let edit = GetDlgItem(hwnd, GROUP_COLOR as i32);
        if list.is_null() || edit.is_null() {
            return;
        }
        let index = SendMessageW(list, LB_GETCURSEL, 0, 0);
        if index < 0 {
            return;
        }
        let Some(id) = state
            .session
            .borrow()
            .as_ref()
            .and_then(|s| s.group_id_at(index as usize))
        else {
            return;
        };
        let mut rgb = None;
        if GetWindowTextLengthW(edit) == 7 {
            let mut units = [0_u16; 8];
            let copied = GetWindowTextW(edit, units.as_mut_ptr(), units.len() as i32);
            if copied == 7
                && let Ok(value) = String::from_utf16(&units[..7])
                && let Some(hex) = value.strip_prefix('#')
                && hex.len() == 6
                && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                rgb = (|| {
                    Some([
                        u8::from_str_radix(&hex[0..2], 16).ok()?,
                        u8::from_str_radix(&hex[2..4], 16).ok()?,
                        u8::from_str_radix(&hex[4..6], 16).ok()?,
                    ])
                })();
            }
        }
        if let Some(rgb) = rgb {
            if let Some(session) = state.session.borrow_mut().as_mut() {
                session.set_group_color(id, rgb);
            }
        } else if let Some(session) = state.session.borrow_mut().as_mut() {
            session.status = "无法设置分组颜色：请输入 #RRGGBB 格式的十六进制颜色。".into();
        }
        refresh_session(hwnd, state);
    }
}

#[cfg(feature = "config-store")]
unsafe fn submit_selected_fold(hwnd: HWND, state: &WindowState) {
    unsafe {
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        if list.is_null() {
            return;
        }
        let index = SendMessageW(list, LB_GETCURSEL, 0, 0);
        if index < 0 {
            return;
        }
        let Some(id) = state
            .session
            .borrow()
            .as_ref()
            .and_then(|s| s.group_id_at(index as usize))
        else {
            return;
        };
        if let Some(session) = state.session.borrow_mut().as_mut() {
            session.toggle_group_collapsed(id);
        }
        refresh_session(hwnd, state);
    }
}

#[cfg(feature = "config-store")]
unsafe fn submit_selected_delete(hwnd: HWND, state: &WindowState) {
    unsafe {
        if !state
            .session
            .borrow()
            .as_ref()
            .is_some_and(Session::can_save)
        {
            disarm_delete(hwnd, state);
            return;
        }
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        if list.is_null() {
            return;
        }
        let index = SendMessageW(list, LB_GETCURSEL, 0, 0);
        if index < 0 {
            return;
        }
        let Some(id) = state
            .session
            .borrow()
            .as_ref()
            .and_then(|s| s.group_id_at(index as usize))
        else {
            return;
        };
        if state.armed_delete.get() == Some(id) {
            disarm_delete(hwnd, state);
            if let Some(session) = state.session.borrow_mut().as_mut() {
                session.delete_group(id);
            }
        } else {
            state.armed_delete.set(Some(id));
            let button = GetDlgItem(hwnd, DELETE_GROUP_BUTTON as i32);
            if !button.is_null() {
                SetWindowTextW(button, windows_sys::core::w!("再次点击确认删除"));
            }
            if let Some(session) = state.session.borrow_mut().as_mut() {
                session.status =
                    "仅删除软件配置中的分组及引用；再次点击确认，不会删除源文件。".into();
            }
        }
        refresh_session(hwnd, state);
    }
}

#[cfg(feature = "config-store")]
unsafe fn submit_selected_remove(hwnd: HWND, state: &WindowState) {
    unsafe {
        if !state
            .session
            .borrow()
            .as_ref()
            .is_some_and(Session::can_save)
        {
            disarm_delete(hwnd, state);
            return;
        }
        let Some((group_id, item_id)) = state.selected_item.get() else {
            return;
        };
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        let index = if list.is_null() {
            -1
        } else {
            SendMessageW(list, LB_GETCURSEL, 0, 0)
        };
        let valid = index >= 0
            && state.session.borrow().as_ref().is_some_and(|session| {
                session.group_id_at(index as usize) == Some(group_id)
                    && session.contains_reference_id(group_id, item_id)
            });
        if !valid {
            disarm_delete(hwnd, state);
            state.selected_item.set(None);
            refresh_session(hwnd, state);
            return;
        }
        if state.armed_remove.get() == Some((group_id, item_id)) {
            disarm_remove(hwnd, state);
            let submitted = state
                .session
                .borrow_mut()
                .as_mut()
                .is_some_and(|session| session.remove_reference(group_id, item_id));
            if submitted {
                state.selected_item.set(None);
            }
        } else {
            disarm_delete(hwnd, state);
            state.armed_remove.set(Some((group_id, item_id)));
            let button = GetDlgItem(hwnd, REMOVE_ITEM_BUTTON as i32);
            if !button.is_null() {
                SetWindowTextW(button, windows_sys::core::w!("再次点击确认移除"));
            }
            if let Some(session) = state.session.borrow_mut().as_mut() {
                session.status =
                    "仅移除软件配置中的这一条引用；再次点击确认，源文件不会被删除。".into();
            }
        }
        refresh_session(hwnd, state);
    }
}

#[cfg(feature = "config-store")]
fn point_from_lparam(value: LPARAM) -> POINT {
    POINT {
        x: (value as u16 as i16) as i32,
        y: ((value >> 16) as u16 as i16) as i32,
    }
}

#[cfg(feature = "config-store")]
fn point_lparam(point: POINT) -> LPARAM {
    ((point.y as u16 as usize) << 16 | point.x as u16 as usize) as LPARAM
}

#[cfg(feature = "config-store")]
unsafe fn preview_layout(hwnd: HWND, dpi: u32, groups: &[GroupPreview]) -> Option<PreviewLayout> {
    unsafe {
        let mut client = RECT::default();
        if GetClientRect(hwnd, &mut client) == 0 {
            return None;
        }
        let view = RectPx {
            left: px(550, dpi),
            top: px(480, dpi),
            right: client.right - px(48, dpi),
            bottom: client.bottom - px(80, dpi),
        };
        let rects: Vec<_> = groups.iter().map(|group| group.rect).collect();
        PreviewLayout::new(view, dpi, &rects)
    }
}

#[cfg(feature = "config-store")]
fn preview_group_rect(layout: PreviewLayout, group: &GroupPreview, dpi: u32) -> RectPx {
    let mut rect = layout.group(group.rect);
    if group.collapsed {
        rect.bottom = rect.top + px(30, dpi);
    }
    rect
}

#[cfg(all(test, feature = "config-store"))]
mod fold_preview_tests {
    use super::*;

    #[test]
    fn collapsed_hit_region_is_header_only_without_changing_saved_height() {
        let group = GroupPreview {
            id: GroupId::new(1).unwrap(),
            title: "测试".into(),
            rect: RectDip {
                x: 0,
                y: 0,
                width: 320,
                height: 220,
            },
            color: [62, 110, 180],
            collapsed: false,
        };
        let view = RectPx {
            left: 550,
            top: 350,
            right: 1050,
            bottom: 700,
        };
        let layout = PreviewLayout::new(view, 96, &[group.rect]).unwrap();
        let expanded = preview_group_rect(layout, &group, 96);
        let folded = preview_group_rect(
            layout,
            &GroupPreview {
                collapsed: true,
                ..group
            },
            96,
        );
        assert_eq!(folded.bottom - folded.top, 30);
        assert!(expanded.bottom > folded.bottom);
        assert!(!folded.contains(folded.left + 5, folded.bottom + 5));
    }
}

#[cfg(feature = "config-store")]
unsafe fn preview_item_at(
    hwnd: HWND,
    state: &WindowState,
    point: POINT,
) -> Option<(GroupId, ItemId)> {
    // SAFETY: geometry and immutable session snapshots belong to this HWND.
    unsafe {
        let groups = state.session.borrow().as_ref()?.group_previews();
        let dpi = GetDpiForWindow(hwnd).max(96);
        let layout = preview_layout(hwnd, dpi, &groups)?;
        if !layout.view.contains(point.x, point.y) {
            return None;
        }
        // Only the topmost hit group may respond; never open a covered item.
        let group = groups
            .iter()
            .rev()
            .find(|group| preview_group_rect(layout, group, dpi).contains(point.x, point.y))?;
        if group.collapsed {
            return None;
        }
        let slots = item_content(preview_group_rect(layout, group, dpi), dpi)
            .map_or_else(Vec::new, |content| item_slots(content, dpi, 24));
        let items = state
            .session
            .borrow()
            .as_ref()?
            .preview_items(group.id, slots.len())
            .1;
        slots
            .iter()
            .zip(items.iter())
            .find(|(slot, _)| slot.contains(point.x, point.y))
            .map(|(_, item)| (group.id, item.id))
    }
}

#[cfg(feature = "config-store")]
unsafe fn open_selected_reference(hwnd: HWND, state: &WindowState) {
    // SAFETY: same UI-thread state/window preconditions as the shared helper.
    unsafe {
        activate_selected_reference(hwnd, state, open_policy::OpenAction::Open);
    }
}

#[cfg(feature = "config-store")]
unsafe fn activate_selected_reference(
    hwnd: HWND,
    state: &WindowState,
    action: open_policy::OpenAction,
) {
    // No filesystem or Shell work is executed while holding UI state borrows.
    if state.item_drag.get().is_some_and(|drag| drag.active) {
        // Enter cannot both cancel a pointer transfer and launch its source.
        unsafe {
            cancel_drag(hwnd, state);
            refresh_session(hwnd, state);
        }
        return;
    }
    let reference = state.selected_item.get().and_then(|(group, item)| {
        state
            .session
            .borrow()
            .as_ref()?
            .reference_for_item(group, item)
    });
    let Some(reference) = reference else {
        return;
    };
    let status = if !state.open_allowed.get() {
        "隐藏验收窗口不执行系统打开操作。".to_owned()
    } else {
        match state
            .icons
            .borrow_mut()
            .as_mut()
            .map(|worker| worker.request_open(reference, action))
        {
            Some(Ok(())) => match action {
                open_policy::OpenAction::Open => "正在请求系统打开；界面无需等待。",
                open_policy::OpenAction::Reveal => "正在请求 Explorer 定位引用本身；界面无需等待。",
            }
            .to_owned(),
            Some(Err(OpenQueueError::Busy)) => {
                "打开服务正忙，请稍后再试；没有重复提交。".to_owned()
            }
            Some(Err(OpenQueueError::Unavailable)) | None => {
                "系统打开服务不可用；引用仍保留。".to_owned()
            }
        }
    };
    if let Some(session) = state.session.borrow_mut().as_mut() {
        session.status = status;
    }
    // SAFETY: native refresh occurs after releasing every RefCell borrow.
    unsafe {
        refresh_session(hwnd, state);
    }
}

#[cfg(feature = "config-store")]
unsafe fn begin_drag(hwnd: HWND, state: &WindowState, point: POINT) {
    unsafe {
        if !state
            .session
            .borrow()
            .as_ref()
            .is_some_and(Session::can_save)
        {
            return;
        }
        let groups = state
            .session
            .borrow()
            .as_ref()
            .map(Session::group_previews)
            .unwrap_or_default();
        let Some(layout) = preview_layout(hwnd, GetDpiForWindow(hwnd).max(96), &groups) else {
            return;
        };
        if !layout.view.contains(point.x, point.y) {
            return;
        }
        let dpi = GetDpiForWindow(hwnd).max(96);
        let Some((index, group)) =
            groups.iter().enumerate().rev().find(|(_, group)| {
                preview_group_rect(layout, group, dpi).contains(point.x, point.y)
            })
        else {
            return;
        };
        let group_box = preview_group_rect(layout, group, dpi);
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        if !list.is_null() {
            SendMessageW(list, LB_SETCURSEL, index, 0);
            sync_group_name(hwnd, state);
            sync_group_position_if_changed(hwnd, state);
        }
        if !group.collapsed {
            let slots = item_content(group_box, dpi)
                .map_or_else(Vec::new, |content| item_slots(content, dpi, 24));
            let items = state
                .session
                .borrow()
                .as_ref()
                .map(|session| session.preview_items(group.id, slots.len()).1)
                .unwrap_or_default();
            if let Some((_, item)) = slots
                .iter()
                .zip(items.iter())
                .find(|(slot, _)| slot.contains(point.x, point.y))
            {
                state.selected_item.set(Some((group.id, item.id)));
                state.item_drag.set(Some(item_drag::ItemDrag::new(
                    group.id, item.id, point.x, point.y,
                )));
                SetCapture(hwnd);
                if GetCapture() != hwnd {
                    state.item_drag.set(None);
                }
                if let Some(session) = state.session.borrow_mut().as_mut() {
                    session.status = format!(
                        "已选择引用 {}；双击或 Enter 打开，移除仅修改配置。",
                        item.label
                    );
                }
                refresh_session(hwnd, state);
                return;
            }
        }
        state.selected_item.set(None);
        let handle_size = px(14, dpi);
        let kind = if !group.collapsed
            && point.x >= group_box.right - handle_size
            && point.y >= group_box.bottom - handle_size
        {
            DragKind::Resize
        } else {
            DragKind::Move
        };
        state.drag.set(Some(DragState {
            id: group.id,
            kind,
            start: point,
            last: point,
            original: group.rect,
            layout,
        }));
        SetCapture(hwnd);
        refresh_session(hwnd, state);
    }
}

#[cfg(feature = "config-store")]
unsafe fn cancel_drag(hwnd: HWND, state: &WindowState) {
    unsafe {
        let item_drag = state.item_drag.take();
        state.item_drag_hover.set(None);
        if state.drag.take().is_some() || item_drag.is_some() {
            if GetCapture() == hwnd {
                ReleaseCapture();
            }
            if item_drag.is_some_and(|drag| drag.active)
                && let Some(session) = state.session.borrow_mut().as_mut()
            {
                session.status = "已取消组间拖动；引用和源文件未改动。".into();
            }
            InvalidateRect(hwnd, null(), 0);
        }
    }
}

#[cfg(feature = "config-store")]
unsafe fn item_drag_destination(hwnd: HWND, state: &WindowState, point: POINT) -> Option<GroupId> {
    // SAFETY: read-only UI-thread geometry; owned snapshots release Session borrows.
    unsafe {
        let groups = state.session.borrow().as_ref()?.group_previews();
        let dpi = GetDpiForWindow(hwnd).max(96);
        let layout = preview_layout(hwnd, dpi, &groups)?;
        if !layout.view.contains(point.x, point.y) {
            return None;
        }
        groups
            .iter()
            .rev()
            .find(|group| preview_group_rect(layout, group, dpi).contains(point.x, point.y))
            .map(|group| group.id)
    }
}

#[cfg(feature = "config-store")]
unsafe fn update_item_drag(hwnd: HWND, state: &WindowState, point: POINT) {
    // SAFETY: only owning UI thread updates capture and drag state.
    unsafe {
        let Some(mut drag) = state.item_drag.get() else {
            return;
        };
        let started = drag.update(point.x, point.y, GetDpiForWindow(hwnd).max(96));
        let destination = if drag.active {
            drag.destination(item_drag_destination(hwnd, state, point))
        } else {
            None
        };
        let changed = state.item_drag_hover.replace(destination) != destination;
        state.item_drag.set(Some(drag));
        if started && let Some(session) = state.session.borrow_mut().as_mut() {
            session.status = "正在拖动软件引用；松到另一分组转移，Esc 取消，源文件不移动。".into();
        }
        if started || changed {
            InvalidateRect(hwnd, null(), 0);
        }
    }
}

#[cfg(feature = "config-store")]
unsafe fn finish_item_drag(hwnd: HWND, state: &WindowState, point: POINT) -> bool {
    // SAFETY: take drag before releasing capture; reentrant capture-change cannot commit it twice.
    unsafe {
        let Some(mut drag) = state.item_drag.take() else {
            return false;
        };
        state.item_drag_hover.set(None);
        if GetCapture() == hwnd {
            ReleaseCapture();
        }
        drag.update(point.x, point.y, GetDpiForWindow(hwnd).max(96));
        if drag.active {
            let destination = drag.destination(item_drag_destination(hwnd, state, point));
            let accepted = if let Some(destination) = destination
                && let Some(session) = state.session.borrow_mut().as_mut()
            {
                session.transfer_reference(drag.source, drag.item, destination)
            } else {
                false
            };
            if accepted {
                state.selected_item.set(None);
            } else if destination.is_none()
                && let Some(session) = state.session.borrow_mut().as_mut()
            {
                session.status = "未移入其它分组；引用仍在原组，源文件未改动。".into();
            }
            refresh_session(hwnd, state);
        }
        true
    }
}

#[cfg(feature = "config-store")]
unsafe fn finish_drag(hwnd: HWND, state: &WindowState, point: POINT) {
    unsafe {
        let Some(drag) = state.drag.take() else {
            return;
        };
        if GetCapture() == hwnd {
            ReleaseCapture();
        }
        let Some((dx, dy)) = drag
            .layout
            .delta_dip(point.x - drag.start.x, point.y - drag.start.y)
        else {
            return;
        };
        if dx == 0 && dy == 0 {
            InvalidateRect(hwnd, null(), 0);
            return;
        }
        match drag.kind {
            DragKind::Move => {
                if let (Some(x), Some(y)) = (
                    drag.original.x.checked_add(dx),
                    drag.original.y.checked_add(dy),
                ) {
                    if let Some(session) = state.session.borrow_mut().as_mut() {
                        session.move_group(drag.id, x, y);
                    }
                } else if let Some(session) = state.session.borrow_mut().as_mut() {
                    session.status = "无法移动分组：目标坐标超出整数范围。".into();
                }
            }
            DragKind::Resize => {
                let width = u32::try_from(i64::from(drag.original.width) + i64::from(dx));
                let height = u32::try_from(i64::from(drag.original.height) + i64::from(dy));
                if let (Ok(width), Ok(height)) = (width, height) {
                    if let Some(session) = state.session.borrow_mut().as_mut() {
                        session.resize_group(drag.id, width, height);
                    }
                } else if let Some(session) = state.session.borrow_mut().as_mut() {
                    session.status = "无法缩放分组：目标尺寸超出整数范围。".into();
                }
            }
        }
        refresh_session(hwnd, state);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Windows invokes this on the owning thread. WM_NCCREATE receives a live
    // CREATESTRUCTW; its state pointer is kept alive by run() until after HWND destruction.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let state = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WindowState).as_ref();
        #[cfg(feature = "ui-acceptance")]
        if let Some(state) = state {
            state.diagnostics.record_message(message);
            if state.diagnostics.is_enabled() {
                state
                    .diagnostics
                    .record_origin(message, InSendMessageEx(null_mut()) != ISMEX_NOSEND);
            }
        }
        match message {
            #[cfg(feature = "config-store")]
            WM_ACTIVATEAPP if wparam != 0 => {
                // Returning from Explorer is a natural point to surface any
                // background-checked path changes, even after DWM occlusion.
                InvalidateRect(hwnd, null(), 0);
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            #[cfg(feature = "config-store")]
            WM_ACTIVATE if wparam & 0xffff == WA_INACTIVE as usize => {
                // Never restore a pending button focus after the user left this window.
                if let Some(state) = state {
                    cancel_drag(hwnd, state);
                    state.restore_save_focus.set(false);
                    state.restore_group_focus.set(false);
                    state.restore_rename_focus.set(false);
                    state.restore_move_focus.set(false);
                    state.restore_resize_focus.set(false);
                    state.restore_delete_focus.set(false);
                    state.restore_remove_focus.set(false);
                    state.restore_undo_focus.set(false);
                    state.restore_sort_earlier_focus.set(false);
                    state.restore_sort_later_focus.set(false);
                    state.restore_reveal_focus.set(false);
                    state.restore_transfer_combo_focus.set(false);
                    state.restore_transfer_focus.set(false);
                    state.restore_color_focus.set(false);
                    state.restore_fold_focus.set(false);
                    disarm_delete(hwnd, state);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_PAINT => {
                paint(hwnd);
                if let Some(state) = state {
                    #[cfg(feature = "ui-acceptance")]
                    state.diagnostics.record(LoopEvent::Paint);
                    state.paints.set(state.paints.get().saturating_add(1));
                }
                0
            }
            WM_ERASEBKGND => 1, // paint() fills the complete invalidated client background.
            WM_SIZE => {
                if let Some(state) = state {
                    state.resizes.set(state.resizes.get().saturating_add(1));
                }
                #[cfg(feature = "config-store")]
                {
                    if let Some(state) = state {
                        cancel_drag(hwnd, state);
                    }
                    let button = GetDlgItem(hwnd, SAVE_BUTTON as i32);
                    if !button.is_null() {
                        let dpi = GetDpiForWindow(hwnd).max(96);
                        SetWindowPos(
                            button,
                            null_mut(),
                            px(48, dpi),
                            px(310, dpi),
                            px(220, dpi),
                            px(34, dpi),
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                    let create = GetDlgItem(hwnd, CREATE_GROUP_BUTTON as i32);
                    if !create.is_null() {
                        let dpi = GetDpiForWindow(hwnd).max(96);
                        SetWindowPos(
                            create,
                            null_mut(),
                            px(285, dpi),
                            px(310, dpi),
                            px(220, dpi),
                            px(34, dpi),
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                    position_group_controls(hwnd, GetDpiForWindow(hwnd).max(96));
                }
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lparam as *mut MINMAXINFO);
                let dpi = GetDpiForWindow(hwnd).max(96);
                info.ptMinTrackSize = POINT {
                    x: px(900, dpi),
                    y: px(850, dpi),
                };
                0
            }
            WM_DPICHANGED => {
                #[cfg(feature = "config-store")]
                if let Some(state) = state {
                    cancel_drag(hwnd, state);
                }
                let rect = &*(lparam as *const RECT);
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                InvalidateRect(hwnd, null(), 0);
                0
            }
            #[cfg(feature = "config-store")]
            WM_LBUTTONDOWN => {
                if let Some(state) = state {
                    disarm_delete(hwnd, state);
                    SetFocus(hwnd);
                    cancel_drag(hwnd, state);
                    begin_drag(hwnd, state, point_from_lparam(lparam));
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_LBUTTONDBLCLK => {
                if let Some(state) = state
                    && let Some(target) = preview_item_at(hwnd, state, point_from_lparam(lparam))
                {
                    cancel_drag(hwnd, state);
                    disarm_delete(hwnd, state);
                    state.selected_item.set(Some(target));
                    SetFocus(hwnd);
                    open_selected_reference(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_KEYDOWN if wparam == 0x0D && GetFocus() == hwnd => {
                if let Some(state) = state {
                    open_selected_reference(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_MOUSEMOVE => {
                if let Some(state) = state {
                    update_item_drag(hwnd, state, point_from_lparam(lparam));
                }
                if let Some(state) = state
                    && let Some(mut drag) = state.drag.get()
                {
                    drag.last = point_from_lparam(lparam);
                    state.drag.set(Some(drag));
                    InvalidateRect(hwnd, null(), 0);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_LBUTTONUP => {
                if let Some(state) = state
                    && !finish_item_drag(hwnd, state, point_from_lparam(lparam))
                {
                    finish_drag(hwnd, state, point_from_lparam(lparam));
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_CAPTURECHANGED => {
                if let Some(state) = state {
                    cancel_drag(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_CANCELMODE => {
                if let Some(state) = state {
                    cancel_drag(hwnd, state);
                }
                0
            }
            WM_KEYDOWN if wparam == 0x1B => {
                #[cfg(feature = "config-store")]
                if let Some(state) = state
                    && (state.drag.get().is_some() || state.item_drag.get().is_some())
                {
                    cancel_drag(hwnd, state);
                    return 0;
                }
                #[cfg(feature = "config-store")]
                if let Some(state) = state
                    && (state.armed_delete.get().is_some() || state.armed_remove.get().is_some())
                {
                    cancel_confirmation(hwnd, state);
                    return 0;
                }
                SendMessageW(hwnd, WM_CLOSE, 0, 0);
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if wparam & 0xffff == SAVE_BUTTON => {
                if let Some(state) = state {
                    cancel_drag(hwnd, state);
                    disarm_delete(hwnd, state);
                    if let Some(session) = state.session.borrow_mut().as_mut() {
                        session.save();
                    }
                    refresh_session(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if wparam & 0xffff == CREATE_GROUP_BUTTON => {
                if let Some(state) = state {
                    cancel_drag(hwnd, state);
                    disarm_delete(hwnd, state);
                    if let Some(session) = state.session.borrow_mut().as_mut() {
                        session.create_group();
                    }
                    refresh_session(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND
                if wparam & 0xffff == GROUP_LIST
                    && (wparam >> 16) & 0xffff == LBN_SELCHANGE as usize =>
            {
                if let Some(state) = state {
                    disarm_delete(hwnd, state);
                    state.selected_item.set(None);
                    sync_group_name(hwnd, state);
                    sync_group_position_if_changed(hwnd, state);
                    sync_group_color_if_changed(hwnd, state);
                    sync_group_fold_if_changed(hwnd, state);
                    refresh_session(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if wparam & 0xffff == RENAME_GROUP_BUTTON => {
                if let Some(state) = state {
                    disarm_delete(hwnd, state);
                    submit_selected_rename(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if wparam & 0xffff == MOVE_GROUP_BUTTON => {
                if let Some(state) = state {
                    disarm_delete(hwnd, state);
                    submit_selected_move(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if wparam & 0xffff == RESIZE_GROUP_BUTTON => {
                if let Some(state) = state {
                    disarm_delete(hwnd, state);
                    submit_selected_resize(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if wparam & 0xffff == DELETE_GROUP_BUTTON => {
                if let Some(state) = state {
                    disarm_remove(hwnd, state);
                    submit_selected_delete(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if wparam & 0xffff == REMOVE_ITEM_BUTTON => {
                if let Some(state) = state {
                    submit_selected_remove(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if wparam & 0xffff == UNDO_REMOVAL_BUTTON => {
                if let Some(state) = state {
                    cancel_drag(hwnd, state);
                    disarm_delete(hwnd, state);
                    if let Some(session) = state.session.borrow_mut().as_mut() {
                        session.undo_removal();
                    }
                    refresh_session(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if wparam & 0xffff == REVEAL_ITEM_BUTTON => {
                if let Some(state) = state {
                    cancel_drag(hwnd, state);
                    disarm_delete(hwnd, state);
                    activate_selected_reference(hwnd, state, open_policy::OpenAction::Reveal);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND
                if wparam & 0xffff == TRANSFER_GROUP_COMBO
                    && (wparam >> 16) & 0xffff == CBN_SELCHANGE as usize =>
            {
                if let Some(state) = state {
                    let combo = GetDlgItem(hwnd, TRANSFER_GROUP_COMBO as i32);
                    let index = SendMessageW(combo, CB_GETCURSEL, 0, 0);
                    let target = (index >= 0)
                        .then(|| {
                            state
                                .session
                                .borrow()
                                .as_ref()
                                .and_then(|s| s.group_id_at(index as usize))
                        })
                        .flatten();
                    state.transfer_target.set(target);
                    disarm_delete(hwnd, state);
                    refresh_session(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if wparam & 0xffff == TRANSFER_ITEM_BUTTON => {
                if let Some(state) = state {
                    cancel_drag(hwnd, state);
                    disarm_delete(hwnd, state);
                    let transferred = if let Some((source, item)) = state.selected_item.get()
                        && let Some(target) = state.transfer_target.get()
                        && let Some(session) = state.session.borrow_mut().as_mut()
                    {
                        session.transfer_reference(source, item, target)
                    } else {
                        false
                    };
                    if transferred {
                        // Keep source group selected; no stale tile or optimistic destination focus.
                        state.selected_item.set(None);
                    }
                    refresh_session(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if matches!(wparam & 0xffff, SORT_EARLIER_BUTTON | SORT_LATER_BUTTON) => {
                if let Some(state) = state {
                    cancel_drag(hwnd, state);
                    disarm_delete(hwnd, state);
                    if let Some((group, item)) = state.selected_item.get()
                        && let Some(session) = state.session.borrow_mut().as_mut()
                    {
                        let step = if wparam & 0xffff == SORT_EARLIER_BUTTON {
                            -1
                        } else {
                            1
                        };
                        if let Some(index) = session.shifted_index(group, item, step) {
                            session.reorder_reference(group, item, index);
                        }
                    }
                    refresh_session(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if wparam & 0xffff == COLOR_GROUP_BUTTON => {
                if let Some(state) = state {
                    disarm_delete(hwnd, state);
                    submit_selected_color(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if wparam & 0xffff == FOLD_GROUP_BUTTON => {
                if let Some(state) = state {
                    disarm_delete(hwnd, state);
                    submit_selected_fold(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if wparam & 0xffff == ADD_FILES_BUTTON => {
                if let Some(state) = state {
                    disarm_delete(hwnd, state);
                    submit_selected_add_files(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_COMMAND if wparam & 0xffff == ADD_FOLDER_BUTTON => {
                if let Some(state) = state {
                    disarm_delete(hwnd, state);
                    submit_selected_add_folder(hwnd, state);
                }
                0
            }
            #[cfg(feature = "config-store")]
            WM_TIMER if wparam == 1 => {
                // Hidden diagnostic watchdog; normal UI uses timer 2 instead.
                KillTimer(hwnd, 1);
                DestroyWindow(hwnd);
                0
            }
            #[cfg(feature = "config-store")]
            WM_TIMER if wparam == 2 => {
                #[cfg(feature = "ui-acceptance")]
                if let Some(state) = state {
                    state.diagnostics.record(LoopEvent::RefreshTimer);
                }
                // Cached path status is refreshed by the icon worker only for
                // entries drawn in the visible preview, never on the UI thread.
                InvalidateRect(hwnd, null(), 0);
                0
            }
            #[cfg(feature = "ui-acceptance")]
            WM_TIMER if wparam == 3 => {
                if let Some(state) = state {
                    state.diagnostic_timeout_fired.set(true);
                }
                KillTimer(hwnd, 3);
                // Diagnostic timeout follows the normal close/save acknowledgement
                // path, rather than forcibly destroying a live storage session.
                SendMessageW(hwnd, WM_CLOSE, 0, 0);
                0
            }
            WM_CLOSE => {
                #[cfg(feature = "ui-acceptance")]
                if let Some(state) = state {
                    state
                        .closing_visibility
                        .set(Some((IsWindowVisible(hwnd) != 0, IsIconic(hwnd) != 0)));
                }
                #[cfg(feature = "config-store")]
                if let Some(state) = state {
                    cancel_drag(hwnd, state);
                    disarm_delete(hwnd, state);
                    let active = state.session.borrow().is_some();
                    if active {
                        state.session.borrow_mut().as_mut().unwrap().close();
                        refresh_session(hwnd, state);
                        if state
                            .session
                            .borrow()
                            .as_ref()
                            .is_some_and(Session::should_destroy)
                        {
                            DestroyWindow(hwnd);
                        }
                        return 0;
                    }
                }
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                #[cfg(feature = "config-store")]
                if let Some(worker) = state.and_then(|state| state.icons.borrow_mut().take()) {
                    // Dropping the shared Shell worker cancels requests not yet entered.
                    drop(worker);
                }
                #[cfg(feature = "config-store")]
                if let Some(state) = state
                    && state.drop_registered.replace(false)
                {
                    // Revoke while HWND is valid; the owning guard later
                    // releases its COM reference and apartment.
                    let _ = windows::Win32::System::Ole::RevokeDragDrop(
                        windows::Win32::Foundation::HWND(hwnd),
                    );
                }
                if let Some(state) = state {
                    state.destroyed.set(true);
                }
                PostQuitMessage(0);
                0
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

unsafe fn paint(hwnd: HWND) {
    // SAFETY: BeginPaint/EndPaint are paired in this callback. HDC is borrowed;
    // owned GDI objects are deselected before deletion and stock objects are never deleted.
    unsafe {
        let labels = [
            "P0-01  /  Windows 原生窗口验证".to_owned(),
            "轻量窗口骨架".to_owned(),
            "已实现窗口启动、按需绘制、缩放处理和正常退出。".to_owned(),
            "此版本还不支持分组、拖入文件或接管桌面图标。".to_owned(),
            "下一步：接入配置与分组操作。".to_owned(),
        ];
        #[cfg(feature = "config-store")]
        let labels = if let Some(state) =
            (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WindowState).as_ref()
            && let Some(session) = state.session.borrow().as_ref()
        {
            [
                "P2  /  分组与引用（普通窗口）".into(),
                "分组与配置".into(),
                session.status.clone(),
                session.details.clone(),
                session.diagnostic_hint().into(),
            ]
        } else {
            labels
        };
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        if !dc.is_null() {
            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect);
            fill(dc, &rect, color(242, 245, 249));
            let dpi = GetDpiForWindow(hwnd).max(96);
            let margin = px(28, dpi);
            let card = RECT {
                left: margin,
                top: px(108, dpi),
                right: rect.right - margin,
                bottom: rect.bottom - px(60, dpi),
            };
            fill(dc, &card, color(255, 255, 255));
            text(
                dc,
                dpi,
                28,
                24,
                rect.right - margin,
                38,
                26,
                color(23, 40, 63),
                "LiteFences",
            );
            text(
                dc,
                dpi,
                28,
                67,
                rect.right - margin,
                28,
                14,
                color(79, 96, 119),
                &labels[0],
            );
            text(
                dc,
                dpi,
                48,
                130,
                rect.right - margin * 2,
                36,
                19,
                color(23, 40, 63),
                &labels[1],
            );
            text(
                dc,
                dpi,
                48,
                183,
                rect.right - margin * 2,
                30,
                14,
                color(79, 96, 119),
                &labels[2],
            );
            text(
                dc,
                dpi,
                48,
                225,
                rect.right - margin * 2,
                30,
                14,
                color(79, 96, 119),
                &labels[3],
            );
            text(
                dc,
                dpi,
                48,
                267,
                rect.right - margin * 2,
                30,
                14,
                color(79, 96, 119),
                &labels[4],
            );
            #[cfg(feature = "config-store")]
            if let Some(state) =
                (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WindowState).as_ref()
            {
                draw_preview(dc, hwnd, dpi, state);
            }
            let footer = RECT {
                left: margin,
                top: rect.bottom - px(40, dpi),
                right: rect.right - margin,
                bottom: rect.bottom - px(12, dpi),
            };
            draw_text(
                dc,
                dpi,
                footer,
                12,
                color(79, 96, 119),
                "Rust + Win32  ·  空闲时等待消息  ·  Esc 退出",
            );
        }
        EndPaint(hwnd, &ps);
    }
}

#[cfg(feature = "config-store")]
unsafe fn draw_preview(dc: HDC, hwnd: HWND, dpi: u32, state: &WindowState) {
    unsafe {
        let groups = state
            .session
            .borrow()
            .as_ref()
            .map(Session::group_previews)
            .unwrap_or_default();
        let Some(layout) = preview_layout(hwnd, dpi, &groups) else {
            return;
        };
        let view = RECT {
            left: layout.view.left,
            top: layout.view.top,
            right: layout.view.right,
            bottom: layout.view.bottom,
        };
        let border = RECT {
            left: view.left - 1,
            top: view.top - 1,
            right: view.right + 1,
            bottom: view.bottom + 1,
        };
        fill(dc, &border, color(185, 199, 216));
        fill(dc, &view, color(246, 249, 253));
        let mut label = RECT {
            left: view.left,
            top: view.top - px(29, dpi),
            right: view.right,
            bottom: view.top - px(4, dpi),
        };
        let preview_label = if state.drop_unavailable.get() {
            "布局预览 · 系统拖入不可用，请使用选择器添加引用"
        } else {
            "布局预览 · 双击项目或 Enter 打开；拖入只添加引用"
        };
        draw_text(dc, dpi, label, 13, color(79, 96, 119), preview_label);
        let list = GetDlgItem(hwnd, GROUP_LIST as i32);
        let selected = if list.is_null() {
            -1
        } else {
            SendMessageW(list, LB_GETCURSEL, 0, 0)
        };
        let drag = state.drag.get();
        let saved_dc = SaveDC(dc);
        IntersectClipRect(dc, view.left, view.top, view.right, view.bottom);
        for (index, group) in groups.iter().enumerate() {
            let mut box_rect = preview_group_rect(layout, group, dpi);
            if let Some(active) = drag
                && active.id == group.id
            {
                box_rect = preview_group_rect(active.layout, group, dpi);
                let dx = active.last.x - active.start.x;
                let dy = active.last.y - active.start.y;
                match active.kind {
                    DragKind::Move => {
                        box_rect.left += dx;
                        box_rect.right += dx;
                        box_rect.top += dy;
                        box_rect.bottom += dy;
                    }
                    DragKind::Resize => {
                        box_rect.right = (box_rect.right + dx).max(box_rect.left + 5);
                        box_rect.bottom = (box_rect.bottom + dy).max(box_rect.top + 5);
                    }
                }
            }
            if box_rect.right <= view.left
                || box_rect.left >= view.right
                || box_rect.bottom <= view.top
                || box_rect.top >= view.bottom
            {
                continue;
            }
            let active = selected == index as isize;
            let accepting_drop = state.drop_hover.get() == Some(group.id)
                || state.item_drag_hover.get() == Some(group.id);
            fill(
                dc,
                &RECT {
                    left: box_rect.left,
                    top: box_rect.top,
                    right: box_rect.right,
                    bottom: box_rect.bottom,
                },
                if accepting_drop {
                    color(29, 148, 111)
                } else if active {
                    color(34, 103, 190)
                } else {
                    color(159, 177, 200)
                },
            );
            if box_rect.right - box_rect.left < 5 || box_rect.bottom - box_rect.top < 5 {
                continue;
            }
            let inner = RECT {
                left: box_rect.left + 2,
                top: box_rect.top + 2,
                right: box_rect.right - 2,
                bottom: box_rect.bottom - 2,
            };
            fill(dc, &inner, color(255, 255, 255));
            let header_h = px(27, dpi).min((inner.bottom - inner.top).max(0));
            let header = RECT {
                bottom: inner.top + header_h,
                ..inner
            };
            let tint = |component: u8| ((u16::from(component) + 255) / 2) as u8;
            fill(
                dc,
                &header,
                color(
                    tint(group.color[0]),
                    tint(group.color[1]),
                    tint(group.color[2]),
                ),
            );
            let slots = if group.collapsed {
                Vec::new()
            } else {
                item_content(box_rect, dpi)
                    .map_or_else(Vec::new, |content| item_slots(content, dpi, 24))
            };
            let (item_count, items) = state
                .session
                .borrow()
                .as_ref()
                .map(|session| session.preview_items(group.id, slots.len()))
                .unwrap_or_default();
            let tiles: Vec<_> = slots
                .into_iter()
                .zip(items.iter())
                .map(|(slot, item)| {
                    let icon = state
                        .icons
                        .borrow_mut()
                        .as_mut()
                        .map(|icons| icons.get_or_request(&item.path))
                        .unwrap_or_else(IconView::pending);
                    (slot, item, icon)
                })
                .collect();
            let invalid_visible = tiles
                .iter()
                .filter(|(_, item, icon)| icon.is_invalid(item.kind))
                .count();
            if box_rect.right - box_rect.left >= px(65, dpi) && header_h >= px(16, dpi) {
                label = RECT {
                    left: header.left + px(7, dpi),
                    top: header.top,
                    right: header.right - px(5, dpi),
                    bottom: header.bottom,
                };
                let title = if invalid_visible != 0 {
                    format!(
                        "{} · {} 项 · {} 失效",
                        group.title, item_count, invalid_visible
                    )
                } else if item_count == 0 {
                    group.title.clone()
                } else {
                    format!("{} · {} 项", group.title, item_count)
                };
                draw_text(dc, dpi, label, 12, color(23, 40, 63), &title);
            }
            for (slot, item, icon) in tiles {
                let selected = state.selected_item.get() == Some((group.id, item.id));
                draw_item_tile(dc, dpi, slot, item, icon, selected);
            }
            if !group.collapsed
                && box_rect.right - box_rect.left >= px(20, dpi)
                && box_rect.bottom - box_rect.top >= px(20, dpi)
            {
                let grip = px(10, dpi);
                fill(
                    dc,
                    &RECT {
                        left: box_rect.right - grip - 3,
                        top: box_rect.bottom - grip - 3,
                        right: box_rect.right - 3,
                        bottom: box_rect.bottom - 3,
                    },
                    color(48, 100, 169),
                );
            }
        }
        if saved_dc != 0 {
            RestoreDC(dc, saved_dc);
        }
    }
}

#[cfg(feature = "config-store")]
unsafe fn draw_item_tile(
    dc: HDC,
    dpi: u32,
    slot: RectPx,
    item: &ItemPreview,
    icon: IconView,
    selected: bool,
) {
    // SAFETY: all drawing uses the borrowed WM_PAINT DC and stack-owned rectangles.
    unsafe {
        if selected {
            fill(
                dc,
                &RECT {
                    left: slot.left,
                    top: slot.top,
                    right: slot.right,
                    bottom: slot.bottom,
                },
                color(47, 112, 190),
            );
            fill(
                dc,
                &RECT {
                    left: slot.left + 1,
                    top: slot.top + 1,
                    right: slot.right - 1,
                    bottom: slot.bottom - 1,
                },
                color(235, 244, 255),
            );
        }
        let invalid = icon.is_invalid(item.kind);
        let (badge_color, badge_letter) = match item.kind {
            ReferenceKind::File => (color(50, 113, 184), "F"),
            ReferenceKind::Directory => (color(181, 127, 38), "D"),
            ReferenceKind::Shortcut => (color(108, 86, 171), "L"),
        };
        let icon_rect = RECT {
            left: slot.left + px(11, dpi),
            top: slot.top + px(2, dpi),
            right: slot.left + px(31, dpi),
            bottom: slot.top + px(22, dpi),
        };
        let drawn = !invalid
            && icon.handle.is_some_and(|handle| {
                DrawIconEx(
                    dc,
                    icon_rect.left,
                    icon_rect.top,
                    handle,
                    icon_rect.right - icon_rect.left,
                    icon_rect.bottom - icon_rect.top,
                    0,
                    null_mut(),
                    DI_NORMAL,
                ) != 0
            });
        if !drawn {
            fill(
                dc,
                &icon_rect,
                if invalid {
                    color(185, 43, 43)
                } else {
                    badge_color
                },
            );
            draw_text(
                dc,
                dpi,
                RECT {
                    left: icon_rect.left + px(6, dpi),
                    top: icon_rect.top,
                    right: icon_rect.right,
                    bottom: icon_rect.bottom,
                },
                11,
                color(255, 255, 255),
                if invalid { "!" } else { badge_letter },
            );
        }
        draw_text(
            dc,
            dpi,
            RECT {
                left: slot.left,
                top: slot.top + px(23, dpi),
                right: slot.right,
                bottom: slot.bottom,
            },
            9,
            if invalid {
                color(165, 31, 31)
            } else {
                color(41, 55, 76)
            },
            &item.label,
        );
    }
}

unsafe fn fill(dc: HDC, rect: &RECT, value: COLORREF) {
    // SAFETY: local brush is owned, used synchronously and not selected into the DC.
    unsafe {
        let brush = CreateSolidBrush(value);
        if !brush.is_null() {
            let _owned = GdiObject(brush);
            FillRect(dc, rect, brush);
        }
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn text(
    dc: HDC,
    dpi: u32,
    x: i32,
    y: i32,
    right: i32,
    height: i32,
    font_size: i32,
    value: COLORREF,
    label: &str,
) {
    // SAFETY: forwards the borrowed paint HDC and client-space rectangle to draw_text.
    unsafe {
        draw_text(
            dc,
            dpi,
            RECT {
                left: px(x, dpi),
                top: px(y, dpi),
                right,
                bottom: px(y + height, dpi),
            },
            font_size,
            value,
            label,
        );
    }
}

unsafe fn draw_text(
    dc: HDC,
    dpi: u32,
    mut rect: RECT,
    font_size: i32,
    value: COLORREF,
    label: &str,
) {
    // SAFETY: font is restored before its guard drops. UTF-16 data lives through DrawTextW.
    unsafe {
        let font = CreateFontW(
            -px(font_size, dpi),
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            DEFAULT_PITCH as u32,
            windows_sys::core::w!("Microsoft YaHei UI"),
        );
        let owned = (!font.is_null()).then(|| GdiObject(font));
        let old = if owned.is_some() {
            SelectObject(dc, font)
        } else {
            null_mut()
        };
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, value);
        let label = wide(label);
        DrawTextW(
            dc,
            label.as_ptr(),
            (label.len() - 1) as i32,
            &mut rect,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        if !old.is_null() {
            SelectObject(dc, old);
        }
        drop(owned);
    }
}

pub fn show_error(message: &str) {
    let text = wide(&format!("原型启动失败：{message}"));
    // SAFETY: null-terminated UTF-16 buffers remain live during this synchronous call.
    unsafe {
        MessageBoxW(
            null_mut(),
            text.as_ptr(),
            windows_sys::core::w!("LiteFences"),
            MB_OK | MB_ICONERROR,
        );
    }
}
