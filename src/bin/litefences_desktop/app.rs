//! 进程入口：单实例、隐藏宿主窗口、托盘、钩子、消息循环。

use std::path::PathBuf;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, OpenMutexW, OpenProcessToken,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, RegisterHotKey, UnregisterHotKey, VK_Q,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, DestroyWindow, GetMessageW, IDC_ARROW,
    KillTimer, LoadCursorW, MB_ICONWARNING, MB_OK, MB_YESNO, IDYES, MSG, PostMessageW,
    PostQuitMessage, RegisterClassW, SetTimer, TranslateMessage, WNDCLASSW, WM_APP, WM_CLOSE,
    WM_COMMAND, WM_DESTROY, WM_ENDSESSION, WM_HOTKEY, WM_LBUTTONUP, WM_QUERYENDSESSION,
    WM_RBUTTONUP, WM_TIMER, WS_EX_TOOLWINDOW, WS_POPUP, MessageBoxW, SW_SHOWNORMAL,
};

use crate::desktop_icons;
use crate::hooks;
use crate::icons;
use crate::log;
use crate::overlay::{self, Layout};
use crate::state::{App, BlurMode, Options, install, state};
use crate::tray;
use crate::util::wide;

const HOST_CLASS: &str = "LiteFencesDesktopHost";
const TIMER_DEMO: usize = 1;
const TIMER_TRAY_RETRY: usize = 2;
const TIMER_DEMO_RESTART: usize = 4;
const TIMER_CAPTURE: usize = 5;
const TIMER_CAPTURE_EARLY: usize = 6;
/// 资源管理器重启后补做一次「隐藏桌面图标」。
const TIMER_REHIDE: usize = 7;
const TIMER_DEVICE_CHANGE: usize = 8;
/// 后台图标扫描完成的自定义消息。
const WM_APP_SCAN: u32 = WM_APP + 2;
/// 后台自定义路径与文件夹入口可用性检测完成的自定义消息。
const WM_APP_REFRESH: u32 = WM_APP + 4;
/// 多步撤销 / 重做历史栈上限。
const MAX_UNDO_STEPS: usize = 20;
/// 托盘不可用时的兜底入口，也方便键盘用户。
const HOTKEY_TOGGLE: i32 = 1;
const HOTKEY_EXIT: i32 = 2;

pub fn run() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    log::init();
    install_panic_hook();
    log::info(&format!(
        "启动 pid={} args={args:?} 日志={}",
        std::process::id(),
        log::path_display()
    ));

    let options = match Options::parse(args) {
        Ok(options) => options,
        Err(error) => {
            log::error(&error);
            return std::process::ExitCode::from(2);
        }
    };

    // SAFETY: 进程级 DPI 感知只设置一次。
    let dpi_ok = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    log::info(&format!(
        "DPI 感知 Per-Monitor-V2：{}，完整性级别 RID={}",
        if dpi_ok != 0 { "成功" } else { "失败" },
        integrity_rid()
    ));

    // 排障入口：只走一次启动流程，不创建任何界面。
    if let Some(target) = options.launch_test.clone() {
        let path = std::path::PathBuf::from(target);
        return match icons::launch(&path) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(_) => std::process::ExitCode::FAILURE,
        };
    }

    // 命令行直接设置开机自启后退出（自动化/排障用）。
    if let Some(enable) = options.autostart {
        let exe = std::env::current_exe().unwrap_or_default();
        return match crate::autostart::set_enabled(&exe, enable) {
            Ok(()) => {
                log::info(&format!("开机自启：{}", if enable { "已开启" } else { "已关闭" }));
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                log::error(&format!("开机自启设置失败：{error}"));
                std::process::ExitCode::FAILURE
            }
        };
    }

    // 排障：只验证更新接口可达且 JSON 能解析。
    if options.update_test {
        return match crate::update::check_now() {
            Ok((tag, url)) => {
                log::info(&format!(
                    "更新接口正常：最新={tag} 当前={} 地址={url}",
                    crate::update::current_version()
                ));
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                log::error(&format!("更新接口失败：{error}"));
                std::process::ExitCode::FAILURE
            }
        };
    }

    // 先看是否有实例在跑：有就静默退出，避免每次从受限环境启动都弹提示。
    if another_instance_running() {
        log::warn("已有实例在运行，本次启动退出");
        return std::process::ExitCode::SUCCESS;
    }

    // 低完整性级别下托盘与多数程序启动会被系统拒绝。这里不再弹模态框、也不代启动任何进程
    // （从低权限派生 explorer.exe 会以 0xc0000142 崩溃），改为在面板顶部显示橙色横幅。
    if integrity_rid() < 0x2000 {
        log::warn("以低完整性级别运行：托盘和多数程序无法启动，需从资源管理器双击启动");
    }

    if !claim_single_instance() {
        log::warn("已有实例在运行，本次启动退出");
        return std::process::ExitCode::SUCCESS;
    }

    desktop_icons::recover_after_crash();

    // SAFETY: 空名称表示取当前模块句柄。
    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let host = match create_host(hinstance) {
        Ok(host) => host,
        Err(error) => {
            log::error(&error);
            return std::process::ExitCode::FAILURE;
        }
    };
    let overlay = match overlay::create(hinstance) {
        Ok(overlay) => overlay,
        Err(error) => {
            log::error(&error);
            return std::process::ExitCode::FAILURE;
        }
    };
    let blur = overlay::apply_blur(overlay, options.blur);
    let self_test = options.self_test;
    let show_ms = options.show_ms;
    let options_cycles = options.cycles;
    let capture_path = options.capture.clone();
    let demo = options.demo.clone();
    let drop_test = options.drop_test.clone();
    let initial_settings = crate::settings::load();
    let initial_config = crate::store::load();
    crate::i18n::set(crate::i18n::resolve(&initial_settings.lang));
    install(App {
        host,
        overlay,
        options,
        blur,
        shown: false,
        shown_at: None,
        anim_started: None,
        entrance: None,
        anim_frames: 0,
        anim_alpha: 0.0,
        icons_hidden: false,
        items: Vec::new(),
        desktop_count: 0,
        custom: icons::load_custom(),
        layout_order: icons::load_order(),
        drag: None,
        header_drag: None,
        selection: Vec::new(),
        selection_anchor: None,
        undo_stack: Vec::new(),
        redo_stack: Vec::new(),
        drag_hidden: None,
        pending_remove: None,
        drop_hover: None,
        drop_notice: None,
        rehide_left: 0,
        remove_mode: false,
        hidden: icons::load_hidden(),
        hotkey: initial_settings.hotkey,
        theme: crate::theme::by_key(&initial_settings.theme),
        lang_setting: initial_settings.lang.clone(),
        onboarded: initial_config.onboarded,
        selected: None,
        search: String::new(),
        check_updates: initial_settings.check_updates,
        density: initial_settings.density,
        search_open: false,
        search_anim: None,
        fold: None,
        search_edit: std::ptr::null_mut(),
        ime_composing: false,
        font: std::ptr::null_mut(),
        font_scale: 0.0,
        groups: crate::groups::load(),
        portals: crate::portals::load(),
        portal_stamp: None,
        buffer_dc: std::ptr::null_mut(),
        buffer_bitmap: 0,
        buffer_width: 0,
        buffer_height: 0,
        static_dirty: true,
        scroll: 0,
        desktop_stamp: None,
        backdrop: None,
        tile_cache: None,
        scan_rx: None,
        scan_pending: false,
        refresh_rx: None,
        refresh_pending: false,
        usage: initial_config.usage,
        layout: Layout::default(),
        hover: None,
        tray_installed: false,
        demo_cycles_left: options_cycles,
        low_integrity: integrity_rid() < 0x2000,
        mouse_hook: 0,
        keyboard_hook: 0,
    });
    // 保证至少有一个“其它”组，并把分组文件落盘一次。
    let app = state();
    crate::groups::prune(&mut app.groups, &app.custom);
    crate::groups::ensure_default(&mut state().groups);
    crate::groups::save(&state().groups);
    rebuild_portal_items();
    state().tray_installed = tray::install(host);
    if !state().tray_installed {
        // 托盘可能因为权限或资源管理器未就绪失败，稍后重试一次。
        // SAFETY: 宿主窗口有效。
        unsafe { SetTimer(host, TIMER_TRAY_RETRY, 3000, None) };
    }
    register_hotkeys(host);
    // 后台检查更新；设置里关掉时 auto_check 会直接返回。
    crate::update::auto_check(host);
    // 启动后立刻在后台扫描桌面图标，这样第一次唤出面板时通常已经准备好。
    begin_scan(host);
    let (mouse_hook, keyboard_hook) = hooks::install();
    {
        let app = state();
        app.mouse_hook = mouse_hook;
        app.keyboard_hook = keyboard_hook;
    }
    log::info("后台常驻已就绪：双击桌面空白处唤出面板，托盘可退出");

    if let Some(source) = drop_test.clone() {
        return run_drop_test(&source);
    }
    if self_test {
        return self_test_report();
    }
    // 排障：直接弹出某个界面看样式（--demo=newgroup）。
    if let Some(demo) = demo.clone() {
        overlay::show_panel();
        log::info(&format!("演示模式：{demo}"));
        match demo.as_str() {
            "newgroup" => {
                crate::text_input::ask("新建分组", "");
            }
            "rename" => {
                crate::text_input::ask("重命名图标", "示例名称");
            }
            other => log::warn(&format!("未知演示项：{other}")),
        }
        // 演示完直接退出，避免留下后台实例。
        overlay::hide_panel();
        return std::process::ExitCode::SUCCESS;
    }
    if let Some(ms) = show_ms {
        overlay::show_panel();
        // SAFETY: 宿主窗口有效，定时器到期后自动收起并退出。
        unsafe { SetTimer(host, TIMER_DEMO, ms, None) };
        if let Some(path) = capture_path.clone() {
            let delay = ms.saturating_sub(500).clamp(800, 2500);
            log::info(&format!("将在 {delay}ms 后抓取面板画面：{path}"));
            // SAFETY: 宿主窗口有效。
            unsafe { SetTimer(host, TIMER_CAPTURE, delay, None) };
        }
    }

    message_loop()
}

