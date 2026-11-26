//! 分组数据：`groups.txt` 里用 `[组名]` 分段，下面每行一个图标路径；
//! 组名写成 `[!组名]` 表示该组默认折叠。

use std::path::{Path, PathBuf};

use crate::icons;

#[derive(Clone, Debug)]
pub struct Group {
    pub offline: bool,
    pub name: String,
    pub collapsed: bool,
    pub paths: Vec<PathBuf>,
    pub rules: Vec<String>,
    /// 文件夹入口：值为 portals 里的序号；普通分组为 None。
    pub portal: Option<usize>,
}

impl Group {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            offline: false,
            name: name.into(),
            collapsed: false,
            paths: Vec::new(),
            rules: Vec::new(),
            portal: None,
        }
    }
}

pub const DEFAULT_GROUP: &str = "其它";

/// 界面上显示的分组名：默认组在英文界面显示 "Others"（文件里仍然存「其它」，不动数据）。
pub fn display_name(name: &str) -> String {
    if name == DEFAULT_GROUP && crate::i18n::is_en() {
        "Others".to_string()
    } else {
        name.to_string()
    }
}

pub fn load() -> Vec<Group> {
    crate::store::load()
        .groups
        .into_iter()
        .map(|entry| Group {
            offline: false,
            name: entry.name,
            collapsed: entry.collapsed,
            paths: entry.paths.into_iter().map(PathBuf::from).collect(),
            rules: entry.rules,
            portal: None,
        })
        .collect()
}

pub fn save(groups: &[Group]) {
    let mut config = crate::store::load();
    // 入口是只读视图，不写进分组（它由 portals 持久化）。
    config.groups = groups
        .iter()
        .filter(|group| group.portal.is_none())
        .map(|group| crate::store::GroupEntry {
            name: group.name.clone(),
            collapsed: group.collapsed,
            paths: group
                .paths
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect(),
            rules: group.rules.clone(),
        })
        .collect();
    crate::store::save(&config);
}

/// 保证至少有一个“其它”组，用来接住未归组的图标。
pub fn ensure_default(groups: &mut Vec<Group>) {
    if !groups.iter().any(|group| group.name == DEFAULT_GROUP) {
        groups.push(Group::new(DEFAULT_GROUP));
    }
}

/// 某个路径所在的组序号。
pub fn group_of(groups: &[Group], path: &Path) -> Option<usize> {
    groups
        .iter()
        .position(|group| group.paths.iter().any(|known| icons::same_path(known, path)))
}

/// 某个路径在界面上的组序号：没归组的图标由默认组「其它」接住。
pub fn group_of_or_default(groups: &[Group], path: &Path) -> Option<usize> {
    if groups.is_empty() {
        return None;
    }
    Some(group_of(groups, path).unwrap_or_else(|| {
        groups
            .iter()
            .position(|group| group.name == DEFAULT_GROUP)
            .unwrap_or(0)
    }))
}

/// 把路径移动到目标组（`None` 表示移到“其它”）。
pub fn assign(groups: &mut [Group], path: &Path, target: Option<usize>) {
    for group in groups.iter_mut() {
        group.paths.retain(|known| !icons::same_path(known, path));
    }
    let index = target.unwrap_or_else(|| {
        groups
            .iter()
            .position(|group| group.name == DEFAULT_GROUP)
            .unwrap_or(0)
    });
    if let Some(group) = groups.get_mut(index) {
        group.paths.push(path.to_path_buf());
    }
}

/// 去掉已经不存在的路径，避免文件越攒越长。
pub fn prune(groups: &mut [Group], custom: &[PathBuf]) {
    for group in groups.iter_mut() {
        group.paths.retain(|path| custom.iter().any(|known| icons::same_path(known, path)) || path.exists());
    }
}

/// 清掉所有分组里的这个路径（移除图标时用）。
pub fn forget(groups: &mut [Group], path: &Path) {
    for group in groups.iter_mut() {
        group.paths.retain(|known| !icons::same_path(known, path));
    }
}

/// 新建分组；重名返回该组序号。
pub fn create(groups: &mut Vec<Group>, name: &str) -> Option<usize> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if let Some(index) = groups.iter().position(|group| group.name == name) {
        return Some(index);
    }
    groups.push(Group::new(name));
    Some(groups.len() - 1)
}

/// 解析用户输入的归组规则：逗号/分号/空白分隔，扩展名（`.` 开头）统一小写，去重保序。
pub fn parse_rules(text: &str) -> Vec<String> {
    let mut rules = Vec::new();
    let mut seen = Vec::new();
    for part in text.split(|ch: char| ch == ',' || ch == '，' || ch == ';' || ch == '；' || ch.is_whitespace()) {
        let trimmed = part.trim();
        if trimmed.is_empty() || trimmed == "." {
            continue;
        }
        let normalized = if let Some(ext) = trimmed.strip_prefix('.') {
            format!(".{}", ext.to_ascii_lowercase())
        } else {
            trimmed.to_string()
        };
        let key = normalized.to_lowercase();
        if !seen.contains(&key) {
            seen.push(key);
            rules.push(normalized);
        }
    }
    rules
}

