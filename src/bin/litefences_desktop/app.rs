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
    MOD_ALT, MOD_CONTROL, RegisterHotKey, UnregisterHotKey, VK_F12, VK_Q,
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
/// 后台图标扫描完成的自定义消息。
const WM_APP_SCAN: u32 = WM_APP + 2;
/// 托盘不可用时的兜底入口，也方便键盘用户。
const HOTKEY_TOGGLE: i32 = 1;
const HOTKEY_EXIT: i32 = 2;
const HOTKEY_CAPTURE: i32 = 3;

pub fn run() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    log::init();
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
        drag_hidden: None,
        pending_remove: None,
        drop_hover: None,
        drop_notice: None,
        rehide_left: 0,
        remove_mode: false,
        hidden: icons::load_hidden(),
        hotkey: crate::settings::load_hotkey(),
        search: String::new(),
        search_open: false,
        search_anim: None,
        search_typing: false,
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
        scan_rx: None,
        scan_pending: false,
        layout: Layout::default(),
        hover: None,
        tray_installed: false,
        demo_cycles_left: options_cycles,
        low_integrity: integrity_rid() < 0x2000,
        mouse_hook: 0,
        keyboard_hook: 0,
    });
    // 保证至少有一个“其它”组，并把分组文件落盘一次。
    crate::groups::prune(&mut state().groups);
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
    app.desktop_count = app.items.len();
    app.desktop_stamp = Some(icons::desktop_stamp());
    append_custom_items();
    // 套用用户保存的顺序与名字。
    icons::apply_order(&mut app.items, &app.layout_order);
    // 再套用用户指定的图标文件（layout.txt 第三列）。
    icons::apply_custom_icons(&mut app.items);
    // 入口项在 replace 时被一并释放了，这里按目录重新建。
    rebuild_portal_items();
    overlay::relayout();
    // 如果面板正开着，用新窗口重画一帧，避免旧图标留在画面上。
    overlay::reshow();
}