/// 后台图标扫描：在独立线程里列举桌面并取图标，完成后用自定义消息通知 UI 线程。
pub fn begin_scan(host: HWND) {
    let app = state();
    if app.scan_pending {
        return;
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    app.scan_rx = Some(receiver);
    app.scan_pending = true;
    // 句柄不能在 Send 闭包里直接传递，转成整数地址。
    let host_address = host as isize;
    let hidden = app.hidden.clone();
    std::thread::spawn(move || {
        let items = icons::collect_items(&hidden).unwrap_or_default();
        let _ = sender.send(items);
        // SAFETY: 只投递一条自定义消息，跨线程 PostMessage 是允许的。
        unsafe { PostMessageW(host_address as HWND, WM_APP_SCAN, 0, 0) };
    });
}

fn finish_scan() {
    let app = state();
    app.scan_pending = false;
    let received = app.scan_rx.take().and_then(|rx| rx.try_recv().ok());
    let Some(desktop_items) = received else {
        log::warn("图标扫描没有返回结果");
        return;
    };
    icons::replace(&mut app.items, desktop_items);
    app.tile_cache = None;
    app.desktop_count = app.items.len();
    app.desktop_stamp = Some(icons::desktop_stamp());
    append_custom_items();
    // 套用用户保存的顺序与名字。
    icons::apply_order(&mut app.items, &app.layout_order);
    // 再套用用户指定的图标文件（layout.txt 第三列）。
    icons::apply_custom_icons(&mut app.items);
    if apply_auto_group_rules(false) > 0 {
        crate::groups::save(&app.groups);
    }
    // 入口项在 replace 时被一并释放了，这里按目录重新建。
    rebuild_portal_items();
    overlay::relayout();
    // 如果面板正开着，用新窗口重画一帧，避免旧图标留在画面上。
    overlay::reshow();
}

/// 重建「文件夹入口」的内容：清掉旧的入口项和入口分组，按 portals 重新扫一遍。
///
/// 入口内容 + 目录状态的版本号：目录改动或配置变化时才重扫（图标加载很慢）。
pub fn portal_group_title(portal: &crate::portals::FolderPortal) -> String {
    let prefix = if crate::i18n::is_en() { "Entry: " } else { "入口：" };
    match portal.relative_subpath() {
        Some(sub) => format!("{prefix}{} / {sub}", portal.name),
        None => format!("{prefix}{}", portal.name),
    }
}

fn portal_stamp(app: &App) -> u64 {
    portal_stamp_for(&app.portals)
}

fn portal_stamp_for(portals: &[crate::portals::FolderPortal]) -> u64 {
    fn mix(hash: &mut u64, value: u64) {
        *hash ^= value;
        *hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for portal in portals {
        let dir = portal.active_dir();
        for byte in dir.to_string_lossy().bytes() {
            mix(&mut hash, u64::from(byte));
        }
        for byte in portal.name.bytes() {
            mix(&mut hash, u64::from(byte));
        }
        mix(&mut hash, u64::from(portal.collapsed));
        for byte in portal.sort.as_str().bytes() {
            mix(&mut hash, u64::from(byte));
        }
        let modified = std::fs::metadata(dir)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        mix(&mut hash, modified);
    }
    hash ^ (portals.len() as u64)
}

/// 后台异步检测自定义路径离线状态与文件夹入口目录变更，避免网络盘断连卡住唤出。
pub fn begin_availability_refresh(host: HWND) {
    let app = state();
    if app.refresh_pending || host.is_null() {
        return;
    }
    let customs: Vec<(PathBuf, bool, String, Option<PathBuf>)> = app
        .items
        .iter()
        .filter(|item| item.source == icons::ItemSource::Custom)
        .map(|item| (item.path.clone(), item.offline, item.name.clone(), item.custom_icon.clone()))
        .collect();
    let portals = app.portals.clone();
    let known_stamp = app.portal_stamp;
    let intact = portals.is_empty()
        || app
            .items
            .iter()
            .any(|item| item.source == icons::ItemSource::Portal);
    if customs.is_empty() && portals.is_empty() {
        return;
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    app.refresh_rx = Some(receiver);
    app.refresh_pending = true;
    let host_address = host as isize;
    std::thread::spawn(move || {
        // SAFETY: 后台线程独立初始化 COM 单元用于图标查询。
        let _ = unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            )
        };
        let mut custom_updates = Vec::new();
        for (path, was_offline, name, custom_icon) in customs {
            let offline = !path.exists();
            if offline == was_offline {
                continue;
            }
            let mut refreshed = icons::make_custom_item(path.clone());
            refreshed.set_name(&name);
            refreshed.custom_icon.clone_from(&custom_icon);
            if let Some(icon_path) = &custom_icon {
                icons::set_custom_icon(&mut refreshed, icon_path);
            }
            custom_updates.push((path, refreshed));
        }

        let stamp = portal_stamp_for(&portals);
        let portals_update = if !portals.is_empty() && (known_stamp != Some(stamp) || !intact) {
            let mut scanned = Vec::with_capacity(portals.len());
            for portal in &portals {
                let contents = crate::portals::scan(portal);
                let offline = contents.is_err();
                scanned.push((portal.active_dir().to_path_buf(), offline, contents.unwrap_or_default()));
            }
            Some((stamp, scanned))
        } else {
            None
        };

        let _ = sender.send(crate::state::AvailabilityRefresh {
            custom_updates,
            portals: portals_update,
        });
        // SAFETY: 跨线程投递自定义消息通知 UI 线程。
        unsafe { PostMessageW(host_address as HWND, WM_APP_REFRESH, 0, 0) };
    });
}

fn finish_availability_refresh() {
    let app = state();
    app.refresh_pending = false;
    let Some(result) = app.refresh_rx.take().and_then(|rx| rx.try_recv().ok()) else {
        return;
    };
    let mut changed = false;

    for (path, mut refreshed) in result.custom_updates {
        if let Some(item) = app
            .items
            .iter_mut()
            .find(|item| item.source == icons::ItemSource::Custom && icons::same_path(&item.path, &path))
        {
            if item.offline != refreshed.offline {
                refreshed.set_name(&item.name);
                if item.custom_icon.is_some() && refreshed.custom_icon != item.custom_icon {
                    refreshed.custom_icon.clone_from(&item.custom_icon);
                    if let Some(icon_path) = &item.custom_icon {
                        icons::set_custom_icon(&mut refreshed, icon_path);
                    }
                }
                icons::destroy_item(std::mem::replace(item, refreshed));
                changed = true;
                continue;
            }
        }
        icons::destroy_item(refreshed);
    }

    if let Some((stamp, scanned)) = result.portals {
        let same_portals = app.portals.len() == scanned.len()
            && app
                .portals
                .iter()
                .zip(&scanned)
                .all(|(portal, (dir, _, _))| icons::same_path(portal.active_dir(), dir));
        if same_portals {
            let mut removed = Vec::new();
            app.items.retain(|item| {
                if item.source == icons::ItemSource::Portal {
                    removed.push(item.clone());
                    false
                } else {
                    true
                }
            });
            for item in removed {
                icons::destroy_item(item);
            }
            app.groups.retain(|group| group.portal.is_none());
            for (index, (portal, (_, offline, items))) in app.portals.iter().zip(scanned).enumerate() {
                let mut group = crate::groups::Group::new(portal_group_title(portal));
                group.collapsed = portal.collapsed;
                group.offline = offline;
                group.portal = Some(index);
                group.paths = items.iter().map(|item| item.path.clone()).collect();
                app.groups.push(group);
                app.items.extend(items);
            }
            app.portal_stamp = Some(stamp);
            changed = true;
        } else {
            for (_, _, items) in scanned {
                for item in items {
                    icons::destroy_item(item);
                }
            }
        }
    }

    if changed {
        app.tile_cache = None;
        if app.shown && app.drag.is_none() && app.header_drag.is_none() {
            overlay::relayout();
        }
    }
}

/// 在唤出面板、后台扫描完成、添加入口后调用。入口始终是最后一个分组。
pub fn rebuild_portal_items() {
    let app = state();
    // 先释放旧入口项的图标句柄，再摘掉它们和用户分组。
    let mut removed = Vec::new();
    app.items.retain(|item| {
        if item.source == icons::ItemSource::Portal {
            removed.push(item.clone());
            false
        } else {
            true
        }
    });
    for item in removed {
        icons::destroy_item(item);
    }
    app.groups.retain(|group| group.portal.is_none());
    for (index, portal) in app.portals.iter().enumerate() {
        let contents = crate::portals::scan(portal);
        let offline = contents.is_err();
        let items = contents.unwrap_or_default();
        let mut group = crate::groups::Group::new(portal_group_title(portal));
        group.collapsed = portal.collapsed;
        group.offline = offline;
        group.portal = Some(index);
        group.paths = items.iter().map(|item| item.path.clone()).collect();
        app.groups.push(group);
        app.items.extend(items);
    }
    app.portal_stamp = Some(portal_stamp(app));
}

/// 改某个入口分组的排序方式，立刻重扫并重排。
pub fn set_portal_sort(group_index: usize, mode: crate::portals::SortMode) {
    let app = state();
    let Some(portal_index) = app.groups.get(group_index).and_then(|group| group.portal) else {
        return;
    };
    let Some(portal) = app.portals.get_mut(portal_index) else {
        return;
    };
    if portal.sort == mode {
        return;
    }
    portal.sort = mode;
    let name = portal.name.clone();
    crate::portals::save(&app.portals);
    log::info(&format!("入口「{name}」排序改为 {}", mode.label()));
    rebuild_portal_items();
    overlay::relayout();
}

/// 切换配色主题：立刻重画界面，磨砂底色的变化下次唤出生效。
pub fn set_theme(key: &str) {
    let theme = crate::theme::by_key(key);
    let app = state();
    if app.theme.key == theme.key {
        return;
    }
    app.theme = theme;
    persist_settings();
    log::info(&format!("主题切换为「{}」（磨砂底色调下次唤出生效）", theme.name));
    overlay::relayout();
}

/// 切换界面语言：`auto` / `zh` / `en`。
pub fn set_language(setting: &str) {
    let app = state();
    if app.lang_setting == setting {
        return;
    }
    app.lang_setting = setting.to_string();
    crate::i18n::set(crate::i18n::resolve(setting));
    persist_settings();
    log::info(&format!("界面语言：{}", setting));
    if state().shown {
        // 标题栏文字也要跟着换，直接重建窗口最省事。
        overlay::reshow();
    } else {
        overlay::relayout();
    }
}

/// 把运行态设置整份写回，避免新增字段时某个入口忘记带。
fn persist_settings() {
    let app = state();
    crate::settings::save(&crate::settings::Settings {
        hotkey: app.hotkey,
        theme: app.theme.key.to_string(),
        lang: app.lang_setting.clone(),
        check_updates: app.check_updates,
        density: app.density,
    });
}

/// 切换图标尺寸档位：清图集缓存并立刻重排重绘。
pub fn set_density(density: crate::settings::IconDensity) {
    let app = state();
    if app.density == density {
        return;
    }
    app.density = density;
    app.tile_cache = None;
    persist_settings();
    log::info(&format!("图标大小改为：{}", density.as_str()));
    overlay::relayout();
}

/// 导出配置到用户选择的 JSON 文件。
pub fn export_config_from_picker() {
    let host = state().host;
    let Some(dest) = overlay::run_unpinned(|| crate::picker::pick_export_config(host)) else {
        return;
    };
    match crate::store::export_to(&dest) {
        Ok(()) => {
            log::info(&format!("已导出配置到：{}", dest.display()));
            if state().shown {
                overlay::show_drop_notice(crate::i18n::t("已导出配置", "Settings exported").into());
            } else {
                tray::notify(
                    host,
                    crate::i18n::t("LiteFences", "LiteFences"),
                    crate::i18n::t("已导出配置", "Settings exported"),
                );
            }
        }
        Err(error) => {
            log::error(&format!("导出配置失败：{error}"));
            if state().shown {
                overlay::show_drop_notice(crate::i18n::t("导出配置失败", "Failed to export settings").into());
            }
        }
    }
}

/// 从用户选择的 JSON 文件导入配置，校验写入并热重载运行态。
pub fn import_config_from_picker() {
    let host = state().host;
    let Some(src) = overlay::run_unpinned(|| crate::picker::pick_import_config(host)) else {
        return;
    };
    let imported = match crate::store::import_from(&src) {
        Ok(config) => config,
        Err(error) => {
            log::error(&format!("导入配置失败：{error}"));
            if state().shown {
                overlay::show_drop_notice(
                    crate::i18n::t("配置格式无效，未更改当前设置", "Invalid settings file").into(),
                );
            } else {
                tray::notify(
                    host,
                    crate::i18n::t("LiteFences", "LiteFences"),
                    crate::i18n::t("配置格式无效，未更改当前设置", "Invalid settings file"),
                );
            }
            return;
        }
    };

    let loaded_settings = crate::settings::load();
    let app = state();
    let old_hotkey = app.hotkey;
    if loaded_settings.hotkey != old_hotkey {
        // SAFETY: 宿主窗口有效。
        unsafe {
            UnregisterHotKey(host, HOTKEY_TOGGLE);
            if RegisterHotKey(
                host,
                HOTKEY_TOGGLE,
                loaded_settings.hotkey.modifiers,
                loaded_settings.hotkey.key,
            ) == 0
            {
                log::warn(&format!(
                    "导入热键 {} 被占用，保留原热键 {}",
                    loaded_settings.hotkey.describe(),
                    old_hotkey.describe()
                ));
                RegisterHotKey(host, HOTKEY_TOGGLE, old_hotkey.modifiers, old_hotkey.key);
            } else {
                app.hotkey = loaded_settings.hotkey;
            }
        }
    }
    app.theme = crate::theme::by_key(&loaded_settings.theme);
    app.lang_setting = loaded_settings.lang.clone();
    crate::i18n::set(crate::i18n::resolve(&loaded_settings.lang));
    app.check_updates = loaded_settings.check_updates;
    app.density = loaded_settings.density;
    app.onboarded = imported.onboarded;
    app.usage = imported.usage;
    app.groups = crate::groups::load();
    app.portals = crate::portals::load();
    app.custom = icons::load_custom();
    app.hidden = icons::load_hidden();
    app.layout_order = icons::load_order();
    app.undo_stack.clear();
    app.redo_stack.clear();
    app.tile_cache = None;
    app.portal_stamp = None;
    app.selected = None;
    clear_selection();

    match icons::collect_items(&app.hidden) {
        Ok(scanned) => icons::replace(&mut app.items, scanned),
        Err(error) => {
            log::error(&format!("导入后重扫桌面失败：{error}"));
            icons::refresh(&mut app.items);
        }
    }
    app.desktop_count = app.items.len();
    app.desktop_stamp = Some(icons::desktop_stamp());
    append_custom_items();
    icons::apply_order(&mut app.items, &app.layout_order);
    icons::apply_custom_icons(&mut app.items);
    crate::groups::prune(&mut app.groups, &app.custom);
    crate::groups::ensure_default(&mut app.groups);
    crate::groups::save(&app.groups);
    rebuild_portal_items();
    if app.hotkey != loaded_settings.hotkey {
        persist_settings();
    }
    log::info(&format!("已导入配置：{}", src.display()));
    if app.shown {
        overlay::reshow();
        overlay::show_drop_notice(crate::i18n::t("已导入配置", "Settings imported").into());
    } else {
        overlay::relayout();
        tray::notify(
            host,
            crate::i18n::t("LiteFences", "LiteFences"),
            crate::i18n::t("已导入配置", "Settings imported"),
        );
    }
}

/// 托盘菜单：开关自动检查更新。
pub fn set_check_updates(enabled: bool) {
    let app = state();
    if app.check_updates == enabled {
        return;
    }
    app.check_updates = enabled;
    persist_settings();
    log::info(&format!("自动检查更新：{}", if enabled { "开启" } else { "关闭" }));
}

/// 托盘/设置菜单：选一个目录，加为面板里的只读入口。
pub fn add_portal_from_picker() {
    let app = state();
    let host = app.host;
    // 面板是置顶全屏的，选目录时要先把它从置顶拿下来，否则对话框会被挡住。
    let Some(dir) = overlay::run_unpinned(|| crate::picker::pick_folder(host)) else {
        return;
    };
    let app = state();
    if app.portals.iter().any(|portal| icons::same_path(&portal.dir, &dir)) {
        log::info("这个目录已经是入口了");
        return;
    }
    let name = dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| dir.display().to_string());
    log::info(&format!("添加文件夹入口：{}（{}）", dir.display(), name));
    app.portals.push(crate::portals::FolderPortal {
        dir,
        name,
        collapsed: false,
        sort: crate::portals::SortMode::FolderFirst,
        current_dir: None,
    });
    crate::portals::save(&app.portals);
    rebuild_portal_items();
    overlay::relayout();
}

