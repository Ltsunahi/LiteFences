//! 受限本地日志。桌面启动器没有控制台，排障只能靠它。
//!
//! 只记录时间、级别和调用方给出的文本。单文件上限 512 KiB，超限时轮换为 `.old`。
//! 目录按「显式覆盖 → 用户数据目录 → 程序目录 → 临时目录」依次尝试，
//! 任何一步失败都不影响主流程。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::System::SystemInformation::GetLocalTime;

const MAX_BYTES: u64 = 512 * 1024;

static SINK: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static BASE: OnceLock<PathBuf> = OnceLock::new();
static PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn init() {
    let candidates = candidate_dirs();
    for dir in candidates {
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let path = dir.join("launcher.log");
        if let Ok(meta) = std::fs::metadata(&path)
            && meta.len() > MAX_BYTES
        {
            let _ = std::fs::rename(&path, dir.join("launcher.old.log"));
        }
        let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
            continue;
        };
        let _ = BASE.set(dir);
        let _ = PATH.set(path);
        let _ = SINK.set(Mutex::new(Some(file)));
        return;
    }
    let _ = SINK.set(Mutex::new(None));
}

/// 日志与自检报告所在目录；可用环境变量覆盖，便于在受限环境下测试。
pub fn base_dir() -> PathBuf {
    BASE.get()
        .cloned()
        .unwrap_or_else(|| std::env::temp_dir().join("LiteFencesDesktop"))
}

pub fn path_display() -> String {
    PATH.get().map(|p| p.display().to_string()).unwrap_or_default()
}

pub fn info(message: &str) {
    write("INFO", message);
}

pub fn warn(message: &str) {
    write("WARN", message);
}

pub fn error(message: &str) {
    write("ERROR", message);
}

fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("LITEFENCES_DESKTOP_LOG_DIR") {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("LiteFencesDesktop"));
    }
    if let Some(exe) = std::env::current_exe().ok().and_then(|path| parent_of(&path)) {
        dirs.push(exe.join("logs"));
    }
    dirs.push(std::env::temp_dir().join("LiteFencesDesktop"));
    dirs
}

fn parent_of(path: &Path) -> Option<PathBuf> {
    path.parent().map(Path::to_path_buf)
}

fn write(level: &str, message: &str) {
    let line = format!("[{}] {level} {message}\n", timestamp());
    if let Some(lock) = SINK.get()
        && let Ok(mut guard) = lock.lock()
        && let Some(file) = guard.as_mut()
    {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
    if cfg!(debug_assertions) {
        eprint!("{line}");
    }
}

fn timestamp() -> String {
    // SAFETY: GetLocalTime 只写入调用方提供的结构体。
    unsafe {
        let mut st = std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>();
        GetLocalTime(&mut st);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
        )
    }
}
