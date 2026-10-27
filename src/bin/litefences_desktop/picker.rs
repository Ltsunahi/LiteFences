//! 选择文件的小封装：用系统「打开」对话框挑一个程序或快捷方式。

use std::path::PathBuf;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST,
    OPENFILENAMEW,
};

use crate::log;
use crate::util::wide;

/// 挑一个图标文件（.ico）。
pub fn pick_icon_file(owner: HWND) -> Option<PathBuf> {
    pick_with(owner, "图标文件\0*.ico\0所有文件\0*.*\0\0", "选择图标文件")
}

fn pick_with(owner: HWND, filter_text: &str, title_text: &str) -> Option<PathBuf> {
    let mut buffer = [0u16; 1024];
    let filter: Vec<u16> = filter_text.encode_utf16().collect();
    let title = wide(title_text);
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: filter.as_ptr(),
        lpstrFile: buffer.as_mut_ptr(),
        nMaxFile: buffer.len() as u32,
        lpstrTitle: title.as_ptr(),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_EXPLORER | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    // SAFETY: 结构体各字段指向本次调用期间有效的缓冲区。
    let ok = unsafe { GetOpenFileNameW(&mut dialog) } != 0;
    if !ok {
        log::info("已取消选择文件");
        return None;
    }
    let end = buffer.iter().position(|&ch| ch == 0).unwrap_or(buffer.len());
    if end == 0 {
        return None;
    }
    let text = String::from_utf16_lossy(&buffer[..end]);
    log::info(&format!("选择文件：{text}"));
    Some(PathBuf::from(text))
}

/// 选一个目录（文件夹入口用）。只读路径，不做任何修改。
pub fn pick_folder(owner: HWND) -> Option<PathBuf> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoTaskMemFree,
    };
    use windows::Win32::UI::Shell::{
        FOS_DONTADDTORECENT, FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS,
        FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
    };
    use windows::core::w;

    // SAFETY: 在 UI 线程创建并显示对话框；调用期间不持有任何跨线程状态。
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = match CoCreateInstance(
            &FileOpenDialog,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            Ok(dialog) => dialog,
            Err(error) => {
                log::warn(&format!("无法创建文件夹选择器（代码 {}）", error.code().0));
                return None;
            }
        };
        if let Ok(options) = dialog.GetOptions()
            && dialog
                .SetOptions(
                    options
                        | FOS_PICKFOLDERS
                        | FOS_FORCEFILESYSTEM
                        | FOS_PATHMUSTEXIST
                        | FOS_DONTADDTORECENT,
                )
                .is_err()
        {
            log::warn("文件夹选择器不接受目录选择选项");
            return None;
        }
        let _ = dialog.SetTitle(w!("选择要作为入口的文件夹（只读展示，不会移动文件）"));
        let owner_hwnd = windows::Win32::Foundation::HWND(owner);
        if let Err(error) = dialog.Show(Some(owner_hwnd)) {
            log::info(&format!("已取消选择文件夹（代码 {}）", error.code().0));
            return None;
        }
        let item = match dialog.GetResult() {
            Ok(item) => item,
            Err(error) => {
                log::warn(&format!("读取所选文件夹失败（代码 {}）", error.code().0));
                return None;
            }
        };
        let raw = match item.GetDisplayName(SIGDN_FILESYSPATH) {
            Ok(raw) => raw,
            Err(error) => {
                log::warn(&format!("所选路径不是本地文件夹（代码 {}）", error.code().0));
                return None;
            }
        };
        let text = String::from_utf16_lossy(raw.as_wide());
        CoTaskMemFree(Some(raw.as_ptr().cast()));
        if text.is_empty() {
            return None;
        }
        let dir = PathBuf::from(text);
        log::info(&format!("选择文件夹：{}", dir.display()));
        Some(dir)
    }
}

pub fn pick_icon_source(owner: HWND) -> Option<PathBuf> {
    pick_with(
        owner,
        "程序与快捷方式\0*.exe;*.lnk;*.bat;*.cmd;*.url\0所有文件\0*.*\0\0",
        "选择要添加到面板的程序或文件",
    )
}
