//! 浮层模块：layout。

use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, PtInRect, UpdateWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer};


use crate::log;
use crate::state::{BlurMode, state};

use super::{TIMER_GROUP_FOLD, FOLD_MS, rebuild_layout};

use super::input::ensure_selected_visible;

#[derive(Clone)]
pub struct Header {
    pub group: usize,
    pub rect: RECT,
    pub count: usize,
}

#[derive(Clone, Default)]
pub struct Layout {
    pub scale: f32,
    pub dpi: u32,
    pub icon: i32,
    pub cell_w: i32,
    pub cell_h: i32,
    pub cols: usize,
    pub width: i32,
    pub height: i32,
    pub tiles: Vec<RECT>,
    pub search_order: Vec<usize>,
    pub headers: Vec<Header>,
    /// 全部内容摆完后的纵向高度（含顶部工具条留白），比面板高时要滚动。
    pub content_height: i32,
    /// 「最近常用」置顶横条及内部卡片矩形（内容坐标系）。
    pub recent_bar: RECT,
    pub recent_tiles: Vec<(usize, RECT)>,
}

#[derive(Clone, Copy)]
pub(crate) struct FoldSample {
    pub(super) at: Instant,
    pub(super) paint: Duration,
    pub(super) backdrop: Duration,
    pub(super) items: Duration,
}

/// 动画只保存两端布局。绘制与鼠标命中共用当前帧的矩形。
pub struct GroupFold {
    pub(super) group: usize,
    pub(super) expanded: Layout,
    pub(super) collapsed: Layout,
    pub(super) start: Instant,
    pub(super) from: f32,
    pub(super) to: f32,
    pub(super) timing: Option<Vec<FoldSample>>,
}

impl GroupFold {
    pub(super) fn openness(&self, now: Instant) -> f32 {
        let distance = (self.from - self.to).abs();
        let elapsed = now.duration_since(self.start).as_secs_f32() * 1000.0;
        if distance == 0.0 || elapsed >= FOLD_MS * distance { return self.to; }
        let t = elapsed / (FOLD_MS * distance);
        self.from + (self.to - self.from) * t * t * (3.0 - 2.0 * t)
    }

    pub(super) fn endpoint(self) -> Layout {
        if self.to == 0.0 { self.collapsed } else { self.expanded }
    }

    pub(super) fn frame(&self, position: f32, layout: &mut Layout) {
        let mix = |a: i32, b: i32| a + ((b - a) as f32 * position).round() as i32;
        let rect = |a: RECT, b: RECT| RECT {
            left: mix(a.left, b.left), right: mix(a.right, b.right),
            top: mix(a.top, b.top), bottom: mix(a.bottom, b.bottom),
        };
        let closed = &self.collapsed;
        let open = &self.expanded;
        layout.content_height = mix(closed.content_height, open.content_height);
        layout.cell_w = mix(closed.cell_w, open.cell_w);
        layout.cell_h = mix(closed.cell_h, open.cell_h);
        layout.icon = mix(closed.icon, open.icon);
        for ((header, a), b) in layout.headers.iter_mut().zip(&closed.headers).zip(&open.headers) {
            header.rect = rect(a.rect, b.rect);
        }
        let top = layout.headers[self.group].rect.bottom;
        let bottom = layout.headers.get(self.group + 1).map_or(layout.content_height, |h| h.rect.top);
        for ((tile, a), b) in layout.tiles.iter_mut().zip(&closed.tiles).zip(&open.tiles) {
            if a.right > a.left || b.right <= b.left {
                *tile = rect(*a, *b);
            } else {
                // 从标题下缘逐行揭开，只允许命中已露出的区域。
                let offset = top - open.headers[self.group].rect.bottom;
                let y = b.top + offset;
                *tile = if y >= bottom { RECT::default() } else {
                    RECT { top: y, bottom: (b.bottom + offset).min(bottom), ..*b }
                };
            }
        }
    }
}

pub(super) fn finish_fold(app: &mut crate::state::App) {
    if let Some(fold) = app.fold.take() {
        unsafe { KillTimer(app.overlay, TIMER_GROUP_FOLD) };
        if let Some(samples) = &fold.timing {
            let mut gaps: Vec<_> = samples.windows(2).map(|w| w[1].at - w[0].at).collect();
            let mut costs: Vec<_> = samples.iter().map(|s| s.paint).collect();
            let mut backdrops: Vec<_> = samples.iter().map(|s| s.backdrop).collect();
            let mut items: Vec<_> = samples.iter().map(|s| s.items).collect();
            gaps.sort_unstable();
            costs.sort_unstable();
            backdrops.sort_unstable();
            items.sort_unstable();
            let ms = |v: &[Duration], pct: usize| v.get(v.len().saturating_sub(1) * pct / 100)
                .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
            log::info(&format!(
                "折叠绘制 frames={} interval_ms={:.2} worst_ms={:.2} paint_ms={:.2} backdrop_ms={:.2} items_ms={:.2}",
                samples.len(),
                ms(&gaps, 50),
                ms(&gaps, 100),
                ms(&costs, 50),
                ms(&backdrops, 50),
                ms(&items, 50)
            ));
        }
        app.layout = fold.endpoint();
        clamp_scroll(app);
        app.static_dirty = true;
    }
}

pub(super) fn group_motion_enabled(app: &crate::state::App) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION};
    let mut enabled = 1i32;
    unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut enabled as *mut i32).cast(), 0) };
    app.shown && enabled != 0 && app.search.is_empty()
        && (app.backdrop.is_some() || matches!(app.blur, BlurMode::Dim | BlurMode::Off))
}

pub(super) fn header_at(x: i32, y: i32) -> Option<usize> {
    let app = state();
    let point = POINT { x, y };
    if let Some((group, rect)) = sticky_header(app)
        // SAFETY: 矩形来自本模块布局。
        && unsafe { PtInRect(&rect, point) != 0 }
    {
        return Some(group);
    }
    let content = content_point(app, x, y);
    app.layout
        .headers
        .iter()
        // SAFETY: 同上。
        .find(|header| unsafe { PtInRect(&header.rect, content) != 0 })
        .map(|header| header.group)
}

/// 拖动标题时应该插到第几个位置（0..=普通分组数，入口组固定在最后）。
pub(super) fn header_insert_target(app: &crate::state::App, y: i32) -> usize {
    let limit = app
        .groups
        .iter()
        .position(|group| group.portal.is_some())
        .unwrap_or(app.layout.headers.len());
    let content_y = y + app.scroll;
    for (index, header) in app.layout.headers.iter().enumerate() {
        if index >= limit {
            break;
        }
        let middle = header.rect.top + (header.rect.bottom - header.rect.top) / 2;
        if content_y < middle {
            return index;
        }
    }
    limit
}

