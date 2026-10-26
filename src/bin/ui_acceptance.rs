//! Isolated visible UI harness. Never opens the ordinary user data root.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(windows)]
#[allow(dead_code)] // Reuses production options; this harness deliberately bypasses CLI parsing.
#[path = "../app_options.rs"]
mod app_options;
#[cfg(windows)]
#[path = "../platform/mod.rs"]
mod platform;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    if std::env::args_os().len() != 1 {
        return std::process::ExitCode::from(2);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let artifacts = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("artifacts");
    if let Err(error) = std::fs::create_dir_all(&artifacts) {
        platform::show_error(&format!("无法准备界面测试目录：{error}"));
        return std::process::ExitCode::FAILURE;
    }
    let root = match resume_root(&artifacts) {
        Ok(Some(root)) => root,
        Ok(None) => {
            let root = artifacts.join(format!("ui-sky-{}-{stamp}", std::process::id()));
            if let Err(error) = std::fs::create_dir(&root) {
                platform::show_error(&format!("无法准备独立界面测试目录：{error}"));
                return std::process::ExitCode::FAILURE;
            }
            root
        }
        Err(error) => {
            platform::show_error(&format!("无法恢复项目内界面测试目录：{error}"));
            return std::process::ExitCode::FAILURE;
        }
    };
    let options = app_options::Options {
        smoke: false,
        data_dir: Some(root),
        storage_smoke: None,
    };
    match platform::run(options) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            platform::show_error(&error.to_string());
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
fn resume_root(artifacts: &std::path::Path) -> std::io::Result<Option<std::path::PathBuf>> {
    let marker = artifacts.join("ui-resume-request.txt");
    let name = match std::fs::read_to_string(marker) {
        Ok(name) => name,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let name = name.trim();
    let (pid, stamp) = name
        .strip_prefix("ui-sky-")
        .and_then(|suffix| suffix.split_once('-'))
        .ok_or_else(|| std::io::Error::other("测试目录名称无效"))?;
    if pid.is_empty()
        || stamp.is_empty()
        || !pid.bytes().all(|byte| byte.is_ascii_digit())
        || !stamp.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(std::io::Error::other("测试目录名称无效"));
    }
    let canonical_artifacts = std::fs::canonicalize(artifacts)?;
    let target = std::fs::canonicalize(artifacts.join(name))?;
    if target.parent() != Some(canonical_artifacts.as_path()) || !target.is_dir() {
        return Err(std::io::Error::other(
            "测试目录必须是项目 artifacts 的直接子目录",
        ));
    }
    Ok(Some(target))
}

#[cfg(not(windows))]
fn main() {
    eprintln!("LiteFences UI acceptance requires Windows 11 x64.");
    std::process::exit(1);
}