/// 把手动添加的图标接在桌面图标后面（同一路径不重复）。
fn append_custom_items() {
    let app = state();
    let customs = app.custom.clone();
    for path in customs {
        if app
            .items
            .iter()
            .any(|item| icons::same_path(&item.path, &path))
        {
            continue;
        }
        app.items.push(icons::make_custom_item(path));
    }
}

pub fn refresh_custom_availability() {
    for item in &mut state().items {
        if item.source != icons::ItemSource::Custom {
            continue;
        }
        let offline = !item.path.exists();
        if offline == item.offline {
            continue;
        }
        let mut refreshed = icons::make_custom_item(item.path.clone());
        refreshed.set_name(&item.name);
        refreshed.custom_icon.clone_from(&item.custom_icon);
        if let Some(path) = &item.custom_icon {
            icons::set_custom_icon(&mut refreshed, path);
        }
        icons::destroy_item(std::mem::replace(item, refreshed));
    }
}

/// 添加一个手动图标并持久化。
pub fn add_custom(path: std::path::PathBuf) {
    let app = state();
    if app
        .items
        .iter()
        .any(|item| icons::same_path(&item.path, &path))
    {
        log::info(&format!("该图标已在面板中：{}", path.display()));
        return;
    }
    log::info(&format!("添加自定义图标：{}", path.display()));
    app.custom.push(path.clone());
    icons::save_custom(&app.custom);
    app.items.push(icons::make_custom_item(path));
    if apply_auto_group_rules(false) > 0 {
        crate::groups::save(&app.groups);
    }
    persist_layout();
    overlay::relayout();
}

