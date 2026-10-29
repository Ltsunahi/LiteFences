//! 配置存储：一个 `config.json`，带版本号、原子写入，旧版 txt 自动迁移。
//!
//! 为什么这么存：
//! - JSON 有结构、能嵌套，加字段不用改文件格式（旧文件少字段用 `serde(default)` 补齐）；
//! - `version` 字段留给以后写迁移；
//! - 先写 `config.json.tmp` 再改名，断电/崩溃不会留下半截配置；
//! - 旧版的分号分隔 txt 只在首次启动时读一次，读完改名成 `*.migrated.txt`。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::log;

/// 当前配置格式版本。加字段不用改它；只有不兼容改动才加 1。
pub const CONFIG_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub settings: SettingsSection,
    /// 面板顺序 + 显示名 + 自定义图标（顺序即面板顺序）。
    #[serde(default)]
    pub items: Vec<ItemEntry>,
    /// 分组与组内路径（入口组不在这里，见 portals）。
    #[serde(default)]
    pub groups: Vec<GroupEntry>,
    #[serde(default)]
    pub portals: Vec<PortalEntry>,
    /// 手动添加的图标。
    #[serde(default)]
    pub custom: Vec<String>,
    /// 从面板移除、不再显示的桌面图标。
    #[serde(default)]
    pub hidden: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SettingsSection {
    pub hotkey_modifiers: u32,
    pub hotkey_key: u32,
    pub theme: String,
    pub lang: String,
}

impl Default for SettingsSection {
    fn default() -> Self {
        Self {
            hotkey_modifiers: 0x0003, // Ctrl + Alt
            hotkey_key: 0x20,         // Space
            theme: "space".to_string(),
            lang: "auto".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ItemEntry {
    pub path: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GroupEntry {
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PortalEntry {
    pub dir: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
    /// `folder` / `name` / `time`。
    #[serde(default)]
    pub sort: String,
}

fn base_dir() -> PathBuf {
    log::base_dir()
}

pub fn config_path() -> PathBuf {
    base_dir().join("config.json")
}

fn backup_path() -> PathBuf {
    base_dir().join("config.json.bak")
}

/// 读配置。文件不存在就走迁移；文件坏了就备份坏文件、退回默认值（不能让面板打不开）。
pub fn load() -> Config {
    let path = config_path();
    if !path.exists() {
        let mut config = migrate_legacy();
        config.version = CONFIG_VERSION;
        if has_any_data(&config) {
            log::info("已把旧的 txt 配置迁移到 config.json");
            save(&config);
            retire_legacy_files();
        }
        return config;
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            log::error(&format!("读取 config.json 失败：{error}"));
            return Config {
                version: CONFIG_VERSION,
                ..Default::default()
            };
        }
    };
    match serde_json::from_str::<Config>(&text) {
        Ok(mut config) => {
            if config.version == 0 {
                config.version = CONFIG_VERSION;
            }
            config
        }
        Err(error) => {
            let broken = base_dir().join("config.broken.json");
            let _ = std::fs::rename(&path, &broken);
            log::error(&format!(
                "config.json 解析失败（{error}），已备份为 {} 并重置",
                broken.display()
            ));
            Config {
                version: CONFIG_VERSION,
                ..Default::default()
            }
        }
    }
}

/// 原子写入：先写临时文件，再改名替换；旧文件留一份 .bak。
pub fn save(config: &Config) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut config = config.clone();
    config.version = CONFIG_VERSION;
    let text = match serde_json::to_string_pretty(&config) {
        Ok(text) => text,
        Err(error) => {
            log::error(&format!("序列化配置失败：{error}"));
            return;
        }
    };
    let temp = path.with_extension("json.tmp");
    if let Err(error) = std::fs::write(&temp, text) {
        log::error(&format!("写入配置失败：{error}"));
        return;
    }
    if path.exists() {
        let _ = std::fs::copy(&path, backup_path());
    }
    if let Err(error) = std::fs::rename(&temp, &path) {
        log::error(&format!("替换 config.json 失败：{error}"));
        let _ = std::fs::remove_file(&temp);
    }
}

fn has_any_data(config: &Config) -> bool {
    !config.items.is_empty()
        || !config.groups.is_empty()
        || !config.portals.is_empty()
        || !config.custom.is_empty()
        || !config.hidden.is_empty()
}

/// 只读一次旧版 txt，把能用的一次性搬进 Config。
fn migrate_legacy() -> Config {
    let dir = base_dir();
    let mut config = Config {
        version: CONFIG_VERSION,
        ..Default::default()
    };
    config.settings = migrate_settings(&dir);
    config.items = migrate_layout(&dir);
    config.groups = migrate_groups(&dir);
    config.portals = migrate_portals(&dir);
    config.custom = read_lines(&dir.join("custom-icons.txt"));
    config.hidden = read_lines(&dir.join("hidden-icons.txt"));
    config
}

fn read_lines(path: &std::path::Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

fn migrate_settings(dir: &std::path::Path) -> SettingsSection {
    let mut settings = SettingsSection::default();
    let Ok(text) = std::fs::read_to_string(dir.join("settings.txt")) else {
        return settings;
    };
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("theme=") {
            if !value.trim().is_empty() {
                settings.theme = value.trim().to_string();
            }
        } else if let Some(value) = line.strip_prefix("lang=") {
            if !value.trim().is_empty() {
                settings.lang = value.trim().to_string();
            }
        } else if let Some(value) = line.strip_prefix("hotkey=") {
            if let Some((modifiers, key)) = value.split_once(',') {
                if let (Ok(modifiers), Ok(key)) = (modifiers.trim().parse(), key.trim().parse()) {
                    settings.hotkey_modifiers = modifiers;
                    settings.hotkey_key = key;
                }
            }
        }
    }
    settings
}

fn migrate_layout(dir: &std::path::Path) -> Vec<ItemEntry> {
    let Ok(text) = std::fs::read_to_string(dir.join("layout.txt")) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut parts = line.split('\t');
            let path = parts.next().unwrap_or_default().to_string();
            let name = parts.next().unwrap_or_default().to_string();
            let icon = parts
                .next()
                .map(str::trim)
                .filter(|icon| !icon.is_empty())
                .map(str::to_string);
            ItemEntry { path, name, icon }
        })
        .collect()
}

fn migrate_groups(dir: &std::path::Path) -> Vec<GroupEntry> {
    let Ok(text) = std::fs::read_to_string(dir.join("groups.txt")) else {
        return Vec::new();
    };
    let mut groups: Vec<GroupEntry> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(inner) = trimmed.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            let (collapsed, name) = match inner.strip_prefix('!') {
                Some(name) => (true, name.trim().to_string()),
                None => (false, inner.trim().to_string()),
            };
            if !name.is_empty() {
                groups.push(GroupEntry {
                    name,
                    collapsed,
                    paths: Vec::new(),
                });
            }
            continue;
        }
        if let Some(group) = groups.last_mut() {
            group.paths.push(trimmed.to_string());
        }
    }
    groups
}