pub fn format_rules(rules: &[String]) -> String {
    rules.join(", ")
}

pub fn rule_matches(rule: &str, path: &Path, name: &str) -> bool {
    let rule = rule.trim();
    if rule.is_empty() {
        return false;
    }
    if let Some(ext_rule) = rule.strip_prefix('.') {
        if ext_rule.is_empty() {
            return false;
        }
        return path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case(ext_rule));
    }
    let needle = rule.to_lowercase();
    if name.to_lowercase().contains(&needle) {
        return true;
    }
    path.file_name()
        .and_then(|file| file.to_str())
        .is_some_and(|file| file.to_lowercase().contains(&needle))
}

/// 按分组规则自动归类未归组项目；绝不覆盖已手动归入普通组或入口组的项目。
/// `include_default` 为 true 时，「其它」组内的项也会重新参与规则整理。
pub fn apply_rules(groups: &mut [Group], items: &[(PathBuf, String)], include_default: bool) -> usize {
    let mut moved = 0usize;
    for (path, name) in items {
        let eligible = match group_of(groups, path) {
            None => true,
            Some(index) => include_default && groups[index].name == DEFAULT_GROUP,
        };
        if !eligible {
            continue;
        }
        let target = groups.iter().position(|group| {
            group.portal.is_none()
                && group.name != DEFAULT_GROUP
                && group.rules.iter().any(|rule| rule_matches(rule, path, name))
        });
        if let Some(index) = target {
            assign(groups, path, Some(index));
            moved += 1;
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ungrouped_path_lands_in_default_group() {
        let groups = vec![Group::new("常用"), Group::new(DEFAULT_GROUP)];
        assert_eq!(group_of_or_default(&groups, Path::new(r"C:\x.lnk")), Some(1));
        let mut known = groups.clone();
        known[0].paths.push(PathBuf::from(r"C:\x.lnk"));
        // 路径比较不区分大小写，和面板归组规则保持一致。
        assert_eq!(group_of_or_default(&known, Path::new(r"c:\X.LNK")), Some(0));
    }

    #[test]
    fn parse_rules_splits_normalizes_and_deduplicates() {
        let rules = parse_rules(".PDF, 会议； .pdf  设计稿\n.DOCX");
        assert_eq!(rules, vec![".pdf", "会议", "设计稿", ".docx"]);
    }

    #[test]
    fn apply_rules_assigns_ungrouped_without_overriding_manual_groups() {
        let mut docs = Group::new("文档");
        docs.rules = vec![".pdf".into(), "周报".into()];
        let mut pinned = Group::new("常用");
        pinned.paths.push(PathBuf::from(r"C:\Desktop\manual.pdf"));
        let mut other = Group::new(DEFAULT_GROUP);
        other.paths.push(PathBuf::from(r"C:\Desktop\old.pdf"));
        let mut groups = vec![docs, pinned, other];

        let items = vec![
            (PathBuf::from(r"C:\Desktop\new.PDF"), "new".to_string()),
            (PathBuf::from(r"C:\Desktop\manual.pdf"), "manual".to_string()),
            (PathBuf::from(r"C:\Desktop\old.pdf"), "old".to_string()),
            (PathBuf::from(r"C:\Desktop\note.txt"), "项目周报".to_string()),
            (PathBuf::from(r"C:\Desktop\game.exe"), "game".to_string()),
        ];

        // 仅新项归组：保留已经在「其它」和「常用」的项。
        assert_eq!(apply_rules(&mut groups, &items, false), 2);
        assert_eq!(group_of(&groups, Path::new(r"C:\Desktop\new.PDF")), Some(0));
        assert_eq!(group_of(&groups, Path::new(r"C:\Desktop\note.txt")), Some(0));
        assert_eq!(group_of(&groups, Path::new(r"C:\Desktop\manual.pdf")), Some(1));
        assert_eq!(group_of(&groups, Path::new(r"C:\Desktop\old.pdf")), Some(2));
        assert_eq!(group_of(&groups, Path::new(r"C:\Desktop\game.exe")), None);

        // include_default = true 时，「其它」组内的匹配项会按规则整理，但「常用」保持不动。
        assert_eq!(apply_rules(&mut groups, &items, true), 1);
        assert_eq!(group_of(&groups, Path::new(r"C:\Desktop\old.pdf")), Some(0));
        assert_eq!(group_of(&groups, Path::new(r"C:\Desktop\manual.pdf")), Some(1));
    }
}