/// 外部拖入：把路径加进面板并归到目标组，返回一句给用户看的结果说明。
///
/// 只加引用——不复制、不移动、不删除任何源文件。已经在面板里的项只改归属。
pub fn drop_paths(paths: Vec<PathBuf>, group: usize) -> String {
    let app = state();
    if app.groups.get(group).is_some_and(|group| group.portal.is_some()) {
        log::info("拒绝把文件拖进只读入口");
        return crate::i18n::t(
            "入口是只读视图，请拖到其它分组",
            "Folder entries are read-only; drop into another group",
        )
        .to_string();
    }
    let mut added = 0usize;
    let mut moved = 0usize;
    let mut skipped = 0usize;
    let mut items_changed = false;
    let mut groups_changed = false;
    let mut seen: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !path.exists() {
            skipped += 1;
            log::warn(&format!("拖入的路径不存在，跳过：{}", path.display()));
            continue;
        }
        if seen.iter().any(|known| icons::same_path(known, &path)) {
            skipped += 1;
            continue;
        }
        seen.push(path.clone());
        // 面板项或手动列表里已经有了：不重复记录，只按需要补出面板项/改归属。
        let in_items = app
            .items
            .iter()
            .any(|item| icons::same_path(&item.path, &path));
        let in_custom = app.custom.iter().any(|known| icons::same_path(known, &path));
        if in_items || in_custom {
            if !in_items {
                // 后台扫描还没把手动项接进面板，这里先补上。
                app.items.push(icons::make_custom_item(path.clone()));
                items_changed = true;
            }
            if crate::groups::group_of(&app.groups, &path) == Some(group) {
                skipped += 1;
            } else {
                crate::groups::assign(&mut app.groups, &path, Some(group));
                moved += 1;
                groups_changed = true;
            }
            continue;
        }
        // 之前从面板移除过、但文件还在桌面上的：顺手取消隐藏，免得下次扫描又冲突。
        if let Some(index) = app
            .hidden
            .iter()
            .position(|known| icons::same_path(known, &path))
        {
            app.hidden.remove(index);
            icons::save_hidden(&app.hidden);
        }
        app.custom.push(path.clone());
        app.items.push(icons::make_custom_item(path.clone()));
        crate::groups::assign(&mut app.groups, &path, Some(group));
        added += 1;
        items_changed = true;
        groups_changed = true;
        log::info(&format!("拖入新增：{}", path.display()));
    }
    if items_changed {
        icons::save_custom(&app.custom);
    }
    // 拖进折叠的组：展开它，否则新项看不见。
    if (added + moved) > 0
        && let Some(target) = app.groups.get_mut(group)
        && target.collapsed
    {
        target.collapsed = false;
        groups_changed = true;
    }
    if groups_changed {
        crate::groups::save(&app.groups);
    }
    let name = app
        .groups
        .get(group)
        .map(|group| crate::groups::display_name(&group.name))
        .unwrap_or_default();
    if items_changed || groups_changed {
        persist_layout();
        overlay::relayout();
    }
    let english = crate::i18n::is_en();
    let mut parts = Vec::new();
    if added > 0 {
        parts.push(if english {
            format!("Added {added} to \"{name}\"")
        } else {
            format!("加入「{name}」{added} 项")
        });
    }
    if moved > 0 {
        parts.push(if english {
            format!("Moved {moved} into \"{name}\"")
        } else {
            format!("移入「{name}」{moved} 项")
        });
    }
    if skipped > 0 {
        parts.push(if english {
            format!("Skipped {skipped} (already present or unavailable)")
        } else {
            format!("跳过 {skipped} 项（已在面板或不可用）")
        });
    }
    if parts.is_empty() {
        if english {
            format!("Nothing new to add to \"{name}\"")
        } else {
            format!("「{name}」里没有可加入的新项目")
        }
    } else {
        parts.join("　·　")
    }
}

/// 给面板上的图标换一个图标文件（.ico）。
pub fn set_item_icon(index: usize, icon_path: &std::path::Path) -> bool {
    let app = state();
    let Some(item) = app.items.get_mut(index) else {
        return false;
    };
    if item.source == icons::ItemSource::Portal {
        log::info("入口是只读视图，不能换图标");
        return false;
    }
    if icon_path.exists() {
        remember_undo("icon");
    }
    if !icons::set_custom_icon(item, icon_path) {
        return false;
    }
    log::info(&format!(
        "图标已更换：{} → {}",
        item.name,
        icon_path.display()
    ));
    persist_layout();
    overlay::relayout();
    true
}

/// 恢复图标的系统默认外观。
pub fn clear_item_icon(index: usize) {
    let app = state();
    let Some(item) = app.items.get_mut(index) else {
        return;
    };
    if item.source == icons::ItemSource::Portal {
        return;
    }
    if item.custom_icon.is_none() {
        return;
    }
    remember_undo("icon");
    icons::clear_custom_icon(item);
    log::info(&format!("已恢复默认图标：{}", item.name));
    persist_layout();
    overlay::relayout();
}

/// 给面板上的图标改显示名（只改面板，不动源文件）。
pub fn rename_item(index: usize, name: &str) {
    let app = state();
    let Some(item) = app.items.get_mut(index) else {
        return;
    };
    if item.source == icons::ItemSource::Portal {
        log::info("入口是只读视图，不能改名");
        return;
    }
    let name = name.trim();
    if name.is_empty() || item.name == name {
        return;
    }
    remember_undo("rename");
    let path = item.path.display().to_string();
    item.set_name(&name);
    log::info(&format!("重命名：{path} → {name}"));
    persist_layout();
    overlay::relayout();
}

fn push_bounded<T>(stack: &mut Vec<T>, item: T, limit: usize) {
    if limit == 0 {
        return;
    }
    if stack.len() >= limit {
        stack.drain(0..=(stack.len() - limit));
    }
    stack.push(item);
}

fn push_undo_step(app: &mut App, step: crate::state::UndoStep) {
    push_bounded(&mut app.undo_stack, step, MAX_UNDO_STEPS);
    app.redo_stack.clear();
}

fn capture_panel_step(app: &App, label: &'static str) -> crate::state::UndoStep {
    crate::state::UndoStep::Panel(crate::state::UndoSnapshot {
        label,
        groups: app
            .groups
            .iter()
            .filter(|group| group.portal.is_none())
            .cloned()
            .collect(),
        custom: app.custom.clone(),
        hidden: app.hidden.clone(),
        items: app
            .items
            .iter()
            .filter(|item| item.source != icons::ItemSource::Portal)
            .map(|item| (item.path.clone(), item.name.clone(), item.custom_icon.clone()))
            .collect(),
        selected: app
            .selected
            .and_then(|index| app.items.get(index))
            .map(|item| item.path.clone()),
        selection: app
            .selection
            .iter()
            .filter_map(|&index| app.items.get(index))
            .map(|item| item.path.clone())
            .collect(),
    })
}

fn capture_order_step(app: &App) -> crate::state::UndoStep {
    let item_path = |index: usize| app.items.get(index).map(|item| item.path.clone());
    crate::state::UndoStep::Order {
        paths: app
            .items
            .iter()
            .filter(|item| item.source != icons::ItemSource::Portal)
            .map(|item| item.path.clone())
            .collect(),
        selected: app.selected.and_then(item_path),
        selection: app.selection.iter().copied().filter_map(item_path).collect(),
        anchor: app.selection_anchor.and_then(item_path),
    }
}

fn capture_portals_step(app: &App) -> crate::state::UndoStep {
    crate::state::UndoStep::Portals(app.portals.clone())
}

/// 记录改动前的面板状态，供 Ctrl+Z 撤销（最多保留 20 步）。
pub fn remember_undo(label: &'static str) {
    let app = state();
    let step = capture_panel_step(app, label);
    push_undo_step(app, step);
}

pub fn remember_group_undo(group: usize) {
    let app = state();
    if app.groups.get(group).is_some_and(|g| g.portal.is_some()) {
        let step = capture_portals_step(app);
        push_undo_step(app, step);
    } else {
        remember_undo("group");
    }
}

/// 撤销最近一步改动。
pub fn undo() -> bool {
    apply_history(false)
}

/// 重做最近一次撤销的改动。
pub fn redo() -> bool {
    apply_history(true)
}

