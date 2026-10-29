//! 全屏虚化浮层：桌面之上、普通窗口之上的一次性面板。
//!
//! - 虚化：优先亚克力（带自定义透明度，能看到桌面内容），再退 DWM 系统背景，最后深色底。
//! - 绘制：GDI 直接画图标与文字；不填背景，让虚化层透出来。
//! - 交互：单击图标启动、单击空白收起、Esc 收起；唤出后 350ms 内与入场动画期间的点击被忽略，
//!   避免唤出来源的那次点击立刻把面板点掉。
//! - 入场动画：图标网格上移 + 虚化浓度淡入，约 170ms。

use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea,
    DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BeginPaint, BitBlt, CLIP_DEFAULT_PRECIS, CreateCompatibleDC,
    CreateDIBSection, CreateFontW, CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DT_CALCRECT,
    DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK,
    DeleteDC, DeleteObject, DrawTextW, EndPaint, FF_DONTCARE, FillRect, GetMonitorInfoW,
    GetStockObject, HDC, HFONT, HOLLOW_BRUSH, InvalidateRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    MonitorFromPoint, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, PtInRect, RoundRect, SRCCOPY,
    SelectObject, SetBkMode, SetTextColor, TRANSPARENT, GetDC, ReleaseDC, NULL_PEN, Ellipse,
    LineTo, MoveToEx, UpdateWindow, IntersectClipRect, RestoreDC, SaveDC, AlphaBlend,
    AC_SRC_OVER, BLENDFUNCTION,
};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::Controls::MARGINS;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CS_DBLCLKS, CreatePopupMenu, CreateWindowExW, DI_NORMAL, DefWindowProcW,
    DestroyMenu, DestroyWindow, DrawIconEx, GetCursorPos, GetWindowLongPtrW, GetWindowRect,
    GWL_EXSTYLE, HTCLIENT, HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW, KillTimer, LoadCursorW,
    MA_NOACTIVATE, MF_CHECKED, MF_SEPARATOR, MF_STRING, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetForegroundWindow, SetTimer,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_TOPALIGN,
    TrackPopupMenu, WM_CHAR, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCHITTEST,
    WM_PAINT,
    WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::desktop_icons;
use crate::icons;
use crate::log;
use crate::state::{BlurMode, state};
use crate::util::{mouse_xy, wide};

const CLASS_NAME: &str = "LiteFencesOverlay";
const TIMER_SEARCH: usize = 2;
/// 拖入结果提示自动淡出。
const TIMER_DROP_NOTICE: usize = 3;

/// 唤出后忽略点击的时间窗口，单位毫秒。
const SHOW_COOLDOWN_MS: u64 = 350;
/// 入场动画时长；帧间隔略大一点，减少分层窗口的合成次数。
const ANIM_MS: u64 = 240;
/// 亚克力最终不透明度。
const TINT_ALPHA: u32 = 0x99;

#[derive(Clone)]
pub struct Header {
    pub group: usize,
    pub rect: RECT,
    pub count: usize,
}

#[derive(Clone, Default)]
pub struct Layout {
    pub scale: f32,
    pub dpi: u32,
    pub icon: i32,
    pub cell_w: i32,
    pub cell_h: i32,
    pub cols: usize,
    pub width: i32,
    pub height: i32,
    pub tiles: Vec<RECT>,
    pub headers: Vec<Header>,
    /// 全部内容摆完后的纵向高度（含顶部工具条留白），比面板高时要滚动。
    pub content_height: i32,
}

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

#[repr(C)]
struct WindowCompositionAttributeData {
    attribute: i32,
    data: *mut c_void,
    size: usize,
}

type SetCompositionAttribute =
    unsafe extern "system" fn(HWND, *mut WindowCompositionAttributeData) -> i32;

/// user32 里没有导出这个未公开函数，只能运行时解析。
fn set_composition_attribute() -> Option<SetCompositionAttribute> {
    static RESOLVED: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    let address = *RESOLVED.get_or_init(|| {
        let module_name = wide("user32.dll");
        let proc_name = b"SetWindowCompositionAttribute\0";
        // SAFETY: 名称以 0 结尾；失败时返回空句柄。
        unsafe {
            let module = LoadLibraryW(module_name.as_ptr());
            if module.is_null() {
                return None;
            }
            GetProcAddress(module, proc_name.as_ptr()).map(|function| function as usize)
        }
    });
    address.map(|value| {
        // SAFETY: 地址来自 user32 的同名导出，签名与本程序声明一致。
        unsafe { std::mem::transmute::<usize, SetCompositionAttribute>(value) }
    })
}

const WCA_ACCENT_POLICY: i32 = 19;
const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;
/// DWMSBT_TRANSIENTWINDOW：亚克力背景。
const DWMSBT_TRANSIENTWINDOW: i32 = 3;

pub fn create(hinstance: HINSTANCE) -> Result<HWND, String> {
    let class_name = wide(CLASS_NAME);
    let cursor = unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) };
    let class = WNDCLASSW {
        style: CS_DBLCLKS,
        lpfnWndProc: Some(overlay_proc),
        hInstance: hinstance,
        hCursor: cursor,
        hbrBackground: std::ptr::null_mut(),
        lpszClassName: class_name.as_ptr(),
        ..Default::default()
    };
    // SAFETY: 类名与回调都在本模块内保持有效。重复注册会返回 0（类已存在），这是正常的：
    // 每次唤出都会重建窗口，但窗口类只需要注册一次。
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        // SAFETY: 无参数读取错误码。
        let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        if error != 1410 {
            return Err(format!("无法注册浮层窗口类 err={error}"));
        }
    }
    let title = wide("LiteFences 面板");
    // SAFETY: 句柄与字符串参数均有效；先创建为隐藏窗口。
    // 注意：这里**不能**用 WS_EX_LAYERED。分层窗口按像素 alpha 合成时，GDI 文字
    // 的 alpha 没被置位会被整段丢掉（实测：DrawText 返回成功但屏幕上看不到字）。
    // 淡入改为调亚克力浓度来做。
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err("无法创建浮层窗口".into());
    }
    // 拖放目标跟着窗口一起注册；窗口重建时由 ensure_window 先注销。
    crate::drop_target::attach(hwnd);
    let dark: i32 = 1;
    // SAFETY: 属性指针与长度匹配。
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &dark as *const i32 as *const c_void,
            4,
        );
    }
    Ok(hwnd)
}

/// 应用虚化方式，返回实际生效的实现。
pub fn apply_blur(hwnd: HWND, requested: BlurMode) -> BlurMode {
    let effective = match requested {
        // 默认走自己画的磨砂底板：不依赖系统合成，文字稳定。
        BlurMode::Frost | BlurMode::Auto => BlurMode::Frost,
        BlurMode::Backdrop => {
            if try_backdrop(hwnd) {
                BlurMode::Backdrop
            } else {
                log::warn("DWM 系统背景失败，退回深色底");
                BlurMode::Dim
            }
        }
        BlurMode::Accent => {
            if apply_accent(hwnd, TINT_ALPHA) {
                BlurMode::Accent
            } else {
                log::warn("亚克力失败，退回深色底");
                BlurMode::Dim
            }
        }
        other => other,
    };
    log::info(&format!(
        "虚化模式：请求 {} 实际 {}",
        requested.as_str(),
        effective.as_str()
    ));
    effective
}

fn try_backdrop(hwnd: HWND) -> bool {
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    // SAFETY: 窗口句柄有效；-1 边距表示把玻璃扩展到整个客户区。
    let frame = unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
    let value = DWMSBT_TRANSIENTWINDOW;
    // SAFETY: 属性缓冲区长度与类型一致。
    let backdrop = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            &value as *const i32 as *const c_void,
            4,
        )
    };
    log::info(&format!("DWM backdrop：frame=0x{frame:08x} attr=0x{backdrop:08x}"));
    backdrop >= 0
}

/// 设置亚克力浓度。`alpha` 为 0 表示完全透明，用于淡入。
fn apply_accent(hwnd: HWND, alpha: u32) -> bool {
    let mut policy = AccentPolicy {
        state: ACCENT_ENABLE_ACRYLICBLURBEHIND,
        flags: 2,
        // AABBGGRR：alpha + 深灰。
        gradient_color: ((alpha & 0xff) << 24) | 0x00_20_20_20,
        animation_id: 0,
    };
    let mut data = WindowCompositionAttributeData {
        attribute: WCA_ACCENT_POLICY,
        data: &mut policy as *mut AccentPolicy as *mut c_void,
        size: std::mem::size_of::<AccentPolicy>(),
    };
    let Some(apply) = set_composition_attribute() else {
        log::warn("user32 未导出 SetWindowCompositionAttribute，亚克力不可用");
        return false;
    };
    // SAFETY: 结构体生命周期覆盖本次调用。
    let ok = unsafe { apply(hwnd, &mut data) } != 0;
    if alpha == TINT_ALPHA && !ok {
        log::warn("亚克力设置失败");
    }
    ok
}

/// 每次唤出都新建一个浮层窗口。
///
/// 亚克力表面上 GDI 画上去的像素无法擦除，复用同一个窗口会把上一次的图标/文字留在
/// 画面上（图标重叠、标签残影）。重建窗口的代价只有 1 毫秒级，换来的是永远干净的一帧。
pub fn ensure_window() -> Result<HWND, String> {
    let app = state();
    if !app.overlay.is_null() {
        // 窗口销毁前必须先注销 OLE 拖放，否则会留下悬空注册。
        crate::drop_target::detach();
        // SAFETY: 句柄由本模块创建；销毁后立即清空。
        unsafe {
            DestroyWindow(app.overlay);
        }
        app.overlay = std::ptr::null_mut();
    }
    // SAFETY: 空名称表示取当前模块句柄。
    let hinstance = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) };
    let hwnd = create(hinstance)?;
    let blur = apply_blur(hwnd, app.options.blur);
    app.blur = blur;
    app.overlay = hwnd;
    Ok(hwnd)
}

