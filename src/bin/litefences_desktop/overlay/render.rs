//! 浮层模块：render。


use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{COLORREF, HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{AC_SRC_OVER, ANTIALIASED_QUALITY, AlphaBlend, BLENDFUNCTION, BeginPaint, BitBlt, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DT_CALCRECT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, DeleteDC, DeleteObject, DrawTextW, Ellipse, EndPaint, FF_DONTCARE, FillRect, GetDC, GetStockObject, HDC, HFONT, HOLLOW_BRUSH, IntersectClipRect, LineTo, MoveToEx, NULL_PEN, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, ReleaseDC, RestoreDC, RoundRect, SRCCOPY, SaveDC, SelectObject, SetBkMode, SetTextColor, TRANSPARENT};
use windows_sys::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx};


use crate::log;
use crate::state::{BlurMode, state};
use crate::util::{wide};

use super::{ANIM_MS};
use super::layout::{Header, FoldSample, view_rect, content_top, max_scroll, sticky_group, sticky_header, group_section_rect, header_indicator_rect, onboarding_rect, onboarding_button_rect, portal_up_rect, toolbar_rect, settings_chip_rect, new_group_chip_rect, remove_chip_rect, fold_all_chip_rect, search_rect, visible, count_matches, hit_test};

pub(super) fn anim_progress() -> f32 {
    let Some(started) = state().anim_started else {
        return 1.0;
    };
    let raw = (started.elapsed().as_secs_f32() / (ANIM_MS as f32 / 1000.0)).clamp(0.0, 1.0);
    // ease-out cubic
    1.0 - (1.0 - raw).powi(3)
}

pub(super) fn animating() -> bool {
    state()
        .anim_started
        .is_some_and(|started| started.elapsed() < Duration::from_millis(ANIM_MS))
}

pub(super) fn paint(hwnd: HWND) {
    // SAFETY: BeginPaint/EndPaint 成对，句柄来自 WM_PAINT。
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let app = state();
        let timing_on = app.fold.as_ref().is_some_and(|f| f.timing.is_some());
        let timed = timing_on.then(Instant::now);
        let mut split = (Duration::ZERO, Duration::ZERO);
        let width = app.layout.width;
        let height = app.layout.height;
        ensure_buffer(hdc, width, height);
        let cache = state().buffer_dc;
        let app = state();
        if cache.is_null() {
            split = render_static(hdc, app, width, height);
        } else {
            let redrawn = app.static_dirty;
            if redrawn {
                split = render_static(cache, app, width, height);
                app.static_dirty = false;
            }
            // 亚克力模式靠窗口自身的浓度做动画（下面 paint 会照常贴图），其它模式用位图叠加淡入。
            if app.anim_started.is_some() && app.blur != BlurMode::Accent {
                let frame_started = Instant::now();
                // 入场动画只做淡入，**不做缩放**：缩放版每帧要重采样整屏，实测一帧 39ms，
                // 240ms 的动画直接掉到 20fps。1:1 叠加是几毫秒的量级，才可能流畅。
                // 第一帧先把唤出瞬间的整屏原图贴上去：窗口新建时表面是空的，
                // 不垫这一下会先黑一帧（看起来就是“闪一下”）。只贴一次，之后靠叠加，
                // 每帧都贴原图会多花一次整屏拷贝（实测帧率从 25 帧掉到 12 帧）。
                if app.anim_frames == 0
                    && let Some(base) = app.entrance.as_ref()
                {
                    base.blit(hdc);
                }
                app.anim_frames = app.anim_frames.saturating_add(1);
                let target = anim_progress();
                // 每帧叠在“上一帧结果”上，按目标值反算增量，整体曲线才不跑偏。
                let current = app.anim_alpha.clamp(0.0, 0.999);
                let step = ((target - current) / (1.0 - current)).clamp(0.0, 1.0);
                app.anim_alpha = target;
                let alpha = (step * 255.0).clamp(0.0, 255.0) as u8;
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: alpha,
                    AlphaFormat: 0,
                };
                // 淡入 + 上滑：整帧先从下往上挪一点点再归位。挪动只做 1:1 平移（不重采样），
                // 顶部露出的那条用底板位图按同样透明度补齐，所以不会露真实桌面、也不会掉帧。
                let slide = if app.backdrop.is_some() {
                    ((1.0 - target) * (20.0 * app.layout.scale)).round() as i32
                } else {
                    0
                };
                if slide > 0 && slide < height {
                    if let Some(backdrop) = app.backdrop.as_ref() {
                        backdrop.blend_into(
                            hdc,
                            RECT {
                                left: 0,
                                top: 0,
                                right: width,
                                bottom: slide,
                            },
                            alpha,
                        );
                    }
                    // SAFETY: 两个 DC 有效；源与目标尺寸一致，只是纵向平移。
                    AlphaBlend(
                        hdc,
                        0,
                        slide,
                        width,
                        height - slide,
                        cache,
                        0,
                        0,
                        width,
                        height - slide,
                        blend,
                    );
                } else {
                    // SAFETY: 两个 DC 有效；源是整屏 32 位 DIB，目标 DC 来自 BeginPaint。
                    AlphaBlend(hdc, 0, 0, width, height, cache, 0, 0, width, height, blend);
                }
                let frame_ms = frame_started.elapsed().as_millis();
                if frame_ms > 24 {
                    log::warn(&format!(
                        "入场动画单帧偏慢：{frame_ms}ms（{width}x{height}）"
                    ));
                }
            } else if redrawn {
                // 内容刚重画过：必须整屏贴一次，否则窗口上会“一半新一半旧”。
                // SAFETY: 两个 DC 都有效。
                BitBlt(hdc, 0, 0, width, height, cache, 0, 0, SRCCOPY);
            } else {
                // 只贴失效区域：拖动时每帧就是几百 KB 的拷贝。
                let region = ps.rcPaint;
                let region_w = region.right - region.left;
                let region_h = region.bottom - region.top;
                if region_w > 0 && region_h > 0 {
                    // SAFETY: 两个 DC 都有效，源与目标区域一致。
                    BitBlt(
                        hdc,
                        region.left,
                        region.top,
                        region_w,
                        region_h,
                        cache,
                        region.left,
                        region.top,
                        SRCCOPY,
                    );
                }
            }
        }
        // 工具条每帧都画（不进静态缓存），搜索栏展开动画只需重画这一小块。
        let full = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        let font = ui_font(app);
        if !font.is_null() {
            // SAFETY: 字体在进程内缓存。
            SelectObject(hdc, font as _);
        }
        // 动画期间工具条已经在缓存里（跟整帧一起淡入），结束后才每帧单独画。
        if app.anim_started.is_none() {
            draw_toolbar(hdc, app, &full);
            draw_search_box(hdc, app, &full);
        }
        draw_drop_hover(hdc, app);
        draw_scroll_indicator(hdc, app);
        draw_onboarding(hdc, app);
        if let Some(header) = app.header_drag
            && header.active
            && let Some(target) = header.target
        {
            draw_header_indicator(hdc, app, target);
        }
        if app.drag.is_some_and(|drag| drag.active) {
            draw_drag(hdc, app, &full);
        }
        draw_empty_hint(hdc, app);
        draw_selection(hdc, app);
        draw_marquee(hdc, app);
        EndPaint(hwnd, &ps);
        if let Some(start) = timed && let Some(samples) = app.fold.as_mut().and_then(|f| f.timing.as_mut()) {
            windows_sys::Win32::Graphics::Gdi::GdiFlush();
            samples.push(FoldSample {
                at: start,
                paint: start.elapsed(),
                backdrop: split.0,
                items: split.1,
            });
        }
    }
}