fn migrate_portals(dir: &std::path::Path) -> Vec<PortalEntry> {
    let Ok(text) = std::fs::read_to_string(dir.join("portals.txt")) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut parts = line.splitn(4, '\t');
            let dir = parts.next().unwrap_or_default().to_string();
            let name = parts.next().map(str::trim).unwrap_or_default().to_string();
            let collapsed = parts.next().is_some_and(|value| value.trim() == "1");
            let sort = parts
                .next()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("folder")
                .to_string();
            PortalEntry {
                dir,
                name,
                collapsed,
                sort,
            }
        })
        .collect()
}

/// 迁移完成后把旧文件改名，避免下次再被当成“新数据”读进来。
pub fn retire_legacy_files() {
    let dir = base_dir();
    for name in [
        "layout.txt",
        "groups.txt",
        "portals.txt",
        "custom-icons.txt",
        "hidden-icons.txt",
        "settings.txt",
    ] {
        let from = dir.join(name);
        if from.exists() {
            let to = dir.join(format!("{name}.migrated"));
            if std::fs::rename(&from, &to).is_err() {
                let _ = std::fs::copy(&from, &to);
                let _ = std::fs::remove_file(&from);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_and_round_trips() {
        let mut config = Config {
            version: CONFIG_VERSION,
            ..Default::default()
        };
        config.groups.push(GroupEntry {
            name: "开发".into(),
            collapsed: true,
            paths: vec!["C:\\dev\\a.txt".into()],
        });
        config.items.push(ItemEntry {
            path: "C:\\dev\\a.txt".into(),
            name: "a".into(),
            icon: None,
        });
        let text = serde_json::to_string_pretty(&config).unwrap();
        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(back.groups.len(), 1);
        assert!(back.groups[0].collapsed);
        assert_eq!(back.items[0].name, "a");
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // 旧版本没有的新字段必须能补默认值，否则升级会丢配置。
        let text = r#"{"version":1,"items":[{"path":"C:\\x"},{"path":"C:\\y","name":"Y"}]}"#;
        let config: Config = serde_json::from_str(text).unwrap();
        assert_eq!(config.items.len(), 2);
        assert_eq!(config.items[1].name, "Y");
        assert_eq!(config.settings.theme, "space");
        assert!(config.groups.is_empty());
    }
}