/// 拖动标题时的插入位置指示线（客户区坐标）。
pub(super) fn header_indicator_rect(app: &crate::state::App, target: usize) -> RECT {
    let y = app
        .layout
        .headers
        .get(target)
        .map(|header| header.rect.top)
        .or_else(|| app.layout.headers.last().map(|header| header.rect.bottom))
        .unwrap_or(app.layout.content_height);
    let y = (y - app.scroll).max(content_top(app));
    let inset = (18.0 * app.layout.scale) as i32;
    RECT {
        left: inset,
        top: y - 2,
        right: app.layout.width - inset,
        bottom: y + 2,
    }
}

/// 分节布局：每个分组一个标题行，下面是该组的图标网格（默认标准缩放）。
pub const RECENT_MAX: usize = 8;

/// 选出最近常用且当前可用的图标序号（按启动次数降序 → 最近时间降序 → 序号升序）。
pub fn select_recent_items(
    items: &[crate::icons::DesktopItem],
    usage: &std::collections::BTreeMap<String, crate::store::UsageEntry>,
    limit: usize,
) -> Vec<usize> {
    if limit == 0 {
        return Vec::new();
    }
    let mut scored: Vec<(u32, u64, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            if item.offline {
                return None;
            }
            let entry = usage.get(&crate::store::usage_key(&item.path))?;
            (entry.count > 0).then_some((entry.count, entry.last_used, index))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)).then_with(|| a.2.cmp(&b.2)));
    scored.into_iter().take(limit).map(|(_, _, idx)| idx).collect()
}

/// 在顶部内容区生成「最近常用」紧凑水平卡片行（不越出当前屏幕宽度）。
pub fn layout_recent_bar(
    indices: &[usize],
    width: i32,
    top: i32,
    scale: f32,
) -> (RECT, Vec<(usize, RECT)>) {
    if indices.is_empty() || width <= 0 {
        return (RECT::default(), Vec::new());
    }
    let margin = (18.0 * scale) as i32;
    let bar_h = (48.0 * scale) as i32;
    let bar = RECT {
        left: margin,
        top: top + (6.0 * scale) as i32,
        right: (width - margin).max(margin + 1),
        bottom: top + (6.0 * scale) as i32 + bar_h,
    };
    let label_w = (76.0 * scale) as i32;
    let gap = (8.0 * scale) as i32;
    let pad = (8.0 * scale) as i32;
    let avail_w = (bar.right - bar.left - label_w - pad * 2).max(0);
    let slot_w = (148.0 * scale) as i32;
    let cap = ((avail_w + gap) / (slot_w + gap).max(1)).max(0) as usize;
    let shown = indices.len().min(cap).min(RECENT_MAX);
    if shown == 0 {
        return (RECT::default(), Vec::new());
    }
    let start_x = bar.left + pad + label_w;
    let item_h = (36.0 * scale) as i32;
    let item_top = bar.top + (bar_h - item_h) / 2;
    let item_w = ((avail_w - gap * (shown as i32 - 1)) / shown as i32).min(slot_w);
    let tiles = indices
        .iter()
        .take(shown)
        .enumerate()
        .map(|(col, &idx)| {
            let left = start_x + col as i32 * (item_w + gap);
            (
                idx,
                RECT {
                    left,
                    top: item_top,
                    right: left + item_w,
                    bottom: item_top + item_h,
                },
            )
        })
        .collect();
    (bar, tiles)
}

#[allow(clippy::too_many_arguments)]
pub fn compute_layout_full(
    items: &[crate::icons::DesktopItem],
    groups: &[crate::groups::Group],
    usage: &std::collections::BTreeMap<String, crate::store::UsageEntry>,
    show_recent: bool,
    width: i32,
    height: i32,
    dpi: u32,
    top_dip: f32,
    density: f32,
) -> Layout {
    let scale = dpi.max(96) as f32 / 96.0;
    let recent = if show_recent {
        select_recent_items(items, usage, RECENT_MAX)
    } else {
        Vec::new()
    };
    let (recent_bar, recent_tiles) = if recent.is_empty() {
        (RECT::default(), Vec::new())
    } else {
        layout_recent_bar(&recent, width, (top_dip * scale) as i32, scale)
    };
    let effective_top = if recent_tiles.is_empty() {
        top_dip
    } else {
        top_dip + 58.0
    };
    let mut layout = compute_layout_scaled(items, groups, width, height, dpi, effective_top, density);
    layout.recent_bar = recent_bar;
    layout.recent_tiles = recent_tiles;
    layout
}

pub fn compute_layout(
    items: &[crate::icons::DesktopItem],
    groups: &[crate::groups::Group],
    width: i32,
    height: i32,
    dpi: u32,
    top_dip: f32,
) -> Layout {
    compute_layout_scaled(items, groups, width, height, dpi, top_dip, 1.0)
}

