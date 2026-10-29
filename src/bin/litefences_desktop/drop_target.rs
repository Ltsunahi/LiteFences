//! 从资源管理器把文件、文件夹、快捷方式拖进面板（OLE `IDropTarget`）。
//!
//! 语义与旧面板保持一致：**只加引用**——把路径记进面板（新项写到 `custom-icons.txt`），
//! 归到鼠标指针所在的分组；从不移动、复制或删除源文件，也永远不向源端报告 MOVE。
//!
//! 面板每次唤出都会重建窗口，所以拖放目标跟着窗口一起注册/注销。

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::PathBuf;

use windows::core::{Ref, implement};
use windows::Win32::Foundation::{HWND, POINTL};
use windows::Win32::System::Com::{
    DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, IDropTarget, IDropTarget_Impl, OleInitialize, RegisterDragDrop,
    ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

use crate::log;

/// 一次拖入最多接受的项数，超出部分请分批拖。
const MAX_PATHS: u32 = 256;
/// 路径长度上限（Win32 的 `\0` 结尾宽字符串按 32767 封顶）。
const MAX_PATH_UNITS: usize = 32_767;

const EFFECT_NONE: u32 = 0;
const EFFECT_COPY: u32 = 1;
const EFFECT_LINK: u32 = 4;

/// 只挑源端允许、而且不会让源端删除文件的效果：优先 LINK（加引用），其次 COPY。
fn choose_effect(allowed: u32, has_target: bool) -> u32 {
    if !has_target {
        return EFFECT_NONE;
    }
    if allowed & EFFECT_LINK != 0 {
        EFFECT_LINK
    } else if allowed & EFFECT_COPY != 0 {
        EFFECT_COPY
    } else {
        EFFECT_NONE
    }
}

thread_local! {
    /// 当前注册在浮层窗口上的拖放目标；窗口重建时要先注销旧的。
    static REGISTERED: RefCell<Option<(isize, IDropTarget)>> = const { RefCell::new(None) };
}

/// OLE 每个进程只初始化一次（面板窗口重建时不再重复计数）。
fn ensure_ole() {
    static READY: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    READY.get_or_init(|| {
        // SAFETY: 只在 UI 线程首次注册拖放时调用一次；失败只影响拖入。
        unsafe {
            let _ = OleInitialize(None);
        }
    });
}

/// 在浮层窗口上注册拖放接收。失败只影响拖入，其余功能照常。
pub fn attach(hwnd: windows_sys::Win32::Foundation::HWND) {
    detach();
    ensure_ole();
    // SAFETY: 只在 UI 线程调用；OLE 初始化失败时拖放不可用，不影响面板。
    unsafe {
        let target: IDropTarget = DropReceiver {
            hwnd: HWND(hwnd),
            has_paths: Cell::new(false),
            hover: Cell::new(None),
        }
        .into();
        match RegisterDragDrop(HWND(hwnd), &target) {
            Ok(()) => {
                REGISTERED.with(|slot| *slot.borrow_mut() = Some((hwnd as isize, target)));
                log::info("面板已注册拖放：可以把文件直接拖进分组");
            }
            Err(error) => log::warn(&format!("注册拖放失败（代码 {}）", error.code().0)),
        }
    }
}

/// 当前是否已经在这个窗口上注册好拖放接收（自检用）。
pub fn is_attached() -> bool {
    REGISTERED.with(|slot| slot.borrow().is_some())
}

/// 注销当前拖放目标；窗口销毁前必须调用。
pub fn detach() {
    let taken = REGISTERED.with(|slot| slot.borrow_mut().take());
    if let Some((hwnd, _target)) = taken {
        // SAFETY: 句柄是本模块上一次注册的浮层窗口。
        unsafe {
            let _ = RevokeDragDrop(HWND(hwnd as *mut c_void));
        }
    }
}

#[implement(IDropTarget)]
struct DropReceiver {
    hwnd: HWND,
    /// 本次拖入是否带本地文件列表（`CF_HDROP`）。
    has_paths: Cell<bool>,
    /// 上一次报告高亮的分组，用来避免重复刷新。
    hover: Cell<Option<usize>>,
}

impl DropReceiver {
    fn format() -> FORMATETC {
        FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    /// 屏幕坐标 → 面板客户区坐标。
    fn client_point(&self, point: &POINTL) -> Option<(i32, i32)> {
        let mut local = windows_sys::Win32::Foundation::POINT {
            x: point.x,
            y: point.y,
        };
        // SAFETY: 只做坐标换算，窗口句柄由注册方保证有效。
        let ok = unsafe {
            windows_sys::Win32::Graphics::Gdi::ScreenToClient(self.hwnd.0, &mut local)
        };
        (ok != 0).then_some((local.x, local.y))
    }

    fn group_at(&self, point: &POINTL) -> Option<usize> {
        let (x, y) = self.client_point(point)?;
        crate::overlay::group_at_point(x, y)
    }

    fn offered(&self, point: &POINTL, allowed: u32) -> u32 {
        let group = if self.has_paths.get() {
            self.group_at(point)
        } else {
            None
        };
        let effect = choose_effect(allowed, group.is_some());
        self.set_hover(if effect == EFFECT_NONE { None } else { group });
        effect
    }

    fn set_hover(&self, group: Option<usize>) {
        if self.hover.replace(group) != group {
            crate::overlay::set_drop_hover(group);
        }
    }

    /// 读出 `CF_HDROP` 里的本地路径；`Medium` 守卫负责释放 medium。
    fn extract_paths(&self, data: &IDataObject) -> Result<Vec<PathBuf>, String> {
        // SAFETY: GetData 返回的 medium 交给守卫释放；只读取文件列表，不碰文件。
        unsafe {
            let medium = data.GetData(&Self::format()).map_err(|_| {
                crate::i18n::t(
                    "拖入的对象里没有本地文件路径（只支持文件、文件夹、快捷方式）。",
                    "Nothing droppable here (files, folders and shortcuts only).",
                )
                .to_string()
            })?;
            let medium = Medium(medium);
            if medium.0.tymed != TYMED_HGLOBAL.0 as u32 {
                return Err(crate::i18n::t(
                    "拖入的对象不是本地文件列表。",
                    "Dropped item is not a local file list.",
                )
                .into());
            }
            let hdrop = HDROP(medium.0.u.hGlobal.0);
            if hdrop.0.is_null() {
                return Err(crate::i18n::t(
                    "拖入的文件列表为空。",
                    "Dropped file list is empty.",
                )
                .into());
            }
            let count = DragQueryFileW(hdrop, u32::MAX, None);
            if count == 0 {
                return Err(crate::i18n::t(
                    "拖入的文件列表为空。",
                    "Dropped file list is empty.",
                )
                .into());
            }
            let take = count.min(MAX_PATHS);
            let mut paths = Vec::with_capacity(take as usize);
            for index in 0..take {
                let length = DragQueryFileW(hdrop, index, None) as usize;
                if length == 0 || length > MAX_PATH_UNITS {
                    continue;
                }
                let mut units = vec![0u16; length + 1];
                if DragQueryFileW(hdrop, index, Some(&mut units)) as usize != length {
                    continue;
                }
                units.truncate(length);
                let raw = String::from_utf16_lossy(&units);
                let cleaned = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
                if cleaned.is_empty() {
                    continue;
                }
                paths.push(PathBuf::from(cleaned));
            }
            Ok(paths)
        }
    }
}

struct Medium(STGMEDIUM);
impl Drop for Medium {
    fn drop(&mut self) {
        // SAFETY: medium 由 GetData 交出所有权，只释放一次。
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
        let has_hdrop = data
            .as_ref()
            .is_some_and(|data| unsafe { data.QueryGetData(&DropReceiver::format()).is_ok() });
        self.has_paths.set(has_hdrop);
        // SAFETY: 指针非空（上面已检查），调用方负责缓冲区。
        unsafe { (*effect).0 = self.offered(point, (*effect).0) };
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
        // SAFETY: 同上。
        unsafe { (*effect).0 = self.offered(point, (*effect).0) };
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        self.has_paths.set(false);
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
        // SAFETY: 指针非空；读一次源端允许的效果，再默认改成“不接受”。
        let offered = unsafe { (*effect).0 };
        unsafe { (*effect).0 = EFFECT_NONE };
        self.has_paths.set(false);
        let group = self.group_at(point);
        let safe = match group {
            Some(_) => choose_effect(offered, true),
            None => EFFECT_NONE,
        };
        self.set_hover(None);
        // 只接受“加引用”的效果；MOVE-only 或指针不在任何分组上都直接忽略。
        if safe == EFFECT_NONE {
            return Ok(());
        }
        let Some(group) = group else {
            return Ok(());
        };
        let Some(data) = data.as_ref() else {
            return Ok(());
        };
        let paths = match self.extract_paths(data) {
            Ok(paths) => paths,
            Err(message) => {
                crate::overlay::show_drop_notice(message);
                return Ok(());
            }
        };
        if paths.is_empty() {
            crate::overlay::show_drop_notice(
                crate::i18n::t(
                    "拖入的项目不可用，没有加入面板。",
                    "Nothing usable was dropped; panel unchanged.",
                )
                .into(),
            );
            return Ok(());
        }
        crate::overlay::show_drop_notice(crate::app::drop_paths(paths, group));
        // SAFETY: 指针非空。
        unsafe { (*effect).0 = safe };
        Ok(())
    }
}


/// 排障入口：用真实 Shell 数据对象对自己的拖放目标投一次，验证“注册 + 提取路径 + 加引用”。
///
/// 只读取源文件的 Shell 元数据，不移动、复制或删除任何文件。
pub fn exercise_drop(sources: &[PathBuf], screen: (i32, i32)) -> Result<String, String> {
    let (raw_hwnd, target) = REGISTERED
        .with(|slot| slot.borrow().clone())
        .ok_or_else(|| "拖放目标没有注册".to_string())?;
    let hwnd: windows_sys::Win32::Foundation::HWND = raw_hwnd as *mut c_void;
    if sources.is_empty() {
        return Err("没有给出源路径".into());
    }
    // SAFETY: 路径都以 0 结尾；只创建 Shell 对象并投递消息，不读写文件内容。
    unsafe {
        let data: IDataObject = shell_data_object(sources)?;
        let point = POINTL {
            x: screen.0,
            y: screen.1,
        };
        let mut enter = DROPEFFECT(EFFECT_LINK | EFFECT_COPY);
        target
            .DragEnter(&data, MODIFIERKEYS_FLAGS(0), point, &mut enter)
            .map_err(|error| format!("DragEnter 失败：{}", error.code().0))?;
        let mut over = DROPEFFECT(EFFECT_LINK | EFFECT_COPY);
        target
            .DragOver(MODIFIERKEYS_FLAGS(0), point, &mut over)
            .map_err(|error| format!("DragOver 失败：{}", error.code().0))?;
        // 悬停高亮在拖拽过程中才看得到，测试时先抓一张留证。
        crate::overlay::force_repaint();
        let shot = crate::log::base_dir().join("drop-hover.bmp");
        if let Err(error) = crate::capture::capture_window(hwnd, &shot) {
            log::warn(&format!("抓取悬停画面失败：{error}"));
        }
        let mut drop = DROPEFFECT(EFFECT_LINK | EFFECT_COPY);
        target
            .Drop(&data, MODIFIERKEYS_FLAGS(0), point, &mut drop)
            .map_err(|error| format!("Drop 失败：{}", error.code().0))?;
        Ok(format!(
            "items={} enter={} over={} drop={}",
            sources.len(),
            enter.0,
            over.0,
            drop.0
        ))
    }
}

/// 由本地路径造一个 Shell 数据对象（单项走 IShellItem，多项走 PIDL 数组）。
unsafe fn shell_data_object(sources: &[PathBuf]) -> Result<IDataObject, String> {
    use windows::Win32::UI::Shell::{
        BHID_DataObject, IShellItem, SHCreateItemFromParsingName, SHCreateShellItemArrayFromIDLists,
        SHParseDisplayName,
    };
    // SAFETY: 调用方保证运行在已初始化 OLE 的 UI 线程上。
    unsafe {
        if let [single] = sources {
            let units = wide_path(single);
            let item: IShellItem = SHCreateItemFromParsingName(windows::core::PCWSTR(units.as_ptr()), None)
                .map_err(|error| format!("取 Shell 项失败：{}", error.code().0))?;
            return item
                .BindToHandler(None, &BHID_DataObject)
                .map_err(|error| format!("取数据对象失败：{}", error.code().0));
        }
        let mut pidls = Vec::with_capacity(sources.len());
        for source in sources {
            let units = wide_path(source);
            let mut pidl: *mut windows::Win32::UI::Shell::Common::ITEMIDLIST = std::ptr::null_mut();
            SHParseDisplayName(windows::core::PCWSTR(units.as_ptr()), None, &mut pidl, 0, None)
                .map_err(|error| format!("解析路径失败：{}", error.code().0))?;
            let owned = TestPidl(pidl);
            if owned.0.is_null() {
                return Err("解析路径返回空 ID".into());
            }
            pidls.push(owned);
        }
        let pointers: Vec<*const windows::Win32::UI::Shell::Common::ITEMIDLIST> =
            pidls.iter().map(|pidl| pidl.0.cast_const()).collect();
        let array = SHCreateShellItemArrayFromIDLists(&pointers)
            .map_err(|error| format!("取项目数组失败：{}", error.code().0))?;
        array
            .BindToHandler(None, &BHID_DataObject)
            .map_err(|error| format!("取数据对象失败：{}", error.code().0))
    }
}

fn wide_path(path: &std::path::Path) -> Vec<u16> {
    let raw = path.to_string_lossy();
    let cleaned = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    cleaned.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 由 `SHParseDisplayName` 分配、需要 `CoTaskMemFree` 的 PIDL。
struct TestPidl(*mut windows::Win32::UI::Shell::Common::ITEMIDLIST);
impl Drop for TestPidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: PIDL 来自任务分配器，只释放一次。
            unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(self.0.cast())) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_safe_effects_are_offered() {
        let cases = [
            ("link", EFFECT_LINK, true, EFFECT_LINK),
            ("copy", EFFECT_COPY, true, EFFECT_COPY),
            ("link+copy", EFFECT_LINK | EFFECT_COPY, true, EFFECT_LINK),
            ("copy+move", EFFECT_COPY | 2, true, EFFECT_COPY),
            ("all", 7, true, EFFECT_LINK),
            ("move only", 2, true, EFFECT_NONE),
            ("nothing", 0, true, EFFECT_NONE),
            ("no target", EFFECT_LINK, false, EFFECT_NONE),
        ];
        for (name, allowed, has_target, expected) in cases {
            let chosen = choose_effect(allowed, has_target);
            assert_eq!(chosen, expected, "case: {name}");
            assert_eq!(chosen & 2, 0, "MOVE selected: {name}");
        }
    }
}
