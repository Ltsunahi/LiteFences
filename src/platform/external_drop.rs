//! OLE CF_HDROP ingestion for the ordinary preview window only.
//! Every reported effect passes the no-MOVE policy; no source file is edited.

use super::{
    DROP_REPORT, WindowState, drop_policy, preview_group_rect, preview_layout, refresh_session,
    wide,
};
use litefences::core::model::{FileReference, GroupId, MAX_PATH_UNITS, NativePath, ReferenceKind};
use std::{cell::Cell, collections::BTreeSet, path::Path};
use windows::{
    Win32::{
        Foundation::{HWND, POINTL},
        System::{
            Com::{
                CoTaskMemFree, DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, TYMED_HGLOBAL,
            },
            Ole::{
                CF_HDROP, DROPEFFECT, IDropTarget, IDropTarget_Impl, OleInitialize,
                OleUninitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
            },
            SystemServices::MODIFIERKEYS_FLAGS,
        },
        UI::Shell::{
            BHID_DataObject, Common::ITEMIDLIST, DragQueryFileW, HDROP, IShellItem,
            SHCreateItemFromParsingName, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
        },
    },
    core::{PCWSTR, Ref, implement},
};
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::Gdi::{InvalidateRect, ScreenToClient},
    Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, GetFileAttributesW, INVALID_FILE_ATTRIBUTES},
    UI::HiDpi::GetDpiForWindow,
    UI::WindowsAndMessaging::{GetDlgItem, GetWindowRect, GetWindowTextW, SetWindowTextW},
};

const MAX_DROP_PATHS: u32 = 256;

struct TestPidl(*mut ITEMIDLIST);
impl Drop for TestPidl {
    fn drop(&mut self) {
        // SAFETY: SHParseDisplayName returns a task-allocator-owned PIDL.
        if !self.0.is_null() {
            unsafe { CoTaskMemFree(Some(self.0.cast())) }
        }
    }
}

fn display_path(units: &[u16]) -> String {
    let mut chars = char::decode_utf16(units.iter().copied())
        .map(|value| value.unwrap_or(char::REPLACEMENT_CHARACTER))
        .filter(|ch| !ch.is_control());
    let mut display: String = chars.by_ref().take(90).collect();
    if chars.next().is_some() {
        display.push('…');
    }
    display
}

#[cfg(test)]
mod tests {
    use super::display_path;

    #[test]
    fn drop_report_path_is_bounded_and_cannot_add_lines() {
        let mut units: Vec<u16> = "C:\\safe\\".encode_utf16().collect();
        units.extend("x".repeat(200).encode_utf16());
        units.extend("\r\nspoof".encode_utf16());
        let shown = display_path(&units);
        assert!(shown.ends_with('…'));
        assert!(!shown.contains('\r'));
        assert!(!shown.contains('\n'));
        assert!(shown.chars().count() <= 91);
    }
}

pub struct Registration {
    hwnd: HWND,
    target: Option<IDropTarget>,
}

impl Registration {
    pub unsafe fn new(
        hwnd: windows_sys::Win32::Foundation::HWND,
        state: &WindowState,
    ) -> Result<Self, String> {
        // SAFETY: called on the owning UI thread; guard revokes before state drops.
        unsafe {
            OleInitialize(None)
                .map_err(|error| format!("无法初始化拖入支持（代码 {}）。", error.code().0))?;
            let hwnd = HWND(hwnd);
            let target: IDropTarget = DropReceiver {
                hwnd,
                state: state as *const _,
                has_format: Cell::new(false),
            }
            .into();
            if let Err(error) = RegisterDragDrop(hwnd, &target) {
                OleUninitialize();
                return Err(format!("无法注册拖入目标（代码 {}）。", error.code().0));
            }
            Ok(Self {
                hwnd,
                target: Some(target),
            })
        }
    }

