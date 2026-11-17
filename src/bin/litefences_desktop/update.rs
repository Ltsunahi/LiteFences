//! 检查更新：后台用 WinHTTP 请求 GitHub Releases API，比对版本号后用托盘气泡提示。

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{GetLastError, HWND};
use windows_sys::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ACCESS_TYPE_NAMED_PROXY, WINHTTP_FLAG_SECURE,
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::log;
use crate::util::wide;

/// 后台检查完成后通知宿主窗口。
pub const WM_APP_UPDATE: u32 = WM_APP + 3;

const API_HOST: &str = "api.github.com";
const API_PATH: &str = "/repos/Ltsunahi/LiteFences/releases/latest";
const RELEASE_PAGE: &str = "https://github.com/Ltsunahi/LiteFences/releases/latest";

#[derive(Clone, Debug)]
pub enum CheckOutcome {
    UpToDate(String),
    Available { version: String, url: String },
    Failed(String),
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static RESULT: Mutex<Option<(bool, CheckOutcome)>> = Mutex::new(None);
static LAST_URL: Mutex<Option<String>> = Mutex::new(None);

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// 启动时自动检查（设置里可关）。
pub fn auto_check(host: HWND) {
    if !crate::state::state().check_updates {
        return;
    }
    check(host, false);
}

/// 手动或自动检查。同一时间只跑一个请求。
pub fn check(host: HWND, manual: bool) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        if manual {
            crate::tray::notify(host, "LiteFences", "正在检查更新…");
        }
        return;
    }
    let address = host as isize;
    std::thread::spawn(move || {
        let outcome = match fetch_latest() {
            Ok((tag, url)) => {
                let latest = tag.trim_start_matches(['v', 'V']).to_string();
                if is_newer(&latest, current_version()) {
                    CheckOutcome::Available {
                        version: latest,
                        url,
                    }
                } else {
                    CheckOutcome::UpToDate(latest)
                }
            }
            Err(error) => CheckOutcome::Failed(error),
        };
        RUNNING.store(false, Ordering::SeqCst);
        if let Ok(mut slot) = RESULT.lock() {
            *slot = Some((manual, outcome));
        }
        // SAFETY: 只投递一条自定义消息，跨线程 PostMessage 是允许的。
        unsafe { PostMessageW(address as HWND, WM_APP_UPDATE, 0, 0) };
    });
}

/// 排障：同步请求一次，返回 `(tag, html_url)`。
pub fn check_now() -> Result<(String, String), String> {
    fetch_latest()
}

/// UI 线程取走检查结果并提示用户。
pub fn handle_result(host: HWND) {
    let Some((manual, outcome)) = RESULT.lock().ok().and_then(|mut slot| slot.take()) else {
        return;
    };
    match outcome {
        CheckOutcome::Available { version, url } => {
            log::info(&format!(
                "发现新版本 {version}（当前 {}）：{url}",
                current_version()
            ));
            if let Ok(mut slot) = LAST_URL.lock() {
                *slot = Some(url);
            }
            let title = crate::i18n::t("发现新版本", "Update available");
            let text = if crate::i18n::is_en() {
                format!(
                    "LiteFences {version} is available (you have {}). Click to open the release page.",
                    current_version()
                )
            } else {
                format!(
                    "LiteFences {version} 可用（当前 {}）。点击气泡打开下载页。",
                    current_version()
                )
            };
            crate::tray::notify(host, title, &text);
        }
        CheckOutcome::UpToDate(version) => {
            log::info(&format!("已是最新版本 {version}"));
            if manual {
                let title = crate::i18n::t("已是最新版本", "Already up to date");
                crate::tray::notify(
                    host,
                    title,
                    &format!("LiteFences {version}"),
                );
            }
        }
        CheckOutcome::Failed(error) => {
            log::warn(&format!("检查更新失败：{error}"));
            if manual {
                crate::tray::notify(
                    host,
                    crate::i18n::t("检查更新失败", "Update check failed"),
                    &error,
                );
            }
        }
    }
}

/// 点击气泡时打开最近的 Release 页面。
pub fn open_latest() {
    let url = LAST_URL
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_else(|| RELEASE_PAGE.to_string());
    if let Err(error) = crate::file_actions::open_url(&url) {
        log::warn(&format!("打开下载页失败：{error}"));
    }
}

/// 简单版本比较：只比前三段数字，`v` 前缀和预发布后缀忽略。
fn is_newer(latest: &str, current: &str) -> bool {
    parse_version(latest) > parse_version(current)
}

