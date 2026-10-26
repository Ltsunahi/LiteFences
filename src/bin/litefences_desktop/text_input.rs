//! 简单文本输入对话框（新建分组用）。模态，OK 返回文本。

use std::cell::UnsafeCell;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BeginPaint, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DT_CENTER,
    DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect,
    GetMonitorInfoW, HBRUSH, HFONT, MONITORINFO, MONITOR_DEFAULTTONEAREST, MonitorFromPoint,
    PAINTSTRUCT, SelectObject, SetBkColor, SetBkMode, SetTextColor, TRANSPARENT, UpdateWindow,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_SELECTED};
use windows_sys::Win32::UI::WindowsAndMessaging::BS_OWNERDRAW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetCursorPos,
    GetMessageW, GetWindowTextW, IDC_ARROW, LoadCursorW, MSG, RegisterClassW,
    SW_SHOW, SetForegroundWindow, ShowWindow, TranslateMessage, WM_CLOSE, WM_COMMAND,
    WM_CTLCOLORBTN, WM_CTLCOLOREDIT, WM_DESTROY, WM_DRAWITEM, WM_ERASEBKGND, WM_KEYDOWN, WM_PAINT,
    WNDCLASSW, WS_CHILD, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE, HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SWP_NOACTIVATE, SetWindowPos,
};

use crate::log;
use crate::util::wide;

const CLASS_NAME: &str = "LiteFencesTextInput";
const EDIT_ID: usize = 3001;
const OK_ID: usize = 3002;
const CANCEL_ID: usize = 3003;
const WIDTH: i32 = 460;
const HEIGHT: i32 = 190;
/// 暗色主题：输入框与按钮的底色、文字色。
const DARK_BG: u32 = 0x0026_2626;
const DARK_TEXT: u32 = 0x00F0_F0F0;

struct State {
    edit: HWND,
    value: String,
    closed: bool,
    accepted: bool,
    title: String,
    /// 暗色输入框用的画刷（要长期存活，才能作为 WM_CTLCOLOREDIT 的返回值）。
    brush: HBRUSH,
}

/// 每次打开都会重置的状态（`None` = 没有正在进行的输入）。
struct Slot(UnsafeCell<Option<State>>);
// SAFETY: 只在 UI 线程访问。
unsafe impl Sync for Slot {}
unsafe impl Send for Slot {}

static STATE: OnceLock<Slot> = OnceLock::new();

fn slot() -> &'static mut Option<State> {
    let slot = STATE.get_or_init(|| Slot(UnsafeCell::new(None)));
    // SAFETY: 只在 UI 线程访问。
    unsafe { &mut *slot.0.get() }
}

fn state() -> Option<&'static mut State> {
    slot().as_mut()
}

/// 输入框是否正开着。
pub fn is_open() -> bool {
    state().is_some_and(|state| !state.closed)
}

/// 取消输入（面板收起、按 Esc、退出时调用），不会返回任何文本。
pub fn cancel() {
    if let Some(state) = state() {
        state.accepted = false;
        state.closed = true;
    }
}

pub fn ask(title: &str, initial: &str) -> Option<String> {
    // 每次打开都换一份干净状态；否则第二次「新建分组」会直接返回上一次的结果。
    *slot() = Some(State {
        edit: std::ptr::null_mut(),
        value: String::new(),
        closed: false,
        accepted: false,
        title: title.to_string(),
        // SAFETY: 画刷由本模块创建，`ask` 返回前释放。
        brush: unsafe { CreateSolidBrush(DARK_BG) },
    });
    let Some(hwnd) = create_window(initial) else {
        finish();
        return None;
    };
    run_modal();
    let result = state().and_then(|state| {
        if state.accepted {
            Some(state.value.clone())
        } else {
            None
        }
    });
    // SAFETY: 句柄由本函数创建。
    unsafe { DestroyWindow(hwnd) };
    finish();
    result
}

/// 收尾：清掉对话框状态并释放它用过的画刷。
fn finish() {
    if let Some(done) = slot().take() {
        // SAFETY: 画刷由本模块创建，销毁前不再被控件引用。
        unsafe { DeleteObject(done.brush as _) };
    }
}