fn apply_history(redo: bool) -> bool {
    let app = state();
    let step = if redo {
        app.redo_stack.pop()
    } else {
        app.undo_stack.pop()
    };
    let Some(step) = step else {
        log::info(if redo { "没有可重做的操作" } else { "没有可撤销的操作" });
        return false;
    };
    let reverse = match &step {
        crate::state::UndoStep::Panel(snapshot) => capture_panel_step(app, snapshot.label),
        crate::state::UndoStep::Order { .. } => capture_order_step(app),
        crate::state::UndoStep::Portals(_) => capture_portals_step(app),
    };
    if redo {
        push_bounded(&mut app.undo_stack, reverse, MAX_UNDO_STEPS);
    } else {
        push_bounded(&mut app.redo_stack, reverse, MAX_UNDO_STEPS);
    }

    let snapshot = match step {
        crate::state::UndoStep::Panel(snapshot) => snapshot,
        crate::state::UndoStep::Order { paths, selected, selection, anchor } => {
            restore_item_order(&mut app.items, &paths);
            save_layout_order(app);
            let locate = |path: &PathBuf| app.items.iter().position(|item|
                item.source != icons::ItemSource::Portal && icons::same_path(&item.path, path));
            app.selected = selected.as_ref().and_then(locate);
            app.selection = selection.iter().filter_map(locate).collect();
            app.selection_anchor = anchor.as_ref().and_then(locate);
            overlay::relayout();
            let notice = if redo {
                crate::i18n::t("已重做排序", "Redid reorder")
            } else {
                crate::i18n::t("已撤销排序", "Undid reorder")
            };
            overlay::show_drop_notice(notice.into());
            return true;
        }
        crate::state::UndoStep::Portals(portals) => {
            app.portals = portals;
            crate::portals::save(&app.portals);
            app.tile_cache = None;
            app.selected = None;
            clear_selection();
            rebuild_portal_items();
            overlay::relayout();
            let notice = if redo {
                crate::i18n::t("已重做入口改动", "Redid folder entry change")
            } else {
                crate::i18n::t("已撤销入口改动", "Undid folder entry change")
            };
            overlay::show_drop_notice(notice.into());
            return true;
        }
    };
    let label = snapshot.label;
    log::info(&format!("{}：{label}", if redo { "重做" } else { "撤销" }));
    app.groups = snapshot.groups.clone();
    app.custom = snapshot.custom.clone();
    app.hidden = snapshot.hidden.clone();
    match icons::collect_items(&app.hidden) {
        Ok(scanned) => icons::replace(&mut app.items, scanned),
        Err(error) => {
            log::error(&format!("历史恢复重扫桌面失败：{error}"));
            icons::refresh(&mut app.items);
        }
    }
    app.tile_cache = None;
    app.desktop_count = app.items.len();
    append_custom_items();
    icons::apply_order_entries(&mut app.items, &snapshot.items);
    crate::groups::save(&app.groups);
    icons::save_custom(&app.custom);
    icons::save_hidden(&app.hidden);
    persist_layout();
    rebuild_portal_items();
    let locate = |path: &PathBuf| app.items.iter().position(|item| icons::same_path(&item.path, path));
    app.selected = snapshot.selected.as_ref().and_then(locate);
    app.selection = snapshot.selection.iter().filter_map(locate).collect();
    app.selection_anchor = app.selection.last().copied();
    overlay::relayout();
    if app.shown {
        let text = if redo {
            match label {
                "remove" => crate::i18n::t("已重做移除", "Redid remove"),
                "move" => crate::i18n::t("已重做移动", "Redid move"),
                "rename" => crate::i18n::t("已重做重命名", "Redid rename"),
                "group" => crate::i18n::t("已重做分组改动", "Redid group change"),
                "icon" => crate::i18n::t("已重做图标改动", "Redid icon change"),
                _ => crate::i18n::t("已重做", "Redone"),
            }
        } else {
            match label {
                "remove" => crate::i18n::t("已撤销移除", "Undid remove"),
                "move" => crate::i18n::t("已撤销移动", "Undid move"),
                "rename" => crate::i18n::t("已撤销重命名", "Undid rename"),
                "group" => crate::i18n::t("已撤销分组改动", "Undid group change"),
                "icon" => crate::i18n::t("已撤销图标改动", "Undid icon change"),
                _ => crate::i18n::t("已撤销", "Undone"),
            }
        };
        overlay::show_drop_notice(text.to_string());
    }
    true
}

/// 记录一次启动，更新内存与配置中的使用次数及最近使用时间。
pub fn record_launch(path: &std::path::Path) {
    let app = state();
    let key = crate::store::usage_key(path);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let entry = app.usage.entry(key).or_default();
    entry.count = entry.count.saturating_add(1);
    entry.last_used = now;
    crate::store::save_usage(&app.usage);
}

fn apply_auto_group_rules(include_default: bool) -> usize {
    let app = state();
    let pairs: Vec<(PathBuf, String)> = app
        .items
        .iter()
        .filter(|item| item.source != icons::ItemSource::Portal)
        .map(|item| (item.path.clone(), item.name.clone()))
        .collect();
    crate::groups::apply_rules(&mut app.groups, &pairs, include_default)
}

/// 为普通非默认分组设置自动归组规则，并立即按规则整理未归组/「其它」组中的匹配项。
pub fn set_group_rules(group_index: usize, raw_rules: &str) -> Option<usize> {
    let app = state();
    let is_editable = app
        .groups
        .get(group_index)
        .is_some_and(|g| g.portal.is_none() && g.name != crate::groups::DEFAULT_GROUP);
    if !is_editable {
        return None;
    }
    remember_undo("group");
    let rules = crate::groups::parse_rules(raw_rules);
    let name = app.groups[group_index].name.clone();
    app.groups[group_index].rules = rules.clone();
    let moved = apply_auto_group_rules(true);
    crate::groups::save(&app.groups);
    app.tile_cache = None;
    log::info(&format!(
        "分组「{name}」自动规则已更新（{} 条），归入 {moved} 项",
        rules.len()
    ));
    overlay::relayout();
    Some(moved)
}

/// 在文件夹入口中点击子目录时原地切入浏览。
pub fn enter_portal_subdir(path: &std::path::Path) -> bool {
    let app = state();
    let Some(group_index) = crate::groups::group_of(&app.groups, path) else {
        return false;
    };
    let Some(portal_index) = app.groups.get(group_index).and_then(|g| g.portal) else {
        return false;
    };
    let Some(portal) = app.portals.get_mut(portal_index) else {
        return false;
    };
    if !portal.enter(path.to_path_buf()) {
        return false;
    }
    let title = portal_group_title(portal);
    app.tile_cache = None;
    app.portal_stamp = None;
    app.selected = None;
    clear_selection();
    rebuild_portal_items();
    log::info(&format!("入口下钻：{title}"));
    overlay::relayout();
    true
}

pub fn portal_step_up(portal_index: usize) -> bool {
    let app = state();
    let Some(portal) = app.portals.get_mut(portal_index) else {
        return false;
    };
    if !portal.step_up() {
        return false;
    }
    let title = portal_group_title(portal);
    app.tile_cache = None;
    app.portal_stamp = None;
    app.selected = None;
    clear_selection();
    rebuild_portal_items();
    log::info(&format!("入口返回上一级：{title}"));
    overlay::relayout();
    true
}

pub fn portal_reset_root(portal_index: usize) -> bool {
    let app = state();
    let Some(portal) = app.portals.get_mut(portal_index) else {
        return false;
    };
    if !portal.reset_root() {
        return false;
    }
    let title = portal_group_title(portal);
    app.tile_cache = None;
    app.portal_stamp = None;
    app.selected = None;
    clear_selection();
    rebuild_portal_items();
    log::info(&format!("入口回到根目录：{title}"));
    overlay::relayout();
    true
}

pub fn refresh_portal(group_index: usize) {
    let app = state();
    if app.groups.get(group_index).and_then(|g| g.portal).is_none() {
        return;
    }
    app.tile_cache = None;
    app.portal_stamp = None;
    rebuild_portal_items();
    overlay::relayout();
    if state().shown {
        overlay::show_drop_notice(
            crate::i18n::t("已刷新文件夹入口", "Folder entry refreshed").into(),
        );
    }
}

/// 清空鼠标多选。
pub fn clear_selection() {
    let app = state();
    if app.selection.is_empty() && app.selection_anchor.is_none() {
        return;
    }
    app.selection.clear();
    app.selection_anchor = None;
    app.static_dirty = true;
}

/// 把多个图标移到 `target` 分组。返回实际移动的数量。
pub fn move_items_to_group(indices: &[usize], target: usize) -> usize {
    let app = state();
    if app
        .groups
        .get(target)
        .is_none_or(|group| group.portal.is_some())
    {
        return 0;
    }
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for &index in indices {
        let Some(item) = app.items.get(index) else {
            continue;
        };
        if item.source == icons::ItemSource::Portal {
            continue;
        }
        if crate::groups::group_of(&app.groups, &item.path) == Some(target) {
            continue;
        }
        if paths.iter().any(|known| icons::same_path(known, &item.path)) {
            continue;
        }
        paths.push(item.path.clone());
    }
    if paths.is_empty() {
        return 0;
    }
    remember_undo("move");
    for path in &paths {
        crate::groups::assign(&mut app.groups, path, Some(target));
    }
    if let Some(group) = app.groups.get_mut(target)
        && group.collapsed
    {
        group.collapsed = false;
        log::info("目标组已折叠，自动展开");
    }
    let name = app
        .groups
        .get(target)
        .map(|group| crate::groups::display_name(&group.name))
        .unwrap_or_default();
    crate::groups::save(&app.groups);
    for path in &paths {
        log::info(&format!("移入分组「{name}」：{}", path.display()));
    }
    persist_layout();
    app.selection.clear();
    app.selection_anchor = None;
    app.selected = None;
    overlay::relayout();
    paths.len()
}

/// 从面板移除多个项：手动图标删记录，桌面图标进隐藏列表（都不动源文件）。
pub fn remove_items(indices: &[usize]) {
    let app = state();
    let mut targets: Vec<usize> = indices
        .iter()
        .copied()
        .filter(|index| *index < app.items.len())
        .collect();
    targets.sort_unstable();
    targets.dedup();
    let mut skipped_portal = false;
    targets.retain(|index| {
        if app.items[*index].source == icons::ItemSource::Portal {
            skipped_portal = true;
            false
        } else {
            true
        }
    });
    if targets.is_empty() {
        if skipped_portal {
            log::info("入口是只读视图，不能用「移除」删掉文件；要移除请右键入口标题");
        }
        return;
    }
    remember_undo("remove");
    let mut removed = Vec::new();
    let mut paths = Vec::new();
    for index in targets.iter().rev() {
        let item = app.items.remove(*index);
        paths.push(item.path.clone());
        removed.push(item);
    }
    for item in &removed {
        match item.source {
            icons::ItemSource::Custom => {
                app.custom
                    .retain(|known| !icons::same_path(known, &item.path));
            }
            icons::ItemSource::Desktop => {
                if !app
                    .hidden
                    .iter()
                    .any(|known| icons::same_path(known, &item.path))
                {
                    app.hidden.push(item.path.clone());
                }
            }
            icons::ItemSource::Portal => {}
        }
    }
    icons::save_custom(&app.custom);
    icons::save_hidden(&app.hidden);
    for path in &paths {
        crate::groups::forget(&mut app.groups, path);
    }
    crate::groups::save(&app.groups);
    for item in removed {
        icons::destroy_item(item);
    }
    log::info(&format!("从面板移除 {} 项", paths.len()));
    persist_layout();
    app.selection.clear();
    app.selection_anchor = None;
    app.selected = None;
    overlay::relayout();
}

