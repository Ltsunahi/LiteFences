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

pub fn pick_icon_source(owner: HWND) -> Option<PathBuf> {
    pick_with(
        owner,
        "程序与快捷方式\0*.exe;*.lnk;*.bat;*.cmd;*.url\0所有文件\0*.*\0\0",
        "选择要添加到面板的程序或文件",
    )
}