/// 支持按 `density`（紧凑/标准/偏大）缩放格子与图标的布局计算。
pub fn compute_layout_scaled(
    items: &[crate::icons::DesktopItem],
    groups: &[crate::groups::Group],
    width: i32,
    height: i32,
    dpi: u32,
    top_dip: f32,
    density: f32,
) -> Layout {
    let scale = dpi as f32 / 96.0;
    let factor = density.clamp(0.75, 1.35);
    let zero = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let mut layout = Layout {
        scale,
        dpi,
        icon: (48.0 * scale * factor).round() as i32,
        cell_w: (120.0 * scale * factor).round() as i32,
        cell_h: (128.0 * scale * factor).round() as i32,
        cols: 1,
        width,
        height,
        tiles: vec![zero; items.len() + 1],
        search_order: Vec::new(),
        headers: Vec::new(),
        content_height: 0,
        recent_bar: zero,
        recent_tiles: Vec::new(),
    };
    if groups.is_empty() {
        return layout;
    }
    // 按分组归集 item 序号；未归组的落到“其它”。
    let default_index = groups
        .iter()
        .position(|group| group.name == crate::groups::DEFAULT_GROUP)
        .unwrap_or(0);
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); groups.len()];
    for (index, item) in items.iter().enumerate() {
        let target = groups
            .iter()
            .position(|group| {
                group
                    .paths
                    .iter()
                    .any(|known| crate::icons::same_path(known, &item.path))
            })
            .unwrap_or(default_index);
        buckets[target].push(index);
    }
    // 「添加图标」按钮排进默认组的末尾。
    buckets[default_index].push(items.len());
    let widest = buckets.iter().map(Vec::len).max().unwrap_or(0).max(1);
    let aspect = width as f32 / height.max(1) as f32;
    let header_height = |scale: f32| (38.0 * scale) as i32;
    // 顶部要给工具条（低权限时还有警告横幅）让位。
    let top_inset = (top_dip * scale) as i32;
    // 当前列数下、按给定格子高度摆放需要多高（含标题行与组间距）。
    let estimate = |cols: usize, cell_h: i32| -> i32 {
        let mut total = top_inset;
        for (group, bucket) in groups.iter().zip(&buckets) {
            total += header_height(scale);
            if !group.collapsed && !bucket.is_empty() {
                let rows = bucket.len().div_ceil(cols.max(1)) as i32;
                total += rows * cell_h + (10.0 * scale) as i32;
            }
        }
        total
    };
    let cols_limit = |cell_w: i32| ((width - 80).max(cell_w) / cell_w).max(1) as usize;
    // 列数不会超过最大一组的项数——多出来的列只会把每组挤到左边。
    layout.cols = ((widest as f32 * aspect).sqrt().round() as usize)
        .clamp(1, cols_limit(layout.cell_w))
        .min(widest);
    // 格子最小按档位缩放兜底；再小名字就看不清了，剩下的交给滚动。
    let floor_factor = factor.clamp(0.8, 1.15);
    let floor_w = ((96.0 * scale * floor_factor) as i32).max(52).min((width - 80).max(56));
    let floor_h = ((104.0 * scale * floor_factor) as i32).max(60);
    // 第一步：放不下就等比缩小格子。
    for _ in 0..6 {
        let total = estimate(layout.cols, layout.cell_h);
        if total <= height - 30 || layout.cell_h <= floor_h {
            break;
        }
        let factor = ((height - 40) as f32 / total as f32).clamp(0.5, 0.92);
        layout.cell_w = ((layout.cell_w as f32 * factor).round() as i32).max(floor_w);
        layout.cell_h = ((layout.cell_h as f32 * factor).round() as i32).max(floor_h);
        layout.icon = ((layout.cell_h as f32 * 0.38).round() as i32).max(28);
    }
    // 第二步：多开几列（行数变少），尽量在一屏里放下。
    while estimate(layout.cols, layout.cell_h) > height - 30
        && layout.cols < cols_limit(layout.cell_w)
        && layout.cols < widest
    {
        layout.cols += 1;
    }
    // 逐组摆放。
    let mut y = top_inset;
    for (group_index, (group, bucket)) in groups.iter().zip(&buckets).enumerate() {
        let header = RECT {
            left: (18.0 * scale) as i32,
            top: y,
            right: width - (18.0 * scale) as i32,
            bottom: y + header_height(scale),
        };
        layout.headers.push(Header {
            group: group_index,
            rect: header,
            count: bucket.len().saturating_sub(usize::from(bucket.contains(&items.len()))),
        });
        y = header.bottom;
        if group.collapsed || bucket.is_empty() {
            continue;
        }
        let grid_w = layout.cols as i32 * layout.cell_w;
        let origin_x = (width - grid_w) / 2;
        for (row, chunk) in bucket.chunks(layout.cols).enumerate() {
            for (col, item_index) in chunk.iter().enumerate() {
                let left = origin_x + col as i32 * layout.cell_w;
                let top = y + row as i32 * layout.cell_h;
                layout.tiles[*item_index] = RECT {
                    left,
                    top,
                    right: left + layout.cell_w,
                    bottom: top + layout.cell_h,
                };
            }
        }
        y += bucket.len().div_ceil(layout.cols) as i32 * layout.cell_h + (10.0 * scale) as i32;
    }
    layout.content_height = y;
    layout
}

/// 内容坐标（layout 里的 rect）→ 客户区坐标：减去滚动量。
pub(super) fn view_rect(app: &crate::state::App, rect: RECT) -> RECT {
    RECT {
        left: rect.left,
        top: rect.top - app.scroll,
        right: rect.right,
        bottom: rect.bottom - app.scroll,
    }
}

/// 客户区坐标 → 内容坐标：加上滚动量。
pub(super) fn content_point(app: &crate::state::App, x: i32, y: i32) -> POINT {
    POINT {
        x,
        y: y + app.scroll,
    }
}

/// 内容区顶边（工具条 / 低权限横幅之下），滚动裁剪和落点判定都用它。
pub(super) fn content_top(app: &crate::state::App) -> i32 {
    let top_dip = if app.low_integrity { 88.0 } else { 48.0 };
    (top_dip * app.layout.scale) as i32
}

/// 还能往下滚多少像素。
pub(super) fn max_scroll(app: &crate::state::App) -> i32 {
    // 底部留出提示条的高度，滚到底时最后一行不会被提示条压住。
    let pad = (56.0 * app.layout.scale) as i32;
    (app.layout.content_height + pad - app.layout.height).max(0)
}

/// 布局/DPI 变化后把滚动量夹回合法范围。
pub(super) fn clamp_scroll(app: &mut crate::state::App) {
    app.scroll = app.scroll.clamp(0, max_scroll(app));
}

/// 当前应该吸顶的分组：标题已经滚出顶部、但这一组的内容还在可见区里。
pub(super) fn sticky_group(app: &crate::state::App) -> Option<usize> {
    let top = content_top(app) + app.scroll;
    for (index, header) in app.layout.headers.iter().enumerate() {
        let end = app
            .layout
            .headers
            .get(index + 1)
            .map(|next| next.rect.top)
            .unwrap_or(app.layout.content_height);
        if header.rect.top < top && end > top {
            return Some(header.group);
        }
    }
    None
}

/// 吸顶标题在客户区里的矩形（绘制与点击判定共用）。
pub(super) fn sticky_header(app: &crate::state::App) -> Option<(usize, RECT)> {
    let group = sticky_group(app)?;
    let header = app.layout.headers.iter().find(|header| header.group == group)?;
    let height = header.rect.bottom - header.rect.top;
    let top = content_top(app);
    Some((
        group,
        RECT {
            left: header.rect.left,
            top,
            right: header.rect.right,
            bottom: top + height,
        },
    ))
}

/// 拖入时鼠标所在的组：按分组标题行划分纵向区域，标题下面到下一个标题都算该组。
pub fn group_at_point(_x: i32, y: i32) -> Option<usize> {
    let app = state();
    // 换算到内容坐标后再按标题行划分。
    let y = y + app.scroll;
    if app.layout.headers.is_empty() || y < app.layout.headers[0].rect.top {
        return None;
    }
    app.layout
        .headers
        .iter()
        .rev()
        .find(|header| y >= header.rect.top)
        .map(|header| header.group)
}

