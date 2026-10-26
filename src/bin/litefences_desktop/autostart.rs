//! 开机自启：只读写 HKCU\...\Run 下本项目自己的一个值，不动其它启动项。

use std::path::Path;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

use crate::util::wide;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "LiteFencesDesktop";

fn open(access: u32) -> Option<HKEY> {
    let path = wide(RUN_KEY);
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: 子键名以 0 结尾；成功时返回需要关闭的句柄。
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, access, &mut key) };
    if status == ERROR_SUCCESS { Some(key) } else { None }
}

/// 注册表里是否已经指向当前程序。
pub fn enabled(exe: &Path) -> bool {
    let Some(key) = open(KEY_QUERY_VALUE) else {
        return false;
    };
    let name = wide(VALUE_NAME);
    let mut size = 2048u32;
    let mut buffer = vec![0u8; size as usize];
    let mut kind = 0u32;
    // SAFETY: 缓冲区与长度匹配；读到的内容立即复制出来。
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            buffer.as_mut_ptr(),
            &mut size,
        )
    };
    // SAFETY: 句柄由本函数打开。
    unsafe { RegCloseKey(key) };
    if status != ERROR_SUCCESS || kind != REG_SZ {
        let _ = ERROR_FILE_NOT_FOUND;
        return false;
    }
    let text: Vec<u16> = buffer[..size as usize]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .filter(|value| *value != 0)
        .collect();
    let stored = String::from_utf16_lossy(&text);
    let expected = format!("\"{}\"", exe.display());
    stored.eq_ignore_ascii_case(&expected)
}

/// 开启或关闭自启；失败时返回可读的原因。
pub fn set_enabled(exe: &Path, enable: bool) -> Result<(), String> {
    let Some(key) = open(KEY_SET_VALUE) else {
        return Err("无法打开启动项注册表".into());
    };
    let name = wide(VALUE_NAME);
    let result = if enable {
        let command = wide(&format!("\"{}\"", exe.display()));
        let bytes: Vec<u8> = command
            .iter()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        // SAFETY: 数据与长度匹配，类型为 REG_SZ。
        let status = unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                bytes.as_ptr(),
                bytes.len() as u32,
            )
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("写入启动项失败 code={status}"))
        }
    } else {
        // SAFETY: 名称以 0 结尾；值不存在也算成功。
        let status = unsafe { RegDeleteValueW(key, name.as_ptr()) };
        if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("删除启动项失败 code={status}"))
        }
    };
    // SAFETY: 句柄由本函数打开。
    unsafe { RegCloseKey(key) };
    result
}
