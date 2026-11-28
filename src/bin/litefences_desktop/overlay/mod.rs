//! 全屏虚化浮层：桌面之上、普通窗口之上的一次性面板。
//!
//! - 虚化：优先亚克力（带自定义透明度，能看到桌面内容），再退 DWM 系统背景，最后深色底。
//! - 绘制：GDI 直接画图标与文字；不填背景，让虚化层透出来。
//! - 交互：单击图标启动、单击空白收起、Esc 收起；唤出后 350ms 内与入场动画期间的点击被忽略，
//!   避免唤出来源的那次点击立刻把面板点掉。
//! - 入场动画：图标网格上移 + 虚化浓度淡入，约 170ms。


pub mod input;
pub mod layout;
pub mod menu;
pub mod render;

pub use input::{
    copy_selected_paths, cycle_group, handle_escape, move_selection, open_selected,
    open_selected_admin, portal_back_if_drilled, redo_last, remove_selection, reveal_selected,
    search_active, search_backspace, search_edit_key, search_report, type_to_search, undo_last,
};
pub use layout::{
    GroupFold, Layout, compute_layout, first_group_screen_point, group_at_point,
};
pub use menu::set_all_groups_collapsed;

use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{DeleteDC, DeleteObject, GetDC, GetMonitorInfoW, InvalidateRect, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint, MonitorFromWindow, ReleaseDC, UpdateWindow};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::Controls::{MARGINS};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::{CS_DBLCLKS, CreateWindowExW, DestroyWindow, GetClientRect, GetCursorPos, GetWindowRect, HWND_NOTOPMOST, HWND_TOPMOST, IDC_ARROW, KillTimer, LoadCursorW, RegisterClassW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE, SetTimer, SetWindowPos, ShowWindow, WNDCLASSW, WS_CLIPCHILDREN, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP};

use crate::desktop_icons;
use crate::icons;
use crate::log;
use crate::state::{BlurMode, state};
use crate::util::{wide};

use layout::{finish_fold, compute_layout_full, clamp_scroll, search_results, search_layout, apply_search};
use render::{anim_progress, ensure_buffer, render_static};
use input::{deactivate_typing, overlay_proc};

pub(super) const CLASS_NAME: &str = "LiteFencesOverlay";
pub(super) const TIMER_SEARCH: usize = 2;
/// 拖入结果提示自动淡出。
pub(super) const TIMER_DROP_NOTICE: usize = 3;
/// 拖动到面板边缘时的自动滚动。
pub(super) const TIMER_AUTOSCROLL: usize = 5;
pub(super) const TIMER_GROUP_FOLD: usize = 6;
pub(super) const FOLD_MS: f32 = 220.0;

/// 唤出后忽略点击的时间窗口，单位毫秒。
pub(super) const SHOW_COOLDOWN_MS: u64 = 350;
/// 入场动画时长；帧间隔略大一点，减少分层窗口的合成次数。
pub(super) const ANIM_MS: u64 = 240;
/// 亚克力最终不透明度。
pub(super) const TINT_ALPHA: u32 = 0x99;

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

pub(super) const WCA_ACCENT_POLICY: i32 = 19;
pub(super) const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;
/// DWMSBT_TRANSIENTWINDOW：亚克力背景。
pub(super) const DWMSBT_TRANSIENTWINDOW: i32 = 3;

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
            WS_POPUP | WS_CLIPCHILDREN,
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