fn parse_version(text: &str) -> (u32, u32, u32) {
    let core = text
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .next()
        .unwrap_or_default();
    let mut parts = core.split('.').map(|part| part.parse::<u32>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

fn fetch_latest() -> Result<(String, String), String> {
    match with_proxy(fetch_api) {
        Ok(result) => return Ok(result),
        Err(error) => {
            log::warn(&format!(
                "GitHub API 检查失败（{error}），改用 Releases 页面"
            ));
        }
    }
    with_proxy(fetch_release_page).map_err(|error| format!("API 与 Releases 页面均失败：{error}"))
}

/// 优先走系统静态代理，失败后再让 WinHTTP 自动发现代理。
fn with_proxy<T>(request: impl Fn(Option<&str>) -> Result<T, String>) -> Result<T, String> {
    if let Some(proxy) = proxy_from_registry() {
        match request(Some(proxy.as_str())) {
            Ok(value) => return Ok(value),
            Err(error) => {
                log::warn(&format!(
                    "通过系统代理请求失败（{error}），改用自动代理重试"
                ));
            }
        }
    }
    request(None)
}

fn fetch_api(proxy: Option<&str>) -> Result<(String, String), String> {
    let body = http_get(
        API_HOST,
        API_PATH,
        "Accept: application/vnd.github+json\r\nUser-Agent: LiteFences\r\n",
        proxy,
    )?;
    let tag = json_string(&body, "tag_name")
        .ok_or_else(|| format!("响应里没有 tag_name：{}", snippet(&body)))?;
    let url = json_string(&body, "html_url").unwrap_or_else(|| RELEASE_PAGE.to_string());
    Ok((tag, url))
}

fn fetch_release_page(proxy: Option<&str>) -> Result<(String, String), String> {
    let body = http_get(
        "github.com",
        "/Ltsunahi/LiteFences/releases/latest",
        "Accept: text/html\r\nUser-Agent: LiteFences\r\n",
        proxy,
    )?;
    let tag = tag_from_release_page(&body)
        .ok_or_else(|| format!("Releases 页面里没有版本标签：{}", snippet(&body)))?;
    Ok((tag, RELEASE_PAGE.to_string()))
}

fn tag_from_release_page(body: &str) -> Option<String> {
    let marker = "/Ltsunahi/LiteFences/releases/tag/";
    let rest = &body[body.find(marker)? + marker.len()..];
    let end = rest
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_'))
        .unwrap_or(rest.len());
    let tag = rest[..end].trim();
    if tag.is_empty() {
        None
    } else {
        Some(tag.to_string())
    }
}

/// 读取 WinINET 的静态代理设置。WinHTTP 默认不会使用这里的配置。
fn proxy_from_registry() -> Option<String> {
    let subkey = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings");
    let enabled_name = wide("ProxyEnable");
    let mut enabled = 0u32;
    let mut enabled_size = std::mem::size_of::<u32>() as u32;
    // SAFETY: 输出缓冲区大小与类型匹配；失败时保持 0。
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            enabled_name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut enabled as *mut u32).cast::<c_void>(),
            &mut enabled_size,
        )
    };
    if status != 0 || enabled == 0 {
        return None;
    }

    let server_name = wide("ProxyServer");
    let mut buffer = [0u16; 512];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    // SAFETY: 缓冲区长度以字节数传入，函数最多写入该长度。
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            server_name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast::<c_void>(),
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    let length = buffer.iter().position(|ch| *ch == 0).unwrap_or(buffer.len());
    let raw = String::from_utf16_lossy(&buffer[..length]).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    if !raw.contains('=') {
        return Some(raw);
    }
    let mut http = None;
    let mut https = None;
    for part in raw.split(';') {
        let Some((scheme, address)) = part.split_once('=') else {
            continue;
        };
        let address = address.trim();
        match scheme.trim().to_ascii_lowercase().as_str() {
            "https" => https = Some(address.to_string()),
            "http" => http = Some(address.to_string()),
            _ => {}
        }
    }
    https.or(http)
}