    pub unsafe fn exercise_drop(&self, source: &Path, point: POINTL) -> Result<(), String> {
        // SAFETY: diagnostic-only call on UI thread, with a path in its explicit
        // test directory. This invokes our target directly, never DoDragDrop.
        unsafe {
            let raw = source.to_string_lossy();
            let path = raw
                .strip_prefix(r"\\?\")
                .unwrap_or(raw.as_ref())
                .replace('/', "\\");
            let mut wide: Vec<u16> = path.encode_utf16().collect();
            wide.push(0);
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)
                .map_err(|error| format!("test Shell item failed: {}", error.code().0))?;
            let data: IDataObject = item
                .BindToHandler(None, &BHID_DataObject)
                .map_err(|error| format!("test data object failed: {}", error.code().0))?;
            self.exercise_data_drop(&data, point, 1)
        }
    }

    pub unsafe fn exercise_mixed_drop(
        &self,
        file: &Path,
        folder: &Path,
        point: POINTL,
    ) -> Result<(), String> {
        // SAFETY: the fixture's two PIDLs and Shell data object are created
        // on the registered OLE UI apartment and released on all exit paths.
        unsafe {
            let mut pidls = Vec::with_capacity(2);
            for source in [file, folder] {
                let raw = source.to_string_lossy();
                let path = raw
                    .strip_prefix(r"\\?\")
                    .unwrap_or(raw.as_ref())
                    .replace('/', "\\");
                let mut wide: Vec<u16> = path.encode_utf16().collect();
                wide.push(0);
                let mut pidl = std::ptr::null_mut();
                let parsed = SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None);
                let owned = TestPidl(pidl);
                parsed.map_err(|error| format!("test PIDL parse failed: {}", error.code().0))?;
                if pidl.is_null() {
                    return Err("test PIDL parse returned null".into());
                }
                pidls.push(owned);
            }
            let pointers: Vec<*const ITEMIDLIST> =
                pidls.iter().map(|pidl| pidl.0.cast_const()).collect();
            let items = SHCreateShellItemArrayFromIDLists(&pointers)
                .map_err(|error| format!("test item array failed: {}", error.code().0))?;
            let data: IDataObject = items
                .BindToHandler(None, &BHID_DataObject)
                .map_err(|error| format!("test mixed data object failed: {}", error.code().0))?;
            self.exercise_data_drop(&data, point, 2)
        }
    }

