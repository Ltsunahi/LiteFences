//! 低层钩子：桌面空白处双击唤出、Esc 收起。
//!
//! 两个钩子都装在 UI 线程上，回调也在该线程派发，因此可以直接访问全局状态。
//! 钩子不注入进程、不需要管理员权限，也不修改任何系统设置。

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetDoubleClickTime, VK_ESCAPE};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetCursorPos, KBDLLHOOKSTRUCT, SetWindowsHookExW, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN, WindowFromPoint,
};

use crate::log;
use crate::overlay;
use crate::state::state;
use crate::util::class_name;

static LAST_CLICK: OnceLock<Mutex<Option<(Instant, i32, i32)>>> = OnceLock::new();

/// 判定“点在桌面空白处”的窗口类。
const DESKTOP_CLASSES: [&str; 3] = ["Progman", "WorkerW", "SHELLDLL_DefView"];

pub fn install() -> (isize, isize) {
    let mouse = install_mouse();
    // SAFETY: 空名称表示当前模块；钩子回调在本线程派发。
    let module = unsafe { GetModuleHandleW(std::ptr::null()) };
    let keyboard = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0) };
    if keyboard.is_null() {
        log::error("键盘钩子安装失败，Esc 收起不可用");
    }
    (mouse, keyboard as isize)
}

/// 只安装鼠标钩子（面板收起时用）。
pub fn install_mouse() -> isize {
    // SAFETY: 空名称表示当前模块；钩子回调在本线程派发。
    let module = unsafe { GetModuleHandleW(std::ptr::null()) };
    let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0) };
    if mouse.is_null() {
        log::error("鼠标钩子安装失败，双击唤出不可用");
    }
    mouse as isize
}

/// 卸载鼠标钩子：面板显示期间用不到它，卸载后钩子回调不再占用 UI 线程。
pub fn remove_mouse(hook: isize) {
    if hook == 0 {
        return;
    }
    // SAFETY: 句柄由本模块安装。
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(hook as _);
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam as u32 == WM_LBUTTONDOWN && !state().shown {
        // SAFETY: 钩子参数由系统给出，只使用 wparam/lparam 的既有语义。
        unsafe { consider_show() };
    }
    // SAFETY: 原样传递给下一个钩子。
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

unsafe fn consider_show() {
    let now = Instant::now();
    let mut point = POINT::default();
    // SAFETY: 输出结构体有效。
    unsafe { GetCursorPos(&mut point) };
    let slot = LAST_CLICK.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = slot.lock() else {
        return;
    };
    let double = match *guard {
        Some((at, x, y)) => {
            now.duration_since(at) <= Duration::from_millis(u64::from(double_click_ms()))
                && (x - point.x).abs() <= 6
                && (y - point.y).abs() <= 6
        }
        None => false,
    };
    *guard = Some((now, point.x, point.y));
    drop(guard);
    if !double {
        return;
    }
    // SAFETY: 只读查询窗口类，不涉及所有权。
    let on_desktop = unsafe { WindowFromPoint(point) };
    let class = class_name(on_desktop);
    if !DESKTOP_CLASSES.contains(&class.as_str()) {
        log::info(&format!("双击命中 {class}，不是桌面空白处，忽略"));
        return;
    }
    log::info("桌面空白处双击，唤出面板");
    overlay::show_panel();
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam as u32 == WM_KEYDOWN {
        // SAFETY: 低层键盘钩子的 lparam 指向有效 KBDLLHOOKSTRUCT。
        let event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        if event.vkCode == u32::from(VK_ESCAPE) && state().shown {
            log::info("Esc：退出移除模式或收起面板");
            overlay::handle_escape();
            return 1;
        }
    }
    // SAFETY: 原样传递给下一个钩子。
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

fn double_click_ms() -> u32 {
    // SAFETY: 无参数，返回系统设置。
    let value = unsafe { GetDoubleClickTime() };
    value.clamp(200, 1200)
}
