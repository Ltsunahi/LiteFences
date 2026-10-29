//! 文件夹入口：把一个目录的一层内容作为只读分组放进面板。
//!
//! 入口是**只读视图**：项不能拖动、改名、换图标，也不接收外部拖入；
//! 「删除分组」只是移除这个入口，不会动目录里的文件。
//!
//! 数据文件 `portals.txt`：每行「目录<TAB>显示名[<TAB>collapsed[<TAB>sort]]」，
//! sort 取值 `folder`（默认，文件夹在前再按名称）/ `name` / `time`。

use std::path::PathBuf;

use crate::icons::{DesktopItem, ItemSource};
use crate::log;

/// 单个入口最多显示的项数，避免超大目录拖慢唤出。
const MAX_ITEMS: usize = 200;

/// 入口内容的排序方式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortMode {
    /// 文件夹在前，各自按名称（默认）。
    FolderFirst,
    /// 不分类别，全部按名称。
    Name,
    /// 按修改时间，新的在前。
    Time,
}

impl SortMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "" | "folder" | "folder-first" => Some(Self::FolderFirst),
            "name" => Some(Self::Name),
            "time" | "modified" => Some(Self::Time),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::FolderFirst => "folder",
            Self::Name => "name",
            Self::Time => "time",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::FolderFirst => crate::i18n::t(
                "文件夹在前，再按名称",
                "Folders first, then by name",
            ),
            Self::Name => crate::i18n::t("按名称", "By name"),
            Self::Time => crate::i18n::t("按修改时间（新的在前）", "By modified time (newest first)"),
        }
    }
}

pub struct FolderPortal {
    pub dir: PathBuf,
    pub name: String,
    pub collapsed: bool,
    pub sort: SortMode,
}

pub fn load() -> Vec<FolderPortal> {
    crate::store::load()
        .portals
        .into_iter()
        .filter_map(|entry| {
            let dir_path = PathBuf::from(&entry.dir);
            if !dir_path.is_dir() {
                log::warn(&format!("入口目录不存在，跳过：{}", entry.dir));
                return None;
            }
            let name = if entry.name.trim().is_empty() {
                dir_path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| dir_path.display().to_string())
            } else {
                entry.name.trim().to_string()
            };
            Some(FolderPortal {
                dir: dir_path,
                name,
                collapsed: entry.collapsed,
                sort: SortMode::parse(&entry.sort).unwrap_or(SortMode::FolderFirst),
            })
        })
        .collect()
}

pub fn save(portals: &[FolderPortal]) {
    let mut config = crate::store::load();
    config.portals = portals
        .iter()
        .map(|portal| crate::store::PortalEntry {
            dir: portal.dir.to_string_lossy().to_string(),
            name: portal.name.clone(),
            collapsed: portal.collapsed,
            sort: portal.sort.as_str().to_string(),
        })
        .collect();
    crate::store::save(&config);
}

/// 目录里的一项，排序用。
struct Entry {
    is_dir: bool,
    /// 小写名字，用于大小写不敏感比较。
    lower: String,
    /// Unix 秒；取不到时为 0。
    modified: u64,
    path: PathBuf,
}

fn sort_entries(entries: &mut [Entry], mode: SortMode) {
    match mode {
        SortMode::FolderFirst => entries.sort_by(|left, right| {
            right
                .is_dir
                .cmp(&left.is_dir)
                .then_with(|| left.lower.cmp(&right.lower))
        }),
        SortMode::Name => entries.sort_by(|left, right| left.lower.cmp(&right.lower)),
        SortMode::Time => entries.sort_by(|left, right| {
            right
                .modified
                .cmp(&left.modified)
                .then_with(|| left.lower.cmp(&right.lower))
        }),
    }
}

/// 读目录的一层内容并生成面板项（图标句柄由调用方负责销毁）。
pub fn scan(portal: &FolderPortal) -> Vec<DesktopItem> {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    let mut entries: Vec<Entry> = Vec::new();
    let read = match std::fs::read_dir(&portal.dir) {
        Ok(read) => read,
        Err(error) => {
            log::warn(&format!("入口目录不可读 {}：{error}", portal.dir.display()));
            return Vec::new();
        }
    };
    for entry in read.flatten() {
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
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        entries.push(Entry {
            is_dir: meta.is_dir(),
            lower: name.to_lowercase(),
            modified,
            path,
        });
        if entries.len() >= MAX_ITEMS {
            break;
        }
    }
    sort_entries(&mut entries, portal.sort);
    entries
        .into_iter()
        .map(|entry| {
            let mut item = crate::icons::make_custom_item(entry.path);
            item.source = ItemSource::Portal;
            item
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool, modified: u64) -> Entry {
        Entry {
            is_dir,
            lower: name.to_lowercase(),
            modified,
            path: PathBuf::from(name),
        }
    }

    fn names(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| entry.path.to_string_lossy().to_string())
            .collect()
    }

    fn scene() -> Vec<Entry> {
        vec![
            entry("b.txt", false, 10),
            entry("A文件夹", true, 5),
            entry("a.txt", false, 30),
            entry("Z文件夹", true, 20),
        ]
    }

    #[test]
    fn folder_first_puts_directories_ahead() {
        let mut entries = scene();
        sort_entries(&mut entries, SortMode::FolderFirst);
        assert_eq!(names(&entries), ["A文件夹", "Z文件夹", "a.txt", "b.txt"]);
    }

    #[test]
    fn name_mode_ignores_kind_and_case() {
        let mut entries = scene();
        sort_entries(&mut entries, SortMode::Name);
        // "a.txt" < "a文件夹" < "b.txt" < "z文件夹"（点号 < 汉字 < b < z）
        assert_eq!(names(&entries), ["a.txt", "A文件夹", "b.txt", "Z文件夹"]);
    }

    #[test]
    fn time_mode_puts_newest_first() {
        let mut entries = scene();
        sort_entries(&mut entries, SortMode::Time);
        assert_eq!(names(&entries), ["a.txt", "Z文件夹", "b.txt", "A文件夹"]);
    }

    #[test]
    fn sort_mode_round_trips() {
        for mode in [SortMode::FolderFirst, SortMode::Name, SortMode::Time] {
            assert_eq!(SortMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(SortMode::parse(""), Some(SortMode::FolderFirst));
        assert_eq!(SortMode::parse("bogus"), None);
    }
}
