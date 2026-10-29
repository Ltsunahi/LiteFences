//! 分组数据：`groups.txt` 里用 `[组名]` 分段，下面每行一个图标路径；
//! 组名写成 `[!组名]` 表示该组默认折叠。

use std::path::{Path, PathBuf};

use crate::icons;

#[derive(Clone, Debug)]
pub struct Group {
    pub name: String,
    pub collapsed: bool,
    pub paths: Vec<PathBuf>,
    /// 文件夹入口：值为 portals 里的序号；普通分组为 None。
    pub portal: Option<usize>,
}

impl Group {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            collapsed: false,
            paths: Vec::new(),
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
            name: entry.name,
            collapsed: entry.collapsed,
            paths: entry.paths.into_iter().map(PathBuf::from).collect(),
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