/// 把当前顺序与名字写回 layout.txt。
///
/// 写入前先把 `items` 整理成“按分组顺序、组内保持当前次序”，这样文件里的顺序
/// 和面板上看到的一致，跨组拖动后也不会越来越乱。
pub fn persist_layout() {
    let app = state();
    let groups = &app.groups;
    app.items.sort_by_cached_key(|item| {
        if item.source == icons::ItemSource::Portal {
            usize::MAX
        } else {
            crate::groups::group_of_or_default(groups, &item.path).unwrap_or(0)
        }
    });
    save_layout_order(app);
}

fn save_layout_order(app: &mut crate::state::App) {
    let saved: Vec<_> = app
        .items
        .iter()
        .filter(|item| item.source != icons::ItemSource::Portal)
        .cloned()
        .collect();
    app.layout_order = saved
        .iter()
        .map(|item| (item.path.clone(), item.name.clone()))
        .collect();
    icons::save_order(&saved);
}

// 刷新后仍按路径识别旧项目。新增项目保持自身次序，入口内容始终排在最后。
fn restore_item_order(items: &mut [icons::DesktopItem], paths: &[PathBuf]) {
    let ranks: std::collections::HashMap<_, _> = paths.iter().enumerate()
        .map(|(rank, path)| (path.to_string_lossy().to_lowercase(), rank)).collect();
    items.sort_by_cached_key(|item| {
        if item.source == icons::ItemSource::Portal {
            usize::MAX
        } else {
            ranks.get(&item.path.to_string_lossy().to_lowercase()).copied().unwrap_or(paths.len())
        }
    });
}

fn block_order(count: usize, selected: &[usize], from: usize, target: usize) -> Option<Vec<usize>> {
    let mut moving = selected.to_vec();
    moving.sort_unstable();
    moving.dedup();
    if target >= count || moving.last().is_none_or(|last| *last >= count)
        || !moving.contains(&from) || moving.contains(&target) {
        return None;
    }
    let mut order: Vec<_> = (0..count).filter(|index| moving.binary_search(index).is_err()).collect();
    let position = order.iter().position(|index| *index == target)? + usize::from(from < target);
    order.splice(position..position, moving);
    (!order.iter().copied().eq(0..count)).then_some(order)
}

pub fn reorder_items(from: usize, target: usize) {
    let app = state();
    let Some(destination) = app.items.get(target) else { return };
    let group = crate::groups::group_of_or_default(&app.groups, &destination.path);
    let selected = if app.selection.contains(&from) { app.selection.as_slice() } else { std::slice::from_ref(&from) };
    if destination.source == icons::ItemSource::Portal || selected.iter().any(|&index| {
        app.items.get(index).is_none_or(|item| item.source == icons::ItemSource::Portal
            || crate::groups::group_of_or_default(&app.groups, &item.path) != group)
    }) { return; }
    let Some(order) = block_order(app.items.len(), selected, from, target) else { return };
    let step = capture_order_step(app);
    push_undo_step(app, step);
    app.items = order.into_iter().map(|index| app.items[index].clone()).collect();
    clear_selection();
    app.selected = None;
    persist_layout();
    overlay::relayout();
}

#[cfg(test)]
mod reorder_tests {
    use super::block_order;

    #[test]
    fn block_drag_keeps_visible_order_in_both_directions() {
        assert_eq!(block_order(6, &[4, 2], 2, 0), Some(vec![2, 4, 0, 1, 3, 5]));
        assert_eq!(block_order(6, &[3, 1], 1, 5), Some(vec![0, 2, 4, 5, 1, 3]));
        assert_eq!(block_order(6, &[1, 3], 1, 3), None);
        assert_eq!(block_order(3, &[1], 1, 2), Some(vec![0, 2, 1]));
    }

    #[test]
    fn undo_restores_order_and_names_after_a_block_drag() {
        use crate::icons::{DesktopItem, ItemSource};
        let mut items: Vec<_> = ["甲", "乙", "丙", "丁"].into_iter().map(|name| DesktopItem {
            search_keys: Default::default(),
            path: name.into(), name: name.into(), source: ItemSource::Custom,
            offline: true, icon: std::ptr::null_mut(), custom_icon: None,
        }).collect();
        let snapshot: Vec<_> = items.iter().map(|item| item.path.clone()).collect();
        let order = block_order(items.len(), &[3, 1], 3, 0).unwrap();
        items = order.into_iter().map(|index| items[index].clone()).collect();
        assert_eq!(items[0].name, "乙");
        items.remove(2); // 甲在刷新中消失，新加入的路径不得被撤销删除。
        items[0].set_name("乙的新名称");
        let mut extra = items[0].clone();
        extra.path = "戊".into();
        items.insert(0, extra);
        super::restore_item_order(&mut items, &snapshot);
        let paths: Vec<_> = items.iter().map(|item| item.path.to_str().unwrap()).collect();
        assert_eq!(paths, ["乙", "丙", "丁", "戊"]);
        assert_eq!(items[0].name, "乙的新名称");
    }

    #[test]
    fn bounded_history_drops_oldest_beyond_limit() {
        let mut stack = Vec::new();
        for i in 1..=25usize {
            super::push_bounded(&mut stack, i, super::MAX_UNDO_STEPS);
        }
        assert_eq!(stack.len(), super::MAX_UNDO_STEPS);
        assert_eq!(stack.first(), Some(&6));
        assert_eq!(stack.last(), Some(&25));
    }

    #[test]
    fn multi_step_undo_redo_and_new_edit_clears_redo() {
        let mut undo = Vec::new();
        let mut redo = Vec::new();
        let mut cur = 0i32;
        for next in 1..=3 {
            super::push_bounded(&mut undo, cur, super::MAX_UNDO_STEPS);
            redo.clear();
            cur = next;
        }
        for _ in 0..2 {
            let prev = undo.pop().unwrap();
            super::push_bounded(&mut redo, cur, super::MAX_UNDO_STEPS);
            cur = prev;
        }
        assert_eq!(cur, 1);
        assert_eq!(undo, vec![0]);
        assert_eq!(redo, vec![3, 2]);
        let next = redo.pop().unwrap();
        super::push_bounded(&mut undo, cur, super::MAX_UNDO_STEPS);
        cur = next;
        assert_eq!(cur, 2);
        // 新操作清空 redo 栈
        super::push_bounded(&mut undo, cur, super::MAX_UNDO_STEPS);
        redo.clear();
        cur = 99;
        assert!(redo.is_empty());
        assert_eq!(undo, vec![0, 1, 2]);
        assert_eq!(cur, 99);
    }
}

/// 当前进程的完整性级别 RID：0x1000 = 低，0x2000 = 中，0x3000 = 高。
fn integrity_rid() -> u32 {
    // SAFETY: 只读取本进程令牌；缓冲区足够容纳一个完整性标签。
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return 0x2000;
        }
        let mut buffer = [0u8; 256];
        let mut needed = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            buffer.as_mut_ptr() as *mut std::ffi::c_void,
            buffer.len() as u32,
            &mut needed,
        );
        CloseHandle(token);
        if ok == 0 {
            return 0x2000;
        }
        let label = &*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let count = *GetSidSubAuthorityCount(label.Label.Sid);
        if count == 0 {
            return 0x2000;
        }
        *GetSidSubAuthority(label.Label.Sid, u32::from(count - 1))
    }
}

/// 旧的低权限引导实现，保留作参考；现在改为面板内的橙色横幅，不再弹框。
#[allow(dead_code)]
fn relaunch_normal_and_exit_obsolete() -> bool {
    let text = wide(
        "LiteFences 现在是以受限权限（低完整性级别）启动的。\n\
         在这个权限下：通知区图标出不来，很多程序点了也打不开。\n\n\
         正确做法：关闭本程序，在资源管理器里双击 LiteFencesDesktop.exe。\n\n\
         点「是」打开程序所在文件夹（在那里双击即可）；\n\
         点「否」继续以受限权限运行（只有热键和面板，没有托盘）。",
    );
    let caption = wide(crate::i18n::t("LiteFences 桌面启动器", "LiteFences"));
    // SAFETY: 文本以 0 结尾，父窗口为空表示桌面模态。
    let choice = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_YESNO | MB_ICONWARNING,
        )
    };
    if choice != IDYES {
        log::warn("用户选择继续以受限权限运行");
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let explorer = wide("explorer.exe");
    let operation = wide("open");
    let parameters = wide(&format!("/select,\"{}\"", exe.to_string_lossy()));
    // SAFETY: 只让资源管理器打开并选中本程序，不会自己启动新进程。
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            explorer.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    let opened = result as isize > 32;
    log::info(&format!(
        "打开程序所在文件夹：{}",
        if opened { "已提交" } else { "失败" }
    ));
    opened
}

/// 单实例对象名。排障时可用环境变量加后缀，避免与正在运行的实例互相干扰。
fn instance_name() -> Vec<u16> {
    let suffix = std::env::var("LITEFENCES_DESKTOP_INSTANCE").unwrap_or_default();
    wide(&format!("Local\\LiteFencesDesktopLauncher{suffix}"))
}

fn another_instance_running() -> bool {
    /// SYNCHRONIZE：只要求同步访问，足以判断互斥体是否存在。
    const SYNCHRONIZE: u32 = 0x0010_0000;
    let name = instance_name();
    // SAFETY: 名称以 0 结尾；只查询是否存在，不取得所有权。
    let handle = unsafe { OpenMutexW(SYNCHRONIZE, 0, name.as_ptr()) };
    if handle.is_null() {
        return false;
    }
    // SAFETY: 句柄由本函数打开，需关闭。
    unsafe { CloseHandle(handle) };
    true
}

