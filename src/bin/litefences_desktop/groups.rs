//! 分组数据：`groups.txt` 里用 `[组名]` 分段，下面每行一个图标路径；
//! 组名写成 `[!组名]` 表示该组默认折叠。

use std::path::{Path, PathBuf};

use crate::icons;
use crate::log;

#[derive(Clone, Debug)]
pub struct Group {
    pub name: String,
    pub collapsed: bool,
    pub paths: Vec<PathBuf>,
}

impl Group {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            collapsed: false,
            paths: Vec::new(),
        }
    }
}

fn groups_path() -> PathBuf {
    log::base_dir().join("groups.txt")
}

pub const DEFAULT_GROUP: &str = "其它";

pub fn load() -> Vec<Group> {
    let Ok(text) = std::fs::read_to_string(groups_path()) else {
        return Vec::new();
    };
    let mut groups: Vec<Group> = Vec::new();
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
            if name.is_empty() {
                continue;
            }
            if let Some(existing) = groups.iter_mut().find(|group| group.name == name) {
                existing.collapsed = collapsed;
            } else {
                groups.push(Group {
                    name,
                    collapsed,
                    paths: Vec::new(),
                });
            }
            continue;
        }
        let Some(group) = groups.last_mut() else {
            continue;
        };
        group.paths.push(PathBuf::from(trimmed));
    }
    groups
}

pub fn save(groups: &[Group]) {
    let path = groups_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut text = String::from("# LiteFences 分组 v1：`[组名]` 开新组，`[!组名]` 表示默认折叠；组内每行一个路径\n");
    for group in groups {
        if group.collapsed {
            text.push_str(&format!("[!{}]\n", group.name));
        } else {
            text.push_str(&format!("[{}]\n", group.name));
        }
        for item in &group.paths {
            text.push_str(&item.to_string_lossy());
            text.push('\n');
        }
    }
    if let Err(error) = std::fs::write(&path, text) {
        log::error(&format!("保存分组失败：{error}"));
    }
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
pub fn prune(groups: &mut [Group]) {
    for group in groups.iter_mut() {
        group.paths.retain(|path| path.exists());
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
