//! 文件夹入口：把一个目录的一层内容作为只读分组放进面板。
//!
//! 入口是**只读视图**：项不能拖动、改名、换图标，也不接收外部拖入；
//! 「删除分组」只是移除这个入口，不会动目录里的文件。
//!
//! 数据文件 `portals.txt`：每行「目录<TAB>显示名[<TAB>collapsed]」。

use std::path::{Path, PathBuf};

use crate::icons::{DesktopItem, ItemSource};
use crate::log;

/// 单个入口最多显示的项数，避免超大目录拖慢唤出。
const MAX_ITEMS: usize = 200;

pub struct FolderPortal {
    pub dir: PathBuf,
    pub name: String,
    pub collapsed: bool,
}

fn file_path() -> PathBuf {
    crate::log::base_dir().join("portals.txt")
}

pub fn load() -> Vec<FolderPortal> {
    let Ok(text) = std::fs::read_to_string(file_path()) else {
        return Vec::new();
    };
    let mut portals = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let Some(dir) = parts.next() else {
            continue;
        };
        let dir_path = PathBuf::from(dir);
        if !dir_path.is_dir() {
            log::warn(&format!("入口目录不存在，跳过：{dir}"));
            continue;
        }
        let name = parts
            .next()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                dir_path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| dir_path.display().to_string())
            });
        let collapsed = parts.next().is_some_and(|value| value.trim() == "1");
        portals.push(FolderPortal {
            dir: dir_path,
            name,
            collapsed,
        });
    }
    portals
}

pub fn save(portals: &[FolderPortal]) {
    let path = file_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut text = String::from(
        "# LiteFences 文件夹入口 v1：每行「目录<TAB>显示名[<TAB>collapsed]」，只读视图\n",
    );
    for portal in portals {
        text.push_str(&format!(
            "{}\t{}\t{}\n",
            portal.dir.display(),
            portal.name,
            if portal.collapsed { 1 } else { 0 }
        ));
    }
    if let Err(error) = std::fs::write(&path, text) {
        log::error(&format!("保存文件夹入口失败：{error}"));
    }
}

/// 读目录的一层内容并生成面板项（图标句柄由调用方负责销毁）。
pub fn scan(dir: &Path) -> Vec<DesktopItem> {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    let mut found: Vec<(bool, PathBuf)> = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            log::warn(&format!("入口目录不可读 {}：{error}", dir.display()));
            return Vec::new();
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.eq_ignore_ascii_case("desktop.ini") || name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.file_attributes() & (HIDDEN | SYSTEM) != 0 {
            continue;
        }
        found.push((meta.is_dir(), path));
        if found.len() >= MAX_ITEMS {
            break;
        }
    }
    found.sort_by(|left, right| {
        right.0.cmp(&left.0).then_with(|| {
            left.1
                .file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
                .cmp(
                    &right
                        .1
                        .file_name()
                        .map(|name| name.to_string_lossy().to_lowercase()),
                )
        })
    });
    found
        .into_iter()
        .map(|(_, path)| {
            let mut item = crate::icons::make_custom_item(path);
            item.source = ItemSource::Portal;
            item
        })
        .collect()
}
