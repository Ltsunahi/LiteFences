//! 隐藏/恢复原生桌面图标。
//!
//! 只在浮层显示期间隐藏，隐藏时写入标记文件。若进程崩溃导致图标停在隐藏状态，
//! 下次启动会先恢复一次再删除标记。这是对用户桌面可见状态的最小侵入。

use std::path::PathBuf;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, SendMessageW, WM_COMMAND,
};

use crate::log;
use crate::util::wide;

/// Explorer 用来切换桌面图标可见性的命令号（未公开但长期稳定）。
const CMD_TOGGLE_DESKTOP_ICONS: usize = 0x7402;

pub fn set_visible(visible: bool) -> bool {
    let Some(view) = desktop_view() else {
        log::warn("未找到桌面图标视图窗口，跳过图标显隐");
        return false;
    };
    // SAFETY: 句柄来自 FindWindowExW；命令号与 wparam/lparam 用法与资源管理器一致。
    unsafe {
        SendMessageW(view, WM_COMMAND, CMD_TOGGLE_DESKTOP_ICONS, 0);
    }
    if visible {
        clear_marker();
    } else {
        write_marker();
    }
    log::info(if visible { "已恢复原生桌面图标" } else { "已隐藏原生桌面图标" });
    true
}

/// 启动时调用：上一次若异常退出在隐藏状态，这里把它恢复回来。
pub fn recover_after_crash() {
    let marker = marker_path();
    if marker.exists() {
        log::warn("发现上次遗留的图标隐藏标记，尝试恢复");
        if set_visible(true) {
            return;
        }
        clear_marker();
    }
}

fn desktop_view() -> Option<HWND> {
    let progman_name = wide("Progman");
    let defview_name = wide("SHELLDLL_DefView");
    // SAFETY: 类名以 0 结尾；找不到时返回空句柄。
    let progman = unsafe { FindWindowW(progman_name.as_ptr(), std::ptr::null()) };
    if !progman.is_null() {
        // SAFETY: 同上。
        let direct = unsafe {
            FindWindowExW(progman, std::ptr::null_mut(), defview_name.as_ptr(), std::ptr::null())
        };
        if !direct.is_null() {
            return Some(direct);
        }
    }
    // 部分配置下图标宿主挂在 WorkerW 子窗口里。
    let worker_name = wide("WorkerW");
    let mut worker: HWND = std::ptr::null_mut();
    loop {
        // SAFETY: 逐个枚举同级窗口，直到返回空句柄。
        worker = unsafe {
            FindWindowExW(
                std::ptr::null_mut(),
                worker,
                worker_name.as_ptr(),
                std::ptr::null(),
            )
        };
        if worker.is_null() {
            return None;
        }
        // SAFETY: 同上。
        let view = unsafe {
            FindWindowExW(worker, std::ptr::null_mut(), defview_name.as_ptr(), std::ptr::null())
        };
        if !view.is_null() {
            return Some(view);
        }
    }
}

fn marker_path() -> PathBuf {
    crate::log::base_dir().join("icons-hidden.flag")
}

fn write_marker() {
    let path = marker_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, "hidden\n");
}

fn clear_marker() {
    let _ = std::fs::remove_file(marker_path());
}
