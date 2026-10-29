//! 读取桌面图标并启动。只读取桌面目录，不修改用户文件。

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM, GetFileAttributesW, INVALID_FILE_ATTRIBUTES,
};
use windows_sys::Win32::UI::Controls::{HIMAGELIST, ILD_TRANSPARENT, ImageList_GetIcon};
use windows_sys::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SYSICONINDEX, SHGetFileInfoW, SHGetImageList,
    SHIL_EXTRALARGE, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, HICON, IMAGE_ICON, LR_DEFAULTSIZE, LR_LOADFROMFILE, LoadImageW, SW_SHOWNORMAL,
};

use crate::log;
use crate::util::wide;

/// IImageList 的接口 ID，用于 SHGetImageList。
const IID_IIMAGELIST: GUID = GUID::from_u128(0x46eb_5926_582e_4017_9fdf_e899_8daa_0950);

const MAX_ITEMS: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ItemSource {
    /// 自动读到的桌面图标。
    Desktop,
    /// 用户手动添加的。
    Custom,
    /// 文件夹入口里的内容（只读视图，不写进 layout/custom 列表）。
    Portal,
}

#[derive(Clone)]
pub struct DesktopItem {
    pub path: PathBuf,
    pub name: String,
    pub icon: HICON,
    pub source: ItemSource,
    /// 用户指定的图标文件（.ico），为空表示用系统默认图标。
    pub custom_icon: Option<PathBuf>,
}

// SAFETY: 图标句柄属于进程范围，不绑定创建线程；所有权在后台线程创建、UI 线程使用与释放。
unsafe impl Send for DesktopItem {}

static SYSTEM_IMAGE_LIST: OnceLock<isize> = OnceLock::new();

/// 重新扫描桌面。旧图标句柄会在这里释放，不会随使用次数增长。
pub fn refresh(items: &mut Vec<DesktopItem>) {
    match collect_items(&[]) {
        Ok(scanned) => replace(items, scanned),
        Err(error) => log::error(&format!("扫描桌面失败：{error}")),
    }
}

/// 用新扫描结果替换旧的，并释放旧图标句柄。只能在 UI 线程调用。
pub fn replace(items: &mut Vec<DesktopItem>, mut scanned: Vec<DesktopItem>) {
    for old in items.drain(..) {
        // SAFETY: 句柄由 DestroyIcon, HICON / SHGetFileInfo 返回，且只释放一次。
        unsafe {
            if !old.icon.is_null() {
                DestroyIcon(old.icon);
            }
        }
    }
    log::info(&format!("桌面图标 {} 项", scanned.len()));
    items.append(&mut scanned);
}

/// 在后台线程里执行扫描；结果通过 channel 交回 UI 线程。`hidden` 里的路径会被跳过。
pub fn collect_items(hidden: &[PathBuf]) -> Result<Vec<DesktopItem>, String> {
    // SAFETY: 后台线程需要自己的 COM 单元；失败只影响图标查询。
    let _ = unsafe {
        windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        )
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut found: Vec<(bool, String, PathBuf)> = Vec::new();
    for dir in desktop_dirs() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                log::warn(&format!("跳过不可读桌面目录 {}：{error}", dir.display()));
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
                continue;
            };
            if name.eq_ignore_ascii_case("desktop.ini") || name.starts_with('.') {
                continue;
            }
            let wide_path = os_wide(&path);
            // SAFETY: 路径以 0 结尾，缓冲区长度正确。
            let attributes = unsafe { GetFileAttributesW(wide_path.as_ptr()) };
            if attributes == INVALID_FILE_ATTRIBUTES {
                continue;
            }
            if attributes & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0 {
                continue;
            }
            let key = path.to_string_lossy().to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            if hidden.iter().any(|hidden_path| same_path(hidden_path, &path)) {
                continue;
            }
            let is_dir = attributes & 0x10 != 0;
            found.push((is_dir, name, path));
            if found.len() >= MAX_ITEMS {
                break;
            }
        }
        if found.len() >= MAX_ITEMS {
            break;
        }
    }
    found.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    let image_list = system_image_list();
    Ok(found
        .into_iter()
        .map(|(_is_dir, file_name, path)| {
            let name = display_name(&file_name);
            let started = std::time::Instant::now();
            let icon = load_icon(&path, image_list);
            let elapsed = started.elapsed().as_millis();
            if elapsed > 200 {
                log::warn(&format!("取图标耗时 {elapsed}ms：{}", path.display()));
            }
            DesktopItem {
                path,
                name,
                icon,
                source: ItemSource::Desktop,
                custom_icon: None,
            }
        })
        .collect())
}