pub(super) fn try_backdrop(hwnd: HWND) -> bool {
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
pub(super) fn apply_accent(hwnd: HWND, alpha: u32) -> bool {
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
    finish_fold(app);
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
    let monitor = match app.monitor_policy {
        crate::settings::MonitorPolicy::Primary => unsafe {
            MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)
        },
        crate::settings::MonitorPolicy::Cursor => {
            let mut point = POINT::default();
            // SAFETY: 输出结构体有效。
            unsafe { GetCursorPos(&mut point) };
            // SAFETY: 点坐标有效；取最近显示器。
            unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) }
        }
    };
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
    // SAFETY: 先按目标显示器铺满（保持隐藏以免磨砂抓屏截进空白窗），再取实际客户区与 DPI，保证绘制坐标与点击坐标同源。
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
            SWP_NOACTIVATE,
        );
        let mut bounds = RECT::default();
        GetWindowRect(app.overlay, &mut bounds);
        if bounds.left != area.left
            || bounds.top != area.top
            || bounds.right != area.right
            || bounds.bottom != area.bottom
        {
            SetWindowPos(
                app.overlay,
                HWND_TOPMOST,
                area.left,
                area.top,
                width,
                height,
                SWP_NOACTIVATE,
            );
        }
        GetClientRect(app.overlay, &mut client);
    }
    // 后台异步检测自定义路径与入口目录可用性，不阻塞面板唤出。
    crate::app::begin_availability_refresh(app.host);
    // DPI 必须在窗口挪到目标显示器之后再取，多屏不同缩放时才对。
    let dpi = unsafe { GetDpiForWindow(app.overlay) };
    let (client_w, client_h) = (client.right - client.left, client.bottom - client.top);
    let top_dip = if app.low_integrity { 88.0 } else { 48.0 };
    let layout = compute_layout_full(
        &app.items,
        &app.groups,
        &app.usage,
        app.show_recent,
        client_w,
        client_h,
        dpi.max(96),
        top_dip,
        app.density.scale_factor(),
    );
    app.layout = layout;
    if !app.search.is_empty() {
        // 搜索词在唤出前就已存在（如 --search-test）：沿用紧凑结果布局。
        let results = search_results(app);
        app.layout = search_layout(app, &results);
        app.selected = results.first().copied();
    }
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
    if crate::store::take_recovery_notice() {
        show_drop_notice(crate::i18n::t("已从备份恢复配置", "Settings restored from backup").into());
    }
    app.hover = None;
    // 每次唤出都从普通模式开始，避免上次的“移除中”状态残留。
    // 排障参数：直接选中某个图标，便于截图验证选中框。
    app.remove_mode = false;
    app.pending_remove = None;
    app.marquee = None;
    app.recent_selected = None;
    app.selection.clear();
    app.selection_anchor = None;
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
    if !state().search_edit.is_null() {
        relayout();
        return;
    }
    // 优先沿用当前所在显示器，避免后台重扫或配置更新把面板拉到鼠标所在的另一块屏幕。
    let current_area = {
        let old = state().overlay;
        if !old.is_null() {
            // SAFETY: 句柄尚未销毁；读取当前显示器范围。
            unsafe {
                let monitor = MonitorFromWindow(old, MONITOR_DEFAULTTONEAREST);
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                (GetMonitorInfoW(monitor, &mut info) != 0).then_some(info.rcMonitor)
            }
        } else {
            None
        }
    };
    if ensure_window().is_err() {
        log::error("重建浮层窗口失败");
        return;
    }
    let app = state();
    let area = current_area.unwrap_or_else(|| unsafe {
        let mut point = POINT::default();
        GetCursorPos(&mut point);
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(monitor, &mut info);
        info.rcMonitor
    });
    let width = area.right - area.left;
    let height = area.bottom - area.top;
    // SAFETY: 句柄有效；直接以最终浓度显示，不再重复淡入。
    let (client_w, client_h, dpi) = unsafe {
        SetWindowPos(
            app.overlay,
            HWND_TOPMOST,
            area.left,
            area.top,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        let mut client = RECT::default();
        GetClientRect(app.overlay, &mut client);
        let dpi = GetDpiForWindow(app.overlay).max(96);
        if app.blur == BlurMode::Accent {
            apply_accent(app.overlay, TINT_ALPHA);
        }
        ShowWindow(app.overlay, SW_SHOWNOACTIVATE);
        if app.drop_notice.is_some() {
            SetTimer(app.overlay, TIMER_DROP_NOTICE, 4000, None);
        }
        (
            (client.right - client.left).max(1),
            (client.bottom - client.top).max(1),
            dpi,
        )
    };
    if app.layout.width != client_w || app.layout.height != client_h || app.layout.dpi != dpi {
        app.layout.width = client_w;
        app.layout.height = client_h;
        app.layout.dpi = dpi;
        app.tile_cache = None;
        if app.blur == BlurMode::Frost {
            app.backdrop =
                crate::backdrop::capture(area, app.theme.tint, app.theme.tint_strength);
        }
        rebuild_layout(app);
    } else {
        // SAFETY: 句柄有效。
        unsafe { InvalidateRect(app.overlay, std::ptr::null(), 0) };
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
    finish_fold(app);
    // 引导卡只出现一次：收起面板就算看过了（点「知道了」只是提前收起）。
    if !app.onboarded {
        app.onboarded = true;
        crate::store::set_onboarded(true);
        log::info("首次引导已自动收起");
    }
    // SAFETY: 句柄有效。
    unsafe {
        KillTimer(app.overlay, TIMER_DROP_NOTICE);
        KillTimer(app.overlay, TIMER_SEARCH);
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
    app.marquee = None;
    app.selection.clear();
    app.selection_anchor = None;
    app.recent_selected = None;
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
pub(super) fn free_caches() {
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
    app.tile_cache = None;
    app.entrance = None;
    app.scroll = 0;
    app.static_dirty = true;
}

/// 后台扫描完成后重排布局并重绘。
pub fn relayout() {
    let app = state();
    if app.search.is_empty() {
        rebuild_layout(app);
    } else {
        // 搜索态下重扫/重命名后仍保持紧凑结果布局。
        apply_search(app);
    }
}

/// 按当前 items/groups 重排；有搜索词时转给 `apply_search` 换成紧凑结果布局。
pub(super) fn rebuild_layout(app: &mut crate::state::App) {
    finish_fold(app);
    let dpi = app.layout.dpi.max(96);
    let (width, height) = (app.layout.width, app.layout.height);
    let top_dip = if app.low_integrity { 88.0 } else { 48.0 };
    let layout = compute_layout_full(
        &app.items,
        &app.groups,
        &app.usage,
        app.show_recent,
        width,
        height,
        dpi,
        top_dip,
        app.density.scale_factor(),
    );
    app.layout = layout;
    // 已有搜索词时（如 --search-test、热重排），重排后立刻换回紧凑结果布局。
    if !app.search.is_empty() {
        apply_search(app);
        return;
    }
    // 排障：--select=N 等图标就绪后再应用（首次唤出时 items 还是空的）。
    if let Some(indices) = app.options.select.clone()
        && app.selected.is_none()
        && app.selection.is_empty()
    {
        let valid: Vec<usize> = indices
            .into_iter()
            .filter(|index| *index < app.items.len())
            .collect();
        app.selected = valid.first().copied();
        if valid.len() > 1 {
            app.selection_anchor = valid.first().copied();
            app.selection = valid;
        }
    }
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

