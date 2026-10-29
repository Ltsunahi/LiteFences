//! 托盘图标：显示/隐藏面板、退出。没有主窗口，托盘是唯一的常驻入口。

use windows_sys::Win32::Foundation::{GetLastError, HWND};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION,
    NOTIFYICONDATAW, NOTIFYICON_VERSION_4, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, IDI_APPLICATION, LoadIconW, MF_CHECKED,
    MF_SEPARATOR, MF_STRING, PostMessageW, RegisterWindowMessageW, SetForegroundWindow,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
};

use crate::log;
use crate::util::wide;

pub const TRAY_CALLBACK: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 1;
pub const MENU_SHOW: usize = 1001;
pub const MENU_HIDE: usize = 1002;
pub const MENU_EXIT: usize = 1003;
pub const MENU_CAPTURE: usize = 1004;
pub const MENU_AUTOSTART: usize = 1005;
pub const MENU_HOTKEY: usize = 1006;
pub const MENU_ADD_PORTAL: usize = 1007;
/// 主题菜单项的起始编号：`MENU_THEME_BASE + 主题序号`。
pub const MENU_THEME_BASE: usize = 1100;
/// 语言菜单项：跟随系统 / 中文 / English。
pub const MENU_LANG_BASE: usize = 1300;

/// Explorer 重启后会广播 `TaskbarCreated`，收到后必须重新添加托盘图标。
pub fn taskbar_created_message() -> u32 {
    static MESSAGE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MESSAGE.get_or_init(|| {
        let name = wide("TaskbarCreated");
        // SAFETY: 名称以 0 结尾，注册的是全局消息号。
        unsafe { RegisterWindowMessageW(name.as_ptr()) }
    })
}

pub fn install(host: HWND) -> bool {
    let icon = unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) };
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: host,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_INFO,
        uCallbackMessage: TRAY_CALLBACK,
        hIcon: icon,
        ..Default::default()
    };
    let tip = crate::i18n::t(
        "LiteFences：双击桌面唤出图标面板",
        "LiteFences: double-click the desktop to open the panel",
    );
    for (slot, ch) in data.szTip.iter_mut().zip(tip.encode_utf16()) {
        *slot = ch;
    }
    // 首次运行提示一次：Win11 会把新图标收进 ^ 折叠区，提示用户去那里找。
    let title = "LiteFences 已在后台运行";
    for (slot, ch) in data.szInfoTitle.iter_mut().zip(title.encode_utf16()) {
        *slot = ch;
    }
    let info = "图标在通知区（若没看到，点击 ^ 展开，可拖到任务栏常驻）。\n双击桌面空白处唤出面板，Ctrl+Alt+Q 退出。";
    for (slot, ch) in data.szInfo.iter_mut().zip(info.encode_utf16()) {
        *slot = ch;
    }
    // SAFETY: 结构体与窗口句柄都有效；NIM_ADD 不会接管所有权。
    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &data) } != 0;
    if ok {
        log::info("托盘图标已创建");
        // SAFETY: 只调整通知参数的版本，结构体仍有效。
        unsafe {
            let mut version = data;
            version.uFlags = NIM_SETVERSION;
            version.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            Shell_NotifyIconW(NIM_SETVERSION, &version);
        }
    } else {
        // SAFETY: 无参数读取错误码。
        let error = unsafe { GetLastError() };
        log::error(&format!("托盘图标创建失败 err={error}（受限权限运行时可能失败）"));
    }
    ok
}

pub fn remove(host: HWND) {
    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: host,
        uID: 1,
        ..Default::default()
    };
    // SAFETY: 结构体有效，仅按句柄删除本程序添加的图标。
    unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
}

/// 弹出右键菜单并返回被选中的命令；0 表示取消。
pub fn show_menu(
    host: HWND,
    autostart_on: bool,
    hotkey_text: &str,
    theme_key: &str,
    lang_setting: &str,
) -> usize {
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return 0;
    }
    let show = wide(crate::i18n::t("打开图标面板", "Open panel"));
    let hide = wide(crate::i18n::t("隐藏图标面板", "Hide panel"));
    let autostart = wide(if autostart_on {
        crate::i18n::t("开机自启（已开启）", "Start with Windows (on)")
    } else {
        crate::i18n::t("开机自启（已关闭）", "Start with Windows (off)")
    });
    let hotkey = wide(&if crate::i18n::is_en() {
        format!("Set hotkey… (now {hotkey_text})")
    } else {
        format!("设置唤出热键…（当前 {hotkey_text}）")
    });
    let capture = wide(crate::i18n::t("抓取面板画面（排障）", "Capture panel (diagnostics)"));
    let exit = wide(crate::i18n::t("退出 LiteFences", "Quit LiteFences"));
    // SAFETY: 菜单句柄有效，字符串在调用期间保持存活。
    unsafe {
        AppendMenuW(menu, MF_STRING, MENU_SHOW, show.as_ptr());
        AppendMenuW(menu, MF_STRING, MENU_HIDE, hide.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        let autostart_flags = if autostart_on {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        AppendMenuW(menu, autostart_flags, MENU_AUTOSTART, autostart.as_ptr());
        AppendMenuW(menu, MF_STRING, MENU_HOTKEY, hotkey.as_ptr());
        let add_portal = wide(crate::i18n::t(
            "添加文件夹入口…（只读展示目录内容）",
            "Add folder entry… (read-only view)",
        ));
        AppendMenuW(menu, MF_STRING, MENU_ADD_PORTAL, add_portal.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        // 主题：每个预设一项，当前项打勾。
        let theme_labels: Vec<_> = crate::theme::THEMES
            .iter()
            .map(|theme| wide(&format!("{} {}", crate::i18n::t("主题：", "Theme:"), theme.display_name())))
            .collect();
        let current = crate::theme::index_of(theme_key);
        for (index, label) in theme_labels.iter().enumerate() {
            let flags = if index == current {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            AppendMenuW(menu, flags, MENU_THEME_BASE + index, label.as_ptr());
        }
        // 语言：跟随系统 / 中文 / English，当前项打勾。
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        let lang_labels = [
            wide(crate::i18n::t("语言：跟随系统", "Language: Follow system")),
            wide("语言：中文"),
            wide("Language: English"),
        ];
        let current_lang = match lang_setting.trim().to_ascii_lowercase().as_str() {
            "zh" | "zh-cn" => 1usize,
            "en" | "en-us" => 2usize,
            _ => 0usize,
        };
        for (index, label) in lang_labels.iter().enumerate() {
            let flags = if index == current_lang {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            AppendMenuW(menu, flags, MENU_LANG_BASE + index, label.as_ptr());
        }
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(menu, MF_STRING, MENU_CAPTURE, capture.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(menu, MF_STRING, MENU_EXIT, exit.as_ptr());
    }
    let mut point = windows_sys::Win32::Foundation::POINT::default();
    // SAFETY: 输出结构体有效。
    unsafe { GetCursorPos(&mut point) };
    // 托盘菜单需要先把自己设为前台，否则点击别处不会关闭菜单。
    unsafe { SetForegroundWindow(host) };
    // SAFETY: 菜单与窗口句柄有效；RETURNCMD 让返回值直接是命令号。
    let command = unsafe {
        TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            host,
            std::ptr::null(),
        )
    };
    // SAFETY: 释放本函数创建的菜单对象。
    unsafe {
        DestroyMenu(menu);
        PostMessageW(host, WM_NULL, 0, 0);
    }
    // 只清理本函数创建的菜单对象：LoadIcon 返回的是共享句柄，不在这里释放。
    log::info(&format!("托盘菜单命令 {command}"));
    command as usize
}
