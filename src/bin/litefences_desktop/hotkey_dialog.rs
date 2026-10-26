//! 热键设置小窗：按下想要的组合键即生效，Esc 取消。模态。

use std::cell::UnsafeCell;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BeginPaint, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DT_CENTER,
    DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FillRect, GetMonitorInfoW, HDC,
    HFONT, InvalidateRect, MONITORINFO, MONITOR_DEFAULTTONEAREST, MonitorFromPoint, PAINTSTRUCT,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT, UpdateWindow,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetCursorPos,
    GetMessageW, IDC_ARROW, LoadCursorW, MSG, RegisterClassW, SW_SHOW,
    SetForegroundWindow, ShowWindow, TranslateMessage, WM_CLOSE, WM_DESTROY,
    WM_ERASEBKGND, WM_KEYDOWN, WM_PAINT, WM_SYSKEYDOWN, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

use crate::log;
use crate::settings::{Hotkey, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};
use crate::util::wide;

const CLASS_NAME: &str = "LiteFencesHotkeyDialog";
const WINDOW_WIDTH: i32 = 460;
const WINDOW_HEIGHT: i32 = 190;

struct DialogState {
    hotkey: Hotkey,
    captured: bool,
    closed: bool,
    message: String,
}

/// 每次打开都会重置的状态（`None` = 没有正在进行的设置）。
struct Slot(UnsafeCell<Option<DialogState>>);
// SAFETY: 只在 UI 线程访问。
unsafe impl Sync for Slot {}
unsafe impl Send for Slot {}

static STATE: OnceLock<Slot> = OnceLock::new();

fn slot() -> &'static mut Option<DialogState> {
    let slot = STATE.get_or_init(|| Slot(UnsafeCell::new(None)));
    // SAFETY: 只在 UI 线程访问。
    unsafe { &mut *slot.0.get() }
}

fn state() -> Option<&'static mut DialogState> {
    slot().as_mut()
}

/// 打开设置窗口；用户按下组合键返回 Some，取消返回 None。
pub fn ask_for_hotkey(current: Hotkey) -> Option<Hotkey> {
    // 每次打开都换一份干净状态，第二次改热键才不会直接复用上次的结果。
    *slot() = Some(DialogState {
        hotkey: current,
        captured: false,
        closed: false,
        message: "按下想要的组合键（需含 Ctrl / Alt / Shift / Win），Esc 取消".into(),
    });
    let Some(hwnd) = create_window() else {
        *slot() = None;
        return None;
    };
    run_modal();
    let result = state().and_then(|state| if state.captured { Some(state.hotkey) } else { None });
    // SAFETY: 句柄由本函数创建，模态循环已结束。
    unsafe { DestroyWindow(hwnd) };
    *slot() = None;
    result
}

fn create_window() -> Option<HWND> {
    // SAFETY: 类名与回调在进程内保持有效。
    unsafe {
        let hinstance = GetModuleHandleW(std::ptr::null());
        let class_name = wide(CLASS_NAME);
        let cursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
        let class = WNDCLASSW {
            lpfnWndProc: Some(dialog_proc),
            hInstance: hinstance,
            hCursor: cursor,
            hbrBackground: CreateSolidBrush(0x0020_1E1E),
            lpszClassName: class_name.as_ptr(),
            ..Default::default()
        };
        // 重复注册返回 0，忽略。
        RegisterClassW(&class);

        let mut point = POINT::default();
        GetCursorPos(&mut point);
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let (left, top) = if GetMonitorInfoW(monitor, &mut info) != 0 {
            let area = info.rcMonitor;
            (
                area.left + (area.right - area.left - WINDOW_WIDTH) / 2,
                area.top + (area.bottom - area.top - WINDOW_HEIGHT) / 2,
            )
        } else {
            (400, 300)
        };
        let title = wide("设置唤出热键");
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            left,
            top,
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            log::error("无法创建热键设置窗口");
            return None;
        }
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        SetForegroundWindow(hwnd);
        Some(hwnd)
    }
}

