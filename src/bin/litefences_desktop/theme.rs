//! 配色主题：一套颜色常量，面板的绘制都从这里取。
//!
//! 颜色都是 `COLORREF`（`0x00BBGGRR`），不是 RGB 顺序，写的时候别搞反。

#[derive(Clone, Copy)]
pub struct Theme {
    pub key: &'static str,
    pub name: &'static str,
    /// 英文名（界面语言为英文时用）。
    pub name_en: &'static str,
    /// 磨砂底板混入的颜色 (R, G, B)。
    pub tint: (u8, u8, u8),
    /// 混入强度：0 = 保持桌面原色，1 = 完全变成 tint 色。浅色主题需要压得更狠才读得清黑字。
    pub tint_strength: f32,
    /// 工具条底色。
    pub bar: u32,
    /// 按钮底色。
    pub chip: u32,
    /// 「移除中」这类强调按钮的底色。
    pub chip_hot: u32,
    /// 主文字（图标名、提示条）。
    pub text: u32,
    /// 次要文字（分组标题、搜索占位）。
    pub text_dim: u32,
    /// 分隔线。
    pub line: u32,
    /// 强调色（滚动条、选中、拖入高亮、插入线）。
    pub accent: u32,
    /// 图标名的阴影色：深色主题用深阴影，浅色主题用浅阴影。
    pub label_shadow: u32,
    /// 底部提示条底色。
    pub hint_bg: u32,
    /// 拖入高亮的填充/描边。
    pub drop_bg: u32,
    pub drop_line: u32,
    /// 受限权限警告横幅。
    pub warn_bg: u32,
    pub warn_text: u32,
}

impl Theme {
    /// 当前界面语言下的显示名。
    pub fn display_name(&self) -> &'static str {
        if crate::i18n::is_en() {
            self.name_en
        } else {
            self.name
        }
    }
}

pub const THEMES: &[Theme] = &[
    Theme {
        key: "space",
        name: "深空蓝",
        name_en: "Space blue",
        tint: (22, 24, 30),
        tint_strength: 0.45,
        bar: 0x0016_1616,
        chip: 0x0040_4040,
        chip_hot: 0x0030_30C0,
        text: 0x00E8_E8E8,
        text_dim: 0x00C8_C8C8,
        line: 0x0050_5050,
        accent: 0x00FF_A86F,
        label_shadow: 0x0010_1010,
        hint_bg: 0x0020_2020,
        drop_bg: 0x0018_2C18,
        drop_line: 0x0090_E090,
        warn_bg: 0x0028_3CB4,
        warn_text: 0x00FF_FFFF,
    },
    Theme {
        key: "graphite",
        name: "石墨灰",
        name_en: "Graphite",
        tint: (26, 26, 28),
        tint_strength: 0.45,
        bar: 0x0018_1818,
        chip: 0x0044_4444,
        chip_hot: 0x0032_36B4,
        text: 0x00F0_F0F0,
        text_dim: 0x00C0_C0C0,
        line: 0x0058_5858,
        accent: 0x00D8_C4A8,
        label_shadow: 0x0012_1212,
        hint_bg: 0x0024_2424,
        drop_bg: 0x0022_2A22,
        drop_line: 0x00C8_E0C8,
        warn_bg: 0x0028_3CB4,
        warn_text: 0x00FF_FFFF,
    },
    Theme {
        key: "midnight",
        name: "午夜紫",
        name_en: "Midnight purple",
        tint: (28, 20, 36),
        tint_strength: 0.55,
        bar: 0x0026_1218,
        chip: 0x004E_2C3A,
        chip_hot: 0x0038_30B4,
        text: 0x00F5_E8F0,
        text_dim: 0x00D6_BAC6,
        line: 0x006E_4658,
        accent: 0x00FF_89C4,
        label_shadow: 0x0014_0810,
        hint_bg: 0x0028_1824,
        drop_bg: 0x001C_2C18,
        drop_line: 0x0096_E196,
        warn_bg: 0x0028_3CB4,
        warn_text: 0x00FF_FFFF,
    },
    Theme {
        key: "forest",
        name: "森林绿",
        name_en: "Forest green",
        tint: (18, 30, 24),
        tint_strength: 0.55,
        bar: 0x0012_1A14,
        chip: 0x0032_4236,
        chip_hot: 0x0030_38B0,
        text: 0x00E8_F0E8,
        text_dim: 0x00C4_D0C4,
        line: 0x0050_6454,
        accent: 0x007F_D9A0,
        label_shadow: 0x0008_1008,
        hint_bg: 0x0018_2418,
        drop_bg: 0x0016_3018,
        drop_line: 0x0088_E890,
        warn_bg: 0x0028_3CB4,
        warn_text: 0x00FF_FFFF,
    },
    Theme {
        key: "sand",
        name: "暖砂（浅色）",
        name_en: "Warm sand (light)",
        tint: (214, 208, 198),
        tint_strength: 0.86,
        bar: 0x00EA_EEF0,
        chip: 0x00CC_D4D8,
        chip_hot: 0x0050_5AD6,
        text: 0x001C_1E20,
        text_dim: 0x0050_565A,
        line: 0x00A0_AAB0,
        accent: 0x00A5_6E3A,
        label_shadow: 0x00FA_FAFA,
        hint_bg: 0x00DA_E2E6,
        drop_bg: 0x00CE_E6CE,
        drop_line: 0x005A_8C3C,
        warn_bg: 0x005A_78D6,
        warn_text: 0x00FF_FFFF,
    },
];

/// 按 key 找主题；找不到就回默认的「深空蓝」。
pub fn by_key(key: &str) -> &'static Theme {
    THEMES
        .iter()
        .find(|theme| theme.key.eq_ignore_ascii_case(key.trim()))
        .unwrap_or(&THEMES[0])
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn default_theme() -> &'static Theme {
    &THEMES[0]
}

/// 在主题列表里的序号，用来给菜单打勾。
pub fn index_of(key: &str) -> usize {
    THEMES
        .iter()
        .position(|theme| theme.key.eq_ignore_ascii_case(key.trim()))
        .unwrap_or(0)
}

/// 把某个颜色按比例混向另一个颜色（用于渐变端点和悬停色）。
pub fn mix(from: u32, to: u32, ratio: f32) -> u32 {
    let ratio = ratio.clamp(0.0, 1.0);
    let channel = |shift: u32| {
        let a = ((from >> shift) & 0xff) as f32;
        let b = ((to >> shift) & 0xff) as f32;
        ((a + (b - a) * ratio) as u32 & 0xff) << shift
    };
    channel(0) | channel(8) | channel(16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_key_falls_back_to_default() {
        assert_eq!(by_key("space").key, "space");
        assert_eq!(by_key("  MIDNIGHT ").key, "midnight");
        assert_eq!(by_key("nope").key, default_theme().key);
    }

    #[test]
    fn mix_moves_towards_target() {
        // 0x00BBGGRR: 全 0 → 全 0xFF
        assert_eq!(mix(0x0000_0000, 0x00FF_FFFF, 0.0), 0x0000_0000);
        assert_eq!(mix(0x0000_0000, 0x00FF_FFFF, 1.0), 0x00FF_FFFF);
        let half = mix(0x0000_0000, 0x00FF_FFFF, 0.5);
        assert_eq!(half, 0x007F_7F7F);
    }

    #[test]
    fn every_theme_has_unique_key_and_name() {
        for (index, theme) in THEMES.iter().enumerate() {
            assert!(!theme.key.is_empty());
            assert!(!theme.name.is_empty());
            assert_eq!(index_of(theme.key), index);
        }
    }
}
