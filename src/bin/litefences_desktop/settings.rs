//! 设置持久化：唤出热键 + 配色主题。

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

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum IconDensity {
    Compact,
    #[default]
    Normal,
    Large,
}

impl IconDensity {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "compact" => Self::Compact,
            "large" => Self::Large,
            _ => Self::Normal,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Normal => "normal",
            Self::Large => "large",
        }
    }

    pub fn scale_factor(self) -> f32 {
        match self {
            Self::Compact => 0.85,
            Self::Normal => 1.0,
            Self::Large => 1.22,
        }
    }

    pub fn all() -> [Self; 3] {
        [Self::Compact, Self::Normal, Self::Large]
    }

    pub fn index(self) -> usize {
        match self {
            Self::Compact => 0,
            Self::Normal => 1,
            Self::Large => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => crate::i18n::t("紧凑", "Compact"),
            Self::Normal => crate::i18n::t("标准", "Standard"),
            Self::Large => crate::i18n::t("偏大", "Large"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MonitorPolicy {
    #[default]
    Cursor,
    Primary,
}

impl MonitorPolicy {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "primary" => Self::Primary,
            _ => Self::Cursor,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Primary => "primary",
        }
    }

    pub fn all() -> [Self; 2] {
        [Self::Cursor, Self::Primary]
    }

    pub fn index(self) -> usize {
        match self {
            Self::Cursor => 0,
            Self::Primary => 1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cursor => crate::i18n::t("跟随鼠标所在屏幕", "Follow cursor monitor"),
            Self::Primary => crate::i18n::t("固定在主屏", "Primary monitor only"),
        }
    }
}

/// 全部设置。字段少，直接整份读整份写，避免改一项冲掉另一项。
#[derive(Clone, Debug)]
pub struct Settings {
    pub hotkey: Hotkey,
    pub theme: String,
    /// `auto`（跟随系统）/ `zh` / `en`。
    pub lang: String,
    /// 是否自动检查更新。
    pub check_updates: bool,
    /// 图标尺寸档位。
    pub density: IconDensity,
    /// 是否显示「最近常用」置顶栏。
    pub show_recent: bool,
    /// 前台全屏应用运行时跳过热键/双击唤出。
    pub ignore_fullscreen: bool,
    /// 多屏唤出位置策略。
    pub monitor: MonitorPolicy,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: Hotkey::default(),
            theme: "space".to_string(),
            lang: "auto".to_string(),
            check_updates: true,
            density: IconDensity::Normal,
            show_recent: true,
            ignore_fullscreen: true,
            monitor: MonitorPolicy::Cursor,
        }
    }
}

pub fn load() -> Settings {
    let section = crate::store::load().settings;
    let mut settings = Settings::default();
    let hotkey = Hotkey {
        modifiers: section.hotkey_modifiers,
        key: section.hotkey_key,
    };
    if hotkey.modifiers != 0 && hotkey.key != 0 {
        settings.hotkey = hotkey;
    }
    if !section.theme.trim().is_empty() {
        settings.theme = section.theme;
    }
    if !section.lang.trim().is_empty() {
        settings.lang = section.lang;
    }
    settings.check_updates = section.check_updates;
    settings.density = IconDensity::parse(&section.density);
    settings.show_recent = section.show_recent;
    settings.ignore_fullscreen = section.ignore_fullscreen;
    settings.monitor = MonitorPolicy::parse(&section.monitor);
    settings
}

pub fn save(settings: &Settings) {
    let mut config = crate::store::load();
    config.settings = crate::store::SettingsSection {
        hotkey_modifiers: settings.hotkey.modifiers,
        hotkey_key: settings.hotkey.key,
        theme: settings.theme.clone(),
        lang: settings.lang.clone(),
        check_updates: settings.check_updates,
        density: settings.density.as_str().to_string(),
        show_recent: settings.show_recent,
        ignore_fullscreen: settings.ignore_fullscreen,
        monitor: settings.monitor.as_str().to_string(),
    };
    crate::store::save(&config);
    log::info(&format!(
        "已保存设置：热键 {}，主题 {}，语言 {}，图标 {}",
        settings.hotkey.describe(),
        settings.theme,
        settings.lang,
        settings.density.as_str()
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_density_parses_and_orders_scales() {
        assert_eq!(IconDensity::parse("compact"), IconDensity::Compact);
        assert_eq!(IconDensity::parse("LARGE"), IconDensity::Large);
        assert_eq!(IconDensity::parse("unknown"), IconDensity::Normal);
        assert!(IconDensity::Compact.scale_factor() < IconDensity::Normal.scale_factor());
        assert!(IconDensity::Normal.scale_factor() < IconDensity::Large.scale_factor());
        for d in IconDensity::all() {
            assert_eq!(IconDensity::parse(d.as_str()), d);
        }
    }

    #[test]
    fn monitor_policy_parses_and_round_trips() {
        assert_eq!(MonitorPolicy::parse("primary"), MonitorPolicy::Primary);
        assert_eq!(MonitorPolicy::parse("CURSOR"), MonitorPolicy::Cursor);
        assert_eq!(MonitorPolicy::parse("other"), MonitorPolicy::Cursor);
        for p in MonitorPolicy::all() {
            assert_eq!(MonitorPolicy::parse(p.as_str()), p);
        }
    }
}
