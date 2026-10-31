//! 桌面图标索引缓存。
//!
//! 启动时最慢的一步是逐个文件调 `SHGetFileInfo`（.lnk 还要解析目标、加载 shell 扩展），
//! 82 个图标实测要 1.7 秒左右。这里把「路径 → 系统图标列表里的索引」缓存下来，
//! 下次启动直接 `ImageList_GetIcon`，跳过 shell 查询。
//!
//! 失效判断只看「修改时间 + 文件大小」：桌面上的快捷方式重命名/重建就会重新取。
//! 换图标主题或 DPI 时索引可能指向旧图标——这是缓存的代价，删掉 icons-cache.json 即可重建。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const CACHE_VERSION: u32 = 1;
/// 超过这个条目数就不再往上长（桌面不会有这么多图标）。
const MAX_ENTRIES: usize = 2000;

#[derive(Serialize, Deserialize, Default)]
struct CacheFile {
    version: u32,
    entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    path: String,
    mtime: u64,
    size: u64,
    icon: i32,
}

pub struct IconCache {
    map: HashMap<String, (u64, u64, i32)>,
    dirty: bool,
    hits: usize,
    misses: usize,
}

fn cache_path() -> PathBuf {
    crate::log::base_dir().join("icons-cache.json")
}

fn key_of(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

/// 文件修改时间（秒）；取不到就当 0，表示“别缓存”。
pub fn mtime_of(path: &Path) -> u64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

pub fn size_of(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

impl IconCache {
    pub fn load() -> Self {
        let mut map = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(cache_path())
            && let Ok(file) = serde_json::from_str::<CacheFile>(&text)
            && file.version == CACHE_VERSION
        {
            for entry in file.entries {
                map.insert(entry.path, (entry.mtime, entry.size, entry.icon));
            }
        }
        Self {
            map,
            dirty: false,
            hits: 0,
            misses: 0,
        }
    }

    /// 命中且「修改时间 + 大小」都对得上才返回索引。
    pub fn lookup(&mut self, path: &Path, mtime: u64, size: u64) -> Option<i32> {
        if mtime == 0 {
            self.misses += 1;
            return None;
        }
        let hit = self
            .map
            .get(&key_of(path))
            .filter(|(known_mtime, known_size, _)| *known_mtime == mtime && *known_size == size)
            .map(|(_, _, icon)| *icon);
        if hit.is_some() {
            self.hits += 1;
        } else {
            self.misses += 1;
        }
        hit
    }

    /// 本轮命中的条目数（用于日志）。
    pub fn hits(&self) -> usize {
        self.hits
    }

    pub fn misses(&self) -> usize {
        self.misses
    }

    pub fn remember(&mut self, path: &Path, mtime: u64, size: u64, icon: i32) {
        if icon < 0 || mtime == 0 {
            return;
        }
        self.map.insert(key_of(path), (mtime, size, icon));
        self.dirty = true;
    }

    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let mut entries: Vec<Entry> = self
            .map
            .iter()
            .take(MAX_ENTRIES)
            .map(|(path, (mtime, size, icon))| Entry {
                path: path.clone(),
                mtime: *mtime,
                size: *size,
                icon: *icon,
            })
            .collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let file = CacheFile {
            version: CACHE_VERSION,
            entries,
        };
        let Ok(text) = serde_json::to_string(&file) else {
            return;
        };
        let path = cache_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let temp = path.with_extension("json.tmp");
        if std::fs::write(&temp, text).is_ok() {
            let _ = std::fs::rename(&temp, &path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_requires_matching_mtime_and_size() {
        let mut cache = IconCache {
            map: HashMap::new(),
            dirty: false,
            hits: 0,
            misses: 0,
        };
        let path = Path::new("C:\\Users\\x\\Desktop\\a.lnk");
        cache.remember(path, 100, 2048, 7);
        assert_eq!(cache.lookup(path, 100, 2048), Some(7));
        // 时间变了（文件被改）就不该再用缓存
        assert_eq!(cache.lookup(path, 101, 2048), None);
        // 大小变了同理
        assert_eq!(cache.lookup(path, 100, 4096), None);
        // 没缓存的路径
        assert_eq!(cache.lookup(Path::new("C:\\nope.lnk"), 100, 2048), None);
    }

    #[test]
    fn refuses_to_cache_unreadable_files() {
        let mut cache = IconCache {
            map: HashMap::new(),
            dirty: false,
            hits: 0,
            misses: 0,
        };
        cache.remember(Path::new("C:\\a.lnk"), 0, 0, 3);
        assert!(cache.map.is_empty());
    }

    #[test]
    fn cache_file_round_trips() {
        let file = CacheFile {
            version: CACHE_VERSION,
            entries: vec![Entry {
                path: "c:\\a.lnk".into(),
                mtime: 1,
                size: 2,
                icon: 3,
            }],
        };
        let text = serde_json::to_string(&file).unwrap();
        let back: CacheFile = serde_json::from_str(&text).unwrap();
        assert_eq!(back.version, CACHE_VERSION);
        assert_eq!(back.entries[0].icon, 3);
    }
}
