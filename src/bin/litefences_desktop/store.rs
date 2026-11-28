//! 配置存储：一个 `config.json`，带版本号、原子写入，旧版 txt 自动迁移。
//!
//! 为什么这么存：
//! - JSON 有结构、能嵌套，加字段不用改文件格式（旧文件少字段用 `serde(default)` 补齐）；
//! - `version` 字段留给以后写迁移；
//! - 先写 `config.json.tmp` 再改名，断电/崩溃不会留下半截配置；
//! - 旧版的分号分隔 txt 只在首次启动时读一次，读完改名成 `*.migrated.txt`。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

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
    /// 首次使用引导是否已经看过。
    #[serde(default)]
    pub onboarded: bool,
    /// 各项目启动频次与最近启动时间（用于搜索同分排序加权）。
    #[serde(default)]
    pub usage: std::collections::BTreeMap<String, UsageEntry>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UsageEntry {
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub last_used: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct SettingsSection {
    pub hotkey_modifiers: u32,
    pub hotkey_key: u32,
    pub theme: String,
    pub lang: String,
    /// 是否自动检查更新；旧配置没有这个字段时补 true。
    #[serde(default = "default_true")]
    pub check_updates: bool,
    /// 图标尺寸档位：`compact` / `normal` / `large`。
    #[serde(default = "default_density")]
    pub density: String,
    /// 是否在面板顶部显示「最近常用」栏。
    #[serde(default = "default_true")]
    pub show_recent: bool,
    /// 前台全屏应用运行时是否跳过唤出。
    #[serde(default = "default_true")]
    pub ignore_fullscreen: bool,
    /// 多屏唤出位置策略：`cursor` / `primary`。
    #[serde(default = "default_monitor")]
    pub monitor: String,
}

fn default_true() -> bool {
    true
}

fn default_density() -> String {
    "normal".to_string()
}

fn default_monitor() -> String {
    "cursor".to_string()
}

impl Default for SettingsSection {
    fn default() -> Self {
        Self {
            hotkey_modifiers: 0x0003, // Ctrl + Alt
            hotkey_key: 0x20,         // Space
            theme: "space".to_string(),
            lang: "auto".to_string(),
            check_updates: true,
            density: default_density(),
            show_recent: true,
            ignore_fullscreen: true,
            monitor: default_monitor(),
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
    /// 自动归组规则：扩展名（如 `.pdf`）或名称关键字（如 `会议`）。
    #[serde(default)]
    pub rules: Vec<String>,
    /// 分组强调色标 key（如 `blue` / `green`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
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

static RECOVERY_NOTICE: AtomicBool = AtomicBool::new(false);

pub fn take_recovery_notice() -> bool {
    RECOVERY_NOTICE.swap(false, Ordering::Relaxed)
}

fn recover_backup(path: &std::path::Path) -> Option<Config> {
    let bytes = std::fs::read(path.with_extension("json.bak")).ok()?;
    let mut config = serde_json::from_slice::<Config>(&bytes).ok()?;
    if config.version > CONFIG_VERSION { return None; }
    if path.exists() {
        let broken = path.with_extension("broken.json");
        if let Err(error) = std::fs::copy(path, broken) {
            log::warn(&format!("保留损坏配置失败：{error}"));
        }
    }
    if let Err(error) = replace_file(path, &bytes) {
        log::warn(&format!("已载入备份，但写回配置失败：{error}"));
    }
    config.version = CONFIG_VERSION;
    RECOVERY_NOTICE.store(true, Ordering::Relaxed);
    log::info("已从备份恢复配置");
    Some(config)
}

fn replace_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    std::fs::write(&temp, bytes)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(temp);
        return Err(error);
    }
    Ok(())
}

/// 读配置。文件不存在就走迁移；文件坏了就备份坏文件、退回默认值（不能让面板打不开）。
pub fn load() -> Config {
    let path = config_path();
    if !path.exists() {
        if let Some(config) = recover_backup(&path) { return config; }
        let mut config = migrate_legacy();
        config.version = CONFIG_VERSION;
        if has_any_data(&config) {
            log::info("已把旧的 txt 配置迁移到 config.json");
            save(&config);
            retire_legacy_files();
        }
        return config;
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            log::error(&format!("读取 config.json 失败：{error}"));
            if let Some(config) = recover_backup(&path) { return config; }
            return Config {
                version: CONFIG_VERSION,
                ..Default::default()
            };
        }
    };
    match serde_json::from_slice::<Config>(&bytes) {
        Ok(mut config) => {
            if config.version == 0 {
                config.version = CONFIG_VERSION;
            }
            config
        }
        Err(error) => {
            if let Some(config) = recover_backup(&path) { return config; }
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
    if let Ok(previous) = std::fs::read(&path) {
        if previous == text.as_bytes() { return; }
        if serde_json::from_slice::<Config>(&previous).is_ok()
            && let Err(error) = replace_file(&path.with_extension("json.bak"), &previous)
        {
            log::warn(&format!("轮换配置备份失败：{error}"));
        }
    }
    if let Err(error) = replace_file(&path, text.as_bytes()) {
        log::error(&format!("替换 config.json 失败：{error}"));
    }
}

pub fn usage_key(path: &std::path::Path) -> String {
    path.to_string_lossy().to_lowercase()
}

pub fn save_usage(usage: &std::collections::BTreeMap<String, UsageEntry>) {
    let mut config = load();
    config.usage = usage.clone();
    save(&config);
}

pub fn parse_import(bytes: &[u8]) -> Result<Config, String> {
    let mut config = serde_json::from_slice::<Config>(bytes)
        .map_err(|error| format!("JSON 格式无效：{error}"))?;
    if config.version > CONFIG_VERSION {
        return Err(format!("配置版本 {} 高于当前支持版本 {CONFIG_VERSION}", config.version));
    }
    config.version = CONFIG_VERSION;
    Ok(config)
}

pub fn export_to(dest: &std::path::Path) -> Result<(), String> {
    let mut config = load();
    config.version = CONFIG_VERSION;
    let text = serde_json::to_string_pretty(&config)
        .map_err(|error| format!("序列化配置失败：{error}"))?;
    if let Some(parent) = dest.parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = std::fs::create_dir_all(parent);
    }
    replace_file(dest, text.as_bytes()).map_err(|error| format!("写入文件失败：{error}"))
}

pub fn import_from(src: &std::path::Path) -> Result<Config, String> {
    let bytes = std::fs::read(src).map_err(|error| format!("读取文件失败：{error}"))?;
    let config = parse_import(&bytes)?;
    save(&config);
    Ok(config)
}

/// 标记「引导已看过」，只改这一个字段，其它配置原样保留。
pub fn set_onboarded(value: bool) {
    let mut config = load();
    if config.onboarded == value {
        return;
    }
    config.onboarded = value;
    save(&config);
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
                    rules: Vec::new(),
                    color: None,
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
            rules: Vec::new(),
            color: None,
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
        assert!(config.usage.is_empty());
        assert!(config.settings.show_recent);
        assert!(config.settings.ignore_fullscreen);
        assert_eq!(config.settings.monitor, "cursor");
    }

    #[test]
    fn usage_entries_round_trip() {
        let mut config = Config {
            version: CONFIG_VERSION,
            ..Default::default()
        };
        config.usage.insert(
            usage_key(std::path::Path::new("C:\\Dev\\App.lnk")),
            UsageEntry {
                count: 5,
                last_used: 1_700_000_000,
            },
        );
        let text = serde_json::to_string(&config).unwrap();
        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(
            back.usage.get("c:\\dev\\app.lnk"),
            Some(&UsageEntry {
                count: 5,
                last_used: 1_700_000_000
            })
        );
    }

    #[test]
    fn parse_import_validates_version_and_json() {
        assert!(parse_import(br#"{"version":1,"settings":{"density":"compact"}}"#).is_ok());
        assert!(parse_import(br#"{broken"#).is_err());
        assert!(parse_import(br#"{"version":99}"#).is_err());
    }

    #[test]
    fn export_and_import_round_trip_file() {
        let dir = std::env::temp_dir().join(format!(
            "litefences-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("exported.json");
        let mut config = Config {
            version: CONFIG_VERSION,
            onboarded: true,
            ..Default::default()
        };
        config.settings.density = "large".into();
        config.groups.push(GroupEntry {
            name: "工具".into(),
            collapsed: true,
            paths: vec!["C:\\Tools\\a.exe".into()],
            rules: Vec::new(),
            color: None,
        });
        let text = serde_json::to_string_pretty(&config).unwrap();
        std::fs::write(&file, &text).unwrap();
        let imported = parse_import(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(imported.settings.density, "large");
        assert!(imported.onboarded);
        assert_eq!(imported.groups.len(), 1);
        assert!(imported.groups[0].collapsed);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn group_rules_serialize_and_default_when_missing() {
        let legacy = r#"{"version":1,"groups":[{"name":"文档","collapsed":false,"paths":[]}]}"#.as_bytes();
        let parsed = parse_import(legacy).unwrap();
        assert!(parsed.groups[0].rules.is_empty());

        let mut cfg = Config {
            version: CONFIG_VERSION,
            ..Default::default()
        };
        cfg.groups.push(GroupEntry {
            name: "文档".into(),
            collapsed: false,
            paths: vec![],
            rules: vec![".pdf".into(), "报告".into()],
            color: None,
        });
        let back: Config = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back.groups[0].rules, vec![".pdf", "报告"]);
    }

    #[test]
    fn group_color_serializes_and_defaults_to_none() {
        let legacy = r#"{"version":1,"groups":[{"name":"开发","collapsed":false,"paths":[]}]}"#.as_bytes();
        let parsed = parse_import(legacy).unwrap();
        assert_eq!(parsed.groups[0].color, None);

        let mut cfg = Config {
            version: CONFIG_VERSION,
            ..Default::default()
        };
        cfg.groups.push(GroupEntry {
            name: "开发".into(),
            collapsed: false,
            paths: vec![],
            rules: vec![],
            color: Some("blue".into()),
        });
        let back: Config = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back.groups[0].color.as_deref(), Some("blue"));
    }

    #[test]
    fn recovery_validates_backup_and_preserves_damaged_bytes() {
        let dir = std::env::temp_dir().join(format!("litefences-recovery-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&dir).unwrap();
        let main = dir.join("config.json");
        let backup = dir.join("config.json.bak");
        std::fs::write(&main, [0xff]).unwrap();
        std::fs::write(&backup, "{").unwrap();
        assert!(recover_backup(&main).is_none());
        assert_eq!(std::fs::read(&main).unwrap(), [0xff]);
        let valid = br#"{"version":1,"custom":["offline.txt"]}"#;
        std::fs::write(&backup, valid).unwrap();
        assert_eq!(recover_backup(&main).unwrap().custom, ["offline.txt"]);
        assert_eq!(std::fs::read(dir.join("config.broken.json")).unwrap(), [0xff]);
        assert_eq!(std::fs::read(&main).unwrap(), valid);
        assert_eq!(std::fs::read(&backup).unwrap(), valid);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
