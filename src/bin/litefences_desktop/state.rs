//! 全局状态。所有窗口过程、钩子都跑在同一个 UI 线程上，因此用单线程访问器。

use std::cell::UnsafeCell;
use std::sync::OnceLock;
use std::time::Instant;

use windows_sys::Win32::Foundation::HWND;

use crate::icons::DesktopItem;
use crate::overlay::Layout;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlurMode {
    /// 自己抓屏 + 模糊 + 压暗，画成不透明底板；文字最稳，默认。
    Frost,
    /// 交给系统亚克力（部分机器上透明窗口里的文字会被合成丢弃）。
    Auto,
    /// Win11 22H2+ 的 DWMWA_SYSTEMBACKDROP_TYPE。
    Backdrop,
    /// 传统的 ACCENT_ENABLE_ACRYLICBLURBEHIND。
    Accent,
    /// 兜底：不透明深色底，仍然可用但没有虚化。
    Dim,
    /// 完全透明（只在排障时使用）。
    Off,
}

impl BlurMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "frost" => Some(Self::Frost),
            "auto" => Some(Self::Auto),
            "backdrop" | "dwm" => Some(Self::Backdrop),
            "accent" | "acrylic" => Some(Self::Accent),
            "dim" | "opaque" => Some(Self::Dim),
            "off" | "none" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Frost => "frost",
            Self::Auto => "auto",
            Self::Backdrop => "backdrop",
            Self::Accent => "accent",
            Self::Dim => "dim",
            Self::Off => "off",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub blur: BlurMode,
    pub hide_desktop_icons: bool,
    pub self_test: bool,
    pub show_ms: Option<u32>,
    /// 配合 `--show-ms`：连续唤出几次，用于测量首次（冷）与后续（热）的耗时。
    pub cycles: u32,
    /// 允许在低完整性级别下继续运行（不弹出“改用正常权限重启”的提示）。
    pub allow_low_integrity: bool,
    /// 排障：直接对指定路径执行一次启动流程，然后退出。
    pub launch_test: Option<String>,
    /// 排障：把面板画面保存成指定 BMP 路径。
    pub capture: Option<String>,
    /// 命令行直接设置开机自启（`--autostart=on|off`），设置完退出。
    pub autostart: Option<bool>,
    /// 排障：启动后直接弹出某个界面（`--demo=newgroup`）。
    pub demo: Option<String>,
    /// 排障：把指定路径当成外部拖入投一次，验证拖放链路后退出。
    pub drop_test: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            blur: BlurMode::Auto,
            hide_desktop_icons: true,
            self_test: false,
            show_ms: None,
            cycles: 1,
            allow_low_integrity: false,
            launch_test: None,
            capture: None,
            autostart: None,
            demo: None,
            drop_test: None,
        }
    }
}

impl Options {
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let mut options = Self::default();
        for arg in args {
            if arg == "--self-test" {
                options.self_test = true;
            } else if arg == "--allow-low-integrity" {
                options.allow_low_integrity = true;
            } else if arg == "--no-hide-desktop-icons" {
                options.hide_desktop_icons = false;
            } else if let Some(value) = arg.strip_prefix("--blur=") {
                options.blur =
                    BlurMode::parse(value).ok_or_else(|| format!("未知虚化模式：{value}"))?;
            } else if let Some(value) = arg.strip_prefix("--launch-test=") {
                options.launch_test = Some(value.to_string());
            } else if let Some(value) = arg.strip_prefix("--capture=") {
                options.capture = Some(value.trim_matches('"').to_string());
            } else if let Some(value) = arg.strip_prefix("--autostart=") {
                options.autostart = match value {
                    "on" | "1" | "true" => Some(true),
                    "off" | "0" | "false" => Some(false),
                    other => return Err(format!("autostart 取值无效：{other}")),
                };
            } else if let Some(value) = arg.strip_prefix("--demo=") {
                options.demo = Some(value.to_string());
            } else if let Some(value) = arg.strip_prefix("--drop-test=") {
                options.drop_test = Some(value.trim_matches('"').to_string());
            } else if let Some(value) = arg.strip_prefix("--show-ms=") {
                let ms: u32 = value.parse().map_err(|_| "show-ms 取值无效".to_string())?;
                options.show_ms = Some(ms.clamp(100, 60_000));
            } else if let Some(value) = arg.strip_prefix("--cycles=") {
                let cycles: u32 = value.parse().map_err(|_| "cycles 取值无效".to_string())?;
                options.cycles = cycles.clamp(1, 20);
            } else if arg.starts_with("--") {
                return Err(format!("未知参数：{arg}"));
            }
        }
        Ok(options)
    }
}

/// 拖动排序的中间状态。
#[derive(Clone, Copy)]
pub struct DragState {
    pub from: usize,
    pub start: (i32, i32),
    pub current: (i32, i32),
    pub active: bool,
    /// 上次重绘时间，用来把拖动重绘节流。
    pub last_paint: Option<Instant>,
    /// 上一帧跟随图标占据的矩形，用于局部失效。
    pub ghost: Option<windows_sys::Win32::Foundation::RECT>,
    /// 上一帧高亮的目标格子序号。
    pub target: Option<usize>,
    /// 拖动悬停到的分组标题（松手就移入该组）。
    pub header: Option<usize>,
    /// 上一帧高亮的分组标题矩形。
    pub header_rect: Option<windows_sys::Win32::Foundation::RECT>,
}

/// 拖动分组标题调整顺序的中间状态。
#[derive(Clone, Copy)]
pub struct HeaderDragState {
    /// 被拖的分组序号。
    pub from: usize,
    pub start: (i32, i32),
    /// 超过阈值才算拖动，否则松开就是单击折叠。
    pub active: bool,
    /// 松手后会插入到的位置（0..=普通分组数）。
    pub target: Option<usize>,
}

