//! 工具条上的搜索输入框。
//!
//! 面板本身是“不抢焦点”的窗口，没法直接收键盘；这里用一个贴边的顶层小窗放一个
//! 原生 EDIT 控件，输入变化即时回传给面板过滤图标。

use std::cell::UnsafeCell;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DeleteObject, HDC, HFONT,
    SelectObject,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowTextW, RegisterClassW, SW_SHOW,
    SetForegroundWindow, SetWindowTextW, ShowWindow, EN_CHANGE, WM_CLOSE, WM_COMMAND,
    WM_ERASEBKGND, WNDCLASSW, WS_CHILD, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

use crate::log;
use crate::util::wide;

const CLASS_NAME: &str = "LiteFencesSearchBox";
const EDIT_ID: usize = 2001;

struct Slot {
    host: HWND,
    font: HFONT,
}

struct Cell(UnsafeCell<Option<Slot>>);
// SAFETY: 只在 UI 线程访问。
unsafe impl Sync for Cell {}
unsafe impl Send for Cell {}

static STATE: OnceLock<Cell> = OnceLock::new();

fn cell() -> &'static Cell {
    STATE.get_or_init(|| Cell(UnsafeCell::new(None)))
}

fn slot() -> Option<&'static mut Slot> {
    // SAFETY: 只在 UI 线程访问。
    let value = unsafe { &mut *cell().0.get() };
    value.as_mut()
}

pub fn is_open() -> bool {
    slot().is_some()
}

/// 在给定屏幕矩形处打开搜索框。
pub fn open(owner: HWND, area: RECT, initial: &str) {
    close();
    // SAFETY: 类名与回调在进程内保持有效。
    unsafe {
        let hinstance = GetModuleHandleW(std::ptr::null());
        let class_name = wide(CLASS_NAME);
        let class = WNDCLASSW {
            lpfnWndProc: Some(box_proc),
            hInstance: hinstance,
            hbrBackground: CreateSolidBrush(0x0016_1616),
            lpszClassName: class_name.as_ptr(),
            ..Default::default()
        };
        RegisterClassW(&class);

        let title = wide("搜索图标");
        let host = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            area.left,
            area.top,
            area.right - area.left,
            area.bottom - area.top,
            owner,
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        );
        if host.is_null() {
            log::error("无法创建搜索框");
            return;
        }
        let edit_class = wide("EDIT");
        let empty = wide("");
        let edit = CreateWindowExW(
            0,
            edit_class.as_ptr(),
            empty.as_ptr(),
            WS_CHILD | WS_VISIBLE | 0x0080, // ES_AUTOHSCROLL
            6,
            4,
            (area.right - area.left) - 12,
            (area.bottom - area.top) - 8,
            host,
            EDIT_ID as _,
            hinstance,
            std::ptr::null(),
        );
        let font_name = wide("Microsoft YaHei UI");
        let font = CreateFontW(
            -18,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            ANTIALIASED_QUALITY as u32,
            0,
            font_name.as_ptr(),
        );
        if !edit.is_null() && !font.is_null() {
            let hdc: HDC = windows_sys::Win32::Graphics::Gdi::GetDC(edit);
            SelectObject(hdc, font as _);
            windows_sys::Win32::Graphics::Gdi::ReleaseDC(edit, hdc);
        }
        if !initial.is_empty() && !edit.is_null() {
            let text = wide(initial);
            SetWindowTextW(edit, text.as_ptr());
        }
        let _ = cell();
        // SAFETY: 写入只在 UI 线程使用的静态状态。
        let value = &mut *cell().0.get();
        *value = Some(Slot { host, font });

        ShowWindow(host, SW_SHOW);
        SetForegroundWindow(host);
        if !edit.is_null() {
            SetFocus(edit);
        }
    }
}

pub fn close() {
    // SAFETY: 只在 UI 线程访问静态状态。
    let value = unsafe { &mut *cell().0.get() };
    let Some(current) = value.take() else {
        return;
    };
    // SAFETY: 句柄由本模块创建。
    unsafe {
        if !current.font.is_null() {
            DeleteObject(current.font as _);
        }
        if !current.host.is_null() {
            DestroyWindow(current.host);
        }
    }
}

unsafe extern "system" fn box_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_COMMAND => {
            let id = wparam & 0xffff;
            let code = (wparam >> 16) & 0xffff;
            if id == EDIT_ID && code == EN_CHANGE as usize {
                // SAFETY: 读取控件文本到本地缓冲区。
                unsafe {
                    let mut buffer = [0u16; 256];
                    let len = GetWindowTextW(
                        lparam as HWND,
                        buffer.as_mut_ptr(),
                        buffer.len() as i32,
                    );
                    let text = if len > 0 {
                        String::from_utf16_lossy(&buffer[..len as usize])
                    } else {
                        String::new()
                    };
                    crate::app::set_search_text(&text);
                }
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_CLOSE => {
            crate::app::set_search_text("");
            close();
            0
        }
        _ => {
            // SAFETY: 默认处理其余消息。
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}