fn claim_single_instance() -> bool {    let name = instance_name();
    // SAFETY: 名称以 0 结尾；句柄有意泄漏到进程结束，用来维持互斥体。
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        log::warn("创建单实例互斥体失败，按单实例继续运行");
        return true;
    }
    // SAFETY: 无参数读取上一调用的错误码。
    let error = unsafe { GetLastError() };
    error != ERROR_ALREADY_EXISTS
}

fn create_host(hinstance: windows_sys::Win32::Foundation::HMODULE) -> Result<HWND, String> {
    let class_name = wide(HOST_CLASS);
    let cursor = unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) };
    let class = WNDCLASSW {
        lpfnWndProc: Some(host_proc),
        hInstance: hinstance,
        hCursor: cursor,
        lpszClassName: class_name.as_ptr(),
        ..Default::default()
    };
    // SAFETY: 窗口类数据在调用期间有效。
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err("无法注册宿主窗口类".into());
    }
    let title = wide("LiteFences 桌面启动器");
    // 托盘需要能被 Shell 找到的顶层窗口；消息专用窗口在部分系统上会被拒绝，
    // 因此用不显示、不进 Alt+Tab 的普通窗口。
    // SAFETY: 类名、标题与模块句柄均有效；窗口不设置 WS_VISIBLE。
    let host = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
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
    if host.is_null() {
        return Err("无法创建宿主窗口".into());
    }
    Ok(host)
}

fn register_hotkeys(host: HWND) {
    let hotkey = state().hotkey;
    // SAFETY: 宿主窗口有效；热键消息回到本进程，不受权限隔离影响。
    unsafe {
        if RegisterHotKey(host, HOTKEY_TOGGLE, hotkey.modifiers, hotkey.key) == 0 {
            log::warn(&format!(
                "注册唤出热键 {} 失败（可能被其他程序占用），可在托盘菜单里改一个",
                hotkey.describe()
            ));
        }
        if RegisterHotKey(host, HOTKEY_EXIT, MOD_CONTROL | MOD_ALT, u32::from(VK_Q)) == 0 {
            log::warn("注册 Ctrl+Alt+Q 失败（可能被其他程序占用）");
        }
    }
    log::info(&format!(
        "快捷键：{} 唤出/收起，Ctrl+Alt+Q 退出，Esc 收起",
        hotkey.describe()
    ));
}

/// 换一个唤出热键：先注销旧的，再注册新的；失败就回到旧热键。
fn change_hotkey() {
    let current = state().hotkey;
    let Some(chosen) = crate::hotkey_dialog::ask_for_hotkey(current) else {
        log::info("取消修改热键");
        return;
    };
    if chosen == current {
        return;
    }
    let host = state().host;
    // SAFETY: 句柄有效。
    unsafe {
        UnregisterHotKey(host, HOTKEY_TOGGLE);
        if RegisterHotKey(host, HOTKEY_TOGGLE, chosen.modifiers, chosen.key) == 0 {
            log::warn(&format!(
                "{} 被占用，恢复原热键 {}",
                chosen.describe(),
                current.describe()
            ));
            RegisterHotKey(host, HOTKEY_TOGGLE, current.modifiers, current.key);
            let text = wide(&format!(
                "{} 已被其它程序占用，仍使用 {}",
                chosen.describe(),
                current.describe()
            ));
            let caption = wide("LiteFences 桌面启动器");
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                caption.as_ptr(),
                MB_OK | MB_ICONWARNING,
            );
            return;
        }
    }
    {
        let app = state();
        app.hotkey = chosen;
        crate::settings::save(&crate::settings::Settings {
            hotkey: chosen,
            theme: app.theme.key.to_string(),
            lang: app.lang_setting.clone(),
            check_updates: app.check_updates,
            density: app.density,
        });
    }
    log::info(&format!("唤出热键已改为 {}", chosen.describe()));
}

/// 崩溃也要留痕：panic 先把信息写进日志，再按默认行为终止。
///
/// 没有这一步，别人机器上闪退就只剩一个“程序没了”，日志里什么都没有。
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "未知位置".to_string());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| (*text).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "（无消息）".to_string());
        log::error(&format!("崩溃：{payload} @ {location}"));
        previous(info);
    }));
}