/// 排障用：第一个分组标题行中心的屏幕坐标，--drop-test 拿它当落点。
pub fn first_group_screen_point() -> Option<(i32, i32)> {
    let app = state();
    let header = app.layout.headers.first()?;
    let mut point = POINT {
        x: (header.rect.left + header.rect.right) / 2,
        y: (header.rect.top + header.rect.bottom) / 2 - app.scroll,
    };
    // SAFETY: 句柄有效；只做坐标换算。
    let ok = unsafe { windows_sys::Win32::Graphics::Gdi::ClientToScreen(app.overlay, &mut point) };
    (ok != 0).then_some((point.x, point.y))
}

/// 一个分组占的整块区域：它自己的标题行到下一个标题行（或面板底部）。
pub(super) fn group_section_rect(app: &crate::state::App, group: usize) -> Option<RECT> {
    let index = app
        .layout
        .headers
        .iter()
        .position(|header| header.group == group)?;
    let header = app.layout.headers.get(index)?;
    let bottom = app
        .layout
        .headers
        .get(index + 1)
        .map(|next| next.rect.top)
        .unwrap_or(app.layout.content_height + (24.0 * app.layout.scale) as i32);
    if bottom <= header.rect.top {
        return None;
    }
    Some(RECT {
        left: header.rect.left,
        top: header.rect.top,
        right: header.rect.right,
        bottom,
    })
}

/// 首次使用的说明卡：只在没看过时出现，不拦任何操作。
pub(super) fn onboarding_rect(app: &crate::state::App) -> RECT {
    let scale = app.layout.scale;
    let width = ((560.0 * scale) as i32).min(app.layout.width - (60.0 * scale) as i32);
    let height = (152.0 * scale) as i32;
    let hint_band = (34.0 * scale) as i32 + (54.0 * scale) as i32;
    let bottom = app.layout.height - hint_band;
    let left = (app.layout.width - width) / 2;
    RECT {
        left,
        top: bottom - height,
        right: left + width,
        bottom,
    }
}

pub(super) fn onboarding_button_rect(app: &crate::state::App) -> RECT {
    let card = onboarding_rect(app);
    let scale = app.layout.scale;
    let width = (86.0 * scale) as i32;
    let height = (28.0 * scale) as i32;
    let margin = (16.0 * scale) as i32;
    RECT {
        left: card.right - width - margin,
        top: card.bottom - height - (12.0 * scale) as i32,
        right: card.right - margin,
        bottom: card.bottom - (12.0 * scale) as i32,
    }
}

pub(super) fn portal_up_rect(header_rect: RECT, scale: f32) -> RECT {
    let width = (86.0 * scale) as i32;
    let height = (24.0 * scale) as i32;
    let top = header_rect.top + (header_rect.bottom - header_rect.top - height) / 2;
    let right = (header_rect.right - (6.0 * scale) as i32).max(header_rect.left + width);
    RECT {
        left: right - width,
        top,
        right,
        bottom: top + height,
    }
}

/// 顶部工具条：整条横贯面板，高度随 DPI 缩放；低权限时让开警告横幅。
pub(super) fn toolbar_rect(app: &crate::state::App) -> RECT {
    let scale = app.layout.scale;
    let top = if app.low_integrity {
        (40.0 * scale) as i32
    } else {
        0
    };
    RECT {
        left: 0,
        top,
        right: app.layout.width,
        bottom: top + (46.0 * scale) as i32,
    }
}

/// 工具条左侧「设置」按钮。
pub(super) fn settings_chip_rect(app: &crate::state::App) -> RECT {
    let scale = app.layout.scale;
    let bar = toolbar_rect(app);
    let width = (96.0 * scale) as i32;
    let height = (32.0 * scale) as i32;
    let top = bar.top + (bar.bottom - bar.top - height) / 2;
    RECT {
        left: (14.0 * scale) as i32,
        top,
        right: (14.0 * scale) as i32 + width,
        bottom: top + height,
    }
}

/// 工具条左侧第二个按钮：新建分组。
pub(super) fn new_group_chip_rect(app: &crate::state::App) -> RECT {
    let scale = app.layout.scale;
    let bar = toolbar_rect(app);
    let height = (32.0 * scale) as i32;
    let top = bar.top + (bar.bottom - bar.top - height) / 2;
    let left = settings_chip_rect(app).right + (10.0 * scale) as i32;
    let width = (124.0 * scale) as i32;
    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// 工具条右侧「移除」按钮。
pub(super) fn remove_chip_rect(app: &crate::state::App) -> RECT {
    let scale = app.layout.scale;
    let bar = toolbar_rect(app);
    let width = (96.0 * scale) as i32;
    let height = (32.0 * scale) as i32;
    let top = bar.top + (bar.bottom - bar.top - height) / 2;
    let right = app.layout.width - (14.0 * scale) as i32;
    RECT {
        left: right - width,
        top,
        right,
        bottom: top + height,
    }
}

/// 工具条右侧第二个按钮：一键全部折叠/全部展开。
pub(super) fn fold_all_chip_rect(app: &crate::state::App) -> RECT {
    let scale = app.layout.scale;
    let bar = toolbar_rect(app);
    let width = (104.0 * scale) as i32;
    let height = (32.0 * scale) as i32;
    let top = bar.top + (bar.bottom - bar.top - height) / 2;
    let right = remove_chip_rect(app).left - (10.0 * scale) as i32;
    RECT {
        left: right - width,
        top,
        right,
        bottom: top + height,
    }
}

/// 画顶部工具条与两个按钮。
/// 工具条中间的搜索框：收起时只有一个放大镜按钮，展开动画到固定宽度。
pub(super) fn search_rect(app: &crate::state::App) -> RECT {
    let scale = app.layout.scale;
    let bar = toolbar_rect(app);
    let height = (32.0 * scale) as i32;
    let top = bar.top + (bar.bottom - bar.top - height) / 2;
    let left = new_group_chip_rect(app).right + (12.0 * scale) as i32;
    let collapsed = (36.0 * scale) as i32;
    let expanded = (360.0 * scale) as i32;
    let width = if app.search_open && app.search_anim.is_none() {
        expanded
    } else if let Some(started) = app.search_anim {
        let raw = (started.elapsed().as_secs_f32() / 0.16).clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - raw).powi(3);
        let to = if app.search_open { expanded } else { collapsed };
        let from = if app.search_open { collapsed } else { expanded };
        (from as f32 + (to - from) as f32 * eased) as i32
    } else {
        collapsed
    };
    RECT {
        left,
        top,
        right: left + width.max(collapsed / 2),
        bottom: top + height,
    }
}