fn run_modal() {
    let mut message = MSG::default();
    loop {
        if state().is_none_or(|state| state.closed) {
            break;
        }
        // SAFETY: 标准消息循环；只处理本线程窗口的消息。
        let result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        // SAFETY: 消息由 GetMessage 填充。
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn modifiers_now() -> u32 {
    // SAFETY: 只读取键盘状态。
    unsafe {
        let mut modifiers = 0;
        if GetKeyState(VK_CONTROL as i32) < 0 {
            modifiers |= MOD_CONTROL;
        }
        if GetKeyState(VK_MENU as i32) < 0 {
            modifiers |= MOD_ALT;
        }
        if GetKeyState(VK_SHIFT as i32) < 0 {
            modifiers |= MOD_SHIFT;
        }
        if GetKeyState(VK_LWIN as i32) < 0 || GetKeyState(VK_RWIN as i32) < 0 {
            modifiers |= MOD_WIN;
        }
        modifiers
    }
}

unsafe extern "system" fn dialog_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let key = wparam as u32;
            // 只按修饰键时不结束，等真正的键。
            if matches!(key, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C) {
                return 0;
            }
            if key == u32::from(VK_ESCAPE) {
                if let Some(state) = state() {
                    state.closed = true;
                }
                return 0;
            }
            let modifiers = modifiers_now();
            if let Some(state) = state() {
                if modifiers == 0 {
                    state.message = "必须带上 Ctrl / Alt / Shift / Win 中的至少一个".into();
                } else {
                    state.hotkey = Hotkey { modifiers, key };
                    state.captured = true;
                    state.closed = true;
                }
            }
            // SAFETY: 句柄有效。
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            0
        }
        WM_CLOSE => {
            if let Some(state) = state() {
                state.closed = true;
            }
            0
        }
        WM_DESTROY => {
            // 只结束本次模态循环；PostQuitMessage 会连同主程序的消息循环一起结束。
            if let Some(state) = state() {
                state.closed = true;
            }
            0
        }
        _ => {
            // SAFETY: 默认处理。
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

fn create_font(scale: f32, pixels: f32, weight: i32) -> HFONT {
    let name = wide("Microsoft YaHei UI");
    // SAFETY: 字体名以 0 结尾；负高度表示字符高度。
    unsafe {
        CreateFontW(
            -((pixels * scale).round() as i32),
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            ANTIALIASED_QUALITY as u32,
            0,
            name.as_ptr(),
        )
    }
}

fn paint(hwnd: HWND) {
    // SAFETY: BeginPaint/EndPaint 成对。
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client);
        let scale = (GetDpiForWindow(hwnd) as f32 / 96.0).max(1.0);
        let background = CreateSolidBrush(0x0020_1E1E);
        FillRect(hdc, &client, background);
        DeleteObject(background as _);

        let title_font = create_font(scale, 15.0, 600);
        let value_font = create_font(scale, 26.0, 700);
        let hint_font = create_font(scale, 12.0, 400);
        SetBkMode(hdc, TRANSPARENT as i32);

        let (title, value, hint) = match state() {
            Some(state) => (
                "设置唤出热键".to_string(),
                state.hotkey.describe(),
                state.message.clone(),
            ),
            None => (
                "设置唤出热键".to_string(),
                "Ctrl+Alt+Space".to_string(),
                String::new(),
            ),
        };

        draw_text(
            hdc,
            title_font,
            &title,
            RECT {
                left: 0,
                top: 24,
                right: client.right,
                bottom: 56,
            },
            0x00D0_D0D0,
        );
        draw_text(
            hdc,
            value_font,
            &value,
            RECT {
                left: 0,
                top: 62,
                right: client.right,
                bottom: 118,
            },
            0x00FF_FFFF,
        );
        draw_text(
            hdc,
            hint_font,
            &hint,
            RECT {
                left: 0,
                top: 126,
                right: client.right,
                bottom: 172,
            },
            0x00A0_A0A0,
        );

        for font in [title_font, value_font, hint_font] {
            if !font.is_null() {
                DeleteObject(font as _);
            }
        }
        EndPaint(hwnd, &ps);
    }
}

unsafe fn draw_text(hdc: HDC, font: HFONT, text: &str, mut rect: RECT, color: u32) {
    let wide_text = wide(text);
    // SAFETY: 字体与 DC 在调用期间有效。
    unsafe {
        let previous = if font.is_null() {
            std::ptr::null_mut()
        } else {
            SelectObject(hdc, font as _)
        };
        SetTextColor(hdc, color);
        DrawTextW(
            hdc,
            wide_text.as_ptr(),
            -1,
            &mut rect,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );
        if !previous.is_null() {
            SelectObject(hdc, previous);
        }
    }
}