/// 桌面目录的“版本号”，用于判断是否需要重新扫描。
pub fn desktop_stamp() -> u64 {
    let mut stamp = 0u64;
    for dir in desktop_dirs() {
        if let Ok(meta) = std::fs::metadata(&dir)
            && let Ok(modified) = meta.modified()
            && let Ok(elapsed) = modified.duration_since(std::time::UNIX_EPOCH)
        {
            stamp ^= elapsed.as_secs().wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ u64::from(elapsed.subsec_nanos());
        }
    }
    stamp
}

fn desktop_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        dirs.push(PathBuf::from(profile).join("Desktop"));
    }
    if let Some(public) = std::env::var_os("PUBLIC") {
        dirs.push(PathBuf::from(public).join("Desktop"));
    }
    if let Some(one_drive) = std::env::var_os("OneDrive") {
        dirs.push(PathBuf::from(one_drive).join("Desktop"));
    }
    dirs.retain(|dir| dir.is_dir());
    dirs
}

fn display_name(file_name: &str) -> String {
    let lower = file_name.to_lowercase();
    for suffix in [".lnk", ".url", ".appref-ms"] {
        if lower.ends_with(suffix) {
            return file_name[..file_name.len() - suffix.len()].to_string();
        }
    }
    file_name.to_string()
}

fn system_image_list() -> isize {
    *SYSTEM_IMAGE_LIST.get_or_init(|| {
        let mut raw: *mut c_void = std::ptr::null_mut();
        // SAFETY: 参数与 SHGetImageList 约定一致；成功时返回借用自系统的图像列表。
        let hr = unsafe { SHGetImageList(SHIL_EXTRALARGE as i32, &IID_IIMAGELIST, &mut raw) };
        if hr < 0 || raw.is_null() {
            log::warn(&format!("SHGetImageList 失败 hr=0x{hr:08x}，改用 32px 图标"));
            0
        } else {
            raw as isize
        }
    })
}