pub fn show_panel() {
    if state().shown {
        return;
    }
    if ensure_window().is_err() {
        log::error("无法创建浮层窗口");
        return;
    }
    let app = state();
    let mut point = POINT::default();
    // SAFETY: 输出结构体有效。
    unsafe { GetCursorPos(&mut point) };
    // SAFETY: 点坐标有效；取最近显示器。
    let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: 结构体大小已填。
    unsafe { GetMonitorInfoW(monitor, &mut info) };
    let area = info.rcMonitor;
    let width = area.right - area.left;
    let height = area.bottom - area.top;

    if app.options.hide_desktop_icons && !app.icons_hidden {
        app.icons_hidden = desktop_icons::set_visible(false);
    }
    // 藏完桌面图标再抓整屏原图：动画的底色里不该有桌面图标，
    // 否则淡入时“桌面图标 + 面板图标”会短暂叠在一起，看起来像闪。
    app.entrance = crate::backdrop::ScreenBase::capture(area);

    // 只有桌面内容变化时才重新扫描；扫描在后台线程进行，不阻塞消息循环。
    let stamp = icons::desktop_stamp();
    if app.items.is_empty() || app.desktop_stamp != Some(stamp) {
        crate::app::begin_scan(app.host);
    }
    // SAFETY: 先按目标显示器铺满，再取实际客户区与 DPI，保证绘制坐标与点击坐标同源。
    let mut client = RECT::default();
    unsafe {
        // 置顶：面板是模态式唤出层，任何窗口被激活都不应该把它盖住。
        SetWindowPos(
            app.overlay,
            HWND_TOPMOST,
            area.left,
            area.top,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        GetWindowRect(app.overlay, &mut client);
    }
    // 入口目录的内容在变化时重读（目录时间戳没变就复用上次结果）。
    crate::app::refresh_portals_if_stale();
    // DPI 必须在窗口挪到目标显示器之后再取，多屏不同缩放时才对。
    let dpi = unsafe { GetDpiForWindow(app.overlay) };
    let (client_w, client_h) = (client.right - client.left, client.bottom - client.top);
    let top_dip = if app.low_integrity { 88.0 } else { 48.0 };
    let layout = compute_layout(
        &app.items,
        &app.groups,
        client_w,
        client_h,
        dpi.max(96),
        top_dip,
    );
    app.layout = layout;
    // 每次唤出都从顶部开始。
    app.scroll = 0;

    // 自己抓一张桌面做磨砂底板（此时面板还没显示、桌面图标已隐藏）。
    if app.blur == BlurMode::Frost {
        let started = Instant::now();
        app.backdrop =
            crate::backdrop::capture(area, app.theme.tint, app.theme.tint_strength);
        log::info(&format!(
            "磨砂底板：{}（{}ms）",
            if app.backdrop.is_some() { "就绪" } else { "抓取失败，退回深色底" },
            started.elapsed().as_millis()
        ));
        if app.backdrop.is_none() {
            app.blur = BlurMode::Dim;
        }
    }

    // 入场动画：整帧淡入（所有虚化模式都有）。
    if app.blur == BlurMode::Accent {
        // 亚克力模式从全透明开始，由下面这个循环推进浓度。
        apply_accent(app.overlay, 0);
    }
    let anim_start = Instant::now();
    app.anim_started = Some(anim_start);
    app.anim_frames = 0;
    app.anim_alpha = 0.0;
    // 先把静态帧画进缓存，避免动画第一帧里再做一次整屏重绘（会卡一下）。
    // SAFETY: 句柄有效；GetDC/ReleaseDC 成对，缓存与窗口同尺寸。
    unsafe {
        let dc = GetDC(app.overlay);
        if !dc.is_null() {
            ensure_buffer(dc, client_w, client_h);
            let cache = state().buffer_dc;
            if !cache.is_null() {
                render_static(cache, state(), client_w, client_h);
                state().static_dirty = false;
            }
            ReleaseDC(app.overlay, dc);
        }
    }
    // 换内容了：先标记静态帧需要重画，再显示窗口，避免第一帧贴到旧缓存。
    state().static_dirty = state().static_dirty || state().buffer_dc.is_null();
    // SAFETY: 句柄有效。
    unsafe {
        ShowWindow(app.overlay, SW_SHOWNOACTIVATE);
        InvalidateRect(app.overlay, std::ptr::null(), 0);
    }
    app.shown = true;
    app.shown_at = Some(Instant::now());
    app.hover = None;
    // 每次唤出都从普通模式开始，避免上次的“移除中”状态残留。
    app.remove_mode = false;
    app.pending_remove = None;
    // 面板显示期间不需要全局鼠标钩子；卸载后钩子回调不再和绘制抢 UI 线程。
    if app.mouse_hook != 0 {
        crate::hooks::remove_mouse(app.mouse_hook);
        app.mouse_hook = 0;
    }
    log::info(&format!(
        "唤出面板：{} 项 + 添加按钮，{client_w}x{client_h} @{dpi}dpi，列 {}，格子 {}x{}，原点 ({},{})，虚化={}",
        app.items.len(),
        app.layout.cols,
        app.layout.cell_w,
        app.layout.cell_h,
        app.layout.tiles.first().map(|t| t.left).unwrap_or(0),
        app.layout.tiles.first().map(|t| t.top).unwrap_or(0),
        app.blur.as_str()
    ));

    // 入场动画在这里同步跑完：用 DwmFlush 跟着合成器节奏走，
    // 比 WM_TIMER 稳（定时器实测会出现 31ms 的空档，观感就是掉帧）。
    let anim_ms = Duration::from_millis(ANIM_MS);
    let mut frames = 0u32;
    while anim_start.elapsed() < anim_ms {
        frames += 1;
        if state().blur == BlurMode::Accent {
            let progress = anim_progress();
            apply_accent(state().overlay, (progress * TINT_ALPHA as f32) as u32);
        }
        // SAFETY: 句柄有效；画一帧后等下一次合成。
        unsafe {
            InvalidateRect(state().overlay, std::ptr::null(), 0);
            UpdateWindow(state().overlay);
            windows_sys::Win32::Graphics::Dwm::DwmFlush();
        }
    }
    log::info(&format!(
        "入场动画：{frames} 帧 / {}ms",
        anim_start.elapsed().as_millis()
    ));
    {
        let app = state();
        app.anim_started = None;
        // 动画期间工具条画进了缓存；收尾重画一版不含工具条的静态帧。
        app.static_dirty = true;
    }
    // SAFETY: 句柄有效；贴最后一帧。
    unsafe {
        InvalidateRect(state().overlay, std::ptr::null(), 0);
        UpdateWindow(state().overlay);
    }
}

/// 面板正显示时内容发生变化（例如后台扫描刚完成）：重建窗口再显示，
/// 保证画面不会叠着上一帧的图标。
pub fn reshow() {
    if !state().shown {
        return;
    }
    if ensure_window().is_err() {
        log::error("重建浮层窗口失败");
        return;
    }
    let app = state();
    let mut point = POINT::default();
    // SAFETY: 输出结构体有效。
    unsafe { GetCursorPos(&mut point) };
    // SAFETY: 点坐标有效；取最近显示器。
    let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: 结构体大小已填。
    unsafe { GetMonitorInfoW(monitor, &mut info) };
    let area = info.rcMonitor;
    let width = area.right - area.left;
    let height = area.bottom - area.top;
    // SAFETY: 句柄有效；直接以最终浓度显示，不再重复淡入。
    unsafe {
        SetWindowPos(
            app.overlay,
            HWND_TOPMOST,
            area.left,
            area.top,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        if app.blur == BlurMode::Accent {
            apply_accent(app.overlay, TINT_ALPHA);
        }
        ShowWindow(app.overlay, SW_SHOWNOACTIVATE);
        InvalidateRect(app.overlay, std::ptr::null(), 0);
    }
}

pub fn hide_panel() {
    // 面板收起时把任何打开着的输入框一并取消，避免它孤零零留在最上层。
    if crate::text_input::is_open() {
        crate::text_input::cancel();
    }
    let app = state();
    if !app.shown {
        return;
    }
    // SAFETY: 句柄有效。
    unsafe {
        KillTimer(app.overlay, TIMER_DROP_NOTICE);
        ShowWindow(app.overlay, SW_HIDE);
        // 收起后取消置顶，避免残留的 topmost 影响其他窗口。
        SetWindowPos(
            app.overlay,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    if app.icons_hidden {
        app.icons_hidden = !desktop_icons::set_visible(true);
    }
    app.shown = false;
    app.shown_at = None;
    app.hover = None;
    app.anim_started = None;
    app.drag = None;
    app.header_drag = None;
    app.drag_hidden = None;
    app.pending_remove = None;
    app.drop_hover = None;
    app.drop_notice = None;
    deactivate_typing();
    app.search.clear();
    app.search_open = false;
    app.search_anim = None;
    // 收起后重新装回鼠标钩子，恢复“双击桌面空白处唤出”。
    if app.mouse_hook == 0 {
        app.mouse_hook = crate::hooks::install_mouse();
    }
    free_caches();
    log::info("收起面板");
}

/// 释放只在面板显示期间需要的资源：整屏位图缓存、字体、磨砂底板。
///
/// 整屏 32 位 DIB 在 2560×1440 下约 14.7 MB；常驻时不该一直背着它
/// （验收线是常驻私有内存 < 20 MB），收起后它们都能按需重建。
fn free_caches() {
    let app = state();
    // SAFETY: 只释放本模块创建的 GDI 对象；先删 DC 再删位图，位图才不会被选中的 DC 占着。
    unsafe {
        if !app.buffer_dc.is_null() {
            DeleteDC(app.buffer_dc);
            app.buffer_dc = std::ptr::null_mut();
        }
        if app.buffer_bitmap != 0 {
            DeleteObject(app.buffer_bitmap as _);
            app.buffer_bitmap = 0;
        }
        if !app.font.is_null() {
            DeleteObject(app.font as _);
            app.font = std::ptr::null_mut();
        }
    }
    app.buffer_width = 0;
    app.buffer_height = 0;
    app.font_scale = 0.0;
    app.backdrop = None;
    app.entrance = None;
    app.scroll = 0;
    app.static_dirty = true;
}

/// 后台扫描完成后重排布局并重绘。
pub fn relayout() {
    let app = state();
    let dpi = app.layout.dpi.max(96);
    let (width, height) = (app.layout.width, app.layout.height);
    let top_dip = if app.low_integrity { 88.0 } else { 48.0 };
    let layout = compute_layout(&app.items, &app.groups, width, height, dpi, top_dip);
    app.layout = layout;
    app.hover = None;
    // 内容变短了就把滚动量收回来，别停在空白处。
    clamp_scroll(app);
    app.static_dirty = true;
    if app.shown {
        // SAFETY: 句柄有效；立刻重画，避免用户马上开始拖动时缓存还是旧布局。
        unsafe {
            InvalidateRect(app.overlay, std::ptr::null(), 0);
            UpdateWindow(app.overlay);
        }
    }
}

/// 鼠标（客户区坐标）落在哪一条分组标题上：吸顶那条优先，否则按内容坐标找整行。
fn header_at(x: i32, y: i32) -> Option<usize> {
    let app = state();
    let point = POINT { x, y };
    if let Some((group, rect)) = sticky_header(app)
        // SAFETY: 矩形来自本模块布局。
        && unsafe { PtInRect(&rect, point) != 0 }
    {
        return Some(group);
    }
    let content = content_point(app, x, y);
    app.layout
        .headers
        .iter()
        // SAFETY: 同上。
        .find(|header| unsafe { PtInRect(&header.rect, content) != 0 })
        .map(|header| header.group)
}

/// 拖动标题时应该插到第几个位置（0..=普通分组数，入口组固定在最后）。
fn header_insert_target(app: &crate::state::App, y: i32) -> usize {
    let limit = app
        .groups
        .iter()
        .position(|group| group.portal.is_some())
        .unwrap_or(app.layout.headers.len());
    let content_y = y + app.scroll;
    for (index, header) in app.layout.headers.iter().enumerate() {
        if index >= limit {
            break;
        }
        let middle = header.rect.top + (header.rect.bottom - header.rect.top) / 2;
        if content_y < middle {
            return index;
        }
    }
    limit
}

/// 拖动标题时的插入位置指示线（客户区坐标）。
fn header_indicator_rect(app: &crate::state::App, target: usize) -> RECT {
    let y = app
        .layout
        .headers
        .get(target)
        .map(|header| header.rect.top)
        .or_else(|| app.layout.headers.last().map(|header| header.rect.bottom))
        .unwrap_or(app.layout.content_height);
    let y = (y - app.scroll).max(content_top(app));
    let inset = (18.0 * app.layout.scale) as i32;
    RECT {
        left: inset,
        top: y - 2,
        right: app.layout.width - inset,
        bottom: y + 2,
    }
}

/// 把分组 `from` 移到插入位置 `to`（0..=普通分组数），并持久化。
fn move_group(from: usize, to: usize) {
    let app = state();
    if from >= app.groups.len() {
        return;
    }
    let limit = app
        .groups
        .iter()
        .position(|group| group.portal.is_some())
        .unwrap_or(app.groups.len());
    let mut target = to.min(limit);
    if target > from {
        target -= 1;
    }
    if target == from || target >= app.groups.len() {
        return;
    }
    let name = app.groups[from].name.clone();
    let group = app.groups.remove(from);
    app.groups.insert(target, group);
    crate::groups::save(&app.groups);
    log::info(&format!(
        "调整分组顺序：「{name}」第 {} 位 → 第 {} 位",
        from + 1,
        target + 1
    ));
    relayout();
}

/// 工具条上的「新建分组」：输入名字后建立一个空组。
fn create_group() {
    let Some(name) = crate::text_input::ask(crate::i18n::t("新建分组", "New group"), "") else {
        return;
    };
    let app = state();
    let Some(index) = crate::groups::create(&mut app.groups, &name) else {
        return;
    };
    crate::groups::save(&app.groups);
    log::info(&format!("新建分组「{}」", app.groups[index].name));
    relayout();
}

/// 点标题行：折叠/展开该组并保存。
fn toggle_group(index: usize) {
    let app = state();
    let Some(group) = app.groups.get_mut(index) else {
        return;
    };
    group.collapsed = !group.collapsed;
    let collapsed = group.collapsed;
    let portal_index = group.portal;
    let name = group.name.clone();
    if let Some(index) = portal_index {
        if let Some(portal) = app.portals.get_mut(index) {
            portal.collapsed = collapsed;
        }
        crate::portals::save(&app.portals);
    }
    crate::groups::save(&app.groups);
    log::info(&format!("分组「{name}」{}", if collapsed { "折叠" } else { "展开" }));
    relayout();
}

/// 右键图标：选择要移入的分组，或新建一个。
fn open_group_menu(item_index: usize) {
    let app = state();
    let Some(item) = app.items.get(item_index) else {
        return;
    };
    if item.source == icons::ItemSource::Portal {
        log::info("入口是只读视图：请右键入口标题管理（重命名/折叠/移除）");
        return;
    }
    let path = item.path.clone();
    let current = crate::groups::group_of(&app.groups, &path);
    // SAFETY: 菜单句柄在函数末尾释放；字符串在调用期间保持存活。
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        let labels: Vec<_> = app
            .groups
            .iter()
            .map(|group| {
                wide(&if crate::i18n::is_en() {
                    format!("Move to \"{}\"", crate::groups::display_name(&group.name))
                } else {
                    format!("移到「{}」", crate::groups::display_name(&group.name))
                })
            })
            .collect();
        let new_label = wide(crate::i18n::t("新建分组并移入…", "New group and move…"));
        let rename_label = wide(crate::i18n::t("重命名…", "Rename…"));
        let icon_label = wide(crate::i18n::t("更换图标…", "Change icon…"));
        let reset_icon_label = wide(crate::i18n::t("恢复默认图标", "Reset icon"));
        AppendMenuW(menu, MF_STRING, 2, rename_label.as_ptr());
        if app
            .items
            .get(item_index)
            .and_then(|item| item.custom_icon.as_ref())
            .is_some()
        {
            AppendMenuW(menu, MF_STRING, 4, reset_icon_label.as_ptr());
        }
        AppendMenuW(menu, MF_STRING, 3, icon_label.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        for (index, label) in labels.iter().enumerate() {
            let flags = if current == Some(index) {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            AppendMenuW(menu, flags, 200 + index, label.as_ptr());
        }
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(menu, MF_STRING, 1, new_label.as_ptr());

        let overlay = app.overlay;
        // 面板是全屏置顶，先放下来菜单才看得见。
        SetWindowPos(
            overlay,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let mut point = POINT::default();
        GetCursorPos(&mut point);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
            point.x,
            point.y,
            0,
            overlay,
            std::ptr::null(),
        );
        SetWindowPos(
            overlay,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        DestroyMenu(menu);
        apply_group_command(command as usize, &path, item_index);
    }
}

/// 右键分组标题：重命名 / 删除。
fn open_header_menu(group_index: usize) {
    let app = state();
    let Some(group) = app.groups.get(group_index) else {
        return;
    };
    let is_default = group.name == crate::groups::DEFAULT_GROUP;
    let collapsed = group.collapsed;
    // SAFETY: 菜单在函数末尾销毁；字符串在调用期间存活。
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        let portal_sort = group.portal.and_then(|index| app.portals.get(index)).map(|portal| portal.sort);
        let rename = wide(crate::i18n::t("重命名分组…", "Rename group…"));
        let toggle = wide(if collapsed {
            crate::i18n::t("展开该组", "Expand")
        } else {
            crate::i18n::t("折叠该组", "Collapse")
        });
        let delete = wide(if portal_sort.is_some() {
            crate::i18n::t("移除入口（目录内容不动）", "Remove folder entry (files untouched)")
        } else {
            crate::i18n::t(
                "删除分组（图标回到「其它」）",
                "Delete group (items go to \"Others\")",
            )
        });
        AppendMenuW(menu, MF_STRING, 10, rename.as_ptr());
        AppendMenuW(menu, MF_STRING, 11, toggle.as_ptr());
        // 入口分组多一份排序方式，当前项打勾。
        if let Some(current) = portal_sort {
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            let modes = [
                (20, crate::portals::SortMode::FolderFirst),
                (21, crate::portals::SortMode::Name),
                (22, crate::portals::SortMode::Time),
            ];
            let labels: Vec<_> = modes
                .iter()
                .map(|(_, mode)| wide(&format!("{} {}", crate::i18n::t("排序：", "Sort:"), mode.label())))
                .collect();
            for ((id, mode), label) in modes.iter().zip(labels.iter()) {
                let flags = if current == *mode {
                    MF_STRING | MF_CHECKED
                } else {
                    MF_STRING
                };
                AppendMenuW(menu, flags, *id, label.as_ptr());
            }
        }
        if !is_default {
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            AppendMenuW(menu, MF_STRING, 12, delete.as_ptr());
        }
        let overlay = app.overlay;
        SetWindowPos(
            overlay,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let mut point = POINT::default();
        GetCursorPos(&mut point);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
            point.x,
            point.y,
            0,
            overlay,
            std::ptr::null(),
        );
        SetWindowPos(
            overlay,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        DestroyMenu(menu);
        match command as usize {
            10 => rename_group(group_index),
            11 => toggle_group(group_index),
            12 => delete_group(group_index),
            20 => crate::app::set_portal_sort(group_index, crate::portals::SortMode::FolderFirst),
            21 => crate::app::set_portal_sort(group_index, crate::portals::SortMode::Name),
            22 => crate::app::set_portal_sort(group_index, crate::portals::SortMode::Time),
            _ => {}
        }
    }
}

fn rename_group(index: usize) {
    let current = state()
        .groups
        .get(index)
        .map(|group| group.name.clone())
        .unwrap_or_default();
    if current.is_empty() {
        return;
    }
    let Some(name) = crate::text_input::ask(crate::i18n::t("重命名分组", "Rename group"), &current) else {
        return;
    };
    let name = name.trim().to_string();
    if name.is_empty() || name == current {
        return;
    }
    let app = state();
    let Some(group) = app.groups.get_mut(index) else {
        return;
    };
    let portal_index = group.portal;
    group.name = name.clone();
    if let Some(index) = portal_index {
        if let Some(portal) = app.portals.get_mut(index) {
            portal.name = name.clone();
        }
        crate::portals::save(&app.portals);
    }
    crate::groups::save(&app.groups);
    log::info(&format!("分组改名：{current} → {name}"));
    relayout();
}

fn delete_group(index: usize) {
    let app = state();
    let Some(group) = app.groups.get(index) else {
        return;
    };
    if group.name == crate::groups::DEFAULT_GROUP {
        return;
    }
    let name = group.name.clone();
    if let Some(portal_index) = group.portal {
        // 只读入口：只摘掉入口本身，不碰目录里的文件。
        if portal_index < app.portals.len() {
            app.portals.remove(portal_index);
        }
        crate::portals::save(&app.portals);
        log::info(&format!("移除文件夹入口「{name}」（目录内容不动）"));
        crate::app::rebuild_portal_items();
        relayout();
        return;
    }
    app.groups.remove(index);
    crate::groups::save(&app.groups);
    log::info(&format!("删除分组「{name}」，图标回到其它"));
    relayout();
}

fn apply_group_command(command: usize, path: &std::path::Path, item_index: usize) {
    if command == 0 {
        return;
    }
    let app = state();
    if command == 1 {
        if let Some(name) = crate::text_input::ask(crate::i18n::t("新建分组", "New group"), "")
            && let Some(index) = crate::groups::create(&mut app.groups, &name)
        {
            crate::groups::assign(&mut app.groups, path, Some(index));
            crate::groups::save(&app.groups);
            log::info(&format!("新建分组「{name}」并移入 {}", path.display()));
            relayout();
        }
        return;
    }
    if command == 2 {
        let current = app
            .items
            .get(item_index)
            .map(|item| item.name.clone())
            .unwrap_or_default();
        if let Some(name) = crate::text_input::ask(crate::i18n::t("重命名图标", "Rename item"), &current) {
            crate::app::rename_item(item_index, &name);
        }
        return;
    }
    if command == 3 {
        let host = app.host;
        if let Some(icon) = crate::picker::pick_icon_file(host)
            && !crate::app::set_item_icon(item_index, &icon)
        {
            log::warn("图标文件无法使用（只支持 .ico）");
        }
        return;
    }
    if command == 4 {
        crate::app::clear_item_icon(item_index);
        return;
    }
    if command >= 200 {
        let target = command - 200;
        crate::groups::assign(&mut app.groups, path, Some(target));
        crate::groups::save(&app.groups);
        let name = app
            .groups
            .get(target)
            .map(|group| group.name.clone())
            .unwrap_or_default();
        log::info(&format!("移到分组「{name}」：{}", path.display()));
        relayout();
    }
}

/// 分节布局：每个分组一个标题行，下面是该组的图标网格。
pub fn compute_layout(
    items: &[crate::icons::DesktopItem],
    groups: &[crate::groups::Group],
    width: i32,
    height: i32,
    dpi: u32,
    top_dip: f32,
) -> Layout {
    let scale = dpi as f32 / 96.0;
    let zero = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let mut layout = Layout {
        scale,
        dpi,
        icon: (48.0 * scale).round() as i32,
        cell_w: (120.0 * scale).round() as i32,
        cell_h: (128.0 * scale).round() as i32,
        cols: 1,
        width,
        height,
        tiles: vec![zero; items.len() + 1],
        headers: Vec::new(),
        content_height: 0,
    };
    if groups.is_empty() {
        return layout;
    }
    // 按分组归集 item 序号；未归组的落到“其它”。
    let default_index = groups
        .iter()
        .position(|group| group.name == crate::groups::DEFAULT_GROUP)
        .unwrap_or(0);
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); groups.len()];
    for (index, item) in items.iter().enumerate() {
        let target = groups
            .iter()
            .position(|group| {
                group
                    .paths
                    .iter()
                    .any(|known| crate::icons::same_path(known, &item.path))
            })
            .unwrap_or(default_index);
        buckets[target].push(index);
    }
    // 「添加图标」按钮排进默认组的末尾。
    buckets[default_index].push(items.len());
    let widest = buckets.iter().map(Vec::len).max().unwrap_or(0).max(1);
    let aspect = width as f32 / height.max(1) as f32;
    let header_height = |scale: f32| (38.0 * scale) as i32;
    // 顶部要给工具条（低权限时还有警告横幅）让位。
    let top_inset = (top_dip * scale) as i32;
    // 当前列数下、按给定格子高度摆放需要多高（含标题行与组间距）。
    let estimate = |cols: usize, cell_h: i32| -> i32 {
        let mut total = top_inset;
        for (group, bucket) in groups.iter().zip(&buckets) {
            total += header_height(scale);
            if !group.collapsed && !bucket.is_empty() {
                let rows = bucket.len().div_ceil(cols.max(1)) as i32;
                total += rows * cell_h + (10.0 * scale) as i32;
            }
        }
        total
    };
    let cols_limit = |cell_w: i32| ((width - 80).max(cell_w) / cell_w).max(1) as usize;
    // 列数不会超过最大一组的项数——多出来的列只会把每组挤到左边。
    layout.cols = ((widest as f32 * aspect).sqrt().round() as usize)
        .clamp(1, cols_limit(layout.cell_w))
        .min(widest);
    // 格子最小只缩到 96×104（按 DPI 缩放），再小名字就看不清了；剩下的交给滚动。
    let floor_w = ((96.0 * scale) as i32).max(56);
    let floor_h = ((104.0 * scale) as i32).max(64);
    // 第一步：放不下就等比缩小格子。
    for _ in 0..6 {
        let total = estimate(layout.cols, layout.cell_h);
        if total <= height - 30 || layout.cell_h <= floor_h {
            break;
        }
        let factor = ((height - 40) as f32 / total as f32).clamp(0.5, 0.92);
        layout.cell_w = ((layout.cell_w as f32 * factor).round() as i32).max(floor_w);
        layout.cell_h = ((layout.cell_h as f32 * factor).round() as i32).max(floor_h);
        layout.icon = ((layout.cell_h as f32 * 0.38).round() as i32).max(28);
    }
    // 第二步：多开几列（行数变少），尽量在一屏里放下。
    while estimate(layout.cols, layout.cell_h) > height - 30
        && layout.cols < cols_limit(layout.cell_w)
        && layout.cols < widest
    {
        layout.cols += 1;
    }
    // 逐组摆放。
    let mut y = top_inset;
    for (group_index, (group, bucket)) in groups.iter().zip(&buckets).enumerate() {
        let header = RECT {
            left: (18.0 * scale) as i32,
            top: y,
            right: width - (18.0 * scale) as i32,
            bottom: y + header_height(scale),
        };
        layout.headers.push(Header {
            group: group_index,
            rect: header,
            count: bucket.len().saturating_sub(usize::from(bucket.contains(&items.len()))),
        });
        y = header.bottom;
        if group.collapsed || bucket.is_empty() {
            continue;
        }
        let grid_w = layout.cols as i32 * layout.cell_w;
        let origin_x = (width - grid_w) / 2;
        for (row, chunk) in bucket.chunks(layout.cols).enumerate() {
            for (col, item_index) in chunk.iter().enumerate() {
                let left = origin_x + col as i32 * layout.cell_w;
                let top = y + row as i32 * layout.cell_h;
                layout.tiles[*item_index] = RECT {
                    left,
                    top,
                    right: left + layout.cell_w,
                    bottom: top + layout.cell_h,
                };
            }
        }
        y += bucket.len().div_ceil(layout.cols) as i32 * layout.cell_h + (10.0 * scale) as i32;
    }
    layout.content_height = y;
    layout
}

/// 内容坐标（layout 里的 rect）→ 客户区坐标：减去滚动量。
fn view_rect(app: &crate::state::App, rect: RECT) -> RECT {
    RECT {
        left: rect.left,
        top: rect.top - app.scroll,
        right: rect.right,
        bottom: rect.bottom - app.scroll,
    }
}

/// 客户区坐标 → 内容坐标：加上滚动量。
fn content_point(app: &crate::state::App, x: i32, y: i32) -> POINT {
    POINT {
        x,
        y: y + app.scroll,
    }
}

/// 内容区顶边（工具条 / 低权限横幅之下），滚动裁剪和落点判定都用它。
fn content_top(app: &crate::state::App) -> i32 {
    let top_dip = if app.low_integrity { 88.0 } else { 48.0 };
    (top_dip * app.layout.scale) as i32
}

/// 还能往下滚多少像素。
fn max_scroll(app: &crate::state::App) -> i32 {
    // 底部留出提示条的高度，滚到底时最后一行不会被提示条压住。
    let pad = (56.0 * app.layout.scale) as i32;
    (app.layout.content_height + pad - app.layout.height).max(0)
}

/// 布局/DPI 变化后把滚动量夹回合法范围。
fn clamp_scroll(app: &mut crate::state::App) {
    app.scroll = app.scroll.clamp(0, max_scroll(app));
}

/// 当前应该吸顶的分组：标题已经滚出顶部、但这一组的内容还在可见区里。
fn sticky_group(app: &crate::state::App) -> Option<usize> {
    let top = content_top(app) + app.scroll;
    for (index, header) in app.layout.headers.iter().enumerate() {
        let end = app
            .layout
            .headers
            .get(index + 1)
            .map(|next| next.rect.top)
            .unwrap_or(app.layout.content_height);
        if header.rect.top < top && end > top {
            return Some(header.group);
        }
    }
    None
}

/// 吸顶标题在客户区里的矩形（绘制与点击判定共用）。
fn sticky_header(app: &crate::state::App) -> Option<(usize, RECT)> {
    let group = sticky_group(app)?;
    let header = app.layout.headers.iter().find(|header| header.group == group)?;
    let height = header.rect.bottom - header.rect.top;
    let top = content_top(app);
    Some((
        group,
        RECT {
            left: header.rect.left,
            top,
            right: header.rect.right,
            bottom: top + height,
        },
    ))
}

/// 拖入时鼠标所在的组：按分组标题行划分纵向区域，标题下面到下一个标题都算该组。
pub fn group_at_point(_x: i32, y: i32) -> Option<usize> {
    let app = state();
    // 换算到内容坐标后再按标题行划分。
    let y = y + app.scroll;
    if app.layout.headers.is_empty() || y < app.layout.headers[0].rect.top {
        return None;
    }
    app.layout
        .headers
        .iter()
        .rev()
        .find(|header| y >= header.rect.top)
        .map(|header| header.group)
}

/// 排障用：第一个分组标题行中心的屏幕坐标，--drop-test 拿它当落点。
pub fn first_group_screen_point() -> Option<(i32, i32)> {
    let app = state();
    let header = app.layout.headers.first()?;
    let mut point = POINT {
        x: (header.rect.left + header.rect.right) / 2,
        y: (header.rect.top + header.rect.bottom) / 2 - app.scroll,
    };
    // SAFETY: 句柄有效；只做坐标换算。
    let ok = unsafe { windows_sys::Win32::Graphics::Gdi::ClientToScreen(app.overlay, &mut point) };
    (ok != 0).then_some((point.x, point.y))
}

/// 一个分组占的整块区域：它自己的标题行到下一个标题行（或面板底部）。
fn group_section_rect(app: &crate::state::App, group: usize) -> Option<RECT> {
    let index = app
        .layout
        .headers
        .iter()
        .position(|header| header.group == group)?;
    let header = app.layout.headers.get(index)?;
    let bottom = app
        .layout
        .headers
        .get(index + 1)
        .map(|next| next.rect.top)
        .unwrap_or(app.layout.content_height + (24.0 * app.layout.scale) as i32);
    if bottom <= header.rect.top {
        return None;
    }
    Some(RECT {
        left: header.rect.left,
        top: header.rect.top,
        right: header.rect.right,
        bottom,
    })
}

/// 拖入时高亮“松手会落进哪个组”。只重贴缓存帧，不动静态帧。
pub fn set_drop_hover(group: Option<usize>) {
    let app = state();
    if app.drop_hover == group {
        return;
    }
    app.drop_hover = group;
    if app.shown {
        // SAFETY: 句柄有效；整屏重贴缓存再画/清除高亮。
        unsafe { InvalidateRect(app.overlay, std::ptr::null(), 0) };
    }
}

/// 立刻把失效区域画出来（排障/测试里没有消息循环时用）。
pub fn force_repaint() {
    let app = state();
    if !app.shown || app.overlay.is_null() {
        return;
    }
    // SAFETY: 句柄有效；只重画当前窗口。
    unsafe {
        InvalidateRect(app.overlay, std::ptr::null(), 0);
        UpdateWindow(app.overlay);
    }
}

/// 在底部提示条里显示一句拖入结果，四秒后自动消失。
pub fn show_drop_notice(text: String) {
    log::info(&format!("拖入结果：{text}"));
    let app = state();
    app.drop_hover = None;
    app.drop_notice = Some((text, Instant::now()));
    app.static_dirty = true;
    if app.shown {
        // SAFETY: 句柄有效；提示条在静态帧里，重画整帧。
        unsafe {
            InvalidateRect(app.overlay, std::ptr::null(), 0);
            UpdateWindow(app.overlay);
            SetTimer(app.overlay, TIMER_DROP_NOTICE, 4000, None);
        }
    }
}

fn anim_progress() -> f32 {
    let Some(started) = state().anim_started else {
        return 1.0;
    };
    let raw = (started.elapsed().as_secs_f32() / (ANIM_MS as f32 / 1000.0)).clamp(0.0, 1.0);
    // ease-out cubic
    1.0 - (1.0 - raw).powi(3)
}

fn animating() -> bool {
    state()
        .anim_started
        .is_some_and(|started| started.elapsed() < Duration::from_millis(ANIM_MS))
}

fn paint(hwnd: HWND) {
    // SAFETY: BeginPaint/EndPaint 成对，句柄来自 WM_PAINT。
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let app = state();
        let width = app.layout.width;
        let height = app.layout.height;
        ensure_buffer(hdc, width, height);
        let cache = state().buffer_dc;
        let app = state();
        if cache.is_null() {
            render_static(hdc, app, width, height);
        } else {
            let redrawn = app.static_dirty;
            if redrawn {
                render_static(cache, app, width, height);
                app.static_dirty = false;
            }
            // 亚克力模式靠窗口自身的浓度做动画（下面 paint 会照常贴图），其它模式用位图叠加淡入。
            if app.anim_started.is_some() && app.blur != BlurMode::Accent {
                let frame_started = Instant::now();
                // 入场动画只做淡入，**不做缩放**：缩放版每帧要重采样整屏，实测一帧 39ms，
                // 240ms 的动画直接掉到 20fps。1:1 叠加是几毫秒的量级，才可能流畅。
                // 第一帧先把唤出瞬间的整屏原图贴上去：窗口新建时表面是空的，
                // 不垫这一下会先黑一帧（看起来就是“闪一下”）。只贴一次，之后靠叠加，
                // 每帧都贴原图会多花一次整屏拷贝（实测帧率从 25 帧掉到 12 帧）。
                if app.anim_frames == 0
                    && let Some(base) = app.entrance.as_ref()
                {
                    base.blit(hdc);
                }
                app.anim_frames = app.anim_frames.saturating_add(1);
                let target = anim_progress();
                // 每帧叠在“上一帧结果”上，按目标值反算增量，整体曲线才不跑偏。
                let current = app.anim_alpha.clamp(0.0, 0.999);
                let step = ((target - current) / (1.0 - current)).clamp(0.0, 1.0);
                app.anim_alpha = target;
                let alpha = (step * 255.0).clamp(0.0, 255.0) as u8;
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: alpha,
                    AlphaFormat: 0,
                };
                // 淡入 + 上滑：整帧先从下往上挪一点点再归位。挪动只做 1:1 平移（不重采样），
                // 顶部露出的那条用底板位图按同样透明度补齐，所以不会露真实桌面、也不会掉帧。
                let slide = if app.backdrop.is_some() {
                    ((1.0 - target) * (20.0 * app.layout.scale)).round() as i32
                } else {
                    0
                };
                if slide > 0 && slide < height {
                    if let Some(backdrop) = app.backdrop.as_ref() {
                        backdrop.blend_into(
                            hdc,
                            RECT {
                                left: 0,
                                top: 0,
                                right: width,
                                bottom: slide,
                            },
                            alpha,
                        );
                    }
                    // SAFETY: 两个 DC 有效；源与目标尺寸一致，只是纵向平移。
                    AlphaBlend(
                        hdc,
                        0,
                        slide,
                        width,
                        height - slide,
                        cache,
                        0,
                        0,
                        width,
                        height - slide,
                        blend,
                    );
                } else {
                    // SAFETY: 两个 DC 有效；源是整屏 32 位 DIB，目标 DC 来自 BeginPaint。
                    AlphaBlend(hdc, 0, 0, width, height, cache, 0, 0, width, height, blend);
                }
                let frame_ms = frame_started.elapsed().as_millis();
                if frame_ms > 24 {
                    log::warn(&format!(
                        "入场动画单帧偏慢：{frame_ms}ms（{width}x{height}）"
                    ));
                }
            } else if redrawn {
                // 内容刚重画过：必须整屏贴一次，否则窗口上会“一半新一半旧”。
                // SAFETY: 两个 DC 都有效。
                BitBlt(hdc, 0, 0, width, height, cache, 0, 0, SRCCOPY);
            } else {
                // 只贴失效区域：拖动时每帧就是几百 KB 的拷贝。
                let region = ps.rcPaint;
                let region_w = region.right - region.left;
                let region_h = region.bottom - region.top;
                if region_w > 0 && region_h > 0 {
                    // SAFETY: 两个 DC 都有效，源与目标区域一致。
                    BitBlt(
                        hdc,
                        region.left,
                        region.top,
                        region_w,
                        region_h,
                        cache,
                        region.left,
                        region.top,
                        SRCCOPY,
                    );
                }
            }
        }
        // 工具条每帧都画（不进静态缓存），搜索栏展开动画只需重画这一小块。
        let full = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        let font = ui_font(app);
        if !font.is_null() {
            // SAFETY: 字体在进程内缓存。
            SelectObject(hdc, font as _);
        }
        // 动画期间工具条已经在缓存里（跟整帧一起淡入），结束后才每帧单独画。
        if app.anim_started.is_none() {
            draw_toolbar(hdc, app, &full);
            draw_search_box(hdc, app, &full);
        }
        draw_drop_hover(hdc, app);
        draw_scroll_indicator(hdc, app);
        if let Some(header) = app.header_drag
            && header.active
            && let Some(target) = header.target
        {
            draw_header_indicator(hdc, app, target);
        }
        if app.drag.is_some_and(|drag| drag.active) {
            draw_drag(hdc, app, &full);
        }
        EndPaint(hwnd, &ps);
    }
}

/// 准备（或重建）与窗口同尺寸的后备位图。
unsafe fn ensure_buffer(hdc: HDC, width: i32, height: i32) {
    let app = state();
    if !app.buffer_dc.is_null() && app.buffer_width == width && app.buffer_height == height {
        return;
    }
    // SAFETY: 只释放本模块创建的 GDI 对象。
    unsafe {
        if !app.buffer_dc.is_null() {
            DeleteDC(app.buffer_dc);
            app.buffer_dc = std::ptr::null_mut();
        }
        if app.buffer_bitmap != 0 {
            DeleteObject(app.buffer_bitmap as _);
            app.buffer_bitmap = 0;
        }
    }
    let header = windows_sys::Win32::Graphics::Gdi::BITMAPINFO {
        bmiHeader: windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER {
            biSize: std::mem::size_of::<windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: windows_sys::Win32::Graphics::Gdi::BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: 参数与 GDI 约定一致。
    let memory = unsafe { CreateCompatibleDC(hdc) };
    let bitmap = unsafe {
        CreateDIBSection(
            memory,
            &header,
            windows_sys::Win32::Graphics::Gdi::DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        )
    };
    if !memory.is_null() && !bitmap.is_null() && !bits.is_null() {
        // SAFETY: 位图选入内存 DC，供后续绘制。
        unsafe {
            SelectObject(memory, bitmap as _);
        }
        let app = state();
        app.buffer_dc = memory;
        app.buffer_bitmap = bitmap as isize;
        app.buffer_width = width;
        app.buffer_height = height;
        app.static_dirty = true;
    } else {
        // SAFETY: 创建失败时释放半成品。
        unsafe {
            if !memory.is_null() {
                DeleteDC(memory);
            }
            if !bitmap.is_null() {
                DeleteObject(bitmap as _);
            }
        }
    }
}

/// 画“静态帧”：底板 + 图标 + 文字。只在内容变化时调用。
fn render_static(hdc: HDC, app: &mut crate::state::App, width: i32, height: i32) {
    // SAFETY: 调用方保证 DC 有效。
    unsafe {
        if let Some(backdrop) = app.backdrop.as_ref() {
            backdrop.draw(hdc, width, height);
        } else if app.blur == BlurMode::Dim || app.blur == BlurMode::Off {
            let brush = CreateSolidBrush(app.theme.bar as COLORREF);
            let full = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            FillRect(hdc, &full, brush);
            DeleteObject(brush as _);
        }
        // 字体走进程内缓存，避免每次重画都 CreateFont。
        let font = ui_font(app);
        if !font.is_null() {
            // SAFETY: 字体在进程内缓存。
            SelectObject(hdc, font as _);
        }
        let full = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        draw_banner(hdc, app, &full);
        // 滚动时内容会移到工具条/横幅底下，这里把内容裁到可见区里。
        let saved = SaveDC(hdc);
        IntersectClipRect(hdc, 0, content_top(app), width, height);
        draw_headers(hdc, app, &full);
        draw_items(hdc, app, &full);
        draw_sticky_header(hdc, app);
        RestoreDC(hdc, saved);
        draw_hint(hdc, app, &full);
        // 入场动画期间把工具条也画进缓存，好让它跟着整帧一起淡入。
        if app.anim_started.is_some() {
            draw_toolbar(hdc, app, &full);
            draw_search_box(hdc, app, &full);
        }
    }
}

fn create_font(scale: f32, pixels: f32) -> HFONT {
    let font_name = wide("Microsoft YaHei UI");
    // SAFETY: 字体名以 0 结尾，负高度表示字符高度。
    unsafe {
        CreateFontW(
            -((pixels * scale).round() as i32),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            // 灰度抗锯齿在亚克力/分层场景下兼容性最好。
            ANTIALIASED_QUALITY as u32,
            FF_DONTCARE as u32,
            font_name.as_ptr(),
        )
    }
}

fn intersects(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
}

/// 跟随图标占据的矩形（略放大，保证旧画面被完整覆盖）。
fn ghost_rect(point: (i32, i32), icon: i32) -> RECT {
    let margin = icon / 2 + 6;
    RECT {
        left: point.0 - margin,
        top: point.1 - margin,
        right: point.0 + margin,
        bottom: point.1 + margin,
    }
}

/// 分组标题行：折叠箭头 + 组名 + 数量。滚动时，已经滚出顶部但内容还可见的那组吸顶。
fn draw_headers(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    let scale = app.layout.scale;
    let sticky = sticky_group(app);
    for header in &app.layout.headers {
        if Some(header.group) == sticky {
            // 这一组改成吸顶绘制，原位置那份跳过，免得重复。
            continue;
        }
        let rect = view_rect(app, header.rect);
        if !intersects(&rect, paint) {
            continue;
        }
        draw_one_header(hdc, app, header, rect, scale);
    }
}

/// 吸顶标题单独画：必须压在图标之上，否则滚过去的图标会盖住它。
fn draw_sticky_header(hdc: HDC, app: &crate::state::App) {
    let Some(group) = sticky_group(app) else {
        return;
    };
    let Some(header) = app.layout.headers.iter().find(|h| h.group == group) else {
        return;
    };
    let scale = app.layout.scale;
    let rect = sticky_header(app).map(|(_, rect)| rect).unwrap_or(header.rect);
    // 吸顶条必须有底色，否则滚过去的图标会透到标题上。
    unsafe {
        let brush = CreateSolidBrush(app.theme.bar);
        FillRect(hdc, &rect, brush);
        DeleteObject(brush as _);
    }
    draw_one_header(hdc, app, header, rect, scale);
}

fn draw_one_header(hdc: HDC, app: &crate::state::App, header: &Header, rect: RECT, scale: f32) {
    let Some(group) = app.groups.get(header.group) else {
        return;
    };
    let count = header.count;
    let arrow = if group.collapsed { "▶" } else { "▼" };
    let text = wide(&format!(
        "{arrow}  {}  ({count})",
        crate::groups::display_name(&group.name)
    ));
    // SAFETY: 画笔与底色用完即释放。
    unsafe {
        let line = RECT {
            left: rect.left,
            top: rect.bottom - 1,
            right: rect.right,
            bottom: rect.bottom,
        };
        let brush = CreateSolidBrush(app.theme.line);
        FillRect(hdc, &line, brush);
        DeleteObject(brush as _);
        let mut text_rect = RECT {
            left: rect.left + (6.0 * scale) as i32,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        };
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, app.theme.text_dim);
        DrawTextW(
            hdc,
            text.as_ptr(),
            -1,
            &mut text_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
    }
}

fn draw_items(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    let scale = app.layout.scale;
    // 注：在玻璃/亚克力表面上无法“擦除”已画像素，所以不做悬停描边，
    // 避免移动鼠标后留下残影。

    // SAFETY: 所有 GDI 对象都在本函数内创建并在末尾释放。
    unsafe {
        SetBkMode(hdc, TRANSPARENT as i32);
        let tile_count = (app.items.len() + 1).min(app.layout.tiles.len());
        for index in 0..tile_count {
            let Some(raw) = app.layout.tiles.get(index).copied() else {
                continue;
            };
            let tile = view_rect(app, raw);
            if !intersects(&tile, paint) || tile.right <= tile.left {
                continue;
            }
            let icon = app.layout.icon;
            if index == app.items.len() {
                draw_add_tile(hdc, tile, scale, icon, app.theme);
                continue;
            }
            let item = &app.items[index];
            if app.drag_hidden == Some(index) || !visible(app, index) {
                // 拖动中的图标由跟随图层绘制；搜索时未命中的先留空。
                continue;
            }
            if !item.icon.is_null() {
                let x = tile.left + (app.layout.cell_w - icon) / 2;
                let y = tile.top + ((12.0 * scale) as i32);
                DrawIconEx(hdc, x, y, item.icon, icon, icon, 0, std::ptr::null_mut(), DI_NORMAL);
            }
            draw_label(hdc, &item.name, tile, scale, icon, app.theme);
        }
    }
}

/// 图标下方的名字：先画深色阴影，再画白字，浅色壁纸上也能看清。
unsafe fn draw_label(
    hdc: HDC,
    name: &str,
    tile: RECT,
    scale: f32,
    icon: i32,
    theme: &crate::theme::Theme,
) {
    let label_top = tile.top + ((12.0 * scale) as i32) + icon + (6.0 * scale) as i32;
    let mut rect = RECT {
        left: tile.left + (4.0 * scale) as i32,
        top: label_top,
        right: tile.right - (4.0 * scale) as i32,
        bottom: label_top + ((34.0 * scale) as i32).max(24),
    };
    let text = wide(name);
    let flags = DT_CENTER | DT_WORDBREAK | DT_END_ELLIPSIS | DT_NOPREFIX;
    let mut shadow = rect;
    shadow.left += 1;
    shadow.top += 1;
    shadow.right += 1;
    shadow.bottom += 1;
    // SAFETY: 字体已由 paint() 选进 DC。
    unsafe {
        SetTextColor(hdc, theme.label_shadow);
        DrawTextW(hdc, text.as_ptr(), -1, &mut shadow, flags);
        SetTextColor(hdc, theme.text);
        DrawTextW(hdc, text.as_ptr(), -1, &mut rect, flags);
    }
}

/// 网格末尾的「添加图标」方块：一个方框加十字（纯 FillRect，不依赖字体）。
unsafe fn draw_add_tile(
    hdc: HDC,
    tile: RECT,
    scale: f32,
    icon: i32,
    theme: &crate::theme::Theme,
) {
    let cell_w = tile.right - tile.left;
    let size = icon + (8.0 * scale) as i32;
    let left = tile.left + (cell_w - size) / 2;
    let top = tile.top + (12.0 * scale) as i32;
    let thickness = (2.0 * scale).max(2.0) as i32;
    // SAFETY: 画刷在本函数内创建并释放。
    unsafe {
        let brush = CreateSolidBrush(theme.text_dim);
        let frame = [
            RECT { left, top, right: left + size, bottom: top + thickness },
            RECT { left, top: top + size - thickness, right: left + size, bottom: top + size },
            RECT { left, top, right: left + thickness, bottom: top + size },
            RECT { left: left + size - thickness, top, right: left + size, bottom: top + size },
        ];
        for part in frame {
            FillRect(hdc, &part, brush);
        }
        let center_x = left + size / 2;
        let center_y = top + size / 2;
        let arm = (size as f32 * 0.30) as i32;
        let bar = (4.0 * scale).max(3.0) as i32;
        FillRect(
            hdc,
            &RECT {
                left: center_x - arm,
                top: center_y - bar / 2,
                right: center_x + arm,
                bottom: center_y + bar / 2,
            },
            brush,
        );
        FillRect(
            hdc,
            &RECT {
                left: center_x - bar / 2,
                top: center_y - arm,
                right: center_x + bar / 2,
                bottom: center_y + arm,
            },
            brush,
        );
        DeleteObject(brush as _);
        draw_label(
            hdc,
            crate::i18n::t("添加图标", "Add"),
            tile,
            scale,
            icon,
            theme,
        );
    }
}

/// 从缓存 DC 贴回一小块（用于擦掉上一帧的跟随图标/描边）。
unsafe fn blit_region(hdc: HDC, cache: HDC, rect: RECT) {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return;
    }
    // SAFETY: 两个 DC 都有效，源与目标区域一致。
    unsafe {
        BitBlt(
            hdc,
            rect.left,
            rect.top,
            width,
            height,
            cache,
            rect.left,
            rect.top,
            SRCCOPY,
        );
    }
}

/// 目标格子的白色描边。
unsafe fn draw_target_outline(hdc: HDC, tile: RECT, scale: f32, theme: &crate::theme::Theme) {
    // SAFETY: 画笔与画刷在本段内创建并释放。
    unsafe { draw_outline(hdc, tile, scale, theme.text) };
}

/// 给格子画一圈描边（颜色用 COLORREF）。
unsafe fn draw_outline(hdc: HDC, tile: RECT, scale: f32, color: u32) {
    // SAFETY: 画笔与画刷在本段内创建并释放。
    unsafe {
        let pen = CreatePen(PS_SOLID, (2.0 * scale).max(2.0) as i32, color);
        let hollow = GetStockObject(HOLLOW_BRUSH);
        let previous = SelectObject(hdc, pen as _);
        SelectObject(hdc, hollow);
        RoundRect(
            hdc,
            tile.left + 4,
            tile.top + 4,
            tile.right - 4,
            tile.bottom - 4,
            (14.0 * scale) as i32,
            (14.0 * scale) as i32,
        );
        SelectObject(hdc, previous);
        DeleteObject(pen as _);
    }
}

/// 跟着鼠标的图标。
unsafe fn draw_ghost_icon(hdc: HDC, app: &crate::state::App, index: usize, point: (i32, i32)) {
    let Some(item) = app.items.get(index) else {
        return;
    };
    if item.icon.is_null() {
        return;
    }
    let size = app.layout.icon;
    // SAFETY: 图标句柄有效。
    unsafe {
        DrawIconEx(
            hdc,
            point.0 - size / 2,
            point.1 - size / 2,
            item.icon,
            size,
            size,
            0,
            std::ptr::null_mut(),
            DI_NORMAL,
        );
    }
}

/// 确保缓存帧与当前布局一致（拖动前必须调用，否则贴回的是旧画面）。
fn ensure_cache(hwnd: HWND) {
    let app = state();
    if !app.static_dirty {
        return;
    }
    let width = app.layout.width;
    let height = app.layout.height;
    let cache = app.buffer_dc;
    if cache.is_null() {
        return;
    }
    // SAFETY: 句柄与 DC 有效。
    unsafe {
        let hdc = GetDC(hwnd);
        if hdc.is_null() {
            return;
        }
        let app = state();
        render_static(cache, app, width, height);
        app.static_dirty = false;
        BitBlt(hdc, 0, 0, width, height, cache, 0, 0, SRCCOPY);
        ReleaseDC(hwnd, hdc);
    }
}

/// 右侧滚动条：只有内容比面板高时才出现。
fn draw_scroll_indicator(hdc: HDC, app: &crate::state::App) {
    let max = max_scroll(app);
    if max <= 0 {
        return;
    }
    let scale = app.layout.scale;
    let track_top = (60.0 * scale) as i32;
    let track_bottom = app.layout.height - (60.0 * scale) as i32;
    let visible = (track_bottom - track_top) as f32;
    if visible < 80.0 {
        return;
    }
    let x = app.layout.width - (12.0 * scale) as i32;
    let width = (5.0 * scale).max(3.0) as i32;
    let ratio = (app.layout.height as f32 / (app.layout.height + max) as f32).clamp(0.08, 1.0);
    let thumb_h = (visible * ratio).max(24.0);
    let progress = (app.scroll as f32 / max as f32).clamp(0.0, 1.0);
    let thumb_top = track_top as f32 + (visible - thumb_h) * progress;
    // SAFETY: 画刷用完即释放。
    unsafe {
        let track = RECT {
            left: x,
            top: track_top,
            right: x + width,
            bottom: track_bottom,
        };
        let brush = CreateSolidBrush(app.theme.line);
        FillRect(hdc, &track, brush);
        DeleteObject(brush as _);
        let thumb = RECT {
            left: x,
            top: thumb_top as i32,
            right: x + width,
            bottom: thumb_top as i32 + thumb_h as i32,
        };
        let brush = CreateSolidBrush(app.theme.accent);
        FillRect(hdc, &thumb, brush);
        DeleteObject(brush as _);
    }
}


/// 拖动标题时的插入位置提示：整条细线，中间略亮、两端略暗，避免一整条死板的亮条。
fn draw_header_indicator(hdc: HDC, app: &crate::state::App, target: usize) {
    // 插在原来的位置（自己前面或自己后面）不会改变顺序，就别画了。
    if let Some(drag) = app.header_drag
        && (target == drag.from || target == drag.from + 1)
    {
        return;
    }
    let rect = header_indicator_rect(app, target);
    let scale = app.layout.scale;
    let y = (rect.top + rect.bottom) / 2;
    let inset = (18.0 * scale) as i32;
    let left = inset;
    let right = app.layout.width - inset;
    let thickness = ((3.0 * scale) as i32).max(3);
    let top = y - thickness / 2;
    let bottom = top + thickness;
    if right <= left {
        return;
    }
    // 96 段线性渐变：中间 #7FB4FF，两端 #4A6B92（不会暗到看不见，保持是一条连续的线）。
    const SEGMENTS: i32 = 96;
    let span = (right - left).max(1);
    // SAFETY: 每段画刷单独创建、画完即释放；字体已由 paint 选好。
    unsafe {
        for index in 0..SEGMENTS {
            let start = left + span * index / SEGMENTS;
            let end = left + span * (index + 1) / SEGMENTS;
            if end <= start {
                continue;
            }
            let t = (index as f32 + 0.5) / SEGMENTS as f32;
            // 0 = 两端，1 = 中间。
            let centered = 1.0 - (2.0 * t - 1.0).abs();
            let amount = centered.powf(0.55);
            // 两端淡向工具条底色，中间用主题强调色。
            let ends = crate::theme::mix(app.theme.accent, app.theme.bar, 0.55);
            let color = crate::theme::mix(ends, app.theme.accent, amount);
            let brush = CreateSolidBrush(color);
            let piece = RECT {
                left: start,
                top,
                right: end,
                bottom,
            };
            FillRect(hdc, &piece, brush);
            DeleteObject(brush as _);
        }
    }
}

/// 拖入时给目标分组画框：标题行填色 + 整块描边，告诉用户“松手就加到这里”。
fn draw_drop_hover(hdc: HDC, app: &crate::state::App) {
    let Some(group) = app.drop_hover else {
        return;
    };
    let Some(section) = group_section_rect(app, group) else {
        return;
    };
    let Some(header) = app
        .layout
        .headers
        .iter()
        .find(|header| header.group == group)
        .cloned()
    else {
        return;
    };
    let scale = app.layout.scale;
    let section = view_rect(app, section);
    let header_rect = view_rect(app, header.rect);
    // SAFETY: 画刷用完即释放；字体已由 paint 选好；只画在内容区里，别压到工具条。
    unsafe {
        let saved = SaveDC(hdc);
        IntersectClipRect(hdc, 0, content_top(app), app.layout.width, app.layout.height);
        let brush = CreateSolidBrush(app.theme.drop_bg);
        FillRect(hdc, &header_rect, brush);
        DeleteObject(brush as _);
        draw_outline(hdc, section, scale, app.theme.drop_line);
        RestoreDC(hdc, saved);
    }
    // 填色盖住了标题文字，补画一次。
    draw_headers(hdc, app, &header_rect);
}

/// 拖动排序的视觉反馈：目标格子描边 + 跟着鼠标的图标。
fn draw_drag(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    let Some(drag) = app.drag else {
        return;
    };
    if !drag.active {
        return;
    }
    let scale = app.layout.scale;
    if let Some(target) = hit_test(drag.current.0, drag.current.1)
        && target < app.items.len()
        && let Some(raw) = app.layout.tiles.get(target).copied()
    {
        let tile = view_rect(app, raw);
        if intersects(&tile, paint) {
            // SAFETY: DC 有效。
            unsafe { draw_target_outline(hdc, tile, scale, app.theme) };
        }
    }
    // SAFETY: DC 有效。
    unsafe { draw_ghost_icon(hdc, app, drag.from, drag.current) };
}

/// 顶部工具条：整条横贯面板，高度随 DPI 缩放；低权限时让开警告横幅。
fn toolbar_rect(app: &crate::state::App) -> RECT {
    let scale = app.layout.scale;
    let top = if app.low_integrity {
        (40.0 * scale) as i32
    } else {
        0
    };
    RECT {
        left: 0,
        top,
        right: app.layout.width,
        bottom: top + (46.0 * scale) as i32,
    }
}

/// 工具条左侧「设置」按钮。
fn settings_chip_rect(app: &crate::state::App) -> RECT {
    let scale = app.layout.scale;
    let bar = toolbar_rect(app);
    let width = (96.0 * scale) as i32;
    let height = (32.0 * scale) as i32;
    let top = bar.top + (bar.bottom - bar.top - height) / 2;
    RECT {
        left: (14.0 * scale) as i32,
        top,
        right: (14.0 * scale) as i32 + width,
        bottom: top + height,
    }
}

/// 工具条左侧第二个按钮：新建分组。
fn new_group_chip_rect(app: &crate::state::App) -> RECT {
    let scale = app.layout.scale;
    let bar = toolbar_rect(app);
    let height = (32.0 * scale) as i32;
    let top = bar.top + (bar.bottom - bar.top - height) / 2;
    let left = settings_chip_rect(app).right + (10.0 * scale) as i32;
    let width = (124.0 * scale) as i32;
    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// 工具条右侧「移除」按钮。
fn remove_chip_rect(app: &crate::state::App) -> RECT {
    let scale = app.layout.scale;
    let bar = toolbar_rect(app);
    let width = (96.0 * scale) as i32;
    let height = (32.0 * scale) as i32;
    let top = bar.top + (bar.bottom - bar.top - height) / 2;
    let right = app.layout.width - (14.0 * scale) as i32;
    RECT {
        left: right - width,
        top,
        right,
        bottom: top + height,
    }
}

/// 画顶部工具条与两个按钮。
fn draw_toolbar(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    let bar = toolbar_rect(app);
    if !intersects(&bar, paint) {
        return;
    }
    let scale = app.layout.scale;
    // SAFETY: 画刷与字体都在本函数内用完即释放。
    unsafe {
        let brush = CreateSolidBrush(app.theme.bar);
        FillRect(hdc, &bar, brush);
        DeleteObject(brush as _);
        // 底边一条分隔线
        let line = RECT {
            left: bar.left,
            top: bar.bottom - 1,
            right: bar.right,
            bottom: bar.bottom,
        };
        let line_brush = CreateSolidBrush(app.theme.line);
        FillRect(hdc, &line, line_brush);
        DeleteObject(line_brush as _);
    }
    let theme = app.theme;
    draw_chip(
        hdc,
        settings_chip_rect(app),
        crate::i18n::t("设置", "Settings"),
        theme.chip,
        theme.text,
        scale,
    );
    draw_chip(
        hdc,
        new_group_chip_rect(app),
        crate::i18n::t("＋ 新建分组", "+ New group"),
        theme.chip,
        theme.text,
        scale,
    );
    draw_chip(
        hdc,
        remove_chip_rect(app),
        if app.remove_mode {
            crate::i18n::t("移除中", "Removing")
        } else {
            crate::i18n::t("移除", "Remove")
        },
        if app.remove_mode { theme.chip_hot } else { theme.chip },
        if app.remove_mode { theme.warn_text } else { theme.text },
        scale,
    );
}

fn draw_chip(hdc: HDC, chip: RECT, label: &str, background: u32, foreground: u32, scale: f32) {
    let text = wide(label);
    // SAFETY: 画刷在函数内创建并释放；字体由 paint 选好。
    unsafe {
        let brush = CreateSolidBrush(background);
        let null_pen = GetStockObject(NULL_PEN);
        let old_brush = SelectObject(hdc, brush as _);
        let old_pen = SelectObject(hdc, null_pen);
        RoundRect(
            hdc,
            chip.left,
            chip.top,
            chip.right,
            chip.bottom,
            (16.0 * scale) as i32,
            (16.0 * scale) as i32,
        );
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        DeleteObject(brush as _);
        let mut rect = chip;
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, foreground);
        DrawTextW(
            hdc,
            text.as_ptr(),
            -1,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
    }
}

/// 打开设置菜单：先把面板从置顶拿下来，菜单关掉后再恢复。
fn open_settings() {
    let app = state();
    let overlay = app.overlay;
    // SAFETY: 句柄有效。
    unsafe {
        SetWindowPos(
            overlay,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    crate::app::open_settings_menu();
    // SAFETY: 同上。
    unsafe {
        SetWindowPos(
            overlay,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// 工具条中间的搜索框：收起时只有一个放大镜按钮，展开动画到固定宽度。
fn search_rect(app: &crate::state::App) -> RECT {
    let scale = app.layout.scale;
    let bar = toolbar_rect(app);
    let height = (32.0 * scale) as i32;
    let top = bar.top + (bar.bottom - bar.top - height) / 2;
    let left = new_group_chip_rect(app).right + (12.0 * scale) as i32;
    let collapsed = (36.0 * scale) as i32;
    let expanded = (360.0 * scale) as i32;
    let width = if app.search_open && app.search_anim.is_none() {
        expanded
    } else if let Some(started) = app.search_anim {
        let raw = (started.elapsed().as_secs_f32() / 0.16).clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - raw).powi(3);
        let to = if app.search_open { expanded } else { collapsed };
        let from = if app.search_open { collapsed } else { expanded };
        (from as f32 + (to - from) as f32 * eased) as i32
    } else {
        collapsed
    };
    RECT {
        left,
        top,
        right: left + width.max(collapsed / 2),
        bottom: top + height,
    }
}

/// 当前关键字下某一项是否显示。
fn visible(app: &crate::state::App, index: usize) -> bool {
    if app.search.is_empty() {
        return true;
    }
    let query = app.search.to_lowercase();
    app.items
        .get(index)
        .is_some_and(|item| item.name.to_lowercase().contains(&query))
}

/// 画工具栏里的搜索框：收起时是放大镜，展开后直接在工具条上显示关键字（无底板）。
fn draw_search_box(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    let box_rect = search_rect(app);
    if !intersects(&box_rect, paint) {
        return;
    }
    let scale = app.layout.scale;
    let expanded = (box_rect.right - box_rect.left) > (70.0 * scale) as i32;
    if expanded {
        let (text, color) = if app.search.is_empty() {
            (
                crate::i18n::t("输入关键字…", "Type to search…").to_string(),
                app.theme.text_dim,
            )
        } else {
            (
                format!("{}　（{}）", app.search, count_matches(app)),
                app.theme.text,
            )
        };
        let label = wide(&text);
        // SAFETY: DC 与缓存字体有效；只画文字和下划线，不画底板（保持透明观感）。
        unsafe {
            let mut rect = RECT {
                left: box_rect.left + (10.0 * scale) as i32,
                top: box_rect.top,
                right: box_rect.right - (6.0 * scale) as i32,
                bottom: box_rect.bottom,
            };
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, color);
            DrawTextW(
                hdc,
                label.as_ptr(),
                -1,
                &mut rect,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            if app.search_typing {
                let underline = RECT {
                    left: box_rect.left + (10.0 * scale) as i32,
                    top: box_rect.bottom - (4.0 * scale) as i32,
                    right: box_rect.right - (6.0 * scale) as i32,
                    bottom: box_rect.bottom - (2.0 * scale) as i32,
                };
                let brush = CreateSolidBrush(app.theme.accent);
                FillRect(hdc, &underline, brush);
                DeleteObject(brush as _);
            }
        }
    } else {
        // 放大镜：一个圆圈加一根手柄。
        let size = (15.0 * scale).max(12.0) as i32;
        let center_x = (box_rect.left + box_rect.right) / 2 - 1;
        let center_y = (box_rect.top + box_rect.bottom) / 2 - 1;
        // SAFETY: 画笔与画刷用完即释放。
        unsafe {
            let pen = CreatePen(PS_SOLID, (2.0 * scale).max(2.0) as i32, app.theme.text_dim);
            let hollow = GetStockObject(HOLLOW_BRUSH);
            let old_pen = SelectObject(hdc, pen as _);
            let old_brush = SelectObject(hdc, hollow);
            Ellipse(
                hdc,
                center_x - size / 2,
                center_y - size / 2,
                center_x + size / 2,
                center_y + size / 2,
            );
            MoveToEx(hdc, center_x + size / 2 - 1, center_y + size / 2 - 1, std::ptr::null_mut());
            LineTo(hdc, center_x + size / 2 + 3, center_y + size / 2 + 3);
            SelectObject(hdc, old_pen);
            SelectObject(hdc, old_brush);
            DeleteObject(pen as _);
        }
    }
}

fn count_matches(app: &crate::state::App) -> usize {
    (0..app.items.len()).filter(|index| visible(app, *index)).count()
}

/// 让面板临时接收键盘：去掉 NOACTIVATE 并抢到前台，搜索结束时再恢复。
fn activate_typing() {
    let app = state();
    // SAFETY: 句柄有效；只临时改扩展样式。
    unsafe {
        let style = GetWindowLongPtrW(app.overlay, GWL_EXSTYLE);
        SetWindowLongPtrW(
            app.overlay,
            GWL_EXSTYLE,
            style & !(WS_EX_NOACTIVATE as isize),
        );
        SetForegroundWindow(app.overlay);
        SetFocus(app.overlay);
    }
    app.search_typing = true;
    log::info("搜索输入已激活");
}

fn deactivate_typing() {
    let app = state();
    if !app.search_typing {
        return;
    }
    app.search_typing = false;
    // SAFETY: 恢复“不抢焦点”的行为。
    unsafe {
        let style = GetWindowLongPtrW(app.overlay, GWL_EXSTYLE);
        SetWindowLongPtrW(
            app.overlay,
            GWL_EXSTYLE,
            style | (WS_EX_NOACTIVATE as isize),
        );
    }
}

/// 点放大镜：展开或收起搜索栏（带动画）。
fn toggle_search() {
    let app = state();
    if app.search_open {
        // 收起：退出输入态，动画缩回，并清空过滤。
        deactivate_typing();
        app.search_open = false;
        app.search_anim = Some(Instant::now());
        crate::app::set_search_text("");
    } else {
        app.search_open = true;
        app.search_anim = Some(Instant::now());
    }
    // SAFETY: 句柄有效；只重画工具条这一块。
    unsafe {
        let bar = toolbar_rect(app);
        InvalidateRect(app.overlay, &bar, 0);
        SetTimer(app.overlay, TIMER_SEARCH, 16, None);
        UpdateWindow(app.overlay);
    }
    log::info(&format!("搜索栏：{}", if app.search_open { "展开" } else { "收起" }));
}

/// 界面字体：按缩放只创建一次，避免每帧重建导致动画卡顿。
fn ui_font(app: &mut crate::state::App) -> HFONT {
    let scale = app.layout.scale;
    if !app.font.is_null() && (app.font_scale - scale).abs() < 0.01 {
        return app.font;
    }
    if !app.font.is_null() {
        // SAFETY: 字体由本模块创建。
        unsafe { DeleteObject(app.font as _) };
        app.font = std::ptr::null_mut();
    }
    let font = create_font(scale, 13.0);
    app.font = font;
    app.font_scale = scale;
    font
}

fn draw_hint(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    let scale = app.layout.scale;
    let theme = app.theme;
    let (text, background, foreground) = if let Some((notice, _)) = app.drop_notice.as_ref() {
        (notice.clone(), crate::theme::mix(theme.hint_bg, theme.drop_line, 0.35), theme.text)
    } else if app.remove_mode {
        (
            crate::i18n::t(
                "移除模式：单击图标从面板移除（源文件不动）　·　再点右上角或按 Esc 退出",
                "Remove mode: click an icon to remove it from the panel (files untouched)　·　click Remove again or press Esc to exit",
            )
            .to_string(),
            theme.chip_hot,
            theme.warn_text,
        )
    } else if !app.search.is_empty() {
        (
            if crate::i18n::is_en() {
                format!("Search \"{}\": {} match(es)　·　Esc to clear", app.search, count_matches(app))
            } else {
                format!("搜索「{}」：{} 个匹配　·　Esc 清除搜索", app.search, count_matches(app))
            },
            theme.hint_bg,
            theme.text,
        )
    } else {
        (
            crate::i18n::t(
                "拖动图标排序　·　单击打开　·　拖入文件即加入　·　单击空白处或按 Esc 收起",
                "Drag to reorder　·　Click to open　·　Drop files to add　·　Click empty space or press Esc to close",
            )
            .to_string(),
            theme.hint_bg,
            theme.text,
        )
    };
    let text = wide(&text);
    let font = create_font(scale, 14.0);
    // SAFETY: 字体用完恢复并释放。
    unsafe {
        let previous = if font.is_null() {
            std::ptr::null_mut()
        } else {
            SelectObject(hdc, font as _)
        };
        // 先量文字尺寸，再画一个深色胶囊当底，保证醒目。
        let mut measure = RECT {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 100,
        };
        DrawTextW(
            hdc,
            text.as_ptr(),
            -1,
            &mut measure,
            DT_CENTER | DT_SINGLELINE | DT_CALCRECT | DT_NOPREFIX,
        );
        let text_width = measure.right - measure.left;
        let text_height = measure.bottom - measure.top;
        let padding_x = (22.0 * scale) as i32;
        let padding_y = (12.0 * scale) as i32;
        let pill_width = text_width + padding_x * 2;
        let pill_height = text_height + padding_y * 2;
        let pill = RECT {
            left: (app.layout.width - pill_width) / 2,
            top: app.layout.height - pill_height - (28.0 * scale) as i32,
            right: (app.layout.width + pill_width) / 2,
            bottom: app.layout.height - (28.0 * scale) as i32,
        };
        if intersects(&pill, paint) {
            let brush = CreateSolidBrush(background);
            let null_pen = GetStockObject(NULL_PEN);
            let old_brush = SelectObject(hdc, brush as _);
            let old_pen = SelectObject(hdc, null_pen);
            RoundRect(
                hdc,
                pill.left,
                pill.top,
                pill.right,
                pill.bottom,
                pill_height,
                pill_height,
            );
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            DeleteObject(brush as _);
            let mut rect = pill;
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, foreground);
            DrawTextW(
                hdc,
                text.as_ptr(),
                -1,
                &mut rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
        if !previous.is_null() {
            SelectObject(hdc, previous);
        }
        if !font.is_null() {
            DeleteObject(font as _);
        }
    }
}
/// 受限权限运行时在顶部画一条警告横幅，说明为什么托盘和程序启动不可用。
fn draw_banner(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    if !app.low_integrity {
        return;
    }
    let scale = app.layout.scale;
    let height = (40.0 * scale) as i32;
    let bar = RECT {
        left: 0,
        top: 0,
        right: app.layout.width,
        bottom: height,
    };
    if !intersects(&bar, paint) {
        return;
    }
    let text = wide(crate::i18n::t(
        "受限权限运行：托盘不可用、多数程序点不开。请关闭本程序，在资源管理器里双击 LiteFencesDesktop.exe",
        "Running with limited rights: tray unavailable, most apps cannot launch. Close this program and start LiteFencesDesktop.exe from Explorer instead.",
    ));
    let mut rect = RECT {
        left: (16.0 * scale) as i32,
        right: app.layout.width - (16.0 * scale) as i32,
        top: 0,
        bottom: height,
    };
    // SAFETY: 画刷与字体都在本函数内创建并释放。
    // SAFETY: 画刷在本函数内创建并释放；字体已由 paint() 选入 DC。
    unsafe {
        let brush = CreateSolidBrush(app.theme.warn_bg);
        FillRect(hdc, &bar, brush);
        DeleteObject(brush as _);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, app.theme.warn_text);
        DrawTextW(
            hdc,
            text.as_ptr(),
            -1,
            &mut rect,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
    }
}

fn hit_test(x: i32, y: i32) -> Option<usize> {
    let app = state();
    // 格子存的是内容坐标，命中前要把鼠标位置换算到同一坐标系。
    let y = y + app.scroll;
    app.layout
        .tiles
        .iter()
        .position(|tile| x >= tile.left && x < tile.right && y >= tile.top && y < tile.bottom)
}

fn within_cooldown() -> bool {
    state()
        .shown_at
        .is_some_and(|shown| shown.elapsed() < Duration::from_millis(SHOW_COOLDOWN_MS))
}

/// 显示系统对话框期间临时取消面板置顶，避免对话框被全屏面板挡住；返回后恢复。
pub fn run_unpinned<T>(body: impl FnOnce() -> T) -> T {
    let overlay = state().overlay;
    let pinned = !overlay.is_null() && state().shown;
    if pinned {
        // SAFETY: 句柄有效；只改置顶状态。
        unsafe {
            SetWindowPos(
                overlay,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
    let value = body();
    if pinned {
        // SAFETY: 同上。
        unsafe {
            SetWindowPos(
                overlay,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
    value
}

/// 点「添加图标」：面板不关，只是暂时取消置顶，让系统选择框显示在它上面。
fn begin_add_icon() {
    let app = state();
    let host = app.host;
    let overlay = app.overlay;
    // SAFETY: 句柄有效；临时取消置顶，选完再恢复。
    unsafe {
        SetWindowPos(
            overlay,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    let picked = crate::picker::pick_icon_source(host);
    // SAFETY: 同上。
    unsafe {
        SetWindowPos(
            overlay,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    if let Some(path) = picked {
        crate::app::add_custom(path);
    }
}

/// Esc：先关搜索、退出移除模式，最后才收起面板。
pub fn handle_escape() {
    // 先处理输入框：Esc 等于取消输入，面板保持显示。
    if crate::text_input::is_open() {
        log::info("Esc：取消输入");
        crate::text_input::cancel();
        return;
    }
    let app = state();
    if app.search_open {
        toggle_search();
        return;
    }
    if !app.search.is_empty() {
        crate::app::set_search_text("");
        return;
    }
    if app.remove_mode {
        app.remove_mode = false;
        app.static_dirty = true;
        log::info("退出移除模式");
        // SAFETY: 句柄有效。
        unsafe { InvalidateRect(app.overlay, std::ptr::null(), 0) };
        return;
    }
    hide_panel();
}

fn launch_and_hide(index: usize) {    let app = state();
    let Some(item) = app.items.get(index) else {
        return;
    };
    let path = item.path.clone();
    hide_panel();
    match icons::launch(&path) {
        Ok(()) => {}
        Err(error) => log::error(&error),
    }
}

unsafe extern "system" fn overlay_proc(
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
        WM_CHAR => {
            let app = state();
            if app.search_typing {
                let value = wparam as u32;
                if value >= 0x20
                    && value != 0x7F
                    && let Some(ch) = char::from_u32(value)
                {
                    app.search.push(ch);
                    app.static_dirty = true;
                    // SAFETY: 句柄有效。
                    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
                }
            }
            0
        }
        WM_KEYDOWN => {
            let app = state();
            if app.search_typing && wparam as u32 == 0x08 {
                app.search.pop();
                app.static_dirty = true;
                // SAFETY: 句柄有效。
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            }
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_NCHITTEST => HTCLIENT as LRESULT,
        WM_TIMER if wparam == TIMER_SEARCH => {
            let app = state();
            let finished = app
                .search_anim
                .is_some_and(|started| started.elapsed() >= Duration::from_millis(160));
            if finished {
                let open = app.search_open;
                app.search_anim = None;
                // SAFETY: 句柄有效。
                unsafe { KillTimer(hwnd, TIMER_SEARCH) };
                if open {
                    activate_typing();
                }
                // SAFETY: 句柄有效；让最终状态整屏落定。
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            } else {
                // SAFETY: 句柄有效；只重画工具条。
                unsafe {
                    let bar = toolbar_rect(app);
                    InvalidateRect(hwnd, &bar, 0);
                    UpdateWindow(hwnd);
                }
            }
            0
        }
        WM_TIMER if wparam == TIMER_DROP_NOTICE => {
            let app = state();
            app.drop_notice = None;
            app.static_dirty = true;
            // SAFETY: 句柄有效；提示条在静态帧里，整屏重画一次去掉它。
            unsafe {
                KillTimer(hwnd, TIMER_DROP_NOTICE);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }

        WM_MOUSEMOVE => {
            let app = state();
            // 拖着分组标题走：算插入位置并画指示线（整屏重贴缓存，拖标题不频繁）。
            if let Some(mut header) = app.header_drag {
                let (x, y) = mouse_xy(lparam);
                let moved = (x - header.start.0).abs() > 4 || (y - header.start.1).abs() > 4;
                if moved {
                    let is_portal = app
                        .groups
                        .get(header.from)
                        .is_some_and(|group| group.portal.is_some());
                    if !is_portal {
                        if !header.active {
                            header.active = true;
                            header.target = Some(header.from);
                        }
                        let target = header_insert_target(app, y);
                        let changed = header.target != Some(target);
                        header.target = Some(target);
                        // 先把新状态写回全局，再重绘——paint 读的是 App 里的副本，
                        // 顺序反了就会一直画上一次的位置（表现为“指示线不出现”）。
                        app.header_drag = Some(header);
                        if changed {
                            log::info(&format!("标题拖动：插入位置 {target}"));
                            // SAFETY: 句柄有效；整屏重贴缓存再画指示线。
                            unsafe {
                                InvalidateRect(hwnd, std::ptr::null(), 0);
                                UpdateWindow(hwnd);
                            }
                        }
                    }
                }
                return 0;
            }
            if let Some(mut drag) = app.drag {
                let previous_point = drag.current;
                drag.current = mouse_xy(lparam);
                let moved_x = (drag.current.0 - drag.start.0).abs();
                let moved_y = (drag.current.1 - drag.start.1).abs();
                if !drag.active && (moved_x > 4 || moved_y > 4) {
                    // 入口项是只读视图，不进入拖动（单击仍然可以打开）。
                    if app
                        .items
                        .get(drag.from)
                        .is_some_and(|item| item.source == crate::icons::ItemSource::Portal)
                    {
                        return 0;
                    }
                    drag.active = true;
                    drag.last_paint = None;
                    drag.ghost = None;
                    drag.target = None;
                    // 让源图标从静态帧里消失，避免和跟随图标重影。
                    app.drag_hidden = Some(drag.from);
                    app.static_dirty = true;
                    // SAFETY: 句柄有效；这一帧要把新的静态帧整屏贴上。
                    unsafe {
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
                if drag.active {
                    // 只在鼠标真的移动了才重画。
                    let moved = (drag.current.0 - previous_point.0).abs() > 0
                        || (drag.current.1 - previous_point.1).abs() > 0;
                    let now = Instant::now();
                    let due = drag
                        .last_paint
                        .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(8));
                    if moved && due {
                        drag.last_paint = Some(now);
                        let icon = app.layout.icon;
                        let new_ghost = ghost_rect(drag.current, icon);
                        let target = hit_test(drag.current.0, drag.current.1)
                            .filter(|index| *index < app.items.len());
                        let point = content_point(app, drag.current.0, drag.current.1);
                        let hover_header = app
                            .layout
                            .headers
                            .iter()
                            .find(|header| unsafe { PtInRect(&header.rect, point) != 0 })
                            .map(|header| (header.group, view_rect(app, header.rect)));
                        // 先把缓存刷成当前布局，否则贴回的是旧画面（表现为“涂抹”）。
                        ensure_cache(hwnd);
                        // 直接画，不进 WM_PAINT：先贴回旧区域，再画新的跟随图标/目标框。
                        // SAFETY: 句柄有效；缓存存在时才有意义。
                        unsafe {
                            let hdc = GetDC(hwnd);
                            if !hdc.is_null() && !app.buffer_dc.is_null() {
                                if let Some(old) = drag.ghost {
                                    blit_region(hdc, app.buffer_dc, old);
                                }
                                if let Some(index) = drag.target
                                    && let Some(raw) = app.layout.tiles.get(index).copied()
                                {
                                    blit_region(hdc, app.buffer_dc, view_rect(app, raw));
                                }
                                if let Some(index) = target
                                    && let Some(raw) = app.layout.tiles.get(index).copied()
                                {
                                    draw_target_outline(hdc, view_rect(app, raw), app.layout.scale, app.theme);
                                }
                                // 悬停到分组标题：整条高亮，松手就移入。
                                if let Some(old) = drag.header_rect {
                                    blit_region(hdc, app.buffer_dc, old);
                                }
                                if let Some((_, rect)) = hover_header {
                                    let inner = RECT {
                                        left: rect.left,
                                        top: rect.top + 2,
                                        right: rect.right,
                                        bottom: rect.bottom - 2,
                                    };
                                    let brush = CreateSolidBrush(app.theme.drop_bg);
                                    FillRect(hdc, &inner, brush);
                                    DeleteObject(brush as _);
                                    draw_outline(hdc, inner, app.layout.scale, app.theme.drop_line);
                                }
                                // 缓存里没有工具条，拖动经过时补画，避免被贴成黑块。
                                let full = RECT {
                                    left: 0,
                                    top: 0,
                                    right: app.layout.width,
                                    bottom: app.layout.height,
                                };
                                let font = ui_font(app);
                                if !font.is_null() {
                                    SelectObject(hdc, font as _);
                                }
                                draw_toolbar(hdc, app, &full);
                                draw_search_box(hdc, app, &full);
                                draw_ghost_icon(hdc, app, drag.from, drag.current);
                            }
                            if !hdc.is_null() {
                                ReleaseDC(hwnd, hdc);
                            }
                        }
                        drag.ghost = Some(new_ghost);
                        drag.target = target;
                        drag.header = hover_header.map(|(group, _)| group);
                        drag.header_rect = hover_header.map(|(_, rect)| rect);
                    }
                }
                app.drag = Some(drag);
            }
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = mouse_xy(lparam);
            let target = hit_test(x, y);
            let app = state();
            // 松手结束标题拖动：拖过就是调顺序，没拖就是单击折叠/展开。
            if let Some(header) = app.header_drag.take() {
                // SAFETY: 捕获由本窗口持有。
                unsafe { ReleaseCapture() };
                if header.active {
                    if let Some(target) = header.target {
                        move_group(header.from, target);
                    }
                    // SAFETY: 句柄有效；清掉指示线。
                    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
                } else {
                    toggle_group(header.from);
                }
                return 0;
            }
            if let Some(drag) = app.drag.take() {
                // SAFETY: 捕获由本窗口持有。
                unsafe { ReleaseCapture() };
                // 还原静态帧：源图标回到格子里。
                app.drag_hidden = None;
                app.static_dirty = true;
                if drag.active {
                    if let Some(group) = drag.header {
                        // 拖到分组标题上：移入该组。
                        let path = app
                            .items
                            .get(drag.from)
                            .map(|item| item.path.clone())
                            .unwrap_or_default();
                        crate::groups::assign(&mut app.groups, &path, Some(group));
                        crate::groups::save(&app.groups);
                        let name = app
                            .groups
                            .get(group)
                            .map(|group| group.name.clone())
                            .unwrap_or_default();
                        log::info(&format!("拖入分组「{name}」：{}", path.display()));
                        crate::app::persist_layout();
                        relayout();
                    } else if let Some(target) = target
                        && target != drag.from
                        && target < app.items.len()
                    {
                        crate::app::move_item(drag.from, target);
                    }
                    // 松手后整屏重贴一次，清掉跟随图标留下的痕迹。
                    // SAFETY: 句柄有效。
                    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
                } else if visible(app, drag.from) {
                    let name = app
                        .items
                        .get(drag.from)
                        .map(|item| item.name.clone())
                        .unwrap_or_default();
                    log::info(&format!("单击图标 #{} {name}", drag.from));
                    launch_and_hide(drag.from);
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            // 面板不抢焦点，靠系统“悬停时滚动非活动窗口”把滚轮送过来。
            let app = state();
            if !app.shown {
                return 0;
            }
            let max = max_scroll(app);
            if max <= 0 {
                return 0;
            }
            let delta = ((wparam >> 16) & 0xffff) as u16 as i16 as i32;
            let step = (app.layout.cell_h / 2).max(48);
            let next = (app.scroll - (delta / 120) * step).clamp(0, max);
            if next != app.scroll {
                app.scroll = next;
                app.static_dirty = true;
                // SAFETY: 句柄有效；内容整体位移，整屏重画一帧。
                unsafe {
                    InvalidateRect(app.overlay, std::ptr::null(), 0);
                    UpdateWindow(app.overlay);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            // 输入框开着时，点面板第一下先关掉输入框。
            if crate::text_input::is_open() {
                crate::text_input::cancel();
                return 0;
            }
            {
                let app = state();
                if app.pending_remove.take().is_some() {
                    app.static_dirty = true;
                }
            }
            if !within_cooldown() && !animating() {
                let (x, y) = mouse_xy(lparam);
                let point = POINT { x, y };
                let (in_settings, in_new_group, in_chip, in_search) = unsafe {
                    let app = state();
                    (
                        PtInRect(&settings_chip_rect(app), point) != 0,
                        PtInRect(&new_group_chip_rect(app), point) != 0,
                        PtInRect(&remove_chip_rect(app), point) != 0,
                        PtInRect(&search_rect(app), point) != 0,
                    )
                };
                if in_settings {
                    log::info("点击左上角「设置」");
                    open_settings();
                    return 0;
                }
                if in_new_group {
                    create_group();
                    return 0;
                }
                if in_search {
                    log::info("点击搜索按钮");
                    toggle_search();
                    return 0;
                }
                if in_chip {
                    let app = state();
                    app.remove_mode = !app.remove_mode;
                    app.static_dirty = true;
                    log::info(&format!(
                        "移除模式：{}",
                        if app.remove_mode { "开启" } else { "关闭" }
                    ));
                    // SAFETY: 句柄有效。
                    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
                    return 0;
                }
                // 搜索栏开着时，点别处先把搜索收起来。
                if state().search_open {
                    toggle_search();
                    return 0;
                }
                // 整条分组标题都能点：按下先记录，松开没拖动就折叠/展开，拖过就是调顺序。
                if let Some(group) = header_at(x, y) {
                    let app = state();
                    app.pending_remove = None;
                    app.header_drag = Some(crate::state::HeaderDragState {
                        from: group,
                        start: (x, y),
                        active: false,
                        target: None,
                    });
                    // SAFETY: 捕获鼠标，拖出标题行也能收到消息。
                    unsafe { SetCapture(hwnd) };
                    return 0;
                }
                if state().remove_mode {
                    if let Some(index) = hit_test(x, y)
                        && index < state().items.len()
                        && visible(state(), index)
                    {
                        crate::app::remove_item(index);
                    }
                    return 0;
                }
                match hit_test(x, y) {
                    Some(index) if index == state().items.len() => {
                        log::info("点击「添加图标」");
                        begin_add_icon();
                    }
                    Some(index) if !visible(state(), index) => {
                        // 搜索过滤掉的项不可操作。
                    }
                    Some(index) => {
                        // 按下先记录，松开时若没拖动就是“打开”，拖动则是排序。
                        let app = state();
                        app.drag = Some(crate::state::DragState {
                            from: index,
                            start: (x, y),
                            current: (x, y),
                            active: false,
                            last_paint: None,
                            ghost: None,
                            target: None,
                            header: None,
                            header_rect: None,
                        });
                        // SAFETY: 句柄有效；捕获鼠标，拖出格子也能收到消息。
                        unsafe { SetCapture(hwnd) };
                    }
                    None => {
                        log::info(&format!("单击空白 ({x},{y})，收起"));
                        hide_panel();
                    }
                }
            }
            0
        }
        WM_RBUTTONUP => {
            if !within_cooldown() && !animating() {
                let (x, y) = mouse_xy(lparam);
                let point = POINT { x, y };
                // 吸顶标题优先按它自己固定的位置判定。
                if let Some((group, rect)) = sticky_header(state()) {
                    // SAFETY: 矩形来自本模块布局。
                    if unsafe { PtInRect(&rect, point) != 0 } {
                        open_header_menu(group);
                        return 0;
                    }
                }
                let hovered_header = {
                    let app = state();
                    let content = content_point(app, point.x, point.y);
                    app.layout
                        .headers
                        .iter()
                        .find(|header| unsafe { PtInRect(&header.rect, content) != 0 })
                        .map(|header| header.group)
                };
                if let Some(group) = hovered_header {
                    open_header_menu(group);
                    return 0;
                }
                if state().remove_mode {
                    if let Some(index) = hit_test(x, y)
                        && index < state().items.len()
                        && visible(state(), index)
                    {
                        crate::app::remove_item(index);
                    }
                } else if let Some(index) = hit_test(x, y)
                    && index < state().items.len()
                    && visible(state(), index)
                {
                    log::info(&format!("右键图标 #{index}：选择分组"));
                    open_group_menu(index);
                }
            }
            0
        }
        WM_LBUTTONDBLCLK => 0,
        WM_DPICHANGED => {
            let app = state();
            let dpi = (wparam >> 16) & 0xffff;
            let dpi = if dpi == 0 { 96 } else { dpi as u32 };
            // lparam 是系统建议的新窗口矩形，跟着它调整，避免缩放比例错位。
            let suggested = lparam as *const RECT;
            let mut width = app.layout.width;
            let mut height = app.layout.height;
            if !suggested.is_null() {
                // SAFETY: 系统在 WM_DPICHANGED 里给出合法 RECT 指针。
                let rect = unsafe { *suggested };
                width = rect.right - rect.left;
                height = rect.bottom - rect.top;
                // SAFETY: 句柄有效。
                unsafe {
                    SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        rect.left,
                        rect.top,
                        width,
                        height,
                        SWP_NOACTIVATE,
                    );
                }
            }
            let top_dip = if app.low_integrity { 88.0 } else { 48.0 };
            let layout = compute_layout(&app.items, &app.groups, width, height, dpi, top_dip);
            app.layout = layout;
            app.scroll = 0;
            app.static_dirty = true;
            log::info(&format!("DPI 变化：{dpi}，面板 {width}x{height}"));
            // SAFETY: 句柄有效。
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            0
        }
        _ => {
            // SAFETY: 默认处理其余消息。
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use crate::groups::Group;
    use crate::icons::{DesktopItem, ItemSource};
    use std::path::PathBuf;

    fn item(name: &str) -> DesktopItem {
        DesktopItem {
            path: PathBuf::from(format!("C:\\桌面\\{name}")),
            name: name.to_string(),
            // 布局不碰图标句柄，空句柄足够。
            icon: std::ptr::null_mut(),
            source: ItemSource::Desktop,
            custom_icon: None,
        }
    }

    /// 造 n 个组、把 items 均分进去，最后一组用默认组名接住「添加图标」。
    fn scene(items_n: usize, groups_n: usize) -> (Vec<DesktopItem>, Vec<Group>) {
        let items: Vec<_> = (0..items_n).map(|i| item(&format!("i{i}"))).collect();
        let mut groups: Vec<Group> = (0..groups_n.saturating_sub(1))
            .map(|i| Group::new(format!("组{i}")))
            .collect();
        groups.push(Group::new(crate::groups::DEFAULT_GROUP));
        for (index, item) in items.iter().enumerate() {
            groups[index % groups_n].paths.push(item.path.clone());
        }
        (items, groups)
    }

    fn check(width: i32, height: i32, groups_n: usize, items_n: usize) {
        let (items, groups) = scene(items_n, groups_n);
        let layout = compute_layout(&items, &groups, width, height, 96, 48.0);
        let label = format!("{width}x{height}, {groups_n} 组, {items_n} 项");
        assert_eq!(layout.tiles.len(), items_n + 1, "格子数量不对：{label}");
        // 横向必须始终放得下；纵向可以超出面板一屏，靠滚动看到（内容高度由 content_height 兜住）。
        for (index, tile) in layout.tiles.iter().enumerate() {
            if tile.right <= tile.left || tile.bottom <= tile.top {
                panic!("第 {index} 格没有排出来：{label}");
            }
            assert!(
                tile.left >= 0 && tile.right <= width,
                "第 {index} 格横向越界 {}..{}：{label}",
                tile.left,
                tile.right
            );
            assert!(
                tile.top >= 0 && tile.bottom <= layout.content_height,
                "第 {index} 格纵向越界 {}..{}（内容高 {}）：{label}",
                tile.top,
                tile.bottom,
                layout.content_height
            );
        }
        assert!(!layout.headers.is_empty(), "没有分组标题：{label}");
        assert!(
            layout.headers.last().expect("标题").rect.bottom <= layout.content_height,
            "最后一个标题行越界：{label}"
        );
        // 内容超过一屏时，必须能靠滚动看到最后一行：内容高度不能超过面板 + 最大滚动量。
        let pad = 24;
        let max_scroll = (layout.content_height + pad - height).max(0);
        assert!(
            layout.content_height + pad <= height + max_scroll,
            "滚动量覆盖不到内容底部：{label}"
        );
    }

    #[test]
    fn tiles_fit_inside_the_panel() {
        // 本机常见情况，以及小屏 + 多组 + 满额图标的极端情况。
        check(2560, 1440, 1, 84);
        check(1920, 1080, 3, 120);
        check(1366, 768, 6, 150);
        check(1280, 720, 8, 200);
    }
}