/// 当前关键字下某一项是否显示。
pub(super) fn visible(app: &crate::state::App, index: usize) -> bool {
    if app.search.is_empty() {
        return true;
    }
    app.layout.tiles.get(index).is_some_and(|tile| tile.right > tile.left)
}

/// 画工具栏里的搜索框：收起时是放大镜，展开后直接在工具条上显示关键字（无底板）。

pub(super) fn count_matches(app: &crate::state::App) -> usize {
    if app.search.is_empty() { app.items.len() } else { app.layout.search_order.len() }
}

pub(super) fn rank_search_candidates(mut scored: Vec<(u32, u32, u64, usize)>) -> Vec<usize> {
    scored.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| left.3.cmp(&right.3))
    });
    scored.into_iter().map(|(_, _, _, index)| index).collect()
}

/// 搜索结果按名称、后缀或分组匹配得分排序；同分按使用次数和最近启动时间加权，再同则保持面板顺序。
pub(super) fn search_results(app: &crate::state::App) -> Vec<usize> {
    if app.search.is_empty() {
        return Vec::new();
    }
    let query = app.search.trim().to_lowercase();
    let group_scores: Vec<Option<u32>> = app
        .groups
        .iter()
        .map(|group| {
            crate::search::group_fallback_score(&crate::groups::display_name(&group.name), &query)
        })
        .collect();
    let scored: Vec<(u32, u32, u64, usize)> = app
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let name_score = crate::search::rank(item.search_keys(), &query);
            let ext_score = crate::search::match_extension(&item.path, &query);
            let group_score = crate::groups::group_of_or_default(&app.groups, &item.path)
                .and_then(|group_idx| group_scores.get(group_idx).copied().flatten());
            let score = [name_score, ext_score, group_score]
                .into_iter()
                .flatten()
                .max()?;
            let usage = app
                .usage
                .get(&crate::store::usage_key(&item.path))
                .copied()
                .unwrap_or_default();
            Some((score, usage.count, usage.last_used, index))
        })
        .collect();
    rank_search_candidates(scored)
}

/// 唤出后直接敲字母/数字：打开搜索并追加字符，不抢焦点也不触发输入法。
/// 搜索结果的紧凑网格：只算位置，方便单测。
pub(super) fn search_grid(width: i32, cell_w: i32, cell_h: i32, top: i32, count: usize) -> (Vec<RECT>, usize, i32) {
    // 列数按面板宽度算，不受上一次结果多少影响（否则删字后会卡在少列数）。
    let capacity = ((width - 80).max(cell_w) / cell_w).max(1) as usize;
    let cols = capacity.min(count.max(1)).max(1);
    let grid_w = cols as i32 * cell_w;
    let origin_x = (width - grid_w) / 2;
    let tiles = (0..count)
        .map(|slot| {
            let row = (slot / cols) as i32;
            let col = (slot % cols) as i32;
            let left = origin_x + col * cell_w;
            let y = top + row * cell_h;
            RECT {
                left,
                top: y,
                right: left + cell_w,
                bottom: y + cell_h,
            }
        })
        .collect();
    let content_height = top + count.div_ceil(cols) as i32 * cell_h;
    (tiles, cols, content_height)
}

/// 搜索态布局：只给命中项排格子，隐藏分组标题。
pub(super) fn search_layout(app: &crate::state::App, results: &[usize]) -> Layout {
    let mut layout = app.layout.clone();
    layout.tiles = vec![
        RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        app.items.len() + 1
    ];
    layout.headers.clear();
    layout.search_order = results.to_vec();
    let top = content_top(app);
    let (tiles, cols, content_height) =
        search_grid(layout.width, layout.cell_w, layout.cell_h, top, results.len());
    layout.cols = cols;
    layout.content_height = content_height;
    for (index, tile) in results.iter().zip(tiles) {
        layout.tiles[*index] = tile;
    }
    layout
}

/// 关键字变化后：换成紧凑结果布局、选中第一个结果，并重画一帧。
pub(super) fn apply_search(app: &mut crate::state::App) {
    finish_fold(app);
    if app.search.is_empty() {
        // 退格删空：恢复分组布局，不突然选中第一项。
        app.selected = None;
        rebuild_layout(app);
        return;
    }
    let results = search_results(app);
    let layout = search_layout(app, &results);
    app.layout = layout;
    app.scroll = 0;
    app.selected = results.first().copied();
    app.selection.clear();
    app.selection_anchor = None;
    if let Some(index) = app.selected {
        ensure_selected_visible(app, index);
    }
    app.static_dirty = true;
    if app.shown {
        // SAFETY: 句柄有效。
        unsafe {
            InvalidateRect(app.overlay, std::ptr::null(), 0);
            UpdateWindow(app.overlay);
        }
    }
}

/// 让面板临时接收键盘：去掉 NOACTIVATE 并抢到前台，搜索结束时再恢复。
/// 内容坐标里的 y 落在哪个分组区域内（从该组标题到下一组标题）。
pub(super) fn group_at_content(headers: &[Header], content_y: i32) -> Option<usize> {
    let mut result = None;
    for header in headers {
        if header.rect.top <= content_y {
            result = Some(header.group);
        } else {
            break;
        }
    }
    result
}

/// 指针（客户区坐标）落在哪个分组区域；只看位置，不排除源组。
pub(super) fn group_under_cursor(app: &crate::state::App, x: i32, y: i32) -> Option<usize> {
    // 工具条和内容区上方留白不算任何分组，避免往下拖经过工具条时误判。
    if y < content_top(app) {
        return None;
    }
    group_at_content(&app.layout.headers, content_point(app, x, y).y)
}

/// 落点判定的纯规则：内容坐标属于哪个分组，排除源组，再按 `accepts` 过滤只读入口。
pub(super) fn drop_group_at_content(
    headers: &[Header],
    content_y: i32,
    source: usize,
    accepts: impl Fn(usize) -> bool,
) -> Option<usize> {
    group_at_content(headers, content_y)
        .filter(|group| *group != source)
        .filter(|group| accepts(*group))
}