fn load_icon(path: &Path, image_list: isize) -> HICON {
    let wide_path = os_wide(path);
    let mut info = SHFILEINFOW::default();
    // SAFETY: 结构体大小与缓冲区一致，路径以 0 结尾。
    let ok = unsafe {
        SHGetFileInfoW(
            wide_path.as_ptr(),
            0,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX,
        )
    };
    if ok != 0 && image_list != 0 {
        // SAFETY: image_list 由 SHGetImageList 返回，索引来自 SHGetFileInfo。
        let icon = unsafe { ImageList_GetIcon(image_list as HIMAGELIST, info.iIcon, ILD_TRANSPARENT) };
        if !icon.is_null() {
            return icon;
        }
    }
    let mut fallback = SHFILEINFOW::default();
    // SAFETY: 同上。
    let ok = unsafe {
        SHGetFileInfoW(
            wide_path.as_ptr(),
            0,
            &mut fallback,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if ok != 0 {
        fallback.hIcon
    } else {
        std::ptr::null_mut()
    }
}

/// 快捷方式解析结果。
struct Shortcut {
    target: String,
    arguments: String,
    directory: String,
}

/// 直接解析 `.lnk`，再启动目标程序。
///
/// 受限权限下 `ShellExecute` 对 `.lnk` 会“返回成功但什么也不启动”，
/// 因此先自己解析出目标，再交给 Shell 启动真实可执行文件。
fn resolve_shortcut(path: &Path) -> Option<Shortcut> {
    use windows::core::{HSTRING, Interface};
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        IPersistFile, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    // SAFETY: COM 初始化失败只影响本函数；接口调用都做了错误处理。
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        file.Load(&HSTRING::from(path.as_os_str()), STGM_READ).ok()?;
        let mut target = [0u16; 1024];
        link.GetPath(&mut target, std::ptr::null_mut(), 0).ok()?;
        let mut arguments = [0u16; 1024];
        let _ = link.GetArguments(&mut arguments);
        let mut directory = [0u16; 1024];
        let _ = link.GetWorkingDirectory(&mut directory);
        let target = to_string(&target);
        if target.is_empty() {
            return None;
        }
        Some(Shortcut {
            target,
            arguments: to_string(&arguments),
            directory: to_string(&directory),
        })
    }
}

fn to_string(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&ch| ch == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

fn shell_execute(file: &str, parameters: &str, directory: &str, original: &Path) -> Result<(), String> {
    let file = wide(file);
    let parameters = wide(parameters);
    let directory = wide(directory);
    let operation = wide("open");
    let (params_ptr, dir_ptr) = if parameters.len() > 1 { (parameters.as_ptr(), directory.as_ptr()) } else { (std::ptr::null(), std::ptr::null()) };
    // SAFETY: 字符串均以 0 结尾，句柄为空表示不指定父窗口。
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut() as HWND,
            operation.as_ptr(),
            file.as_ptr(),
            params_ptr,
            dir_ptr,
            SW_SHOWNORMAL,
        )
    };
    let code = result as isize;
    log::info(&format!("ShellExecute 返回 {code}：{}", original.display()));
    if code > 32 {
        log::info(&format!("启动 {}", original.display()));
        Ok(())
    } else {
        let message = format!("启动失败 code={code}：{}", original.display());
        log::error(&message);
        Err(message)
    }
}

/// 释放单个面板项的图标句柄（移除自定义图标时用）。
pub fn destroy_item(item: DesktopItem) {
    // SAFETY: 句柄由本模块创建，只释放一次。
    unsafe {
        if !item.icon.is_null() {
            DestroyIcon(item.icon);
        }
    }
}

/// 为一个手动添加的路径生成面板项。
pub fn make_custom_item(path: PathBuf) -> DesktopItem {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| crate::i18n::t("未命名", "Untitled"))
        .to_string();
    let name = display_name(&file_name);
    let icon = load_icon(&path, system_image_list());
    DesktopItem {
        path,
        name,
        icon,
        source: ItemSource::Custom,
        custom_icon: None,
    }
}

/// 读取手动添加的图标路径。
pub fn load_custom() -> Vec<PathBuf> {
    crate::store::load()
        .custom
        .into_iter()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect()
}

/// 保存手动添加的图标路径。
pub fn save_custom(paths: &[PathBuf]) {
    let mut config = crate::store::load();
    config.custom = paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    crate::store::save(&config);
}

/// 路径比较：Windows 下大小写不敏感。
pub fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

