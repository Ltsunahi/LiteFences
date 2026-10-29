//! 设置持久化：唤出热键 + 配色主题。

use std::path::PathBuf;

use crate::log;

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hotkey {
    pub modifiers: u32,
    pub key: u32,
}

impl Default for Hotkey {
    fn default() -> Self {
        // Ctrl + Alt + Space
        Self {
            modifiers: MOD_CONTROL | MOD_ALT,
            key: 0x20,
        }
    }
}

impl Hotkey {
    pub fn describe(self) -> String {
        let mut text = String::new();
        if self.modifiers & MOD_CONTROL != 0 {
            text.push_str("Ctrl+");
        }
        if self.modifiers & MOD_ALT != 0 {
            text.push_str("Alt+");
        }
        if self.modifiers & MOD_SHIFT != 0 {
            text.push_str("Shift+");
        }
        if self.modifiers & MOD_WIN != 0 {
            text.push_str("Win+");
        }
        text.push_str(&key_name(self.key));
        text
    }
}

/// 常见按键的可读名字；其它键显示 VK 码。
fn key_name(key: u32) -> String {
    match key {
        0x08 => "Backspace".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x1B => "Esc".into(),
        0x20 => "Space".into(),
        0x25 => "←".into(),
        0x26 => "↑".into(),
        0x27 => "→".into(),
        0x28 => "↓".into(),
        0x2C => "PrintScreen".into(),
        0x70..=0x87 => format!("F{}", key - 0x6F),
        value if (0x30..=0x39).contains(&value) => char::from_u32(value).unwrap().to_string(),
        value if (0x41..=0x5A).contains(&value) => char::from_u32(value).unwrap().to_string(),
        value => format!("VK({value})"),
    }
}

fn settings_path() -> PathBuf {
    log::base_dir().join("settings.txt")
}

/// 全部设置。字段少，直接整份读整份写，避免改一项冲掉另一项。
#[derive(Clone, Debug)]
pub struct Settings {
    pub hotkey: Hotkey,
    pub theme: String,
    /// `auto`（跟随系统）/ `zh` / `en`。
    pub lang: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: Hotkey::default(),
            theme: "space".to_string(),
            lang: "auto".to_string(),
        }
    }
}

pub fn load() -> Settings {
    let mut settings = Settings::default();
    let Ok(text) = std::fs::read_to_string(settings_path()) else {
        return settings;
    };
    for line in text.lines() {
        if let Some(value) = line.trim().strip_prefix("lang=") {
            let value = value.trim();
            if !value.is_empty() {
                settings.lang = value.to_string();
            }
            continue;
        }
        if let Some(value) = line.trim().strip_prefix("theme=") {
            let value = value.trim();
            if !value.is_empty() {
                settings.theme = value.to_string();
            }
            continue;
        }
        let Some(value) = line.trim().strip_prefix("hotkey=") else {
            continue;
        };
        let Some((modifiers, key)) = value.split_once(',') else {
            continue;
        };
        if let (Ok(modifiers), Ok(key)) = (modifiers.trim().parse(), key.trim().parse()) {
            let hotkey = Hotkey { modifiers, key };
            if hotkey.modifiers != 0 && hotkey.key != 0 {
                settings.hotkey = hotkey;
            }
        }
    }
    settings
}

pub fn save(settings: &Settings) {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = format!(
        "# LiteFences 设置\nhotkey={},{}\ntheme={}\nlang={}\n",
        settings.hotkey.modifiers, settings.hotkey.key, settings.theme, settings.lang
    );
    if let Err(error) = std::fs::write(&path, text) {
        log::error(&format!("保存设置失败：{error}"));
    } else {
        log::info(&format!(
            "已保存设置：热键 {}，主题 {}",
            settings.hotkey.describe(),
            settings.theme
        ));
    }
}