fn message_loop() -> std::process::ExitCode {
    let mut message = MSG::default();
    // 主循环退出一定要留痕：以前对话框里的 PostQuitMessage 会让软件“自己关掉”，
    // 日志里却看不出是正常退出还是崩溃。
    log::info("进入主消息循环");
    loop {
        // SAFETY: 标准 Win32 消息循环；取到 0 或 -1 时结束。
        let result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        if overlay::search_edit_key(&message) {
            continue;
        }
        // SAFETY: 消息结构由 GetMessage 填充。
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    log::info("主消息循环结束，进程退出");
    std::process::ExitCode::SUCCESS
}

fn request_exit(host: HWND) {
    let app = state();
    if app.icons_hidden {
        app.icons_hidden = !desktop_icons::set_visible(true);
    }
    tray::remove(app.host);
    // SAFETY: 句柄有效。
    unsafe { DestroyWindow(host) };
}

unsafe extern "system" fn host_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // 资源管理器重启后托盘图标会丢失，需要重新添加；桌面视图句柄也换了。
    let taskbar_created = tray::taskbar_created_message();
    if taskbar_created != 0 && message == taskbar_created {
        log::info("收到 TaskbarCreated（资源管理器重启）：重建托盘、重扫桌面图标");
        state().tray_installed = tray::install(hwnd);
        let app = state();
        // 面板还开着时，Explorer 会把桌面图标重新显示出来。刚重启时 DefView 可能还没建好，
        // 所以交给定时器重试几次，而不是当场只试一次。
        if app.shown && app.icons_hidden {
            app.rehide_left = 3;
            // SAFETY: 宿主窗口有效；定时器到期后自己重试。
            unsafe { SetTimer(hwnd, TIMER_REHIDE, 700, None) };
        }
        // 桌面上的图标集合可能变了（新增/删除/重排），重新扫描一遍。
        begin_scan(hwnd);
        return 0;
    }
    match message {
        tray::TRAY_CALLBACK => {
            let event = (lparam as u32) & 0xffff;
            match event {
                WM_LBUTTONUP => overlay::show_panel(),
                WM_RBUTTONUP => handle_menu(hwnd),
                tray::NIN_BALLOONUSERCLICK => crate::update::open_latest(),
                _ => {}
            }
            0
        }
        WM_COMMAND => {
            handle_command(wparam & 0xffff);
            0
        }
        WM_HOTKEY => {
            match wparam as i32 {
                HOTKEY_TOGGLE => {
                    if state().shown {
                        overlay::hide_panel();
                    } else {
                        overlay::show_panel();
                    }
                }
                HOTKEY_EXIT => request_exit(hwnd),
                _ => {}
            }
            0
        }
        WM_TIMER if wparam == TIMER_DEMO => {
            overlay::hide_panel();
            let app = state();
            if app.demo_cycles_left > 1 {
                app.demo_cycles_left -= 1;
                // SAFETY: 句柄有效；稍后再唤出一次。
                unsafe { SetTimer(hwnd, TIMER_DEMO_RESTART, 600, None) };
            } else {
                request_exit(hwnd);
            }
            0
        }
        WM_TIMER if wparam == TIMER_DEMO_RESTART => {
            // SAFETY: 句柄有效；一次性定时器。
            unsafe { KillTimer(hwnd, TIMER_DEMO_RESTART) };
            let app = state();
            if let Some(ms) = app.options.show_ms {
                overlay::show_panel();
                // SAFETY: 句柄有效。
                unsafe { SetTimer(hwnd, TIMER_DEMO, ms, None) };
            }
            0
        }
        windows_sys::Win32::UI::WindowsAndMessaging::WM_DEVICECHANGE => {
            use windows_sys::Win32::UI::WindowsAndMessaging::{DBT_DEVICEARRIVAL, DBT_DEVICEREMOVECOMPLETE};
            if matches!(wparam as u32, DBT_DEVICEARRIVAL | DBT_DEVICEREMOVECOMPLETE) {
                unsafe { SetTimer(hwnd, TIMER_DEVICE_CHANGE, 500, None) };
            }
            1
        }
        WM_TIMER if wparam == TIMER_DEVICE_CHANGE => {
            if state().drag.is_some() || state().header_drag.is_some() { return 0; }
            unsafe { KillTimer(hwnd, TIMER_DEVICE_CHANGE) };
            if state().shown {
                begin_availability_refresh(hwnd);
            }
            0
        }
        WM_APP_REFRESH => {
            finish_availability_refresh();
            0
        }
        WM_APP_SCAN => {
            finish_scan();
            // 排障：面板就绪后按顺序输入关键字，把匹配结果打进日志。
            if let Some(query) = state().options.search_test.clone() {
                for ch in query.chars() {
                    overlay::type_to_search(ch);
                }
                let (count, first) = overlay::search_report();
                log::info(&format!(
                    "搜索测试：「{query}」→ {count} 个匹配，第一个={}",
                    first.as_deref().unwrap_or("（无）")
                ));
            }
            0
        }
        crate::update::WM_APP_UPDATE => {
            crate::update::handle_result(hwnd);
            0
        }
        WM_TIMER if wparam == TIMER_REHIDE => {
            let app = state();
            if !app.shown || !app.icons_hidden || app.rehide_left == 0 {
                // SAFETY: 句柄有效；不再需要补做。
                unsafe { KillTimer(hwnd, TIMER_REHIDE) };
                return 0;
            }
            if desktop_icons::set_visible(false) {
                app.rehide_left = 0;
                log::info("资源管理器重启后已重新隐藏桌面图标");
                // SAFETY: 句柄有效；任务完成。
                unsafe { KillTimer(hwnd, TIMER_REHIDE) };
            } else if app.rehide_left > 1 {
                app.rehide_left -= 1;
                log::warn(&format!("桌面图标还没藏住，稍后重试（还剩 {} 次）", app.rehide_left));
            } else {
                app.rehide_left = 0;
                app.icons_hidden = false;
                log::error("资源管理器重启后没能重新隐藏桌面图标，放弃并等待下次唤出");
                // SAFETY: 句柄有效。
                unsafe { KillTimer(hwnd, TIMER_REHIDE) };
            }
            0
        }
        WM_TIMER if wparam == TIMER_CAPTURE_EARLY => {
            // SAFETY: 句柄有效；只抓一次。
            unsafe { KillTimer(hwnd, TIMER_CAPTURE_EARLY) };
            let app = state();
            let path = log::base_dir().join("panel-capture-early.bmp");
            if let Err(error) = crate::capture::capture_window(app.overlay, &path) {
                log::error(&format!("抓取面板（早）失败：{error}"));
            }
            0
        }
        WM_TIMER if wparam == TIMER_CAPTURE => {
            // SAFETY: 句柄有效；只抓一次。
            unsafe { KillTimer(hwnd, TIMER_CAPTURE) };
            let app = state();
            if let Some(path) = app.options.capture.clone()
                && let Err(error) =
                    crate::capture::capture_window(app.overlay, std::path::Path::new(&path))
            {
                log::error(&format!("抓取面板失败：{error}"));
            }
            0
        }
        WM_TIMER if wparam == TIMER_TRAY_RETRY => {
            // SAFETY: 句柄有效；只重试一次。
            unsafe { KillTimer(hwnd, TIMER_TRAY_RETRY) };
            let app = state();
            if !app.tray_installed {
                log::info("重试添加托盘图标");
                app.tray_installed = tray::install(app.host);
            }
            0
        }
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if wparam != 0 {
                let app = state();
                if app.icons_hidden {
                    app.icons_hidden = !desktop_icons::set_visible(true);
                }
                tray::remove(app.host);
            }
            0
        }
        WM_CLOSE => {
            request_exit(hwnd);
            0
        }
        WM_DESTROY => {
            // SAFETY: 结束消息循环。
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => {
            // SAFETY: 默认处理其余消息。
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

fn handle_menu(host: HWND) {
    let exe = std::env::current_exe().unwrap_or_default();
    let autostart_on = crate::autostart::enabled(&exe);
    let hotkey_text = state().hotkey.describe();
    let theme_key = state().theme.key;
    let density = state().density;
    let lang_setting = state().lang_setting.clone();
    let check_updates = state().check_updates;
    let command = tray::show_menu(
        host,
        autostart_on,
        &hotkey_text,
        theme_key,
        density,
        &lang_setting,
        check_updates,
    );
    handle_command(command);
}

fn handle_command(command: usize) {
    match command {
        tray::MENU_SHOW => overlay::show_panel(),
        tray::MENU_HIDE => overlay::hide_panel(),
        tray::MENU_AUTOSTART => toggle_autostart(),
        tray::MENU_HOTKEY => change_hotkey(),
        tray::MENU_ADD_PORTAL => add_portal_from_picker(),
        tray::MENU_EXPORT_CONFIG => export_config_from_picker(),
        tray::MENU_IMPORT_CONFIG => import_config_from_picker(),
        tray::MENU_CHECK_UPDATE => crate::update::check(state().host, true),
        tray::MENU_AUTO_UPDATE => {
            let enabled = !state().check_updates;
            set_check_updates(enabled);
        }
        tray::MENU_EXIT => request_exit(state().host),
        other if other >= tray::MENU_LANG_BASE => {
            let index = other - tray::MENU_LANG_BASE;
            if let Some(setting) = ["auto", "zh", "en"].get(index) {
                set_language(setting);
            }
        }
        other if other >= tray::MENU_DENSITY_BASE => {
            let index = other - tray::MENU_DENSITY_BASE;
            if let Some(&density) = crate::settings::IconDensity::all().get(index) {
                set_density(density);
            }
        }
        other if other >= tray::MENU_THEME_BASE => {
            let index = other - tray::MENU_THEME_BASE;
            if let Some(theme) = crate::theme::THEMES.get(index) {
                set_theme(theme.key);
            }
        }
        _ => {}
    }
}

/// 切换开机自启（只写 HKCU 下本项目自己的 Run 值）。
fn toggle_autostart() {
    let exe = std::env::current_exe().unwrap_or_default();
    let currently = crate::autostart::enabled(&exe);
    match crate::autostart::set_enabled(&exe, !currently) {
        Ok(()) => log::info(&format!(
            "开机自启已{}（{}）",
            if currently { "关闭" } else { "开启" },
            exe.display()
        )),
        Err(error) => {
            log::error(&format!("设置开机自启失败：{error}"));
            let text = wide(&format!(
            "{}：{error}",
            crate::i18n::t("设置开机自启失败", "Failed to change startup setting")
        ));
            let caption = wide("LiteFences 桌面启动器");
            // SAFETY: 文本以 0 结尾；父窗口为空表示桌面模态。
            unsafe {
                MessageBoxW(
                    std::ptr::null_mut(),
                    text.as_ptr(),
                    caption.as_ptr(),
                    MB_OK | MB_ICONWARNING,
                );
            }
        }
    }
}

/// 搜索关键字变化：更新过滤条件并重画面板。
pub fn set_search_text(text: &str) {
    let app = state();
    if app.search == text {
        return;
    }
    app.search = text.to_string();
    app.static_dirty = true;
    if app.shown {
        // SAFETY: 句柄有效。
        unsafe {
            windows_sys::Win32::Graphics::Gdi::InvalidateRect(
                app.overlay,
                std::ptr::null(),
                0,
            )
        };
    }
}

/// 从面板左上角的「设置」按钮打开同一个托盘菜单。
pub fn open_settings_menu() {
    let host = state().host;
    handle_menu(host);
}

/// 排障：把指定路径当成一次真实的外部拖入投给面板，验证拖放注册、路径提取与加引用。
fn run_drop_test(source: &str) -> std::process::ExitCode {
    let sources: Vec<PathBuf> = source
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(PathBuf::from)
        .collect();
    overlay::show_panel();
    let before = state().items.len();
    let Some(point) = overlay::first_group_screen_point() else {
        log::error("拖放测试：拿不到落点（面板没有分组标题）");
        overlay::hide_panel();
        return std::process::ExitCode::FAILURE;
    };
    let result = crate::drop_target::exercise_drop(&sources, point);
    let after = state().items.len();
    let group = state()
        .groups
        .first()
        .map(|group| group.name.clone())
        .unwrap_or_default();
    let assigned = sources.iter().all(|path| {
        state()
            .groups
            .iter()
            .any(|group| group.paths.iter().any(|known| icons::same_path(known, path)))
    });
    let listed = sources
        .iter()
        .all(|path| state().custom.iter().any(|known| icons::same_path(known, path)));
    let passed = result.is_ok() && after > before && assigned && listed;
    let summary = format!(
        "point=({}, {}) sources={} effect={:?} items {}→{} 全部在组「{}」={} 全部已持久化={} => {}",
        point.0,
        point.1,
        sources.len(),
        result.as_deref().unwrap_or_else(|error| error),
        before,
        after,
        group,
        assigned,
        listed,
        if passed { "通过" } else { "失败" }
    );
    log::info(&format!("拖放测试：{summary}"));
    let report = report_path().with_file_name("drop-test.txt");
    if let Some(parent) = report.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&report, format!("{summary}\n"));
    overlay::hide_panel();
    if passed {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

fn self_test_report() -> std::process::ExitCode {
    let app = state();
    icons::refresh(&mut app.items);
    append_custom_items();
    rebuild_portal_items();
    let layout = overlay::compute_layout(&app.items, &app.groups, 1920, 1080, 96, 48.0);
    let mut lines = Vec::new();
    lines.push(format!("blur={}", app.blur.as_str()));
    lines.push(format!("items={}", app.items.len()));
    lines.push(format!("offline_items={} offline_entries={}",
        app.items.iter().filter(|item| item.offline).count(),
        app.groups.iter().filter(|group| group.offline).count()));
    lines.push(format!("layout_cols={} tiles={}", layout.cols, layout.tiles.len()));
    // 分组：数量、组内引用数、是否有面板上不存在的路径（失效引用会在这里暴露）。
    let grouped: usize = app.groups.iter().map(|group| group.paths.len()).sum();
    let stale = app
        .groups
        .iter()
        .flat_map(|group| group.paths.iter())
        .filter(|path| path.exists())
        .filter(|path| !app.items.iter().any(|item| icons::same_path(&item.path, path)))
        .count();
    lines.push(format!("groups={} grouped_refs={} stale_refs={stale}", app.groups.len(), grouped));
    lines.push(format!("drop_target={}", crate::drop_target::is_attached()));
    let mut failures = Vec::new();
    if app.items.is_empty() {
        failures.push("没有读到任何桌面图标");
    }
    // tiles 多一格是「添加图标」按钮。
    if layout.tiles.len() != app.items.len() + 1 {
        failures.push("布局格子数量与图标数量不一致");
    }
    if app.groups.is_empty() {
        failures.push("没有任何分组");
    }
    if !crate::drop_target::is_attached() {
        failures.push("拖放接收没有注册");
    }
    if stale > 0 {
        failures.push("分组里有面板上不存在的路径");
    }
    if app.blur == BlurMode::Dim {
        failures.push("虚化退化成了深色底");
    }
    lines.push(format!("failures={}", failures.len()));
    for failure in &failures {
        lines.push(format!("failure={failure}"));
    }
    let report = lines.join("\n");
    log::info(&format!("自检结果：{}", report.replace('\n', "; ")));
    let path = report_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, format!("{report}\n"));
    if failures.is_empty() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

fn report_path() -> PathBuf {
    log::base_dir().join("self-test.txt")
}