/// 重建「文件夹入口」的内容：清掉旧的入口项和入口分组，按 portals 重新扫一遍。
///
/// 入口内容 + 目录状态的版本号：目录改动或配置变化时才重扫（图标加载很慢）。
fn portal_stamp(app: &App) -> u64 {
    fn mix(hash: &mut u64, value: u64) {
        *hash ^= value;
        *hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for portal in &app.portals {
        for byte in portal.dir.to_string_lossy().bytes() {
            mix(&mut hash, u64::from(byte));
        }
        for byte in portal.name.bytes() {
            mix(&mut hash, u64::from(byte));
        }
        mix(&mut hash, u64::from(portal.collapsed));
        for byte in portal.sort.as_str().bytes() {
            mix(&mut hash, u64::from(byte));
        }
        let modified = std::fs::metadata(&portal.dir)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        mix(&mut hash, modified);
    }
    hash ^ (app.portals.len() as u64)
}

/// 目录和配置都没变就跳过重建，避免每次唤出都重新取图标。
pub fn refresh_portals_if_stale() {
    let app = state();
    let stamp = portal_stamp(app);
    let intact = app.portals.is_empty()
        || app
            .items
            .iter()
            .any(|item| item.source == icons::ItemSource::Portal);
    if app.portal_stamp == Some(stamp) && intact {
        return;
    }
    rebuild_portal_items();
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
        let items = crate::portals::scan(portal);
        let mut group = crate::groups::Group::new(format!("入口：{}", portal.name));
        group.collapsed = portal.collapsed;
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
        return "入口是只读视图，请拖到其它分组".to_string();
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
        .map(|group| group.name.clone())
        .unwrap_or_default();
    if items_changed || groups_changed {
        persist_layout();
        overlay::relayout();
    }
    let mut parts = Vec::new();
    if added > 0 {
        parts.push(format!("加入「{name}」{added} 项"));
    }
    if moved > 0 {
        parts.push(format!("移入「{name}」{moved} 项"));
    }
    if skipped > 0 {
        parts.push(format!("跳过 {skipped} 项（已在面板或不可用）"));
    }
    if parts.is_empty() {
        format!("「{name}」里没有可加入的新项目")
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
    let path = item.path.display().to_string();
    item.name = name.to_string();
    log::info(&format!("重命名：{path} → {name}"));
    persist_layout();
    overlay::relayout();
}

/// 移除面板上的某一项：手动图标删记录，桌面图标进隐藏列表（都不动源文件）。
pub fn remove_item(index: usize) {
    let app = state();
    let Some(item) = app.items.get(index) else {
        return;
    };
    if item.source == icons::ItemSource::Portal {
        log::info("入口是只读视图，不能用「移除」删掉文件；要移除请右键入口标题");
        return;
    }
    let path = item.path.clone();
    let source = item.source;
    match source {
        icons::ItemSource::Custom => {
            app.custom.retain(|known| !icons::same_path(known, &path));
            icons::save_custom(&app.custom);
        }
        icons::ItemSource::Desktop => {
            if !app.hidden.iter().any(|known| icons::same_path(known, &path)) {
                app.hidden.push(path.clone());
                icons::save_hidden(&app.hidden);
            }
        }
        icons::ItemSource::Portal => return,
    }
    let removed = app.items.remove(index);
    // 分组里对应的记录也一并清掉。
    crate::groups::forget(&mut app.groups, &path);
    crate::groups::save(&app.groups);
    icons::destroy_item(removed);
    log::info(&format!("从面板移除：{}", path.display()));
    persist_layout();
    overlay::relayout();
}

/// 把当前顺序与名字写回 layout.txt。
///
/// 写入前先把 `items` 整理成“按分组顺序、组内保持当前次序”，这样文件里的顺序
/// 和面板上看到的一致，跨组拖动后也不会越来越乱。
pub fn persist_layout() {
    let app = state();
    let mut order: Vec<usize> = Vec::with_capacity(app.items.len());
    let mut used = std::collections::HashSet::new();
    for group in &app.groups {
        // 入口是只读视图，不进 layout.txt。
        if group.portal.is_some() {
            continue;
        }
        for (index, item) in app.items.iter().enumerate() {
            if used.contains(&index) {
                continue;
            }
            if group
                .paths
                .iter()
                .any(|known| icons::same_path(known, &item.path))
            {
                order.push(index);
                used.insert(index);
            }
        }
    }
    // 没归组的排在最后。
    for index in 0..app.items.len() {
        if app.items[index].source == icons::ItemSource::Portal {
            continue;
        }
        if !used.contains(&index) {
            order.push(index);
        }
    }
    // 入口项保留在面板末尾，但不进 layout.txt（它们由 portals.txt + 目录内容决定）。
    for index in 0..app.items.len() {
        if app.items[index].source == icons::ItemSource::Portal {
            order.push(index);
            used.insert(index);
        }
    }
    if order.len() == app.items.len() {
        let reordered: Vec<_> = order.iter().map(|index| app.items[*index].clone()).collect();
        app.items = reordered;
    }
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

/// 拖动排序：把第 `from` 项移动到 `to` 位置。
pub fn move_item(from: usize, to: usize) {
    let app = state();
    if from >= app.items.len() || to >= app.items.len() || from == to {
        return;
    }
    if app.items[from].source == icons::ItemSource::Portal || app.items[to].source == icons::ItemSource::Portal {
        log::info("入口是只读视图，不能拖动排序");
        return;
    }
    let item = app.items.remove(from);
    app.items.insert(to, item);
    log::info(&format!("调整顺序：{from} → {to}"));
    persist_layout();
    overlay::relayout();
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
    let caption = wide("LiteFences 桌面启动器");
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
        if RegisterHotKey(host, HOTKEY_CAPTURE, MOD_CONTROL | MOD_ALT, u32::from(VK_F12)) == 0 {
            log::warn("注册 Ctrl+Alt+F12 失败（可能被其他程序占用）");
        }
    }
    log::info(&format!(
        "快捷键：{} 唤出/收起，Ctrl+Alt+F12 抓取面板，Ctrl+Alt+Q 退出，Esc 收起",
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
    state().hotkey = chosen;
    crate::settings::save_hotkey(chosen);
    log::info(&format!("唤出热键已改为 {}", chosen.describe()));
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
                HOTKEY_CAPTURE => start_capture(hwnd),
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
        WM_APP_SCAN => {
            finish_scan();
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
    let command = tray::show_menu(host, autostart_on, &hotkey_text);
    handle_command(command);
}

fn handle_command(command: usize) {
    match command {
        tray::MENU_SHOW => overlay::show_panel(),
        tray::MENU_HIDE => overlay::hide_panel(),
        tray::MENU_CAPTURE => start_capture(state().host),
        tray::MENU_AUTOSTART => toggle_autostart(),
        tray::MENU_HOTKEY => change_hotkey(),
        tray::MENU_ADD_PORTAL => add_portal_from_picker(),
        tray::MENU_EXIT => request_exit(state().host),
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
            let text = wide(&format!("设置开机自启失败：{error}"));
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

/// 从当前正在运行的实例里抓一张面板画面（排障用）：先抓一张立刻的，再抓一张稳定后的。
fn start_capture(host: HWND) {
    let path = log::base_dir().join("panel-capture.bmp");
    {
        let app = state();
        app.options.capture = Some(path.display().to_string());
        if !app.shown {
            overlay::show_panel();
        }
    }
    // SAFETY: 句柄有效；两个时间点各抓一张，能看出文字是否“先有后无”。
    unsafe {
        SetTimer(host, TIMER_CAPTURE_EARLY, 250, None);
        SetTimer(host, TIMER_CAPTURE, 1400, None);
    }
    log::info(&format!("准备抓取当前面板：{}", path.display()));
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
    let layout = overlay::compute_layout(&app.items, &app.groups, 1920, 1080, 96, 48.0);
    let mut lines = Vec::new();
    lines.push(format!("blur={}", app.blur.as_str()));
    lines.push(format!("items={}", app.items.len()));
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