/// 内部拖动时可以落进去的分组：排除源组；文件夹入口是只读视图，不收。
pub(super) fn drag_drop_group(app: &crate::state::App, from: usize, x: i32, y: i32) -> Option<usize> {
    let item = app.items.get(from)?;
    let source = crate::groups::group_of_or_default(&app.groups, &item.path)?;
    if y < content_top(app) {
        return None;
    }
    let content_y = content_point(app, x, y).y;
    drop_group_at_content(&app.layout.headers, content_y, source, |group| {
        app.groups
            .get(group)
            .is_some_and(|entry| entry.portal.is_none())
    })
}

/// 目标分组标题行的填充矩形（上下各内缩 2px，给描边留位置）。
pub(super) fn drag_header_inner(app: &crate::state::App, group: usize) -> Option<RECT> {
    let header = app.layout.headers.iter().find(|header| header.group == group)?;
    let rect = view_rect(app, header.rect);
    Some(RECT {
        left: rect.left,
        top: rect.top + 2,
        right: rect.right,
        bottom: rect.bottom - 2,
    })
}

/// 按当前指针刷新拖动目标；滚动时指针没动，目标组也会变。
pub(super) fn refresh_drag_hover(app: &mut crate::state::App, from: usize, x: i32, y: i32) -> Option<usize> {
    let group = drag_drop_group(app, from, x, y);
    let rect = group.and_then(|group| drag_header_inner(app, group));
    if let Some(mut drag) = app.drag {
        drag.header = group;
        drag.header_rect = rect;
        app.drag = Some(drag);
    }
    group
}

pub(super) fn hit_test(x: i32, y: i32) -> Option<usize> {
    let app = state();
    // 工具条覆盖的内容被裁掉了，不算可见图标，避免点到看不见的项。
    if y < content_top(app) {
        return None;
    }
    // 格子存的是内容坐标，命中前要把鼠标位置换算到同一坐标系。
    let y = y + app.scroll;
    let index = app.layout.tiles.iter().position(|tile| {
        x >= tile.left && x < tile.right && y >= tile.top && y < tile.bottom
    })?;
    // 搜索时「添加图标」不参与命中，避免结果里混进一个无关的方块。
    if index == app.items.len() && !app.search.is_empty() {
        return None;
    }
    Some(index)
}

pub(super) fn hit_test_recent(app: &crate::state::App, x: i32, y: i32) -> Option<usize> {
    if !app.search.is_empty() || y < content_top(app) {
        return None;
    }
    let pt = content_point(app, x, y);
    app.layout
        .recent_tiles
        .iter()
        .find(|(_, rect)| pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom)
        .map(|&(idx, _)| idx)
}

pub(super) fn in_recent_bar(app: &crate::state::App, x: i32, y: i32) -> bool {
    if !app.search.is_empty() || y < content_top(app) || app.layout.recent_tiles.is_empty() {
        return false;
    }
    let pt = content_point(app, x, y);
    let r = app.layout.recent_bar;
    pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom
}

/// 将任意两点规范化为左上到右下有序矩形。
pub fn normalized_rect(a: (i32, i32), b: (i32, i32)) -> RECT {
    RECT {
        left: a.0.min(b.0),
        top: a.1.min(b.1),
        right: a.0.max(b.0),
        bottom: a.1.max(b.1),
    }
}

/// 计算内容区拉框矩形命中的所有有效图标下标（排除末尾「添加图标」格）。
pub fn marquee_hit_items(tiles: &[RECT], items_len: usize, rect: RECT) -> Vec<usize> {
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return Vec::new();
    }
    tiles
        .iter()
        .take(items_len)
        .enumerate()
        .filter(|(_, tile)| {
            tile.right > tile.left
                && tile.bottom > tile.top
                && tile.left < rect.right
                && tile.right > rect.left
                && tile.top < rect.bottom
                && tile.bottom > rect.top
        })
        .map(|(index, _)| index)
        .collect()
}

/// 在「最近常用」栏与分组网格之间做方向键导航，返回 `(recent_selected, grid_selected)`。
pub fn step_recent_nav(
    recent_len: usize,
    cols: usize,
    items_len: usize,
    recent_selected: Option<usize>,
    selected: Option<usize>,
    dx: i32,
    dy: i32,
) -> (Option<usize>, Option<usize>) {
    if items_len == 0 {
        return (None, None);
    }
    let cols = cols.max(1) as i32;
    let total = items_len as i32;
    if recent_len > 0 {
        if let Some(slot) = recent_selected {
            if dy > 0 {
                return (None, Some(0));
            }
            if dy < 0 {
                return (Some(slot.min(recent_len - 1)), None);
            }
            let next_slot = (slot as i32 + dx).clamp(0, recent_len as i32 - 1) as usize;
            return (Some(next_slot), None);
        }
        if dy < 0 && selected.is_none_or(|s| s < cols as usize) {
            return (Some(0), None);
        }
    }
    let base = selected.unwrap_or(0) as i32;
    let next = (base + dx + dy * cols).clamp(0, total - 1) as usize;
    (None, Some(next))
}

#[cfg(test)]
mod layout_tests {

    #[test]
    fn ctrl_click_keeps_keyboard_focus_inside_remaining_selection() {
        assert_eq!(super::super::input::selection_focus(&[1, 3], 1), Some(1));
        assert_eq!(super::super::input::selection_focus(&[1], 3), Some(1));
        assert_eq!(super::super::input::selection_focus(&[], 1), None);
    }

    #[test]
    fn candidates_receive_keys_before_search_commands() {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        for key in [VK_RETURN, VK_ESCAPE, VK_UP, VK_DOWN] {
            assert!(!super::super::input::is_search_command(key, true));
            assert!(super::super::input::is_search_command(key, false));
        }
        for key in [VK_PROCESSKEY, VK_SPACE, VK_LEFT, VK_RIGHT, VK_BACK, VK_DELETE] {
            assert!(!super::super::input::is_search_command(key, false));
        }
        assert!(crate::search::matches("企业微信", "微信"));
    }

    #[test]
    fn group_at_content_follows_section_boundaries() {
        let headers = vec![
            Header {
                group: 0,
                rect: RECT { left: 0, top: 100, right: 100, bottom: 130 },
                count: 2,
            },
            Header {
                group: 1,
                rect: RECT { left: 0, top: 300, right: 100, bottom: 330 },
                count: 1,
            },
        ];
        // 第一组标题之前：还不属于任何组
        assert_eq!(group_at_content(&headers, 50), None);
        // 第一组区域内（标题下、第二组标题之前）
        assert_eq!(group_at_content(&headers, 200), Some(0));
        // 正好在第二组标题上
        assert_eq!(group_at_content(&headers, 300), Some(1));
        // 最后一组之后的所有内容都算最后一组
        assert_eq!(group_at_content(&headers, 900), Some(1));
    }

