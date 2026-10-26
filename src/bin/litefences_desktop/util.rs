//! 小工具：宽字符串、窗口类名、鼠标坐标。
//!
//! 这些函数只在 UI 线程上使用，不涉及跨线程共享。

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW;

/// 转成以 0 结尾的 UTF-16，调用方负责保持生命周期。
pub fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 读取窗口类名；失败时返回空串。
pub fn class_name(hwnd: HWND) -> String {
    if hwnd.is_null() {
        return String::new();
    }
    let mut buffer = [0u16; 256];
    // SAFETY: 缓冲区长度与传入的容量一致，窗口句柄来自调用方。
    let len = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..len as usize])
}

/// 把 `lparam` 里的两个有符号 16 位鼠标坐标取出来。
pub fn mouse_xy(lparam: isize) -> (i32, i32) {
    let x = (lparam & 0xffff) as u16 as i16 as i32;
    let y = ((lparam >> 16) & 0xffff) as u16 as i16 as i32;
    (x, y)
}