pub struct App {
    pub host: HWND,
    pub overlay: HWND,
    pub options: Options,
    /// 实际生效的虚化方式（Auto 会被解析成具体实现）。
    pub blur: BlurMode,
    pub shown: bool,
    pub shown_at: Option<Instant>,
    /// 入场动画计时起点；None 表示动画已结束。
    pub anim_started: Option<Instant>,
    /// 唤出瞬间的整屏原图，入场动画第一帧拿它当底（收起后释放）。
    pub entrance: Option<crate::backdrop::ScreenBase>,
    /// 入场动画已画过的帧数（0 = 还没画）。
    pub anim_frames: u32,
    /// 当前已经叠到窗口上的不透明度，用来按目标值反算每帧增量。
    pub anim_alpha: f32,
    pub icons_hidden: bool,
    pub items: Vec<DesktopItem>,
    /// `items` 里前多少个是自动读到的桌面图标（其余是手动添加的）。
    pub desktop_count: usize,
    /// 用户手动添加的路径列表，持久化在数据目录。
    pub custom: Vec<std::path::PathBuf>,
    /// 面板布局（顺序 + 显示名），持久化在 layout.txt。
    pub layout_order: Vec<(std::path::PathBuf, String)>,
    /// 拖动排序状态。
    pub drag: Option<DragState>,
    /// 拖动分组标题调整顺序。
    pub header_drag: Option<HeaderDragState>,
    /// 拖动时暂时从静态帧里隐藏的项（让图标“浮起来”，避免重影）。
    pub drag_hidden: Option<usize>,
    /// 右键“待确认移除”的项与时间（面板内二次确认，不弹窗）。
    pub pending_remove: Option<(usize, Instant)>,
    /// 外部文件拖入时高亮的分组（Some = 松手会加进这个组）。
    pub drop_hover: Option<usize>,
    /// 拖入结果提示（文字 + 出现时间），显示在底部提示条里。
    pub drop_notice: Option<(String, Instant)>,
    /// 资源管理器重启后重新隐藏桌面图标的剩余重试次数。
    pub rehide_left: u32,
    /// 面板内容的纵向滚动量（0 = 顶部）。内容比屏幕高时才用得上。
    pub scroll: i32,
    /// 右上角「移除」开关是否开启：开启后单击图标即从面板移除。
    pub remove_mode: bool,
    /// 被手动隐藏的桌面图标。
    pub hidden: Vec<std::path::PathBuf>,
    /// 唤出热键（可在托盘菜单里改）。
    pub hotkey: crate::settings::Hotkey,
    /// 搜索关键字（空 = 不过滤）。
    pub search: String,
    /// 搜索栏是否展开。
    pub search_open: bool,
    /// 搜索栏展开/收起的动画起点。
    pub search_anim: Option<Instant>,
    /// 正在接收键盘输入（面板临时激活）。
    pub search_typing: bool,
    /// 缓存的界面字体（按缩放创建一次，收起面板时释放）。
    pub font: windows_sys::Win32::Graphics::Gdi::HFONT,
    pub font_scale: f32,
    /// 分组（顺序即显示顺序）。
    pub groups: Vec<crate::groups::Group>,
    /// 文件夹入口列表（持久化在 portals.txt）。
    pub portals: Vec<crate::portals::FolderPortal>,
    /// 上次重建入口内容时的目录版本号；没变化就不重扫（图标加载很慢）。
    pub portal_stamp: Option<u64>,
    /// 双缓冲：内存 DC 与位图，避免整屏重绘时闪。
    pub buffer_dc: windows_sys::Win32::Graphics::Gdi::HDC,
    pub buffer_bitmap: isize,
    pub buffer_width: i32,
    pub buffer_height: i32,
    /// 静态帧是否需要重画（底板 + 图标 + 文字）；拖动时只贴它，不重画。
    pub static_dirty: bool,
    /// 上次扫描时桌面目录的版本号；相同就复用图标缓存，避免每次唤出都重新取图标。
    pub desktop_stamp: Option<u64>,
    /// 面板的不透明磨砂底板（fx 模式不用）。
    pub backdrop: Option<crate::backdrop::Backdrop>,
    /// 后台图标扫描的结果通道。
    pub scan_rx: Option<std::sync::mpsc::Receiver<Vec<DesktopItem>>>,
    pub scan_pending: bool,
    pub layout: Layout,
    pub hover: Option<usize>,
    pub tray_installed: bool,
    /// 演示模式下还剩余的唤出次数。
    pub demo_cycles_left: u32,
    /// 以低完整性级别运行时为 true：托盘不可用，部分程序也无法启动。
    pub low_integrity: bool,
    pub mouse_hook: isize,
    pub keyboard_hook: isize,
}

struct Slot(UnsafeCell<App>);
// SAFETY: 只允许 UI 线程通过 `state()` 访问；钩子和窗口过程都在该线程上派发。
unsafe impl Sync for Slot {}
// SAFETY: 同上，状态从不跨线程传递。
unsafe impl Send for Slot {}

static SLOT: OnceLock<Slot> = OnceLock::new();

pub fn install(app: App) {
    let _ = SLOT.set(Slot(UnsafeCell::new(app)));
}

/// 取得唯一的可变状态。调用者必须在 UI 线程上，且不能重入。
#[allow(clippy::mut_from_ref)]
pub fn state() -> &'static mut App {
    let slot = SLOT.get().expect("app state installed before use");
    // SAFETY: 见 `unsafe impl Sync` 的说明。
    unsafe { &mut *slot.0.get() }
}