    #[test]
    fn drop_group_excludes_source_and_read_only_entries() {
        let headers = vec![
            Header {
                group: 0,
                rect: RECT { left: 0, top: 100, right: 100, bottom: 130 },
                count: 2,
            },
            Header {
                group: 1,
                rect: RECT { left: 0, top: 300, right: 100, bottom: 330 },
                count: 1,
            },
            Header {
                group: 2,
                rect: RECT { left: 0, top: 500, right: 100, bottom: 530 },
                count: 1,
            },
        ];
        // 源组是 0 时，落在 0 的区域里不算移入；落在 1 里才算。
        assert_eq!(drop_group_at_content(&headers, 200, 0, |_| true), None);
        assert_eq!(drop_group_at_content(&headers, 320, 0, |_| true), Some(1));
        // 组 2 是只读文件夹入口：即使指针在里面也不接收。
        assert_eq!(drop_group_at_content(&headers, 600, 0, |group| group != 2), None);
        // 最后一组之后的内容仍算最后一组。
        assert_eq!(drop_group_at_content(&headers, 900, 1, |_| true), Some(2));
    }

    use super::*;
    use crate::groups::Group;
    use crate::icons::{DesktopItem, ItemSource};
    use std::path::PathBuf;

    fn item(name: &str) -> DesktopItem {
        DesktopItem {
            search_keys: Default::default(),
            offline: false,
            path: PathBuf::from(format!("C:\\桌面\\{name}")),
            name: name.to_string(),
            // 布局不碰图标句柄，空句柄足够。
            icon: std::ptr::null_mut(),
            source: ItemSource::Desktop,
            custom_icon: None,
        }
    }

    /// 造 n 个组、把 items 均分进去，最后一组用默认组名接住「添加图标」。
    fn scene(items_n: usize, groups_n: usize) -> (Vec<DesktopItem>, Vec<Group>) {
        let items: Vec<_> = (0..items_n).map(|i| item(&format!("i{i}"))).collect();
        let mut groups: Vec<Group> = (0..groups_n.saturating_sub(1))
            .map(|i| Group::new(format!("组{i}")))
            .collect();
        groups.push(Group::new(crate::groups::DEFAULT_GROUP));
        for (index, item) in items.iter().enumerate() {
            groups[index % groups_n].paths.push(item.path.clone());
        }
        (items, groups)
    }

    #[test]
    fn fold_clips_hit_rects_and_moves_the_next_header() {
        let (items, mut groups) = scene(12, 2);
        let expanded = compute_layout(&items, &groups, 1280, 800, 96, 48.0);
        groups[0].collapsed = true;
        let collapsed = compute_layout(&items, &groups, 1280, 800, 96, 48.0);
        let now = std::time::Instant::now();
        let mut layout = expanded.clone();
        let mut fold = super::GroupFold { group: 0, expanded, collapsed, start: now, from: 1.0, to: 0.0, timing: None };
        let mid = now + std::time::Duration::from_millis(110);
        let position = fold.openness(mid);
        fold.frame(position, &mut layout);
        let header_y = layout.headers[1].rect.top;
        assert!(header_y > fold.collapsed.headers[1].rect.top && header_y < fold.expanded.headers[1].rect.top);
        for index in (0..items.len()).step_by(2) {
            assert!(layout.tiles[index].bottom <= header_y);
        }
        // 反向时从同一帧继续，完成后返回精确的终点布局。
        fold.from = position;
        fold.start = mid;
        fold.to = 1.0;
        assert_eq!(fold.openness(mid), position);
        assert_eq!(fold.openness(mid + std::time::Duration::from_millis(220)), 1.0);
        fold.frame(0.0, &mut layout);
        assert_eq!(layout.tiles[0].right, 0);
        let final_right = fold.expanded.tiles[0].right;
        assert!(final_right > 0);
        assert_eq!(fold.endpoint().tiles[0].right, final_right);
    }

    #[test]
    fn search_keys_follow_names_across_clones_and_renames() {
        let mut entry = item("重庆");
        assert!(crate::search::rank(entry.search_keys(), "chongqing").is_some());
        let copy = entry.clone();
        assert!(std::ptr::eq(copy.search_keys(), entry.search_keys()));
        entry.set_name("微信");
        assert!(crate::search::rank(entry.search_keys(), "chongqing").is_none());
        assert!(crate::search::rank(entry.search_keys(), "wx").is_some());
        assert!(crate::search::rank(copy.search_keys(), "cq").is_some());
    }

    fn check(width: i32, height: i32, groups_n: usize, items_n: usize) {
        let (items, groups) = scene(items_n, groups_n);
        let layout = compute_layout(&items, &groups, width, height, 96, 48.0);
        let label = format!("{width}x{height}, {groups_n} 组, {items_n} 项");
        assert_eq!(layout.tiles.len(), items_n + 1, "格子数量不对：{label}");
        // 横向必须始终放得下；纵向可以超出面板一屏，靠滚动看到（内容高度由 content_height 兜住）。
        for (index, tile) in layout.tiles.iter().enumerate() {
            if tile.right <= tile.left || tile.bottom <= tile.top {
                panic!("第 {index} 格没有排出来：{label}");
            }
            assert!(
                tile.left >= 0 && tile.right <= width,
                "第 {index} 格横向越界 {}..{}：{label}",
                tile.left,
                tile.right
            );
            assert!(
                tile.top >= 0 && tile.bottom <= layout.content_height,
                "第 {index} 格纵向越界 {}..{}（内容高 {}）：{label}",
                tile.top,
                tile.bottom,
                layout.content_height
            );
        }
        assert!(!layout.headers.is_empty(), "没有分组标题：{label}");
        assert!(
            layout.headers.last().expect("标题").rect.bottom <= layout.content_height,
            "最后一个标题行越界：{label}"
        );
        // 内容超过一屏时，必须能靠滚动看到最后一行：内容高度不能超过面板 + 最大滚动量。
        let pad = 24;
        let max_scroll = (layout.content_height + pad - height).max(0);
        assert!(
            layout.content_height + pad <= height + max_scroll,
            "滚动量覆盖不到内容底部：{label}"
        );
    }

