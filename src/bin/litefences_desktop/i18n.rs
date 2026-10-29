//! 界面文案的中英双语。
//!
//! 做法很直接：每个用户可见的字符串就地写成 `t("中文", "English")`，
//! 语言由设置里的 `lang=auto|zh|en` 决定（auto 跟随系统 UI 语言）。
//! 日志仍然固定中文——那是给排障看的，不需要翻译。

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Zh,
    En,
}

const ZH: u8 = 0;
const EN: u8 = 1;
static CURRENT: AtomicU8 = AtomicU8::new(ZH);

pub fn lang() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == EN {
        Lang::En
    } else {
        Lang::Zh
    }
}

pub fn set(lang: Lang) {
    CURRENT.store(
        match lang {
            Lang::Zh => ZH,
            Lang::En => EN,
        },
        Ordering::Relaxed,
    );
}

pub fn is_en() -> bool {
    lang() == Lang::En
}

/// 取当前语言下的文案。
pub fn t(zh: &'static str, en: &'static str) -> &'static str {
    if is_en() { en } else { zh }
}

/// 把 `auto|zh|en` 解析成具体语言；auto 看系统 UI 语言（中文系用中文，其它用英文）。
pub fn resolve(setting: &str) -> Lang {
    match setting.trim().to_ascii_lowercase().as_str() {
        "zh" | "zh-cn" => Lang::Zh,
        "en" | "en-us" => Lang::En,
        _ => system_lang(),
    }
}

fn system_lang() -> Lang {
    // SAFETY: GetUserDefaultUILanguage 只返回一个整数。
    let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() } as u32;
    // 主语言 ID 0x04 = 中文（简体/繁体都算）。
    if (id & 0x3ff) == 0x04 {
        Lang::Zh
    } else {
        Lang::En
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_switches_with_language() {
        set(Lang::Zh);
        assert_eq!(t("设置", "Settings"), "设置");
        set(Lang::En);
        assert_eq!(t("设置", "Settings"), "Settings");
        set(Lang::Zh);
    }

    #[test]
    fn resolve_maps_known_values() {
        assert_eq!(resolve("zh"), Lang::Zh);
        assert_eq!(resolve("EN"), Lang::En);
        assert_eq!(resolve("en-us"), Lang::En);
        // auto / 未知值都走系统语言，不做断言（取决于机器），只要求能返回。
        let _ = resolve("auto");
        let _ = resolve("garbage");
    }
}
