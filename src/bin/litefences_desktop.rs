//! LiteFences 桌面启动器（新形态）。
//!
//! 后台常驻、无主窗口；在桌面空白处双击唤出全屏亚克力浮层与图标面板。
//! 该二进制与旧的 `litefences.exe` 面板原型相互独立，便于并行验证。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(windows)]
#[path = "litefences_desktop/app.rs"]
mod app;
#[cfg(windows)]
#[path = "litefences_desktop/autostart.rs"]
mod autostart;
#[cfg(windows)]
#[path = "litefences_desktop/backdrop.rs"]
mod backdrop;
#[cfg(windows)]
#[path = "litefences_desktop/capture.rs"]
mod capture;
#[cfg(windows)]
#[path = "litefences_desktop/desktop_icons.rs"]
mod desktop_icons;
#[cfg(windows)]
#[path = "litefences_desktop/drop_target.rs"]
mod drop_target;
#[cfg(windows)]
#[path = "litefences_desktop/groups.rs"]
mod groups;
#[cfg(windows)]
#[path = "litefences_desktop/hooks.rs"]
mod hooks;
#[cfg(windows)]
#[path = "litefences_desktop/hotkey_dialog.rs"]
mod hotkey_dialog;
#[cfg(windows)]
#[path = "litefences_desktop/icons.rs"]
mod icons;
#[cfg(windows)]
#[path = "litefences_desktop/log.rs"]
mod log;
#[cfg(windows)]
#[path = "litefences_desktop/overlay.rs"]
mod overlay;
#[cfg(windows)]
#[path = "litefences_desktop/picker.rs"]
mod picker;
#[cfg(windows)]
#[path = "litefences_desktop/portals.rs"]
mod portals;
#[cfg(windows)]
#[path = "litefences_desktop/settings.rs"]
mod settings;
#[cfg(windows)]
#[path = "litefences_desktop/state.rs"]
mod state;
#[cfg(windows)]
#[path = "litefences_desktop/text_input.rs"]
mod text_input;
#[cfg(windows)]
#[path = "litefences_desktop/tray.rs"]
mod tray;
#[cfg(windows)]
#[path = "litefences_desktop/util.rs"]
mod util;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    app::run()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("LiteFences desktop launcher requires Windows 11 x64.");
    std::process::exit(1);
}
