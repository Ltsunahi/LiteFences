//! 右键菜单里的系统动作：在资源管理器中定位、打开属性、复制路径。

use std::path::Path;

use windows_sys::Win32::Foundation::{GetLastError, GlobalFree, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW, ShellExecuteExW, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::util::wide;

fn shell_code(result: isize) -> Result<(), String> {
    if result > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW code={result}"))
    }
}

/// 在资源管理器里选中这个文件/快捷方式。
pub fn reveal(path: &Path) -> Result<(), String> {
    let operation = wide("open");
    let file = wide("explorer.exe");
    let parameters = wide(&format!("/select,\"{}\"", path.display()));
    // SAFETY: 所有字符串以 0 结尾；父窗口为空表示不指定。
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    shell_code(result as isize)
}

/// 弹出系统「属性」对话框。
pub fn properties(hwnd: HWND, path: &Path) -> Result<(), String> {
    let verb = wide("properties");
    let file = wide(&path.display().to_string());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST,
        hwnd,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        nShow: SW_SHOWNORMAL,
        ..Default::default()
    };
    // SAFETY: 结构体已填 cbSize，字符串在调用期间保持存活。
    let ok = unsafe { ShellExecuteExW(&mut info) };
    if ok != 0 {
        Ok(())
    } else {
        // SAFETY: 无参数，读取本线程最近一次错误码。
        let code = unsafe { GetLastError() };
        Err(format!("ShellExecuteExW 失败 err={code}"))
    }
}

/// 打开一个 URL（Releases 页面等）。
pub fn open_url(url: &str) -> Result<(), String> {
    let operation = wide("open");
    let target = wide(url);
    // SAFETY: 字符串以 0 结尾；父窗口为空。
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    shell_code(result as isize)
}

/// 复制一段 Unicode 文本到剪贴板；剪贴板被占用时短暂重试。
pub fn copy_text(hwnd: HWND, text: &str) -> Result<(), String> {
    let mut opened = false;
    for _ in 0..5 {
        // SAFETY: 句柄可以为空；成功后必须 CloseClipboard。
        if unsafe { OpenClipboard(hwnd) } != 0 {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if !opened {
        return Err("剪贴板被其它程序占用".to_string());
    }
    let result = (|| {
        // SAFETY: 剪贴板已打开，成对清空并写入。
        unsafe {
            if EmptyClipboard() == 0 {
                return Err("EmptyClipboard 失败".to_string());
            }
            let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = units.len() * 2;
            let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if handle.is_null() {
                return Err("GlobalAlloc 失败".to_string());
            }
            let target = GlobalLock(handle);
            if target.is_null() {
                GlobalFree(handle);
                return Err("GlobalLock 失败".to_string());
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), target as *mut u16, units.len());
            GlobalUnlock(handle);
            if SetClipboardData(u32::from(CF_UNICODETEXT), handle).is_null() {
                GlobalFree(handle);
                return Err("SetClipboardData 失败".to_string());
            }
        }
        Ok(())
    })();
    // SAFETY: 与 OpenClipboard 成对。
    unsafe { CloseClipboard() };
    result
}