    unsafe fn exercise_data_drop(
        &self,
        data: &IDataObject,
        point: POINTL,
        expected_items: usize,
    ) -> Result<(), String> {
        // SAFETY: diagnostic-only calls invoke this target directly, never DoDragDrop.
        unsafe {
            let mut move_only = DROPEFFECT(drop_policy::DROPEFFECT_MOVE);
            self.target
                .as_ref()
                .expect("registered target")
                .DragEnter(data, MODIFIERKEYS_FLAGS(0), point, &mut move_only)
                .map_err(|error| format!("test move DragEnter failed: {}", error.code().0))?;
            if move_only.0 != drop_policy::DROPEFFECT_NONE {
                return Err("target advertised MOVE".into());
            }
            let mut move_drop = DROPEFFECT(drop_policy::DROPEFFECT_MOVE);
            self.target
                .as_ref()
                .expect("registered target")
                .Drop(data, MODIFIERKEYS_FLAGS(0), point, &mut move_drop)
                .map_err(|error| format!("test move Drop failed: {}", error.code().0))?;
            if move_drop.0 != drop_policy::DROPEFFECT_NONE {
                return Err("target completed MOVE".into());
            }
            let mut copy_only = DROPEFFECT(drop_policy::DROPEFFECT_COPY);
            self.target
                .as_ref()
                .expect("registered target")
                .DragEnter(data, MODIFIERKEYS_FLAGS(0), point, &mut copy_only)
                .map_err(|error| format!("test copy DragEnter failed: {}", error.code().0))?;
            if copy_only.0 != drop_policy::DROPEFFECT_COPY {
                return Err("target did not advertise COPY fallback".into());
            }
            let mut copy_over = DROPEFFECT(drop_policy::DROPEFFECT_COPY);
            self.target
                .as_ref()
                .expect("registered target")
                .DragOver(MODIFIERKEYS_FLAGS(0), point, &mut copy_over)
                .map_err(|error| format!("test copy DragOver failed: {}", error.code().0))?;
            if copy_over.0 != drop_policy::DROPEFFECT_COPY {
                return Err("target lost COPY while hovering a group".into());
            }
            let mut window_rect = RECT::default();
            if GetWindowRect(self.hwnd.0, &mut window_rect) == 0 {
                return Err("test window rectangle unavailable".into());
            }
            let outside_group = POINTL {
                x: window_rect.left.saturating_add(8),
                y: window_rect.top.saturating_add(8),
            };
            let mut copy_outside = DROPEFFECT(drop_policy::DROPEFFECT_COPY);
            self.target
                .as_ref()
                .expect("registered target")
                .DragOver(MODIFIERKEYS_FLAGS(0), outside_group, &mut copy_outside)
                .map_err(|error| format!("test outside DragOver failed: {}", error.code().0))?;
            if copy_outside.0 != drop_policy::DROPEFFECT_NONE {
                return Err("target still advertised COPY outside a group".into());
            }
            let mut copy_reenter = DROPEFFECT(drop_policy::DROPEFFECT_COPY);
            self.target
                .as_ref()
                .expect("registered target")
                .DragOver(MODIFIERKEYS_FLAGS(0), point, &mut copy_reenter)
                .map_err(|error| format!("test reenter DragOver failed: {}", error.code().0))?;
            if copy_reenter.0 != drop_policy::DROPEFFECT_COPY {
                return Err("target did not restore COPY on group reentry".into());
            }
            self.target
                .as_ref()
                .expect("registered target")
                .DragLeave()
                .map_err(|error| format!("test copy DragLeave failed: {}", error.code().0))?;
            let allowed = drop_policy::DROPEFFECT_LINK | drop_policy::DROPEFFECT_MOVE;
            let mut effect = DROPEFFECT(allowed);
            self.target
                .as_ref()
                .expect("registered target")
                .DragEnter(data, MODIFIERKEYS_FLAGS(0), point, &mut effect)
                .map_err(|error| format!("test link DragEnter failed: {}", error.code().0))?;
            if effect.0 != drop_policy::DROPEFFECT_LINK {
                return Err(format!("target did not advertise LINK: {}", effect.0));
            }
            let mut drop_effect = DROPEFFECT(allowed);
            self.target
                .as_ref()
                .expect("registered target")
                .Drop(data, MODIFIERKEYS_FLAGS(0), point, &mut drop_effect)
                .map_err(|error| format!("test Drop failed: {}", error.code().0))?;
            if drop_effect.0 != drop_policy::DROPEFFECT_LINK {
                return Err(format!("target did not complete LINK: {}", drop_effect.0));
            }
            let report = GetDlgItem(self.hwnd.0, DROP_REPORT as i32);
            let mut text = vec![0_u16; 512];
            let read = GetWindowTextW(report, text.as_mut_ptr(), text.len() as i32);
            let accepted = if read <= 0 {
                0
            } else {
                String::from_utf16_lossy(&text[..read as usize])
                    .matches("已提交保存：")
                    .count()
            };
            if accepted != expected_items {
                return Err(format!(
                    "drop report showed {accepted} accepted items, expected {expected_items}"
                ));
            }
            Ok(())
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: the UI thread drops this guard before destroying the window
        // or the state pointer retained by DropReceiver.
        unsafe {
            let _ = RevokeDragDrop(self.hwnd);
            self.target.take();
            OleUninitialize();
        }
    }
}

#[implement(IDropTarget)]
struct DropReceiver {
    hwnd: HWND,
    state: *const WindowState,
    has_format: Cell<bool>,
}

impl DropReceiver {
    fn state(&self) -> &WindowState {
        // SAFETY: Registration is dropped before its owning WindowState.
        unsafe { &*self.state }
    }

