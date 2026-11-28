//! 小工具：宽字符串、窗口类名、鼠标坐标。
//!
//! 这些函数只在 UI 线程上使用，不涉及跨线程共享。

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITORINFO, MONITOR_DEFAULTTONEAREST, MonitorFromPoint, MonitorFromWindow,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowRect, IsIconic, IsWindowVisible,
};

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

/// 按显示器真实 DPI（基线 96）缩放像素尺寸。
pub fn scale_i32(px: i32, dpi: u32) -> i32 {
    (px as f32 * (dpi.max(96) as f32 / 96.0)).round() as i32
}

/// 将尺寸 `(width, height)` 居中放到 `area` 内（支持负坐标副屏），并夹紧不越界。
pub fn center_in_rect(area: RECT, width: i32, height: i32) -> (i32, i32) {
    let max_left = (area.right - width).max(area.left);
    let max_top = (area.bottom - height).max(area.top);
    let left = (area.left + (area.right - area.left - width) / 2).clamp(area.left, max_left);
    let top = (area.top + (area.bottom - area.top - height) / 2).clamp(area.top, max_top);
    (left, top)
}

/// 将屏幕坐标点夹紧在给定矩形内部（支持负坐标副屏）。
pub fn clamp_point_in_rect(point: POINT, area: RECT) -> POINT {
    if area.right <= area.left || area.bottom <= area.top {
        return point;
    }
    POINT {
        x: point.x.clamp(area.left, area.right - 1),
        y: point.y.clamp(area.top, area.bottom - 1),
    }
}

/// 读取屏幕坐标所在显示器的工作区（支持负坐标副屏）。
pub fn monitor_work_area_at(point: POINT) -> Option<RECT> {
    // SAFETY: 点坐标值传递；info 结构体大小已设置。
    unsafe {
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info) != 0 {
            Some(info.rcWork)
        } else {
            None
        }
    }
}

/// 判断窗口矩形是否铺满所在显示器的完整屏幕矩形（支持负坐标副屏）。
pub fn is_fullscreen_rect(window: RECT, monitor: RECT) -> bool {
    monitor.right > monitor.left
        && monitor.bottom > monitor.top
        && window.left <= monitor.left
        && window.top <= monitor.top
        && window.right >= monitor.right
        && window.bottom >= monitor.bottom
}

/// 前台是否正有全屏独占/全屏应用运行（排除桌面和任务栏）。
pub fn is_foreground_fullscreen(ignore_hwnds: &[HWND]) -> bool {
    // SAFETY: 仅查询前台窗口句柄、可见性与矩形，不修改系统或他人窗口状态。
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() || ignore_hwnds.contains(&fg) {
            return false;
        }
        if IsWindowVisible(fg) == 0 || IsIconic(fg) != 0 {
            return false;
        }
        let class = class_name(fg);
        if matches!(
            class.as_str(),
            "Progman"
                | "WorkerW"
                | "SHELLDLL_DefView"
                | "Shell_TrayWnd"
                | "Shell_SecondaryTrayWnd"
        ) {
            return false;
        }
        let mut win_rect = RECT::default();
        if GetWindowRect(fg, &mut win_rect) == 0 {
            return false;
        }
        let monitor = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return false;
        }
        is_fullscreen_rect(win_rect, info.rcMonitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_and_clamp_support_negative_secondary_monitors() {
        // 左侧负坐标副屏
        let left_mon = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert_eq!(center_in_rect(left_mon, 460, 190), (-1190, 445));
        let p_left = clamp_point_in_rect(POINT { x: -2000, y: 1200 }, left_mon);
        assert_eq!((p_left.x, p_left.y), (-1920, 1079));

        // 上方负坐标副屏
        let top_mon = RECT {
            left: 0,
            top: -1440,
            right: 2560,
            bottom: 0,
        };
        assert_eq!(center_in_rect(top_mon, 690, 285), (935, -863));
        let p_top = clamp_point_in_rect(POINT { x: 2600, y: -1500 }, top_mon);
        assert_eq!((p_top.x, p_top.y), (2559, -1440));
    }

    #[test]
    fn scale_i32_respects_mixed_dpi_factors() {
        assert_eq!(scale_i32(460, 96), 460);
        assert_eq!(scale_i32(460, 120), 575);
        assert_eq!(scale_i32(460, 144), 690);
        assert_eq!(scale_i32(460, 192), 920);
        assert_eq!(scale_i32(460, 0), 460);
    }

    #[test]
    fn fullscreen_rect_detects_coverage_across_monitors() {
        let primary = RECT {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1440,
        };
        assert!(is_fullscreen_rect(primary, primary));
        assert!(!is_fullscreen_rect(
            RECT {
                left: 0,
                top: 0,
                right: 2560,
                bottom: 1392,
            },
            primary
        ));

        let neg_left = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        assert!(is_fullscreen_rect(
            RECT {
                left: -1920,
                top: 0,
                right: 0,
                bottom: 1080,
            },
            neg_left
        ));
        assert!(!is_fullscreen_rect(
            RECT {
                left: -1800,
                top: 10,
                right: -100,
                bottom: 900,
            },
            neg_left
        ));
    }
}