/// 窗口类的底色画刷：注册类只需要一份，重复创建会一直泄漏。
fn class_brush() -> HBRUSH {
    static BRUSH: OnceLock<isize> = OnceLock::new();
    let raw = *BRUSH.get_or_init(|| {
        // SAFETY: 画刷由窗口类持有，进程结束前一直有效。
        unsafe { CreateSolidBrush(0x0020_1E1E) as isize }
    });
    raw as HBRUSH
}

fn create_window(initial: &str) -> Option<HWND> {
    // SAFETY: 类名与回调在进程内保持有效。
    unsafe {
        let hinstance = GetModuleHandleW(std::ptr::null());
        let class_name = wide(CLASS_NAME);
        let cursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: hinstance,
            hCursor: cursor,
            hbrBackground: class_brush(),
            lpszClassName: class_name.as_ptr(),
            ..Default::default()
        };
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
                area.left + (area.right - area.left - WIDTH) / 2,
                area.top + (area.bottom - area.top - HEIGHT) / 2,
            )
        } else {
            (400, 300)
        };
        let title = wide("新建分组");
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            left,
            top,
            WIDTH,
            HEIGHT,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            // SAFETY: 无参数读取上一调用的错误码。
            let error = windows_sys::Win32::Foundation::GetLastError();
            log::error(&format!("无法创建输入窗口，错误码 {error}"));
            return None;
        }
        log::info("输入窗口已创建");
        let edit_class = wide("EDIT");
        let value = wide(initial);
        let edit = CreateWindowExW(
            0,
            edit_class.as_ptr(),
            value.as_ptr(),
            WS_CHILD | WS_VISIBLE | 0x0080,
            24,
            70,
            WIDTH - 48,
            34,
            hwnd,
            EDIT_ID as _,
            hinstance,
            std::ptr::null(),
        );
        let button_class = wide("BUTTON");
        let ok = wide("确定");
        let cancel = wide("取消");
        CreateWindowExW(
            0,
            button_class.as_ptr(),
            ok.as_ptr(),
            WS_CHILD | WS_VISIBLE | BS_OWNERDRAW as u32,
            WIDTH - 220,
            126,
            90,
            34,
            hwnd,
            OK_ID as _,
            hinstance,
            std::ptr::null(),
        );
        CreateWindowExW(
            0,
            button_class.as_ptr(),
            cancel.as_ptr(),
            WS_CHILD | WS_VISIBLE | BS_OWNERDRAW as u32,
            WIDTH - 118,
            126,
            90,
            34,
            hwnd,
            CANCEL_ID as _,
            hinstance,
            std::ptr::null(),
        );
        if let Some(state) = state() {
            state.edit = edit;
        }
        // 输入框也用统一字体。
        if !edit.is_null() {
            let font = create_font(
                (GetDpiForWindow(hwnd) as f32 / 96.0).max(1.0),
                14.0,
                400,
            );
            if !font.is_null() {
                // 控件句柄有效。
                let dc = windows_sys::Win32::Graphics::Gdi::GetDC(edit);
                SelectObject(dc, font as _);
                windows_sys::Win32::Graphics::Gdi::ReleaseDC(edit, dc);
            }
        }
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        // 面板也是置顶窗口，这里显式把输入框抬到最上层，避免被面板盖住。
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
        SetForegroundWindow(hwnd);
        if !edit.is_null() {
            SetFocus(edit);
        }
        Some(hwnd)
    }
}