    fn format() -> FORMATETC {
        FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    fn target_group(&self, point: &POINTL) -> Option<GroupId> {
        // SAFETY: called on UI apartment, while Registration keeps HWND and
        // WindowState alive; native calls only query geometry.
        unsafe {
            let state = self.state();
            let session = state.session.try_borrow().ok()?;
            let session = session.as_ref()?;
            if !session.can_save() {
                return None;
            }
            let groups = session.group_previews();
            let hwnd = self.hwnd.0;
            let dpi = GetDpiForWindow(hwnd).max(96);
            let layout = preview_layout(hwnd, dpi, &groups)?;
            let mut local = windows_sys::Win32::Foundation::POINT {
                x: point.x,
                y: point.y,
            };
            if ScreenToClient(hwnd, &mut local) == 0 {
                return None;
            }
            groups
                .iter()
                .rev()
                .find(|group| preview_group_rect(layout, group, dpi).contains(local.x, local.y))
                .map(|group| group.id)
        }
    }

    fn offered_effect(&self, point: &POINTL, allowed: u32) -> u32 {
        let group = if self.has_format.get() {
            self.target_group(point)
        } else {
            None
        };
        let effect = drop_policy::choose_drop_effect(allowed, group.is_some());
        self.set_hover(if effect == drop_policy::DROPEFFECT_NONE {
            None
        } else {
            group
        });
        debug_assert_eq!(effect & drop_policy::DROPEFFECT_MOVE, 0);
        effect
    }

    fn set_hover(&self, group: Option<GroupId>) {
        let state = self.state();
        if state.drop_hover.replace(group) != group {
            // SAFETY: this HWND is owned by the registered UI thread.
            unsafe { InvalidateRect(self.hwnd.0, std::ptr::null(), 0) };
        }
    }

    fn set_status(&self, message: &str) {
        // SAFETY: COM invokes this on the registered UI apartment.
        unsafe {
            let state = self.state();
            if let Ok(mut session) = state.session.try_borrow_mut()
                && let Some(session) = session.as_mut()
            {
                session.status = message.into();
            }
            refresh_session(self.hwnd.0, state);
        }
    }

    fn set_report(&self, lines: &[String]) {
        // SAFETY: the report edit belongs to this UI window; SetWindowTextW
        // copies the bounded, UI-only text before the vector is dropped.
        unsafe {
            let edit = GetDlgItem(self.hwnd.0, DROP_REPORT as i32);
            if !edit.is_null() {
                SetWindowTextW(edit, wide(&lines.join("\r\n")).as_ptr());
            }
        }
    }

    fn extract_paths(
        &self,
        data: &IDataObject,
    ) -> Result<(Vec<FileReference>, Vec<String>), String> {
        // SAFETY: GetData owns the medium until ReleaseStgMedium. Querying file
        // attributes only occurs after the user drops a bounded local-path set.
        unsafe {
            let medium = data
                .GetData(&Self::format())
                .map_err(|_| "拖入对象未提供可读取的本地路径。".to_string())?;
            let medium = Medium(medium);
            if medium.0.tymed != TYMED_HGLOBAL.0 as u32 {
                return Err("拖入对象不是本地文件列表。".into());
            }
            let hdrop = HDROP(medium.0.u.hGlobal.0);
            if hdrop.0.is_null() {
                return Err("拖入对象的文件列表为空。".into());
            }
            let count = DragQueryFileW(hdrop, u32::MAX, None);
            if count == 0 || count > MAX_DROP_PATHS {
                return Err("拖入项目为空或超过每次 256 项上限，请分批添加。".into());
            }
            let mut references = Vec::with_capacity(count as usize);
            let mut report = Vec::with_capacity(count as usize);
            let mut seen = BTreeSet::new();
            for index in 0..count {
                let length = DragQueryFileW(hdrop, index, None) as usize;
                if length == 0 || length > MAX_PATH_UNITS {
                    report.push(format!("跳过项目 {}：路径为空或过长", index + 1));
                    continue;
                }
                let mut units = vec![0_u16; length + 1];
                if DragQueryFileW(hdrop, index, Some(&mut units)) as usize != length {
                    report.push(format!("跳过项目 {}：路径读取失败", index + 1));
                    continue;
                }
                let Ok(path) = NativePath::from_utf16(units[..length].to_vec()) else {
                    report.push(format!(
                        "跳过 {}：不是支持的本地规范路径",
                        display_path(&units[..length])
                    ));
                    continue;
                };
                if !seen.insert(path.clone()) {
                    report.push(format!(
                        "跳过 {}：本次拖入重复",
                        display_path(path.as_utf16())
                    ));
                    continue;
                }
                let mut terminated = path.as_utf16().to_vec();
                terminated.push(0);
                let attributes = GetFileAttributesW(terminated.as_ptr());
                if attributes == INVALID_FILE_ATTRIBUTES {
                    report.push(format!(
                        "跳过 {}：路径不可访问",
                        display_path(path.as_utf16())
                    ));
                    continue;
                }
                let kind = if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                    ReferenceKind::Directory
                } else if super::reference_kind(&path) == ReferenceKind::Shortcut {
                    ReferenceKind::Shortcut
                } else {
                    ReferenceKind::File
                };
                references.push(FileReference { kind, path });
            }
            // The medium guard is dropped only after all DragQueryFileW calls.
            Ok((references, report))
        }
    }
}

struct Medium(STGMEDIUM);
impl Drop for Medium {
    fn drop(&mut self) {
        // SAFETY: GetData returned this medium and transferred its ownership.
        unsafe { ReleaseStgMedium(&mut self.0) }
    }
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for DropReceiver_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        if effect.is_null() {
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                0x8007_0057_u32 as i32,
            )));
        }
        self.has_format.set(
            data.as_ref()
                .is_some_and(|data| unsafe { data.QueryGetData(&DropReceiver::format()).is_ok() }),
        );
        unsafe { (*effect).0 = self.offered_effect(point, (*effect).0) };
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        if effect.is_null() {
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                0x8007_0057_u32 as i32,
            )));
        }
        unsafe { (*effect).0 = self.offered_effect(point, (*effect).0) };
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        self.has_format.set(false);
        self.set_hover(None);
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        if effect.is_null() {
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                0x8007_0057_u32 as i32,
            )));
        }
        let offered = unsafe { (*effect).0 };
        unsafe { (*effect).0 = drop_policy::DROPEFFECT_NONE };
        self.set_hover(None);
        let Some(group_id) = self.target_group(point) else {
            self.has_format.set(false);
            return Ok(());
        };
        let safe_effect = self.offered_effect(point, offered);
        // DragOver may have highlighted a group. A completed or rejected drop
        // must not leave that hover border visible after the cursor leaves.
        self.set_hover(None);
        self.has_format.set(false);
        if safe_effect == drop_policy::DROPEFFECT_NONE {
            return Ok(());
        }
        let Some(data) = data.as_ref() else {
            return Ok(());
        };
        let (references, mut report) = match self.extract_paths(data) {
            Ok(value) => value,
            Err(message) => {
                self.set_report(std::slice::from_ref(&message));
                self.set_status(&message);
                return Ok(());
            }
        };
        let staged = {
            let state = self.state();
            let Ok(mut session_ref) = state.session.try_borrow_mut() else {
                return Ok(());
            };
            let Some(session) = session_ref.as_mut() else {
                return Ok(());
            };
            let mut accepted = Vec::new();
            let mut accepted_paths = Vec::new();
            for reference in references {
                if session.contains_reference(group_id, &reference.path) {
                    report.push(format!(
                        "跳过 {}：组内已存在",
                        display_path(reference.path.as_utf16())
                    ));
                } else {
                    accepted_paths.push(display_path(reference.path.as_utf16()));
                    accepted.push(reference);
                }
            }
            let staged = !accepted.is_empty() && session.add_references(group_id, accepted);
            let rejected = report.len();
            for path in accepted_paths {
                report.push(format!(
                    "{} {}",
                    if staged {
                        "已提交保存："
                    } else {
                        "未添加："
                    },
                    path
                ));
            }
            if staged {
                session.status = format!("已提交引用保存；{rejected} 项跳过，详见拖入结果。");
            } else if rejected != 0 {
                session.status = format!("拖入未添加引用；{rejected} 项跳过，详见拖入结果。");
            }
            staged
        };
        self.set_report(&report);
        // SAFETY: COM callback executes on the registered UI thread.
        unsafe { refresh_session(self.hwnd.0, self.state()) };
        if staged {
            unsafe { (*effect).0 = safe_effect };
        }
        Ok(())
    }
}
