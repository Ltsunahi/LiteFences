//! 搜索匹配：名字子串 + 英文单词首字母 + 中文拼音首字母。
//!
//! 拼音首字母不依赖词库：GB2312 一级汉字按拼音排序，用 `WideCharToMultiByte(936)`
//! 拿到 GBK 编码后查声母边界即可。只覆盖常用汉字；生僻字按原字符处理。

use windows_sys::Win32::Globalization::WideCharToMultiByte;

/// GB2312 一级汉字里每个声母的起始 GBK 编码（按编码升序）。
const INITIAL_BOUNDS: [(u16, u8); 23] = [
    (0xB0A1, b'A'),
    (0xB0C5, b'B'),
    (0xB2C1, b'C'),
    (0xB4EE, b'D'),
    (0xB6EA, b'E'),
    (0xB7A2, b'F'),
    (0xB8C1, b'G'),
    (0xB9FE, b'H'),
    (0xBBF7, b'J'),
    (0xBFA6, b'K'),
    (0xC0AC, b'L'),
    (0xC2E8, b'M'),
    (0xC4C3, b'N'),
    (0xC5B6, b'O'),
    (0xC5BE, b'P'),
    (0xC6DA, b'Q'),
    (0xC8BB, b'R'),
    (0xC8F6, b'S'),
    (0xCBFD, b'T'),
    (0xCDDA, b'W'),
    (0xCEF4, b'X'),
    (0xD1B9, b'Y'),
    (0xD4D1, b'Z'),
];

/// 一级汉字的结束编码；再往后是按部首排序的生僻字，不能按拼音推断。
const INITIAL_END: u16 = 0xD7F9;

/// 单个汉字的拼音声母；非汉字或生僻字返回 None。
fn initial_of(ch: char) -> Option<char> {
    if !('\u{4e00}'..='\u{9fff}').contains(&ch) {
        return None;
    }
    let wide = [ch as u16];
    let mut bytes = [0u8; 2];
    // SAFETY: 输入是一个有效的 UTF-16 单元，输出缓冲正好 2 字节；失败时返回 0。
    let written = unsafe {
        WideCharToMultiByte(
            936,
            0,
            wide.as_ptr(),
            1,
            bytes.as_mut_ptr(),
            2,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    };
    if written != 2 {
        return None;
    }
    let code = u16::from_be_bytes(bytes);
    if !(INITIAL_BOUNDS[0].0..=INITIAL_END).contains(&code) {
        return None;
    }
    let mut letter = None;
    for (start, candidate) in INITIAL_BOUNDS {
        if code >= start {
            letter = Some(candidate as char);
        } else {
            break;
        }
    }
    letter
}

/// 搜索键：英文按单词取首字母，中文按每个字取拼音声母。
///
/// 例：`Visual Studio Code` → `vsc`，`百度网盘` → `bdwp`。
pub fn initials(text: &str) -> String {
    let mut key = String::new();
    let mut word_start = true;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            if word_start {
                key.push(ch.to_ascii_lowercase());
            }
            word_start = false;
        } else if let Some(letter) = initial_of(ch) {
            key.push(letter.to_ascii_lowercase());
            word_start = true;
        } else {
            // 空格、标点、生僻字都当作单词边界。
            word_start = true;
        }
    }
    key
}

/// 去掉空格和标点的紧凑形式，支持 `visualstudiocode` 这类连续输入。
fn compact(text: &str) -> String {
    text.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

/// 拼音键：汉字取声母，ASCII 原样保留，便于「百度 Netdisk」这类中英混排。
fn pinyin_key(text: &str) -> String {
    let mut key = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            key.push(ch.to_ascii_lowercase());
        } else if let Some(letter) = initial_of(ch) {
            key.push(letter.to_ascii_lowercase());
        }
    }
    key
}

/// 匹配得分：分数越高越靠前；None 表示不匹配。
///
/// 顺序：名字完全匹配 > 名字前缀 > 名字子串 > 紧凑/拼音/缩写。
pub fn score(name: &str, query: &str) -> Option<u32> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let lower = name.to_lowercase();
    if lower == query {
        return Some(1000);
    }
    if lower.starts_with(&query) {
        return Some(900);
    }
    if lower.contains(&query) {
        return Some(800);
    }
    // 紧凑/拼音/缩写只对 ASCII 关键字有意义，中文关键字走上面的原文子串。
    if !query.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return None;
    }
    let compact = compact(name);
    if compact == query {
        return Some(780);
    }
    if compact.starts_with(&query) {
        return Some(760);
    }
    let pinyin = pinyin_key(name);
    if pinyin == query {
        return Some(720);
    }
    if pinyin.starts_with(&query) {
        return Some(700);
    }
    if pinyin.contains(&query) {
        return Some(640);
    }
    let acronym = initials(name);
    if acronym == query {
        return Some(600);
    }
    if acronym.starts_with(&query) {
        return Some(580);
    }
    if compact.contains(&query) {
        return Some(540);
    }
    if acronym.contains(&query) {
        return Some(480);
    }
    None
}

/// 关键字是否命中某个名字。
pub fn matches(name: &str, query: &str) -> bool {
    score(name, query).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinyin_initials_of_common_names() {
        assert_eq!(initials("百度网盘"), "bdwp");
        assert_eq!(initials("微信"), "wx");
        assert_eq!(initials("Visual Studio Code"), "vsc");
        assert_eq!(initials("百度 Netdisk"), "bdn");
    }

    #[test]
    fn pinyin_exact_beats_incidental_substring() {
        let wechat = score("微信", "wx").unwrap();
        let long = score("LiveCaptionsTranslator-win-x64-withruntime_6.exe", "wx").unwrap();
        assert!(wechat > long, "微信 {wechat} 应该排在长文件名 {long} 前面");
    }

    #[test]
    fn matches_substring_acronym_and_pinyin() {
        assert!(matches("百度网盘", "网盘"));
        assert!(matches("百度网盘", "bdwp"));
        assert!(matches("百度网盘", "bd"));
        assert!(!matches("百度网盘", "wx"));
        assert!(matches("Visual Studio Code", "vsc"));
        assert!(matches("Visual Studio Code", "studio"));
        assert!(matches("Visual Studio Code", "visualstudiocode"));
        assert!(matches("任意名字", ""));
    }
}