fn run_modal() {
    let mut message = MSG::default();
    loop {
        if state().is_none_or(|state| state.closed) {
            break;
        }
        // SAFETY: 标准消息循环。
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

fn read_edit() -> String {
    let Some(state) = state() else {
        return String::new();
    };
    if state.edit.is_null() {
        return String::new();
    }
    let mut buffer = [0u16; 256];
    // SAFETY: 控件句柄有效。
    unsafe {
        let len = GetWindowTextW(state.edit, buffer.as_mut_ptr(), buffer.len() as i32);
        if len <= 0 {
            String::new()
        } else {
            String::from_utf16_lossy(&buffer[..len as usize])
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_COMMAND => {
            let id = wparam & 0xffff;
            if id == OK_ID {
                if let Some(state) = state() {
                    state.value = read_edit();
                    state.accepted = true;
                    state.closed = true;
                }
            } else if id == CANCEL_ID
                && let Some(state) = state()
            {
                state.closed = true;
            }
            0
        }
        WM_KEYDOWN => {
            if wparam as u32 == 0x1B
                && let Some(state) = state()
            {
                state.closed = true;
            }
            0
        }
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORBTN => {
            // 让输入框跟随暗色主题。
            // SAFETY: wparam 是控件的 DC。
            unsafe {
                let dc = wparam as windows_sys::Win32::Graphics::Gdi::HDC;
                SetTextColor(dc, DARK_TEXT);
                SetBkColor(dc, DARK_BG);
                if let Some(state) = state() {
                    return state.brush as LRESULT;
                }
            }
            0
        }
        WM_DRAWITEM => {
            // 自绘按钮：原生 BUTTON 不吃 WM_CTLCOLORBTN，只能自己画。
            // SAFETY: lparam 是系统给出的 DRAWITEMSTRUCT。
            unsafe {
                let item = &*(lparam as *const DRAWITEMSTRUCT);
                let id = item.CtlID as usize;
                if id == OK_ID || id == CANCEL_ID {
                    let pressed = item.itemState & ODS_SELECTED != 0;
                    // 统一中性深灰：主按钮只比次按钮亮一点，不再用彩色，
                    // 否则容易被看成“卡在选中状态”。
                    let background = if pressed { 0x0048_4848 } else { 0x0032_3232 };
                    let client = item.rcItem;
                    let brush = CreateSolidBrush(background);
                    FillRect(item.hDC, &client, brush);
                    DeleteObject(brush as _);
                    // 一圈细边框：确定稍亮，区分主次。
                    let border = CreateSolidBrush(if id == OK_ID {
                        0x0080_8080
                    } else {
                        0x0058_5858
                    });
                    FrameRect(item.hDC, &client, border);
                    DeleteObject(border as _);
                    let mut buffer = [0u16; 32];
                    let length = GetWindowTextW(item.hwndItem, buffer.as_mut_ptr(), 32);
                    let text = if length > 0 {
                        String::from_utf16_lossy(&buffer[..length as usize])
                    } else {
                        String::new()
                    };
                    let label = wide(&text);
                    let mut rect = client;
                    SetBkMode(item.hDC, TRANSPARENT as i32);
                    SetTextColor(item.hDC, 0x00F5_F5F5);
                    let font = create_font(
                        (GetDpiForWindow(item.hwndItem) as f32 / 96.0).max(1.0),
                        14.0,
                        500,
                    );
                    let previous = if font.is_null() {
                        std::ptr::null_mut()
                    } else {
                        SelectObject(item.hDC, font as _)
                    };
                    DrawTextW(
                        item.hDC,
                        label.as_ptr(),
                        -1,
                        &mut rect,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                    );
                    if !previous.is_null() {
                        SelectObject(item.hDC, previous);
                    }
                    if !font.is_null() {
                        DeleteObject(font as _);
                    }
                    return 1;
                }
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_CLOSE => {
            if let Some(state) = state() {
                state.closed = true;
            }
            0
        }
        WM_DESTROY => {
            // 只结束本次模态循环。这里**不能** PostQuitMessage：
            // 对话框和主程序在同一个消息队列里，那条 WM_QUIT 会把常驻进程也一起结束
            // （现象：新建完分组，软件自己关了）。
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
    // SAFETY: 字体名以 0 结尾。
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
        let font = create_font(scale, 15.0, 500);
        let title = state()
            .map(|state| state.title.clone())
            .unwrap_or_else(|| "新建分组".to_string());
        let label = wide(&title);
        let mut rect = RECT {
            left: 24,
            top: 24,
            right: client.right - 24,
            bottom: 60,
        };
        // SAFETY: DC 与字体有效。
        let previous = if font.is_null() {
            std::ptr::null_mut()
        } else {
            SelectObject(hdc, font as _)
        };
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, 0x00D0_D0D0);
        DrawTextW(
            hdc,
            label.as_ptr(),
            -1,
            &mut rect,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );
        if !previous.is_null() {
            SelectObject(hdc, previous);
        }
        if !font.is_null() {
            DeleteObject(font as _);
        }
        EndPaint(hwnd, &ps);
    }
}