fn http_get(
    host_name: &str,
    path_text: &str,
    extra_headers: &str,
    proxy: Option<&str>,
) -> Result<String, String> {
    let agent = wide(&format!("LiteFences/{}", current_version()));
    let proxy_wide = proxy.map(wide);
    let (access_type, proxy_ptr) = match proxy_wide.as_ref() {
        Some(value) => (WINHTTP_ACCESS_TYPE_NAMED_PROXY, value.as_ptr()),
        None => (WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null()),
    };
    // SAFETY: 字符串以 0 结尾；句柄在函数末尾逐个关闭。
    let session = unsafe {
        WinHttpOpen(
            agent.as_ptr(),
            access_type,
            proxy_ptr,
            std::ptr::null(),
            0,
        )
    };
    if session.is_null() {
        return Err("WinHttpOpen 失败".to_string());
    }
    let result = (|| {
        // SAFETY: 会话句柄有效；超时单位毫秒。
        unsafe { WinHttpSetTimeouts(session, 4000, 4000, 4000, 6000) };
        let host = wide(host_name);
        // SAFETY: 主机名以 0 结尾；443 是 HTTPS 默认端口。
        let connect = unsafe { WinHttpConnect(session, host.as_ptr(), 443, 0) };
        if connect.is_null() {
            return Err("WinHttpConnect 失败".to_string());
        }
        let path = wide(path_text);
        let verb = wide("GET");
        // SAFETY: 路径/动词以 0 结尾；HTTPS 由 SECURE 标志启用。
        let request = unsafe {
            WinHttpOpenRequest(
                connect,
                verb.as_ptr(),
                path.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        };
        if request.is_null() {
            // SAFETY: 只关闭本函数创建的句柄。
            unsafe { WinHttpCloseHandle(connect) };
            return Err("WinHttpOpenRequest 失败".to_string());
        }
        let outcome = (|| {
            let headers = wide(extra_headers);
            // SAFETY: 头字符串以 0 结尾，长度为 -1（u32::MAX）表示自动计算。
            let sent = unsafe {
                WinHttpSendRequest(
                    request,
                    headers.as_ptr(),
                    u32::MAX,
                    std::ptr::null(),
                    0,
                    0,
                    0,
                )
            };
            if sent == 0 {
                // SAFETY: 无参数，读取本线程最近一次错误码。
                let code = unsafe { GetLastError() };
                return Err(format!("WinHttpSendRequest 失败 err={code}"));
            }
            // SAFETY: 请求句柄有效。
            if unsafe { WinHttpReceiveResponse(request, std::ptr::null_mut()) } == 0 {
                // SAFETY: 无参数，读取本线程最近一次错误码。
                let code = unsafe { GetLastError() };
                return Err(format!("WinHttpReceiveResponse 失败 err={code}"));
            }
            let mut body = Vec::new();
            loop {
                let mut available = 0u32;
                // SAFETY: 输出参数有效。
                if unsafe { WinHttpQueryDataAvailable(request, &mut available) } == 0 {
                    return Err("WinHttpQueryDataAvailable 失败".to_string());
                }
                if available == 0 {
                    break;
                }
                let mut chunk = vec![0u8; available.min(16 * 1024) as usize];
                let mut read = 0u32;
                // SAFETY: 缓冲区长度与传入长度一致。
                if unsafe {
                    WinHttpReadData(
                        request,
                        chunk.as_mut_ptr() as *mut c_void,
                        chunk.len() as u32,
                        &mut read,
                    )
                } == 0
                {
                    return Err("WinHttpReadData 失败".to_string());
                }
                if read == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..read as usize]);
                if body.len() > 256 * 1024 {
                    break;
                }
            }
            Ok(String::from_utf8_lossy(&body).into_owned())
        })();
        // SAFETY: 句柄由本函数创建，各关闭一次。
        unsafe {
            WinHttpCloseHandle(request);
            WinHttpCloseHandle(connect);
        }
        outcome
    })();
    // SAFETY: 会话句柄由本函数创建，只关闭一次。
    unsafe { WinHttpCloseHandle(session) };
    result
}

fn snippet(text: &str) -> String {
    text.chars().take(120).collect()
}

/// 从 JSON 里取一个顶层字符串字段（只处理本项目需要的简单结构）。
fn json_string(body: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\"");
    let index = body.find(&marker)? + marker.len();
    let rest = &body[index..];
    let rest = &rest[rest.find(':')? + 1..];
    let rest = &rest[rest.find('"')? + 1..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                '/' => out.push('/'),
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_numeric_versions() {
        assert!(is_newer("0.3.5", "0.3.4"));
        assert!(is_newer("v0.4.0", "0.3.9"));
        assert!(is_newer("0.3.10", "0.3.9"));
        assert!(!is_newer("0.3.4", "0.3.4"));
        assert!(!is_newer("0.3.3", "0.3.4"));
        assert!(!is_newer("0.3.4-beta", "0.3.4"));
    }

    #[test]
    fn parses_release_fields() {
        let body = r#"{"tag_name":"v0.3.5","html_url":"https:\/\/github.com\/x","draft":false}"#;
        assert_eq!(json_string(body, "tag_name").as_deref(), Some("v0.3.5"));
        assert_eq!(
            json_string(body, "html_url").as_deref(),
            Some("https://github.com/x")
        );
    }

    #[test]
    fn reads_tag_from_release_page() {
        let body = r#"<link rel="canonical" href="https://github.com/Ltsunahi/LiteFences/releases/tag/v0.3.5">"#;
        assert_eq!(tag_from_release_page(body).as_deref(), Some("v0.3.5"));
        assert!(tag_from_release_page("<html>no tag</html>").is_none());
    }
}