    #[test]
    fn tiles_fit_inside_the_panel() {
        // 本机常见情况，以及小屏 + 多组 + 满额图标的极端情况。
        check(2560, 1440, 1, 84);
        check(1920, 1080, 3, 120);
        check(1366, 768, 6, 150);
        check(1280, 720, 8, 200);
    }
    #[test]
    fn search_grid_caps_columns_and_centers_rows() {
        // 一行放得下时：结果整体居中，列数等于结果数。
        let (tiles, cols, height) = search_grid(800, 100, 100, 60, 5);
        assert_eq!(cols, 5);
        assert_eq!(tiles[0].left, 150);
        assert_eq!(tiles[4].right, 650);
        assert_eq!(tiles[0].top, 60);
        assert_eq!(height, 160);
        // 超过一行时按宽度上限换行，第二行继续从居中起点排。
        let (tiles, cols, height) = search_grid(800, 100, 100, 60, 10);
        assert_eq!(cols, 7);
        assert_eq!(tiles[7].left, 50);
        assert_eq!(tiles[7].top, 160);
        assert_eq!(height, 260);
        // 0 个结果也要有合法列数，tiles 为空。
        let (tiles, cols, _) = search_grid(800, 100, 100, 60, 0);
        assert!(tiles.is_empty());
        assert_eq!(cols, 1);
    }

    #[test]
    fn search_results_prefer_higher_usage_on_tied_score() {
        // 匹配分同为 800 时，启动次数更多者排前；启动次数相同，最近使用时间更新者排前；
        // 匹配分更高者（900）即使未使用过，依然排在低匹配分（800）之前。
        let ordered = super::rank_search_candidates(vec![
            (800, 1, 100, 0),
            (800, 5, 50, 1),
            (800, 5, 200, 2),
            (900, 0, 0, 3),
        ]);
        assert_eq!(ordered, vec![3, 2, 1, 0]);
    }

    #[test]
    fn density_scales_tiles_monotonically() {
        let (items, groups) = scene(20, 2);
        let compact = super::compute_layout_scaled(
            &items,
            &groups,
            2560,
            1440,
            120,
            48.0,
            crate::settings::IconDensity::Compact.scale_factor(),
        );
        let normal = super::compute_layout_scaled(
            &items,
            &groups,
            2560,
            1440,
            120,
            48.0,
            crate::settings::IconDensity::Normal.scale_factor(),
        );
        let large = super::compute_layout_scaled(
            &items,
            &groups,
            2560,
            1440,
            120,
            48.0,
            crate::settings::IconDensity::Large.scale_factor(),
        );
        assert!(compact.cell_w < normal.cell_w && normal.cell_w < large.cell_w);
        assert!(compact.icon < normal.icon && normal.icon < large.icon);
        assert!(large.tiles[0].right <= 2560);
    }

    #[test]
    fn mixed_dpi_layouts_stay_within_monitor_bounds() {
        let (items, groups) = scene(48, 3);
        for (width, height, dpi) in [
            (1920, 1080, 96),
            (2560, 1440, 120),
            (3840, 2160, 144),
            (3840, 2160, 192),
        ] {
            let layout = compute_layout_scaled(&items, &groups, width, height, dpi, 48.0, 1.0);
            assert_eq!(layout.dpi, dpi);
            assert!(layout.cell_w > 0 && layout.cell_h > 0);
            for tile in &layout.tiles {
                assert!(tile.left >= 0 && tile.right <= width);
                assert!(tile.top >= 0 && tile.bottom <= layout.content_height);
            }
        }
    }

    #[test]
    fn recent_items_selection_ranks_and_caps() {
        let mut items: Vec<_> = (0..10).map(|i| item(&format!("app{i}"))).collect();
        items[1].offline = true;
        let mut usage = std::collections::BTreeMap::new();
        for (i, count, ts) in [
            (0usize, 5u32, 100u64),
            (1, 99, 999), // 离线应过滤
            (2, 5, 200),  // 与 0 同次但更近，排前
            (3, 10, 50),
            (4, 1, 10),
            (5, 2, 10),
            (6, 3, 10),
            (7, 4, 10),
            (8, 6, 10),
            (9, 7, 10),
        ] {
            usage.insert(
                crate::store::usage_key(&items[i].path),
                crate::store::UsageEntry { count, last_used: ts },
            );
        }
        let picked = super::select_recent_items(&items, &usage, super::RECENT_MAX);
        assert_eq!(picked, vec![3, 9, 8, 2, 0, 7, 6, 5]);
    }

    #[test]
    fn recent_bar_layout_fits_monitor_width() {
        let indices: Vec<_> = (0..8).collect();
        let (bar_wide, tiles_wide) = super::layout_recent_bar(&indices, 2560, 48, 1.0);
        assert_eq!(tiles_wide.len(), 8);
        assert!(tiles_wide.last().unwrap().1.right <= bar_wide.right);

        // 窄屏自动减少槽位，确保不越界也不重叠。
        let (bar_narrow, tiles_narrow) = super::layout_recent_bar(&indices, 800, 48, 1.0);
        assert!(tiles_narrow.len() < 8 && !tiles_narrow.is_empty());
        for pair in tiles_narrow.windows(2) {
            assert!(pair[0].1.right <= pair[1].1.left);
        }
        assert!(tiles_narrow.last().unwrap().1.right <= bar_narrow.right);
    }

    #[test]
    fn marquee_rect_normalizes_and_hits_overlapping_tiles() {
        let r = super::normalized_rect((300, 250), (100, 120));
        assert_eq!((r.left, r.top, r.right, r.bottom), (100, 120, 300, 250));

        let tiles = vec![
            RECT { left: 20, top: 20, right: 80, bottom: 80 },     // 不相交
            RECT { left: 90, top: 110, right: 160, bottom: 180 },  // 相交
            RECT { left: 250, top: 200, right: 340, bottom: 290 }, // 相交
            RECT { left: 120, top: 130, right: 180, bottom: 200 }, // 末尾添加按钮（应排除）
        ];
        assert_eq!(super::marquee_hit_items(&tiles, 3, r), vec![1, 2]);
    }

    #[test]
    fn recent_nav_moves_between_strip_and_grid() {
        // 网格首行按上箭头进入常用栏第 0 项
        assert_eq!(super::step_recent_nav(4, 5, 12, None, Some(2), 0, -1), (Some(0), None));
        // 常用栏内部左右切换，不越界
        assert_eq!(super::step_recent_nav(4, 5, 12, Some(0), None, 1, 0), (Some(1), None));
        assert_eq!(super::step_recent_nav(4, 5, 12, Some(3), None, 1, 0), (Some(3), None));
        // 常用栏按下箭头回到网格第 0 项
        assert_eq!(super::step_recent_nav(4, 5, 12, Some(2), None, 0, 1), (None, Some(0)));
        // 非首行按上箭头仍在网格内上移
        assert_eq!(super::step_recent_nav(4, 5, 12, None, Some(7), 0, -1), (None, Some(2)));
    }
}