pub fn load_hidden() -> Vec<PathBuf> {
    crate::store::load()
        .hidden
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

pub fn save_hidden(paths: &[PathBuf]) {
    let mut config = crate::store::load();
    config.hidden = paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    crate::store::save(&config);
}

/// 读取布局：顺序即面板顺序，名字与图标在同一个条目里。
pub fn load_order() -> Vec<(PathBuf, String)> {
    crate::store::load()
        .items
        .into_iter()
        .map(|entry| (PathBuf::from(entry.path), entry.name))
        .collect()
}

/// 保存布局：路径 + 显示名 + 可选自定义图标。
pub fn save_order(items: &[DesktopItem]) {
    let mut config = crate::store::load();
    config.items = items
        .iter()
        .map(|item| crate::store::ItemEntry {
            path: item.path.to_string_lossy().to_string(),
            name: item.name.clone(),
            icon: item
                .custom_icon
                .as_ref()
                .map(|icon| icon.to_string_lossy().to_string()),
        })
        .collect();
    crate::store::save(&config);
}

/// 「路径 → 自定义图标」的对应关系。
pub fn load_custom_icons() -> Vec<(PathBuf, PathBuf)> {
    crate::store::load()
        .items
        .into_iter()
        .filter_map(|entry| {
            let icon = entry.icon?;
            if icon.trim().is_empty() {
                return None;
            }
            Some((PathBuf::from(entry.path), PathBuf::from(icon)))
        })
        .collect()
}

/// 按 layout.txt 的记录替换自定义图标（会释放原来的图标句柄）。
pub fn apply_custom_icons(items: &mut [DesktopItem]) {
    let entries = load_custom_icons();
    if entries.is_empty() {
        return;
    }
    for item in items.iter_mut() {
        let Some((_, icon_path)) = entries
            .iter()
            .find(|(known, _)| same_path(known, &item.path))
        else {
            continue;
        };
        if let Some(icon) = load_icon_file(icon_path) {
            if !item.icon.is_null() {
                // SAFETY: 图标句柄由本模块创建。
                unsafe { DestroyIcon(item.icon) };
            }
            item.icon = icon;
            item.custom_icon = Some(icon_path.clone());
        }
    }
}

/// 从 .ico 文件加载图标。
pub fn load_icon_file(path: &Path) -> Option<HICON> {
    if !path.exists() {
        return None;
    }
    let wide_path = os_wide(path);
    // SAFETY: 路径以 0 结尾；只读取文件。
    let icon = unsafe {
        LoadImageW(
            std::ptr::null_mut(),
            wide_path.as_ptr(),
            IMAGE_ICON,
            0,
            0,
            LR_LOADFROMFILE | LR_DEFAULTSIZE,
        )
    };
    if icon.is_null() {
        log::warn(&format!("无法加载图标文件：{}", path.display()));
        None
    } else {
        Some(icon)
    }
}

/// 换一个图标文件，返回是否成功。
pub fn set_custom_icon(item: &mut DesktopItem, icon_path: &Path) -> bool {
    let Some(icon) = load_icon_file(icon_path) else {
        return false;
    };
    if !item.icon.is_null() {
        // SAFETY: 图标句柄由本模块创建。
        unsafe { DestroyIcon(item.icon) };
    }
    item.icon = icon;
    item.custom_icon = Some(icon_path.to_path_buf());
    true
}

/// 恢复系统默认图标。
pub fn clear_custom_icon(item: &mut DesktopItem) {
    let icon = load_icon(&item.path, system_image_list());
    if !item.icon.is_null() {
        // SAFETY: 图标句柄由本模块创建。
        unsafe { DestroyIcon(item.icon) };
    }
    item.icon = icon;
    item.custom_icon = None;
}

/// 按保存的顺序和名字整理面板项；未记录的项目排在后面。
pub fn apply_order(items: &mut [DesktopItem], entries: &[(PathBuf, String)]) {
    for item in items.iter_mut() {
        if let Some((_, name)) = entries.iter().find(|(path, _)| same_path(path, &item.path))
            && !name.is_empty()
        {
            item.name = name.clone();
        }
    }
    let rank = |path: &Path| {
        entries
            .iter()
            .position(|(known, _)| same_path(known, path))
            .unwrap_or(usize::MAX)
    };
    items.sort_by(|left, right| {
        rank(&left.path)
            .cmp(&rank(&right.path))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
}

/// 用系统关联程序打开；快捷方式先解析再启动目标。
pub fn launch(path: &Path) -> Result<(), String> {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
    {
        if let Some(shortcut) = resolve_shortcut(path) {
            log::info(&format!("快捷方式目标：{}", shortcut.target));
            return shell_execute(
                &shortcut.target,
                &shortcut.arguments,
                &shortcut.directory,
                path,
            );
        }
        log::warn(&format!("快捷方式未能解析，退回直接打开：{}", path.display()));
    }
    shell_execute(&path.to_string_lossy(), "", "", path)
}

fn os_wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