/// 准备（或重建）与窗口同尺寸的后备位图。
pub(super) unsafe fn ensure_buffer(hdc: HDC, width: i32, height: i32) {
    let app = state();
    if !app.buffer_dc.is_null() && app.buffer_width == width && app.buffer_height == height {
        return;
    }
    // SAFETY: 只释放本模块创建的 GDI 对象。
    unsafe {
        if !app.buffer_dc.is_null() {
            DeleteDC(app.buffer_dc);
            app.buffer_dc = std::ptr::null_mut();
        }
        if app.buffer_bitmap != 0 {
            DeleteObject(app.buffer_bitmap as _);
            app.buffer_bitmap = 0;
        }
    }
    let header = windows_sys::Win32::Graphics::Gdi::BITMAPINFO {
        bmiHeader: windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER {
            biSize: std::mem::size_of::<windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: windows_sys::Win32::Graphics::Gdi::BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: 参数与 GDI 约定一致。
    let memory = unsafe { CreateCompatibleDC(hdc) };
    let bitmap = unsafe {
        CreateDIBSection(
            memory,
            &header,
            windows_sys::Win32::Graphics::Gdi::DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        )
    };
    if !memory.is_null() && !bitmap.is_null() && !bits.is_null() {
        // SAFETY: 位图选入内存 DC，供后续绘制。
        unsafe {
            SelectObject(memory, bitmap as _);
        }
        let app = state();
        app.buffer_dc = memory;
        app.buffer_bitmap = bitmap as isize;
        app.buffer_width = width;
        app.buffer_height = height;
        app.static_dirty = true;
    } else {
        // SAFETY: 创建失败时释放半成品。
        unsafe {
            if !memory.is_null() {
                DeleteDC(memory);
            }
            if !bitmap.is_null() {
                DeleteObject(bitmap as _);
            }
        }
    }
}

pub(super) fn draw_onboarding(hdc: HDC, app: &crate::state::App) {
    if app.onboarded {
        return;
    }
    let card = onboarding_rect(app);
    let scale = app.layout.scale;
    let theme = app.theme;
    // SAFETY: 画刷/字体用完即释放，DC 由 paint 提供。
    unsafe {
        let brush = CreateSolidBrush(theme.bar);
        FillRect(hdc, &card, brush);
        DeleteObject(brush as _);
        draw_outline(hdc, card, scale, theme.accent);
        SetBkMode(hdc, TRANSPARENT as i32);
        let mut title = RECT {
            left: card.left + (18.0 * scale) as i32,
            top: card.top + (12.0 * scale) as i32,
            right: card.right - (18.0 * scale) as i32,
            bottom: card.top + (40.0 * scale) as i32,
        };
        SetTextColor(hdc, theme.text);
        let text = wide(crate::i18n::t(
            "欢迎使用 LiteFences",
            "Welcome to LiteFences",
        ));
        DrawTextW(
            hdc,
            text.as_ptr(),
            -1,
            &mut title,
            DT_LEFT | DT_SINGLELINE | DT_NOPREFIX,
        );
        let font = create_font(scale, 13.0);
        let previous = if font.is_null() {
            std::ptr::null_mut()
        } else {
            SelectObject(hdc, font as _)
        };
        SetTextColor(hdc, theme.text_dim);
        let lines = [
            crate::i18n::t(
                "拖动图标换位置，按住分组标题可调整分组顺序",
                "Drag icons to reorder; drag a group title to move the group",
            ),
            crate::i18n::t(
                "从资源管理器拖文件到面板即可加入（只记录路径）",
                "Drop files from Explorer to add them (paths only)",
            ),
            crate::i18n::t(
                "单击空白处或按 Esc 收起面板",
                "Click empty space or press Esc to close",
            ),
        ];
        let mut y = card.top + (42.0 * scale) as i32;
        for line in lines {
            let mut rect = RECT {
                left: card.left + (18.0 * scale) as i32,
                top: y,
                right: card.right - (18.0 * scale) as i32,
                bottom: y + (20.0 * scale) as i32,
            };
            let line = wide(&format!("· {line}"));
            DrawTextW(
                hdc,
                line.as_ptr(),
                -1,
                &mut rect,
                DT_LEFT | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
            y += (21.0 * scale) as i32;
        }
        if !previous.is_null() {
            SelectObject(hdc, previous);
        }
        if !font.is_null() {
            DeleteObject(font as _);
        }
        draw_chip(
            hdc,
            onboarding_button_rect(app),
            crate::i18n::t("知道了", "Got it"),
            theme.accent,
            theme.bar,
            scale,
        );
    }
}

/// 画“静态帧”：底板 + 图标 + 文字。只在内容变化时调用。
pub(super) fn render_static(hdc: HDC, app: &mut crate::state::App, width: i32, height: i32) -> (Duration, Duration) {
    let timing_on = app.fold.as_ref().is_some_and(|f| f.timing.is_some());
    // SAFETY: 调用方保证 DC 有效。
    unsafe {
        let backdrop_start = timing_on.then(Instant::now);
        if let Some(backdrop) = app.backdrop.as_mut() {
            backdrop.draw(hdc, width, height);
        } else if app.blur == BlurMode::Dim || app.blur == BlurMode::Off {
            let brush = CreateSolidBrush(app.theme.bar as COLORREF);
            let full = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            FillRect(hdc, &full, brush);
            DeleteObject(brush as _);
        }
        let backdrop_cost = backdrop_start.map_or(Duration::ZERO, |t| t.elapsed());
        // 字体走进程内缓存，避免每次重画都 CreateFont。
        let font = ui_font(app);
        if !font.is_null() {
            // SAFETY: 字体在进程内缓存。
            SelectObject(hdc, font as _);
        }
        let full = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        draw_banner(hdc, app, &full);
        // 滚动时内容会移到工具条/横幅底下，这里把内容裁到可见区里。
        let top = content_top(app);
        let content_rect = RECT {
            left: 0,
            top,
            right: width,
            bottom: height,
        };
        let saved = SaveDC(hdc);
        IntersectClipRect(hdc, 0, top, width, height);
        draw_recent_strip(hdc, app, &content_rect);
        draw_headers(hdc, app, &content_rect);
        draw_group_accents(hdc, app, &content_rect);
        let items_start = timing_on.then(Instant::now);
        draw_items(hdc, app, &content_rect, font);
        let items_cost = items_start.map_or(Duration::ZERO, |t| t.elapsed());
        draw_search_empty(hdc, app);
        draw_sticky_header(hdc, app);
        RestoreDC(hdc, saved);
        draw_hint(hdc, app, &full);
        // 入场动画期间把工具条也画进缓存，好让它跟着整帧一起淡入。
        if app.anim_started.is_some() {
            draw_toolbar(hdc, app, &full);
            draw_search_box(hdc, app, &full);
        }
        (backdrop_cost, items_cost)
    }
}

pub(super) fn create_font(scale: f32, pixels: f32) -> HFONT {
    let font_name = wide("Microsoft YaHei UI");
    // SAFETY: 字体名以 0 结尾，负高度表示字符高度。
    unsafe {
        CreateFontW(
            -((pixels * scale).round() as i32),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            // 灰度抗锯齿在亚克力/分层场景下兼容性最好。
            ANTIALIASED_QUALITY as u32,
            FF_DONTCARE as u32,
            font_name.as_ptr(),
        )
    }
}

pub(super) fn intersects(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
}

/// 跟随图标占据的矩形（略放大，保证旧画面被完整覆盖）。
pub(super) fn ghost_rect(point: (i32, i32), icon: i32) -> RECT {
    let margin = icon / 2 + 6;
    RECT {
        left: point.0 - margin,
        top: point.1 - margin,
        right: point.0 + margin,
        bottom: point.1 + margin,
    }
}

/// 绘制顶部「最近常用」水平置顶栏。
fn draw_recent_strip(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    if !app.search.is_empty() || app.layout.recent_tiles.is_empty() {
        return;
    }
    let bar = view_rect(app, app.layout.recent_bar);
    if !intersects(&bar, paint) {
        return;
    }
    let scale = app.layout.scale;
    let radius = (8.0 * scale) as i32;
    let strip_bg = crate::theme::mix(app.theme.bar, app.theme.chip, 0.38);
    let chip_bg = crate::theme::mix(app.theme.chip, app.theme.bar, 0.25);
    // SAFETY: DC 有效，画刷、画笔与图标句柄均配对使用且用后即释放。
    unsafe {
        let brush = CreateSolidBrush(strip_bg);
        let pen = CreatePen(PS_SOLID as i32, 1, app.theme.line);
        let old_brush = SelectObject(hdc, brush as _);
        let old_pen = SelectObject(hdc, pen as _);
        RoundRect(hdc, bar.left, bar.top, bar.right, bar.bottom, radius, radius);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        DeleteObject(brush as _);
        DeleteObject(pen as _);

        let pad = (10.0 * scale) as i32;
        let label_w = (68.0 * scale) as i32;
        let mut title_rect = RECT {
            left: bar.left + pad,
            top: bar.top,
            right: (bar.left + pad + label_w).min(bar.right),
            bottom: bar.bottom,
        };
        let label = wide(crate::i18n::t("最近常用", "Recent"));
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, app.theme.text_dim);
        DrawTextW(
            hdc,
            label.as_ptr(),
            -1,
            &mut title_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );

        let item_radius = (6.0 * scale) as i32;
        let icon_size = (22.0 * scale).round().max(16.0) as i32;
        let card_brush = CreateSolidBrush(chip_bg);
        let card_pen = CreatePen(PS_SOLID as i32, 1, crate::theme::mix(app.theme.line, app.theme.bar, 0.4));
        for &(idx, raw_rect) in &app.layout.recent_tiles {
            let Some(item) = app.items.get(idx) else {
                continue;
            };
            let rect = view_rect(app, raw_rect);
            if !intersects(&rect, paint) {
                continue;
            }
            let ob = SelectObject(hdc, card_brush as _);
            let op = SelectObject(hdc, card_pen as _);
            RoundRect(hdc, rect.left, rect.top, rect.right, rect.bottom, item_radius, item_radius);
            SelectObject(hdc, ob);
            SelectObject(hdc, op);

            let icon_left = rect.left + (8.0 * scale) as i32;
            let icon_top = rect.top + (rect.bottom - rect.top - icon_size) / 2;
            if !item.icon.is_null() {
                DrawIconEx(
                    hdc,
                    icon_left,
                    icon_top,
                    item.icon,
                    icon_size,
                    icon_size,
                    0,
                    std::ptr::null_mut(),
                    DI_NORMAL,
                );
            }
            let mut text_rect = RECT {
                left: icon_left + icon_size + (6.0 * scale) as i32,
                top: rect.top,
                right: (rect.right - (6.0 * scale) as i32).max(icon_left + icon_size),
                bottom: rect.bottom,
            };
            let name = wide(&item.name);
            SetTextColor(hdc, app.theme.text);
            DrawTextW(
                hdc,
                name.as_ptr(),
                -1,
                &mut text_rect,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
        }
        DeleteObject(card_brush as _);
        DeleteObject(card_pen as _);
    }
}

/// 分组标题行：折叠箭头 + 组名 + 数量。滚动时，已经滚出顶部但内容还可见的那组吸顶。
pub(super) fn draw_headers(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    let scale = app.layout.scale;
    let sticky = sticky_group(app);
    for header in &app.layout.headers {
        if Some(header.group) == sticky {
            // 这一组改成吸顶绘制，原位置那份跳过，免得重复。
            continue;
        }
        let rect = view_rect(app, header.rect);
        if !intersects(&rect, paint) {
            continue;
        }
        draw_one_header(hdc, app, header, rect, scale);
    }
}

/// 为带色标的普通分组在内容区左边缘绘制竖直强调色线（随折叠动画裁剪）。
fn draw_group_accents(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    if !app.search.is_empty() {
        return;
    }
    let scale = app.layout.scale;
    let bar_w = (3.0 * scale).round().max(2.0) as i32;
    let headers = &app.layout.headers;
    for (pos, header) in headers.iter().enumerate() {
        let Some(group) = app.groups.get(header.group) else {
            continue;
        };
        if group.portal.is_some() {
            continue;
        }
        let Some(color) = group.color.as_deref().and_then(crate::theme::group_color_by_key) else {
            continue;
        };
        let section_top = header.rect.bottom;
        let section_bottom = headers
            .get(pos + 1)
            .map(|next| next.rect.top)
            .unwrap_or(app.layout.content_height);
        if section_bottom <= section_top + (4.0 * scale) as i32 {
            continue;
        }
        let rect = view_rect(
            app,
            RECT {
                left: header.rect.left,
                top: section_top + (2.0 * scale) as i32,
                right: header.rect.left + bar_w,
                bottom: section_bottom - (4.0 * scale) as i32,
            },
        );
        if rect.bottom <= rect.top || !intersects(&rect, paint) {
            continue;
        }
        // SAFETY: DC 有效，画刷用完即释放。
        unsafe {
            let brush = CreateSolidBrush(color);
            FillRect(hdc, &rect, brush);
            DeleteObject(brush as _);
        }
    }
}

/// 吸顶标题单独画：必须压在图标之上，否则滚过去的图标会盖住它。
pub(super) fn draw_sticky_header(hdc: HDC, app: &crate::state::App) {
    let Some(group) = sticky_group(app) else {
        return;
    };
    let Some(header) = app.layout.headers.iter().find(|h| h.group == group) else {
        return;
    };
    let scale = app.layout.scale;
    let rect = sticky_header(app).map(|(_, rect)| rect).unwrap_or(header.rect);
    // 吸顶条必须有底色，否则滚过去的图标会透到标题上。
    unsafe {
        let brush = CreateSolidBrush(app.theme.bar);
        FillRect(hdc, &rect, brush);
        DeleteObject(brush as _);
    }
    draw_one_header(hdc, app, header, rect, scale);
}

pub(super) fn draw_one_header(hdc: HDC, app: &crate::state::App, header: &Header, rect: RECT, scale: f32) {
    let Some(group) = app.groups.get(header.group) else {
        return;
    };
    let drilled = group
        .portal
        .and_then(|index| app.portals.get(index))
        .is_some_and(|portal| portal.is_drilled());
    // 搜索时标题显示命中数，而不是分组总数。
    let count = if app.search.is_empty() {
        header.count
    } else {
        app.items
            .iter()
            .enumerate()
            .filter(|(index, item)| {
                visible(app, *index)
                    && crate::groups::group_of_or_default(&app.groups, &item.path)
                        == Some(header.group)
            })
            .count()
    };
    let arrow = if group.collapsed { "▶" } else { "▼" };
    let status = if group.offline { crate::i18n::t(" · 暂不可用", " · Offline") } else { "" };
    let text = wide(&format!(
        "{arrow}  {}  ({count}){status}",
        crate::groups::display_name(&group.name)
    ));
    // SAFETY: 画笔与底色用完即释放。
    unsafe {
        let line = RECT {
            left: rect.left,
            top: rect.bottom - 1,
            right: rect.right,
            bottom: rect.bottom,
        };
        let brush = CreateSolidBrush(app.theme.line);
        FillRect(hdc, &line, brush);
        DeleteObject(brush as _);
        let group_color = group
            .portal
            .is_none()
            .then(|| group.color.as_deref().and_then(crate::theme::group_color_by_key))
            .flatten();
        let mut text_left = rect.left + (6.0 * scale) as i32;
        if let Some(color) = group_color {
            let bar_w = (4.0 * scale).round().max(2.0) as i32;
            let bar_pad = (7.0 * scale) as i32;
            let bar = RECT {
                left: rect.left,
                top: rect.top + bar_pad,
                right: rect.left + bar_w,
                bottom: (rect.bottom - bar_pad).max(rect.top + bar_pad + 2),
            };
            let color_brush = CreateSolidBrush(color);
            FillRect(hdc, &bar, color_brush);
            let dot = (8.0 * scale).round().max(6.0) as i32;
            let dot_left = bar.right + (6.0 * scale) as i32;
            let dot_top = rect.top + (rect.bottom - rect.top - dot) / 2;
            let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN as i32));
            let old_brush = SelectObject(hdc, color_brush as _);
            Ellipse(hdc, dot_left, dot_top, dot_left + dot, dot_top + dot);
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            DeleteObject(color_brush as _);
            text_left = dot_left + dot + (8.0 * scale) as i32;
        }
        let up_rect = drilled.then(|| portal_up_rect(rect, scale));
        let mut text_rect = RECT {
            left: text_left,
            top: rect.top,
            right: up_rect.map_or(rect.right, |r| r.left - (8.0 * scale) as i32),
            bottom: rect.bottom,
        };
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, app.theme.text_dim);
        DrawTextW(
            hdc,
            text.as_ptr(),
            -1,
            &mut text_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
        if let Some(up_rect) = up_rect {
            draw_chip(
                hdc,
                up_rect,
                crate::i18n::t("← 上一级", "← Up"),
                app.theme.chip,
                app.theme.text,
                scale,
            );
        }
    }
}

/// 搜索没有命中时，内容区给一句提示，避免只剩空背景。
pub(super) fn draw_search_empty(hdc: HDC, app: &crate::state::App) {
    if app.search.is_empty() || count_matches(app) > 0 {
        return;
    }
    let scale = app.layout.scale;
    let top = content_top(app);
    let center = top + (app.layout.height - top) / 2;
    let title = wide(crate::i18n::t("没有匹配的图标", "No matching icons"));
    let hint = wide(crate::i18n::t(
        "换个关键字，或按 Esc 清除搜索",
        "Try another keyword, or press Esc to clear",
    ));
    // SAFETY: DC 有效；字体用完即释放。
    unsafe {
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, app.theme.text);
        let mut title_rect = RECT {
            left: 0,
            top: center - (34.0 * scale) as i32,
            right: app.layout.width,
            bottom: center - (8.0 * scale) as i32,
        };
        DrawTextW(
            hdc,
            title.as_ptr(),
            -1,
            &mut title_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        let font = create_font(scale, 13.0);
        let previous = if font.is_null() {
            std::ptr::null_mut()
        } else {
            SelectObject(hdc, font as _)
        };
        SetTextColor(hdc, app.theme.text_dim);
        let mut hint_rect = RECT {
            left: 0,
            top: center,
            right: app.layout.width,
            bottom: center + (22.0 * scale) as i32,
        };
        DrawTextW(
            hdc,
            hint.as_ptr(),
            -1,
            &mut hint_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        if !previous.is_null() {
            SelectObject(hdc, previous);
        }
        if !font.is_null() {
            DeleteObject(font as _);
        }
    }
}

pub(super) fn draw_items(hdc: HDC, app: &mut crate::state::App, paint: &RECT, font: HFONT) {
    let scale = app.layout.scale;
    // 注：在玻璃/亚克力表面上无法“擦除”已画像素，所以不做悬停描边，
    // 避免移动鼠标后留下残影。
    let use_cache = (app.backdrop.is_some() || matches!(app.blur, BlurMode::Dim | BlurMode::Off))
        && !app.items.is_empty();
    let mut tile_cache = if use_cache {
        let spec = crate::tile_cache::TileSpec {
            cell_w: app.layout.cell_w,
            icon: app.layout.icon,
            scale_bits: scale.to_bits(),
            text_color: app.theme.text,
            shadow_color: app.theme.label_shadow,
        };
        let mut cached = app.tile_cache.take();
        let ready = match cached.as_mut() {
            Some(c) => c.prepare(hdc, spec, app.items.len(), app.fold.is_some()),
            None => {
                cached = crate::tile_cache::TileCache::new(hdc, spec, app.items.len());
                cached.is_some()
            }
        };
        if ready { cached } else { None }
    } else {
        None
    };

    // SAFETY: 所有 GDI 对象都在本函数内创建并在末尾释放。
    unsafe {
        SetBkMode(hdc, TRANSPARENT as i32);
        let tile_count = (app.items.len() + 1).min(app.layout.tiles.len());
        for index in 0..tile_count {
            let Some(raw) = app.layout.tiles.get(index).copied() else {
                continue;
            };
            let tile = view_rect(app, raw);
            if !intersects(&tile, paint) || tile.right <= tile.left {
                continue;
            }
            let icon = app.layout.icon;
            if index == app.items.len() {
                if !app.search.is_empty() {
                    continue;
                }
                let saved = clip_fold_tile(hdc, app, tile);
                draw_add_tile(hdc, tile, scale, icon, app.theme);
                if saved != 0 { RestoreDC(hdc, saved); }
                continue;
            }
            let item = &app.items[index];
            // 多选整体拖动时，所有被选中的图标都让位给跟随图标。
            if app.drag.is_some_and(|drag| drag.active)
                && app.selection.len() > 1
                && app.selection.contains(&index)
            {
                continue;
            }
            if app.drag_hidden == Some(index) || !visible(app, index) {
                // 拖动中的图标由跟随图层绘制；搜索时未命中的先留空。
                continue;
            }
            let saved = clip_fold_tile(hdc, app, tile);
            let label = if item.offline {
                Some(format!("{} · {}", crate::i18n::t("暂不可用", "Offline"), item.name))
            } else {
                None
            };
            let label_ref = label.as_deref().unwrap_or(item.name.as_str());
            let drew_cached = tile_cache.as_mut().is_some_and(|cache| {
                cache.draw_tile(
                    hdc,
                    index,
                    item.icon,
                    label_ref,
                    font,
                    tile,
                    app.layout.cell_w,
                    icon,
                    scale,
                )
            });
            if !drew_cached {
                if !item.icon.is_null() {
                    let x = tile.left + (app.layout.cell_w - icon) / 2;
                    let y = tile.top + ((12.0 * scale) as i32);
                    DrawIconEx(hdc, x, y, item.icon, icon, icon, 0, std::ptr::null_mut(), DI_NORMAL);
                }
                draw_label(hdc, label_ref, tile, scale, icon, app.theme);
            }
            if saved != 0 { RestoreDC(hdc, saved); }
        }
    }
    if use_cache {
        app.tile_cache = tile_cache;
    }
}

pub(super) fn clip_fold_tile(hdc: HDC, app: &crate::state::App, tile: RECT) -> i32 {
    if app.fold.is_none() || (tile.bottom - tile.top) >= app.layout.cell_h {
        return 0;
    }
    unsafe {
        let saved = SaveDC(hdc);
        if saved != 0 { IntersectClipRect(hdc, tile.left, tile.top, tile.right, tile.bottom); }
        saved
    }
}

/// 图标下方的名字：先画深色阴影，再画白字，浅色壁纸上也能看清。
pub(super) unsafe fn draw_label(
    hdc: HDC,
    name: &str,
    tile: RECT,
    scale: f32,
    icon: i32,
    theme: &crate::theme::Theme,
) {
    let label_top = tile.top + ((12.0 * scale) as i32) + icon + (6.0 * scale) as i32;
    let mut rect = RECT {
        left: tile.left + (4.0 * scale) as i32,
        top: label_top,
        right: tile.right - (4.0 * scale) as i32,
        bottom: label_top + ((34.0 * scale) as i32).max(24),
    };
    let text = wide(name);
    let flags = DT_CENTER | DT_WORDBREAK | DT_END_ELLIPSIS | DT_NOPREFIX;
    let mut shadow = rect;
    shadow.left += 1;
    shadow.top += 1;
    shadow.right += 1;
    shadow.bottom += 1;
    // SAFETY: 字体已由 paint() 选进 DC。
    unsafe {
        SetTextColor(hdc, theme.label_shadow);
        DrawTextW(hdc, text.as_ptr(), -1, &mut shadow, flags);
        SetTextColor(hdc, theme.text);
        DrawTextW(hdc, text.as_ptr(), -1, &mut rect, flags);
    }
}

/// 网格末尾的「添加图标」方块：一个方框加十字（纯 FillRect，不依赖字体）。
pub(super) unsafe fn draw_add_tile(
    hdc: HDC,
    tile: RECT,
    scale: f32,
    icon: i32,
    theme: &crate::theme::Theme,
) {
    let cell_w = tile.right - tile.left;
    let size = icon + (8.0 * scale) as i32;
    let left = tile.left + (cell_w - size) / 2;
    let top = tile.top + (12.0 * scale) as i32;
    let thickness = (2.0 * scale).max(2.0) as i32;
    // SAFETY: 画刷在本函数内创建并释放。
    unsafe {
        let brush = CreateSolidBrush(theme.text_dim);
        let frame = [
            RECT { left, top, right: left + size, bottom: top + thickness },
            RECT { left, top: top + size - thickness, right: left + size, bottom: top + size },
            RECT { left, top, right: left + thickness, bottom: top + size },
            RECT { left: left + size - thickness, top, right: left + size, bottom: top + size },
        ];
        for part in frame {
            FillRect(hdc, &part, brush);
        }
        let center_x = left + size / 2;
        let center_y = top + size / 2;
        let arm = (size as f32 * 0.30) as i32;
        let bar = (4.0 * scale).max(3.0) as i32;
        FillRect(
            hdc,
            &RECT {
                left: center_x - arm,
                top: center_y - bar / 2,
                right: center_x + arm,
                bottom: center_y + bar / 2,
            },
            brush,
        );
        FillRect(
            hdc,
            &RECT {
                left: center_x - bar / 2,
                top: center_y - arm,
                right: center_x + bar / 2,
                bottom: center_y + arm,
            },
            brush,
        );
        DeleteObject(brush as _);
        draw_label(
            hdc,
            crate::i18n::t("添加图标", "Add"),
            tile,
            scale,
            icon,
            theme,
        );
    }
}

/// 从缓存 DC 贴回一小块（用于擦掉上一帧的跟随图标/描边）。
pub(super) unsafe fn blit_region(hdc: HDC, cache: HDC, rect: RECT) {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 {
        return;
    }
    // SAFETY: 两个 DC 都有效，源与目标区域一致。
    unsafe {
        BitBlt(
            hdc,
            rect.left,
            rect.top,
            width,
            height,
            cache,
            rect.left,
            rect.top,
            SRCCOPY,
        );
    }
}

/// 目标格子的白色描边。
pub(super) unsafe fn draw_target_outline(hdc: HDC, tile: RECT, scale: f32, theme: &crate::theme::Theme) {
    // SAFETY: 画笔与画刷在本段内创建并释放。
    unsafe { draw_outline(hdc, tile, scale, theme.text) };
}

/// 给格子画一圈描边（颜色用 COLORREF）。
pub(super) unsafe fn draw_outline(hdc: HDC, tile: RECT, scale: f32, color: u32) {
    // SAFETY: 画笔与画刷在本段内创建并释放。
    unsafe {
        let pen = CreatePen(PS_SOLID, (2.0 * scale).max(2.0) as i32, color);
        let hollow = GetStockObject(HOLLOW_BRUSH);
        let previous = SelectObject(hdc, pen as _);
        SelectObject(hdc, hollow);
        RoundRect(
            hdc,
            tile.left + 4,
            tile.top + 4,
            tile.right - 4,
            tile.bottom - 4,
            (14.0 * scale) as i32,
            (14.0 * scale) as i32,
        );
        SelectObject(hdc, previous);
        DeleteObject(pen as _);
    }
}

/// 跟着鼠标的图标。
pub(super) unsafe fn draw_ghost_icon(hdc: HDC, app: &crate::state::App, index: usize, point: (i32, i32)) {
    let Some(item) = app.items.get(index) else {
        return;
    };
    if item.icon.is_null() {
        return;
    }
    let size = app.layout.icon;
    // SAFETY: 图标句柄有效。
    unsafe {
        DrawIconEx(
            hdc,
            point.0 - size / 2,
            point.1 - size / 2,
            item.icon,
            size,
            size,
            0,
            std::ptr::null_mut(),
            DI_NORMAL,
        );
    }
}

/// 确保缓存帧与当前布局一致（拖动前必须调用，否则贴回的是旧画面）。
pub(super) fn ensure_cache(hwnd: HWND) {
    let app = state();
    if !app.static_dirty {
        return;
    }
    let width = app.layout.width;
    let height = app.layout.height;
    let cache = app.buffer_dc;
    if cache.is_null() {
        return;
    }
    // SAFETY: 句柄与 DC 有效。
    unsafe {
        let hdc = GetDC(hwnd);
        if hdc.is_null() {
            return;
        }
        let app = state();
        render_static(cache, app, width, height);
        app.static_dirty = false;
        BitBlt(hdc, 0, 0, width, height, cache, 0, 0, SRCCOPY);
        ReleaseDC(hwnd, hdc);
    }
}

/// 右侧滚动条：只有内容比面板高时才出现。
pub(super) fn draw_scroll_indicator(hdc: HDC, app: &crate::state::App) {
    let max = max_scroll(app);
    if max <= 0 {
        return;
    }
    let scale = app.layout.scale;
    let track_top = (60.0 * scale) as i32;
    let track_bottom = app.layout.height - (60.0 * scale) as i32;
    let visible = (track_bottom - track_top) as f32;
    if visible < 80.0 {
        return;
    }
    let x = app.layout.width - (12.0 * scale) as i32;
    let width = (5.0 * scale).max(3.0) as i32;
    let ratio = (app.layout.height as f32 / (app.layout.height + max) as f32).clamp(0.08, 1.0);
    let thumb_h = (visible * ratio).max(24.0);
    let progress = (app.scroll as f32 / max as f32).clamp(0.0, 1.0);
    let thumb_top = track_top as f32 + (visible - thumb_h) * progress;
    // SAFETY: 画刷用完即释放。
    unsafe {
        let track = RECT {
            left: x,
            top: track_top,
            right: x + width,
            bottom: track_bottom,
        };
        let brush = CreateSolidBrush(app.theme.line);
        FillRect(hdc, &track, brush);
        DeleteObject(brush as _);
        let thumb = RECT {
            left: x,
            top: thumb_top as i32,
            right: x + width,
            bottom: thumb_top as i32 + thumb_h as i32,
        };
        let brush = CreateSolidBrush(app.theme.accent);
        FillRect(hdc, &thumb, brush);
        DeleteObject(brush as _);
    }
}


/// 拖动标题时的插入位置提示：整条细线，中间略亮、两端略暗，避免一整条死板的亮条。
pub(super) fn draw_header_indicator(hdc: HDC, app: &crate::state::App, target: usize) {
    // 插在原来的位置（自己前面或自己后面）不会改变顺序，就别画了。
    if let Some(drag) = app.header_drag
        && (target == drag.from || target == drag.from + 1)
    {
        return;
    }
    let rect = header_indicator_rect(app, target);
    let scale = app.layout.scale;
    let y = (rect.top + rect.bottom) / 2;
    let inset = (18.0 * scale) as i32;
    let left = inset;
    let right = app.layout.width - inset;
    let thickness = ((3.0 * scale) as i32).max(3);
    let top = y - thickness / 2;
    let bottom = top + thickness;
    if right <= left {
        return;
    }
    // 96 段线性渐变：中间 #7FB4FF，两端 #4A6B92（不会暗到看不见，保持是一条连续的线）。
    const SEGMENTS: i32 = 96;
    let span = (right - left).max(1);
    // SAFETY: 每段画刷单独创建、画完即释放；字体已由 paint 选好。
    unsafe {
        for index in 0..SEGMENTS {
            let start = left + span * index / SEGMENTS;
            let end = left + span * (index + 1) / SEGMENTS;
            if end <= start {
                continue;
            }
            let t = (index as f32 + 0.5) / SEGMENTS as f32;
            // 0 = 两端，1 = 中间。
            let centered = 1.0 - (2.0 * t - 1.0).abs();
            let amount = centered.powf(0.55);
            // 两端淡向工具条底色，中间用主题强调色。
            let ends = crate::theme::mix(app.theme.accent, app.theme.bar, 0.55);
            let color = crate::theme::mix(ends, app.theme.accent, amount);
            let brush = CreateSolidBrush(color);
            let piece = RECT {
                left: start,
                top,
                right: end,
                bottom,
            };
            FillRect(hdc, &piece, brush);
            DeleteObject(brush as _);
        }
    }
}

/// 拖入时给目标分组画框：标题行填色 + 整块描边，告诉用户“松手就加到这里”。
pub(super) fn draw_drop_hover(hdc: HDC, app: &crate::state::App) {
    let Some(group) = app.drop_hover else {
        return;
    };
    let Some(section) = group_section_rect(app, group) else {
        return;
    };
    let Some(header) = app
        .layout
        .headers
        .iter()
        .find(|header| header.group == group)
        .cloned()
    else {
        return;
    };
    let scale = app.layout.scale;
    let section = view_rect(app, section);
    let header_rect = view_rect(app, header.rect);
    // SAFETY: 画刷用完即释放；字体已由 paint 选好；只画在内容区里，别压到工具条。
    unsafe {
        let saved = SaveDC(hdc);
        IntersectClipRect(hdc, 0, content_top(app), app.layout.width, app.layout.height);
        let brush = CreateSolidBrush(app.theme.drop_bg);
        FillRect(hdc, &header_rect, brush);
        DeleteObject(brush as _);
        draw_outline(hdc, section, scale, app.theme.drop_line);
        RestoreDC(hdc, saved);
    }
    // 填色盖住了标题文字，补画一次。
    draw_headers(hdc, app, &header_rect);
}

/// 内部拖动悬停在目标组：标题行填色 + 描边，并把被盖住的标题文字补画回来。
pub(super) fn draw_drag_header_hover(hdc: HDC, app: &crate::state::App, group: usize) {
    let Some(header) = app
        .layout
        .headers
        .iter()
        .find(|header| header.group == group)
    else {
        return;
    };
    let rect = view_rect(app, header.rect);
    let inner = RECT {
        left: rect.left,
        top: rect.top + 2,
        right: rect.right,
        bottom: rect.bottom - 2,
    };
    // SAFETY: 画刷用完即释放；填色会盖住标题文字，下面按本帧布局补画。
    unsafe {
        let brush = CreateSolidBrush(app.theme.drop_bg);
        FillRect(hdc, &inner, brush);
        DeleteObject(brush as _);
        draw_outline(hdc, inner, app.layout.scale, app.theme.drop_line);
    }
    draw_one_header(hdc, app, header, rect, app.layout.scale);
}

/// 拖动排序的视觉反馈：目标格子描边 + 跟着鼠标的图标。
pub(super) fn draw_drag(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    let Some(drag) = app.drag else {
        return;
    };
    if !drag.active {
        return;
    }
    let scale = app.layout.scale;
    if let Some(target) = hit_test(drag.current.0, drag.current.1)
        && target < app.items.len()
        && let Some(raw) = app.layout.tiles.get(target).copied()
    {
        let tile = view_rect(app, raw);
        if intersects(&tile, paint) {
            // SAFETY: DC 有效。
            unsafe { draw_target_outline(hdc, tile, scale, app.theme) };
        }
    }
    if let Some(group) = drag.header {
        // SAFETY: DC 有效；整帧重画时也要把目标组画出来。
        draw_drag_header_hover(hdc, app, group);
    }
    // SAFETY: DC 有效。
    unsafe { draw_ghost_icon(hdc, app, drag.from, drag.current) };
}

pub(super) fn draw_toolbar(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    let bar = toolbar_rect(app);
    if !intersects(&bar, paint) {
        return;
    }
    let scale = app.layout.scale;
    // SAFETY: 画刷与字体都在本函数内用完即释放。
    unsafe {
        let brush = CreateSolidBrush(app.theme.bar);
        FillRect(hdc, &bar, brush);
        DeleteObject(brush as _);
        // 底边一条分隔线
        let line = RECT {
            left: bar.left,
            top: bar.bottom - 1,
            right: bar.right,
            bottom: bar.bottom,
        };
        let line_brush = CreateSolidBrush(app.theme.line);
        FillRect(hdc, &line, line_brush);
        DeleteObject(line_brush as _);
    }
    let theme = app.theme;
    draw_chip(
        hdc,
        settings_chip_rect(app),
        crate::i18n::t("设置", "Settings"),
        theme.chip,
        theme.text,
        scale,
    );
    draw_chip(
        hdc,
        new_group_chip_rect(app),
        crate::i18n::t("＋ 新建分组", "+ New group"),
        theme.chip,
        theme.text,
        scale,
    );
    let all_collapsed = !app.groups.is_empty() && app.groups.iter().all(|g| g.collapsed);
    draw_chip(
        hdc,
        fold_all_chip_rect(app),
        if all_collapsed {
            crate::i18n::t("全部展开", "Expand all")
        } else {
            crate::i18n::t("全部折叠", "Collapse all")
        },
        theme.chip,
        theme.text,
        scale,
    );
    draw_chip(
        hdc,
        remove_chip_rect(app),
        if app.remove_mode {
            crate::i18n::t("移除中", "Removing")
        } else {
            crate::i18n::t("移除", "Remove")
        },
        if app.remove_mode { theme.chip_hot } else { theme.chip },
        if app.remove_mode { theme.warn_text } else { theme.text },
        scale,
    );
}

pub(super) fn draw_chip(hdc: HDC, chip: RECT, label: &str, background: u32, foreground: u32, scale: f32) {
    let text = wide(label);
    // SAFETY: 画刷在函数内创建并释放；字体由 paint 选好。
    unsafe {
        let brush = CreateSolidBrush(background);
        let null_pen = GetStockObject(NULL_PEN);
        let old_brush = SelectObject(hdc, brush as _);
        let old_pen = SelectObject(hdc, null_pen);
        RoundRect(
            hdc,
            chip.left,
            chip.top,
            chip.right,
            chip.bottom,
            (16.0 * scale) as i32,
            (16.0 * scale) as i32,
        );
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        DeleteObject(brush as _);
        let mut rect = chip;
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, foreground);
        DrawTextW(
            hdc,
            text.as_ptr(),
            -1,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
    }
}

pub(super) fn draw_search_box(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    let box_rect = search_rect(app);
    if !intersects(&box_rect, paint) {
        return;
    }
    let scale = app.layout.scale;
    let expanded = (box_rect.right - box_rect.left) > (70.0 * scale) as i32;
    if expanded {
        let (text, color) = if app.search.is_empty() {
            (
                crate::i18n::t("输入关键字…", "Type to search…").to_string(),
                app.theme.text_dim,
            )
        } else {
            (
                format!("{}　（{}）", app.search, count_matches(app)),
                app.theme.text,
            )
        };
        let label = wide(&text);
        // SAFETY: DC 与缓存字体有效；只画文字和下划线，不画底板（保持透明观感）。
        unsafe {
            let mut rect = RECT {
                left: box_rect.left + (10.0 * scale) as i32,
                top: box_rect.top,
                right: box_rect.right - (6.0 * scale) as i32,
                bottom: box_rect.bottom,
            };
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, color);
            DrawTextW(
                hdc,
                label.as_ptr(),
                -1,
                &mut rect,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
            if !app.search_edit.is_null() {
                let underline = RECT {
                    left: box_rect.left + (10.0 * scale) as i32,
                    top: box_rect.bottom - (4.0 * scale) as i32,
                    right: box_rect.right - (6.0 * scale) as i32,
                    bottom: box_rect.bottom - (2.0 * scale) as i32,
                };
                let brush = CreateSolidBrush(app.theme.accent);
                FillRect(hdc, &underline, brush);
                DeleteObject(brush as _);
            }
        }
    } else {
        // 放大镜：一个圆圈加一根手柄。
        let size = (15.0 * scale).max(12.0) as i32;
        let center_x = (box_rect.left + box_rect.right) / 2 - 1;
        let center_y = (box_rect.top + box_rect.bottom) / 2 - 1;
        // SAFETY: 画笔与画刷用完即释放。
        unsafe {
            let pen = CreatePen(PS_SOLID, (2.0 * scale).max(2.0) as i32, app.theme.text_dim);
            let hollow = GetStockObject(HOLLOW_BRUSH);
            let old_pen = SelectObject(hdc, pen as _);
            let old_brush = SelectObject(hdc, hollow);
            Ellipse(
                hdc,
                center_x - size / 2,
                center_y - size / 2,
                center_x + size / 2,
                center_y + size / 2,
            );
            MoveToEx(hdc, center_x + size / 2 - 1, center_y + size / 2 - 1, std::ptr::null_mut());
            LineTo(hdc, center_x + size / 2 + 3, center_y + size / 2 + 3);
            SelectObject(hdc, old_pen);
            SelectObject(hdc, old_brush);
            DeleteObject(pen as _);
        }
    }
}
/// 界面字体：按缩放只创建一次，避免每帧重建导致动画卡顿。
pub(super) fn ui_font(app: &mut crate::state::App) -> HFONT {
    let scale = app.layout.scale;
    if !app.font.is_null() && (app.font_scale - scale).abs() < 0.01 {
        return app.font;
    }
    if !app.font.is_null() {
        // SAFETY: 字体由本模块创建。
        unsafe { DeleteObject(app.font as _) };
        app.font = std::ptr::null_mut();
    }
    let font = create_font(scale, 13.0);
    app.font = font;
    app.font_scale = scale;
    font
}

pub(super) fn draw_hint(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    let scale = app.layout.scale;
    let theme = app.theme;
    let (text, background, foreground) = if let Some((notice, _)) = app.drop_notice.as_ref() {
        (notice.clone(), crate::theme::mix(theme.hint_bg, theme.drop_line, 0.35), theme.text)
    } else if app.remove_mode {
        (
            crate::i18n::t(
                "移除模式：单击图标从面板移除（源文件不动）　·　再点右上角或按 Esc 退出",
                "Remove mode: click an icon to remove it from the panel (files untouched)　·　click Remove again or press Esc to exit",
            )
            .to_string(),
            theme.chip_hot,
            theme.warn_text,
        )
    } else if app.selection.len() > 1 {
        (
            if crate::i18n::is_en() {
                format!(
                    "{} selected　·　Drag to reorder or move to a group　·　Delete to remove　·　Esc to clear",
                    app.selection.len()
                )
            } else {
                format!(
                    "已选 {} 项　·　组内拖动排序，跨组拖动移入　·　Delete 移除　·　Esc 取消",
                    app.selection.len()
                )
            },
            theme.chip_hot,
            theme.text,
        )
    } else if !app.search.is_empty() {
        (
            if crate::i18n::is_en() {
                format!("Search \"{}\": {} match(es)　·　Esc to clear", app.search, count_matches(app))
            } else {
                format!("搜索「{}」：{} 个匹配　·　Esc 清除搜索", app.search, count_matches(app))
            },
            theme.hint_bg,
            theme.text,
        )
    } else {
        (
            crate::i18n::t(
                "直接打首字母搜索（bdwp = 百度网盘）　·　单击打开　·　拖动排序　·　Esc 收起",
                "Type initials to search (vsc = Visual Studio Code)　·　Click to open　·　Drag to reorder　·　Esc to close",
            )
            .to_string(),
            theme.hint_bg,
            theme.text,
        )
    };
    let text = wide(&text);
    let font = create_font(scale, 14.0);
    // SAFETY: 字体用完恢复并释放。
    unsafe {
        let previous = if font.is_null() {
            std::ptr::null_mut()
        } else {
            SelectObject(hdc, font as _)
        };
        // 先量文字尺寸，再画一个深色胶囊当底，保证醒目。
        let mut measure = RECT {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 100,
        };
        DrawTextW(
            hdc,
            text.as_ptr(),
            -1,
            &mut measure,
            DT_CENTER | DT_SINGLELINE | DT_CALCRECT | DT_NOPREFIX,
        );
        let text_width = measure.right - measure.left;
        let text_height = measure.bottom - measure.top;
        let padding_x = (22.0 * scale) as i32;
        let padding_y = (12.0 * scale) as i32;
        let pill_width = text_width + padding_x * 2;
        let pill_height = text_height + padding_y * 2;
        let pill = RECT {
            left: (app.layout.width - pill_width) / 2,
            top: app.layout.height - pill_height - (28.0 * scale) as i32,
            right: (app.layout.width + pill_width) / 2,
            bottom: app.layout.height - (28.0 * scale) as i32,
        };
        if intersects(&pill, paint) {
            let brush = CreateSolidBrush(background);
            let null_pen = GetStockObject(NULL_PEN);
            let old_brush = SelectObject(hdc, brush as _);
            let old_pen = SelectObject(hdc, null_pen);
            RoundRect(
                hdc,
                pill.left,
                pill.top,
                pill.right,
                pill.bottom,
                pill_height,
                pill_height,
            );
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            DeleteObject(brush as _);
            let mut rect = pill;
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, foreground);
            DrawTextW(
                hdc,
                text.as_ptr(),
                -1,
                &mut rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
        if !previous.is_null() {
            SelectObject(hdc, previous);
        }
        if !font.is_null() {
            DeleteObject(font as _);
        }
    }
}
/// 受限权限运行时在顶部画一条警告横幅，说明为什么托盘和程序启动不可用。
pub(super) fn draw_banner(hdc: HDC, app: &crate::state::App, paint: &RECT) {
    if !app.low_integrity {
        return;
    }
    let scale = app.layout.scale;
    let height = (40.0 * scale) as i32;
    let bar = RECT {
        left: 0,
        top: 0,
        right: app.layout.width,
        bottom: height,
    };
    if !intersects(&bar, paint) {
        return;
    }
    let text = wide(crate::i18n::t(
        "受限权限运行：托盘不可用、多数程序点不开。请关闭本程序，在资源管理器里双击 LiteFencesDesktop.exe",
        "Running with limited rights: tray unavailable, most apps cannot launch. Close this program and start LiteFencesDesktop.exe from Explorer instead.",
    ));
    let mut rect = RECT {
        left: (16.0 * scale) as i32,
        right: app.layout.width - (16.0 * scale) as i32,
        top: 0,
        bottom: height,
    };
    // SAFETY: 画刷与字体都在本函数内创建并释放。
    // SAFETY: 画刷在本函数内创建并释放；字体已由 paint() 选入 DC。
    unsafe {
        let brush = CreateSolidBrush(app.theme.warn_bg);
        FillRect(hdc, &bar, brush);
        DeleteObject(brush as _);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, app.theme.warn_text);
        DrawTextW(
            hdc,
            text.as_ptr(),
            -1,
            &mut rect,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
    }
}

/// 键盘选中框：主题强调色描两圈，裁剪到内容区。
pub(super) fn draw_selection(hdc: HDC, app: &crate::state::App) {
    let scale = app.layout.scale;
    if app.search.is_empty()
        && let Some(slot) = app.recent_selected
        && let Some((_, raw)) = app.layout.recent_tiles.get(slot).copied()
    {
        let rect = view_rect(app, raw);
        // SAFETY: DC 有效；SaveDC/RestoreDC 成对恢复裁剪区。
        unsafe {
            let saved = SaveDC(hdc);
            IntersectClipRect(
                hdc,
                0,
                content_top(app),
                app.layout.width,
                app.layout.height - (56.0 * scale) as i32,
            );
            draw_outline(hdc, rect, scale, app.theme.accent);
            let inner = RECT {
                left: rect.left + 2,
                top: rect.top + 2,
                right: rect.right - 2,
                bottom: rect.bottom - 2,
            };
            draw_outline(hdc, inner, scale, app.theme.accent);
            RestoreDC(hdc, saved);
        }
    }
    let mut indices = app.selection.clone();
    if let Some(selected) = app.selected
        && !indices.contains(&selected)
    {
        indices.push(selected);
    }
    if indices.is_empty() {
        return;
    }
    // SAFETY: DC 有效；SaveDC/RestoreDC 成对恢复裁剪区。
    unsafe {
        let saved = SaveDC(hdc);
        IntersectClipRect(
            hdc,
            0,
            content_top(app),
            app.layout.width,
            app.layout.height - (56.0 * scale) as i32,
        );
        for index in indices {
            let Some(tile) = app.layout.tiles.get(index).copied() else {
                continue;
            };
            if tile.right <= tile.left || tile.bottom <= tile.top {
                continue;
            }
            let rect = view_rect(app, tile);
            draw_outline(hdc, rect, scale, app.theme.accent);
            let inner = RECT {
                left: rect.left + 2,
                top: rect.top + 2,
                right: rect.right - 2,
                bottom: rect.bottom - 2,
            };
            draw_outline(hdc, inner, scale, app.theme.accent);
        }
        RestoreDC(hdc, saved);
    }
}

/// 空白区域鼠标拉框多选半透明选框。
pub(super) fn draw_marquee(hdc: HDC, app: &crate::state::App) {
    let Some(marquee) = app.marquee.as_ref().filter(|m| m.active) else {
        return;
    };
    let rect = super::layout::normalized_rect(marquee.start, marquee.current);
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return;
    }
    let scale = app.layout.scale;
    // COLORREF 为 0x00BBGGRR；DIB 像素为预乘 BGRA。
    let accent = app.theme.accent;
    let alpha: u32 = 56;
    let r = (accent & 0xff) * alpha / 255;
    let g = ((accent >> 8) & 0xff) * alpha / 255;
    let b = ((accent >> 16) & 0xff) * alpha / 255;
    // SAFETY: 所有 GDI 对象在本函数内创建并配对释放。
    unsafe {
        let saved = SaveDC(hdc);
        IntersectClipRect(
            hdc,
            0,
            content_top(app),
            app.layout.width,
            app.layout.height - (56.0 * scale) as i32,
        );
        let mem = CreateCompatibleDC(hdc);
        let header = windows_sys::Win32::Graphics::Gdi::BITMAPINFO {
            bmiHeader: windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER {
                biSize: std::mem::size_of::<windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32,
                biWidth: 1,
                biHeight: -1,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: windows_sys::Win32::Graphics::Gdi::BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(
            mem,
            &header,
            windows_sys::Win32::Graphics::Gdi::DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if !mem.is_null() && !bmp.is_null() && !bits.is_null() {
            let px = bits as *mut u8;
            *px = b as u8;
            *px.add(1) = g as u8;
            *px.add(2) = r as u8;
            *px.add(3) = alpha as u8;
            let old = SelectObject(mem, bmp as _);
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: 1, // AC_SRC_ALPHA
            };
            AlphaBlend(
                hdc,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                mem,
                0,
                0,
                1,
                1,
                blend,
            );
            SelectObject(mem, old);
        }
        if !bmp.is_null() {
            DeleteObject(bmp as _);
        }
        if !mem.is_null() {
            DeleteDC(mem);
        }
        draw_outline(hdc, rect, scale, accent);
        RestoreDC(hdc, saved);
    }
}

/// 拖动时指针停在面板上/下边缘：持续滚动内容。
/// 桌面一个图标都没有时，给一句说明，免得以为面板坏了。
pub(super) fn draw_empty_hint(hdc: HDC, app: &crate::state::App) {
    if !app.items.is_empty() && !app.options.empty {
        return;
    }
    let scale = app.layout.scale;
    // 内容区垂直居中。
    let center = (content_top(app) + app.layout.height) / 2;
    let mut rect = RECT {
        left: (app.layout.width as f32 * 0.15) as i32,
        top: center - (60.0 * scale) as i32,
        right: (app.layout.width as f32 * 0.85) as i32,
        bottom: center + (60.0 * scale) as i32,
    };
    let text = wide(crate::i18n::t(
        "没读到桌面图标。\n点右下角「添加图标」，或从资源管理器把文件拖进来。",
        "No desktop icons found.\nClick \"Add\" or drop files here from Explorer.",
    ));
    // SAFETY: 字体已由 paint 选进 DC。
    unsafe {
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, app.theme.text_dim);
        DrawTextW(
            hdc,
            text.as_ptr(),
            -1,
            &mut rect,
            DT_CENTER | DT_WORDBREAK | DT_NOPREFIX,
        );
    }
}

