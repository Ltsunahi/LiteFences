//! 把图标和版本信息塞进 exe（Windows 资源段）。
//!
//! Rust 不会编译 .rc，这里直接找 Windows SDK 的 rc.exe：
//! `RC` 环境变量 → PATH → `%ProgramFiles(x86)%\Windows Kits\10\bin\@@<version>\x64\rc.exe`（取最高版本）。
//! 找不到就只打一条 warning，不阻断构建。

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=packaging/app.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest.join("packaging").join("app.ico");
    if !icon.exists() {
        println!("cargo:warning=缺少 packaging/app.ico，跳过资源");
        return;
    }
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| String::from("0.0.0"));
    let mut parts = [0u32; 4];
    for (index, part) in version.split('.').take(4).enumerate() {
        parts[index] = part.parse().unwrap_or(0);
    }

    // 用字符而不是转义序列拼 rc 文本，避免在不同平台上被转义处理。
    let q = char::from(34u8);
    let nl = char::from(10u8);
    let mut text = String::new();
    // 源文件是 UTF-8（里面要写中文），明确告诉 rc 用 65001 解析。
    text.push_str(&format!("#pragma code_page(65001){nl}"));
    text.push_str(&format!("1 ICON {q}{}{q}{nl}", double_backslashes(&icon.display().to_string())));
    text.push_str(&format!("1 VERSIONINFO{nl}"));
    let numbers = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);
    text.push_str(&format!("FILEVERSION {numbers}{nl}"));
    text.push_str(&format!("PRODUCTVERSION {numbers}{nl}"));
    text.push_str(&format!("FILEFLAGSMASK 0x3fL{nl}"));
    text.push_str(&format!("FILEFLAGS 0x0L{nl}"));
    text.push_str(&format!("FILEOS 0x40004L{nl}"));
    text.push_str(&format!("FILETYPE 0x1L{nl}"));
    text.push_str(&format!("FILESUBTYPE 0x0L{nl}"));
    text.push_str(&format!("BEGIN{nl}"));
    text.push_str(&format!("  BLOCK {q}StringFileInfo{q}{nl}"));
    text.push_str(&format!("  BEGIN{nl}"));
    text.push_str(&format!("    BLOCK {q}080404b0{q}{nl}"));
    text.push_str(&format!("    BEGIN{nl}"));
    for (key, value) in [
        ("CompanyName", String::from("Ltsunahi")),
        ("FileDescription", String::from("LiteFences 桌面启动器")),
        ("FileVersion", version.clone()),
        ("InternalName", String::from("litefences-desktop")),
        ("LegalCopyright", String::from("Copyright (c) 2026 Ltsunahi. MIT License.")),
        ("OriginalFilename", String::from("LiteFencesDesktop.exe")),
        ("ProductName", String::from("LiteFences")),
        ("ProductVersion", version.clone()),
        ("Comments", String::from("https://github.com/Ltsunahi/LiteFences")),
    ] {
        text.push_str(&format!("      VALUE {q}{key}{q}, {q}{value}{q}{nl}"));
    }
    text.push_str(&format!("    END{nl}"));
    text.push_str(&format!("  END{nl}"));
    text.push_str(&format!("  BLOCK {q}VarFileInfo{q}{nl}"));
    text.push_str(&format!("  BEGIN{nl}"));
    text.push_str(&format!("    VALUE {q}Translation{q}, 0x804, 1200{nl}"));
    text.push_str(&format!("  END{nl}"));
    text.push_str(&format!("END{nl}"));

    let rc_source = out_dir.join("litefences.rc");
    if let Err(error) = std::fs::write(&rc_source, text) {
        println!("cargo:warning=写 rc 文件失败：{error}");
        return;
    }

    let res = out_dir.join("litefences.res");
    if compile_with_rc(&rc_source, &res) || compile_with_windres(&rc_source, &res) {
        println!("cargo:rustc-link-arg={}", res.display());
    } else {
        println!("cargo:warning=找不到可用的 rc.exe 或 windres.exe，本次构建不带图标和版本信息");
    }
}

fn compile_with_rc(rc_source: &std::path::Path, res: &std::path::Path) -> bool {
    let Some(rc) = find_rc() else {
        return false;
    };
    Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(res)
        .arg(rc_source)
        .status()
        .is_ok_and(|s| s.success())
        && res.is_file()
}

fn compile_with_windres(rc_source: &std::path::Path, res: &std::path::Path) -> bool {
    let Some(windres) = find_windres() else {
        println!("cargo:warning=未找到 windres.exe");
        return false;
    };
    let output = Command::new(&windres)
        .args(["-c", "65001", "-i"])
        .arg(rc_source)
        .args(["-O", "res", "-o"])
        .arg(res)
        .output();
    match output {
        Ok(out) if out.status.success() && res.is_file() => true,
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            println!(
                "cargo:warning=windres 失败 ({}) {}: {}",
                windres.display(),
                out.status,
                err.trim()
            );
            false
        }
        Err(error) => {
            println!("cargo:warning=调用 windres 失败 ({}): {error}", windres.display());
            false
        }
    }
}

fn find_windres() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("WINDRES") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("windres.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// rc.exe 的字符串里，反斜杠会被当成转义（`\n` 变换行），所以路径里的反斜杠要写两遍。
fn double_backslashes(path: &str) -> String {
    let bs = char::from(92).to_string();
    path.replace(&bs, &(bs.clone() + &bs))
}

fn find_rc() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("RC") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(output) = Command::new("where").arg("rc.exe").output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(line) = stdout.lines().next() {
            let path = PathBuf::from(line.trim());
            if path.is_file() {
                return Some(path);
            }
        }
    }
    let kits = std::env::var("ProgramFiles(x86)").ok()?;
    let bins = PathBuf::from(kits).join("Windows Kits").join("10").join("bin");
    let mut best: Option<(Vec<u32>, PathBuf)> = None;
    for entry in std::fs::read_dir(&bins).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let candidate = entry.path().join("x64").join("rc.exe");
        if !candidate.is_file() {
            continue;
        }
        let version: Vec<u32> = name
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect();
        if best.as_ref().is_none_or(|(known, _)| &version > known) {
            best = Some((version, candidate));
        }
    }
    best.map(|(_, path)| path)
}
