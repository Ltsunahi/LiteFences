//! 浮层模块：input。

use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetDC, GetMonitorInfoW, InvalidateRect, MONITORINFO, MONITOR_DEFAULTTONEAREST, MonitorFromWindow, PtInRect, ReleaseDC, SelectObject, UpdateWindow};
use windows_sys::Win32::UI::Controls::{EM_SETSEL};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_CONTROL, VK_SHIFT};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, DestroyWindow, EN_CHANGE, ES_AUTOHSCROLL, GWL_EXSTYLE, GetCursorPos, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, HTCLIENT, HWND_TOPMOST, KillTimer, MA_NOACTIVATE, MSG, SWP_NOACTIVATE, SendMessageW, SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowPos, WM_COMMAND, WM_DPICHANGED, WM_ERASEBKGND, WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCHITTEST, WM_PAINT, WM_RBUTTONUP, WM_SETFONT, WM_TIMER, WS_CHILD, WS_EX_NOACTIVATE, WS_VISIBLE};

use crate::icons;
use crate::log;
use crate::state::{state};
use crate::util::{mouse_xy, wide};

use super::{TIMER_SEARCH, TIMER_DROP_NOTICE, TIMER_AUTOSCROLL, TIMER_GROUP_FOLD, SHOW_COOLDOWN_MS, reshow, hide_panel, relayout, rebuild_layout, show_drop_notice};
use super::layout::{finish_fold, compute_layout_full, view_rect, content_point, content_top, max_scroll, clamp_scroll, sticky_header, header_at, header_insert_target, onboarding_button_rect, portal_up_rect, toolbar_rect, settings_chip_rect, new_group_chip_rect, remove_chip_rect, fold_all_chip_rect, search_rect, visible, search_results, apply_search, group_under_cursor, drag_drop_group, drag_header_inner, refresh_drag_hover, hit_test, hit_test_recent, in_recent_bar, normalized_rect, marquee_hit_items, step_recent_nav};
use super::render::{animating, paint, ghost_rect, blit_region, draw_target_outline, draw_ghost_icon, ensure_cache, draw_drag_header_hover, draw_toolbar, draw_search_box, ui_font};
use super::menu::{move_group, create_group, toggle_group, open_group_menu, open_header_menu, dismiss_onboarding, open_settings, begin_add_icon, toggle_all_groups};

pub fn type_to_search(ch: char) {
    let app = state();
    if crate::text_input::is_open() {
        return;
    }
    if !app.search_open {
        log::info("唤出后直接打字：进入搜索");
        unsafe { KillTimer(app.overlay, TIMER_SEARCH) };
        app.search_open = true;
        app.search_anim = None;
    }
    app.search.push(ch);
    apply_search(app);
}

/// 退格：删一个字；搜索框开着但已经空了就收起来。返回 true 表示按键已处理。
pub fn search_backspace() -> bool {
    let app = state();
    if !app.search.is_empty() {
        app.search.pop();
        apply_search(app);
        return true;
    }
    if app.search_open {
        close_search();
        return true;
    }
    false
}

/// 当前是否有搜索关键字（键盘钩子用）。
pub fn search_active() -> bool {
    !state().search.is_empty()
}

/// 排障：当前关键字下的匹配数量和第一个结果（`--search-test` 用）。
pub fn search_report() -> (usize, Option<String>) {
    let app = state();
    let results = search_results(app);
    let first = results
        .first()
        .and_then(|index| app.items.get(*index))
        .map(|item| item.name.clone());
    (results.len(), first)
}

pub(super) fn activate_typing() {
    let app = state();
    unsafe { KillTimer(app.overlay, TIMER_SEARCH) };
    app.search_open = true;
    app.search_anim = None;
    let rect = search_rect(app);
    let parent = app.overlay;
    let text = wide(&app.search);
    let font = ui_font(app);
    let edit = unsafe {
        CreateWindowExW(0, wide("EDIT").as_ptr(), text.as_ptr(),
            WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32, rect.left, rect.top,
            rect.right - rect.left, rect.bottom - rect.top, parent, 2001 as _,
            std::ptr::null_mut(), std::ptr::null())
    };
    if edit.is_null() {
        log::warn("无法创建搜索输入框");
        return;
    }
    if unsafe { SetWindowSubclass(edit, Some(search_edit_proc), 1, 0) } == 0 {
        unsafe { DestroyWindow(edit) };
        log::warn("无法初始化搜索输入框");
        return;
    }
    app.search_edit = edit;
    app.ime_composing = false;
    // SAFETY: 句柄有效；只临时改扩展样式。
    unsafe {
        let style = GetWindowLongPtrW(app.overlay, GWL_EXSTYLE);
        SetWindowLongPtrW(
            app.overlay,
            GWL_EXSTYLE,
            style & !(WS_EX_NOACTIVATE as isize),
        );
        SetForegroundWindow(app.overlay);
        SendMessageW(edit, WM_SETFONT, font as usize, 1);
        let end = text.len() - 1;
        SendMessageW(edit, EM_SETSEL, end, end as isize);
        SetFocus(edit);
    }
    if unsafe { GetFocus() } != edit {
        deactivate_typing();
        log::warn("搜索输入框未获得焦点");
        return;
    }
    log::info("搜索输入已激活");
}

pub(super) fn deactivate_typing() {
    let app = state();
    if app.search_edit.is_null() {
        return;
    }
    let edit = std::mem::replace(&mut app.search_edit, std::ptr::null_mut());
    app.ime_composing = false;
    // SAFETY: 恢复“不抢焦点”的行为。
    unsafe {
        DestroyWindow(edit);
        let style = GetWindowLongPtrW(app.overlay, GWL_EXSTYLE);
        SetWindowLongPtrW(
            app.overlay,
            GWL_EXSTYLE,
            style | (WS_EX_NOACTIVATE as isize),
        );
    }
}

pub(super) unsafe extern "system" fn search_edit_proc(
    hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize,
) -> LRESULT {
    match message {
        WM_IME_STARTCOMPOSITION => state().ime_composing = true,
        WM_IME_ENDCOMPOSITION => state().ime_composing = false,
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

pub fn search_edit_key(message: &MSG) -> bool {
    let app = state();
    if message.message != WM_KEYDOWN || app.search_edit.is_null() || message.hwnd != app.search_edit
    {
        return false;
    }
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP};
    if !is_search_command(message.wParam as u16, app.ime_composing) {
        return false;
    }
    match message.wParam as u16 {
        VK_RETURN if message.lParam & (1 << 30) != 0 => {}
        VK_RETURN => {
            // SAFETY: 只读取按键修饰状态。
            let (ctrl, shift) = unsafe {
                (
                    GetKeyState(i32::from(VK_CONTROL)) < 0,
                    GetKeyState(i32::from(VK_SHIFT)) < 0,
                )
            };
            if ctrl && shift {
                open_selected_admin();
            } else {
                open_selected();
            }
        }
        VK_ESCAPE => handle_escape(),
        VK_UP => move_selection(0, -1),
        VK_DOWN => move_selection(0, 1),
        _ => return false,
    }
    true
}

pub(super) fn is_search_command(key: u16, composing: bool) -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP};
    !composing && matches!(key, VK_RETURN | VK_ESCAPE | VK_UP | VK_DOWN)
}

pub(super) fn close_search() {
    let app = state();
    deactivate_typing();
    app.search_open = false;
    app.search_anim = Some(Instant::now());
    crate::app::set_search_text("");
    rebuild_layout(app);
    // SAFETY: 句柄有效；只重画工具条这一块。
    unsafe {
        let bar = toolbar_rect(app);
        InvalidateRect(app.overlay, &bar, 0);
        SetTimer(app.overlay, TIMER_SEARCH, 16, None);
        UpdateWindow(app.overlay);
    }
    log::info("搜索栏收起");
}

pub(super) fn within_cooldown() -> bool {
    state()
        .shown_at
        .is_some_and(|shown| shown.elapsed() < Duration::from_millis(SHOW_COOLDOWN_MS))
}

/// Esc：先关拉框、搜索、退出移除模式，最后才收起面板。
pub fn handle_escape() {
    // 先处理输入框：Esc 等于取消输入，面板保持显示。
    if crate::text_input::is_open() {
        log::info("Esc：取消输入");
        crate::text_input::cancel();
        return;
    }
    let app = state();
    if app.marquee.take().is_some() {
        // SAFETY: 鼠标捕获由本窗口持有。
        unsafe {
            ReleaseCapture();
            InvalidateRect(app.overlay, std::ptr::null(), 0);
        }
        return;
    }
    if app.search_open {
        close_search();
        return;
    }
    if !app.search.is_empty() {
        crate::app::set_search_text("");
        rebuild_layout(app);
        return;
    }
    if app.remove_mode {
        app.remove_mode = false;
        app.static_dirty = true;
        log::info("退出移除模式");
        // SAFETY: 句柄有效。
        unsafe { InvalidateRect(app.overlay, std::ptr::null(), 0) };
        return;
    }
    if app.recent_selected.take().is_some() || !app.selection.is_empty() {
        app.selection.clear();
        app.selected = None;
        app.selection_anchor = None;
        app.static_dirty = true;
        log::info("Esc：取消多选");
        // SAFETY: 句柄有效。
        unsafe { InvalidateRect(app.overlay, std::ptr::null(), 0) };
        return;
    }
    hide_panel();
}

/// 键盘：在网格与「最近常用」栏间移动选中项；搜索时改为在结果列表里前后移动。
pub fn move_selection(dx: i32, dy: i32) {
    let app = state();
    if app.items.is_empty() {
        return;
    }
    if !app.search.is_empty() {
        let step = (dx + dy).signum();
        if step == 0 {
            return;
        }
        let results = &app.layout.search_order;
        if results.is_empty() {
            return;
        }
        let position = app
            .selected
            .and_then(|selected| results.iter().position(|index| *index == selected))
            .unwrap_or(0) as i32;
        let next = results[(position + step).clamp(0, results.len() as i32 - 1) as usize];
        if app.selected == Some(next) {
            return;
        }
        app.recent_selected = None;
        app.selected = Some(next);
        app.selection.clear();
        app.selection.push(next);
        app.selection_anchor = Some(next);
        ensure_selected_visible(app, next);
        unsafe {
            InvalidateRect(app.overlay, std::ptr::null(), 0);
            UpdateWindow(app.overlay);
        }
        return;
    }
    let (next_recent, next_grid) = step_recent_nav(
        app.layout.recent_tiles.len(),
        app.layout.cols,
        app.items.len(),
        app.recent_selected,
        app.selected,
        dx,
        dy,
    );
    if app.recent_selected == next_recent && app.selected == next_grid {
        return;
    }
    app.recent_selected = next_recent;
    app.selected = next_grid;
    app.selection.clear();
    if let Some(next) = next_grid {
        app.selection.push(next);
        app.selection_anchor = Some(next);
        ensure_selected_visible(app, next);
    } else {
        app.selection_anchor = None;
        if app.scroll != 0 {
            app.scroll = 0;
            app.static_dirty = true;
        }
    }
    // SAFETY: 句柄有效；选中框是每帧画的，整屏重画一帧最省事。
    unsafe {
        InvalidateRect(app.overlay, std::ptr::null(), 0);
        UpdateWindow(app.overlay);
    }
}

fn active_selected_item(app: &crate::state::App) -> Option<usize> {
    if !app.search.is_empty() {
        app.selected
            .filter(|index| visible(app, *index))
            .or_else(|| app.layout.search_order.first().copied())
    } else if let Some(slot) = app.recent_selected {
        app.layout.recent_tiles.get(slot).map(|&(idx, _)| idx)
    } else {
        app.selected.filter(|index| *index < app.items.len())
    }
}

/// 键盘 Enter：打开当前选中项；搜索时打开第一个可见结果。
pub fn open_selected() {
    open_selected_with(false);
}

/// Ctrl+Shift+Enter：以管理员身份运行当前选中项。
pub fn open_selected_admin() {
    open_selected_with(true);
}

/// Ctrl+Enter：打开当前选中项所在位置。
pub fn reveal_selected() {
    let path = {
        let app = state();
        active_selected_item(app).and_then(|idx| app.items.get(idx).map(|item| item.path.clone()))
    };
    if let Some(path) = path {
        hide_panel();
        if let Err(error) = crate::file_actions::reveal(&path) {
            log::error(&format!("打开所在位置失败：{error}"));
        }
    }
}

pub(super) fn open_selected_with(as_admin: bool) {
    let selected = active_selected_item(state());
    if let Some(index) = selected {
        launch_and_hide(index, as_admin);
    }
}

/// 键盘 Tab / Shift+Tab：跳到下一个分组的第一个图标（顺带展开该组）。
pub fn cycle_group(back: bool) {
    let app = state();
    if app.items.is_empty() || app.groups.is_empty() {
        return;
    }
    let current = app.selected.unwrap_or(0).min(app.items.len() - 1);
    let path = app.items[current].path.clone();
    let group = crate::groups::group_of(&app.groups, &path).unwrap_or(0);
    let count = app.groups.len();
    let target = if back {
        (group + count - 1) % count
    } else {
        (group + 1) % count
    };
    let mut changed = false;
    if let Some(entry) = app.groups.get_mut(target)
        && entry.collapsed
    {
        entry.collapsed = false;
        changed = true;
    }
    if changed {
        crate::groups::save(&app.groups);
        relayout();
        reshow();
    }
    let app = state();
    if let Some(first) = app
        .items
        .iter()
        .position(|item| crate::groups::group_of(&app.groups, &item.path) == Some(target))
    {
        app.selected = Some(first);
        ensure_selected_visible(app, first);
        // SAFETY: 句柄有效。
        unsafe {
            InvalidateRect(app.overlay, std::ptr::null(), 0);
            UpdateWindow(app.overlay);
        }
    }
}

/// 选中的格子滚出可视区时，把滚动量调回来。
pub(super) fn ensure_selected_visible(app: &mut crate::state::App, index: usize) {
    let Some(tile) = app.layout.tiles.get(index).copied() else {
        return;
    };
    let scale = app.layout.scale;
    let pad = (56.0 * scale) as i32;
    let top = content_top(app) + app.scroll;
    let bottom = app.layout.height - pad + app.scroll;
    let mut scroll = app.scroll;
    if tile.top < top {
        scroll = tile.top - content_top(app);
    } else if tile.bottom > bottom {
        scroll += tile.bottom - bottom;
    }
    if scroll != app.scroll {
        app.scroll = scroll;
        clamp_scroll(app);
        app.static_dirty = true;
    }
}

/// 当前可见项按屏幕顺序排列（搜索态只剩命中的格子，也适用）。
pub(super) fn ordered_visible_items(app: &crate::state::App) -> Vec<usize> {
    let mut entries: Vec<(i32, i32, usize)> = app
        .layout
        .tiles
        .iter()
        .enumerate()
        .filter(|(index, tile)| {
            *index < app.items.len()
                && tile.right > tile.left
                && tile.bottom > tile.top
                && visible(app, *index)
        })
        .map(|(index, tile)| (tile.top, tile.left, index))
        .collect();
    entries.sort_unstable();
    entries.into_iter().map(|(_, _, index)| index).collect()
}

/// Ctrl 点选追加/取消，Shift 连选；普通点击由调用方处理。
pub(super) fn click_select(index: usize, ctrl: bool, shift: bool) {
    let app = state();
    if index >= app.items.len() {
        return;
    }
    if shift {
        let ordered = ordered_visible_items(app);
        let anchor = app
            .selection_anchor
            .filter(|anchor| ordered.contains(anchor))
            .unwrap_or(index);
        let from = ordered.iter().position(|item| *item == anchor);
        let to = ordered.iter().position(|item| *item == index);
        if let (Some(from), Some(to)) = (from, to) {
            let (low, high) = if from <= to { (from, to) } else { (to, from) };
            if !ctrl {
                app.selection.clear();
            }
            for item in &ordered[low..=high] {
                if !app.selection.contains(item) {
                    app.selection.push(*item);
                }
            }
            app.selection_anchor = Some(anchor);
        }
    } else if ctrl {
        if let Some(position) = app.selection.iter().position(|item| *item == index) {
            app.selection.remove(position);
        } else {
            app.selection.push(index);
        }
        app.selection_anchor = selection_focus(&app.selection, index);
    } else {
        app.selection.clear();
        app.selection.push(index);
        app.selection_anchor = Some(index);
    }
    app.selected = selection_focus(&app.selection, index);
    app.static_dirty = true;
    log::info(&format!("多选：{} 项", app.selection.len()));
    // SAFETY: 句柄有效；选中框在静态帧之后画，整屏重画一帧。
    unsafe {
        InvalidateRect(app.overlay, std::ptr::null(), 0);
        UpdateWindow(app.overlay);
    }
}

pub(super) fn selection_focus(selection: &[usize], clicked: usize) -> Option<usize> {
    if selection.contains(&clicked) { Some(clicked) } else { selection.last().copied() }
}

/// Ctrl+Z：撤销最近一步。
pub fn undo_last() {
    crate::app::undo();
}

/// Ctrl+Y / Ctrl+Shift+Z：重做最近一次撤销的改动。
pub fn redo_last() {
    crate::app::redo();
}

pub(super) fn copy_paths_to_clipboard(hwnd: HWND, paths: &[std::path::PathBuf]) -> bool {
    if paths.is_empty() {
        return false;
    }
    let text = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join("\r\n");
    match crate::file_actions::copy_text(hwnd, &text) {
        Ok(()) => {
            log::info(&format!("已复制 {} 条路径", paths.len()));
            let notice = if crate::i18n::is_en() {
                if paths.len() > 1 {
                    format!("Copied {} paths", paths.len())
                } else {
                    "Copied path".to_string()
                }
            } else if paths.len() > 1 {
                format!("已复制 {} 条路径", paths.len())
            } else {
                "已复制路径".to_string()
            };
            show_drop_notice(notice);
            true
        }
        Err(error) => {
            log::error(&format!("复制路径失败：{error}"));
            false
        }
    }
}

/// Ctrl+C：复制当前选中的图标路径（支持多选和搜索结果首项）。
pub fn copy_selected_paths() -> bool {
    let app = state();
    let mut indices = app.selection.clone();
    if indices.is_empty() {
        if let Some(index) = active_selected_item(app) {
            indices.push(index);
        }
    }
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for index in indices {
        let Some(item) = app.items.get(index) else {
            continue;
        };
        if paths.iter().any(|known| icons::same_path(known, &item.path)) {
            continue;
        }
        paths.push(item.path.clone());
    }
    copy_paths_to_clipboard(app.overlay, &paths)
}

/// 非搜索态按 Backspace：若当前有下钻中的文件夹入口，则返回上一级目录。
pub fn portal_back_if_drilled() -> bool {
    let portal_index = {
        let app = state();
        let from_selected = app
            .selected
            .and_then(|index| app.items.get(index))
            .filter(|item| item.source == icons::ItemSource::Portal)
            .and_then(|item| crate::groups::group_of(&app.groups, &item.path))
            .and_then(|g| app.groups.get(g))
            .and_then(|g| g.portal)
            .filter(|&p| app.portals.get(p).is_some_and(|portal| portal.is_drilled()));
        from_selected.or_else(|| {
            app.portals
                .iter()
                .enumerate()
                .rev()
                .find_map(|(i, portal)| portal.is_drilled().then_some(i))
        })
    };
    portal_index.is_some_and(crate::app::portal_step_up)
}

/// Delete：移除多选（或键盘选中）的项；返回是否处理了按键。
pub fn remove_selection() -> bool {
    let app = state();
    if !app.search_edit.is_null() || !app.search.is_empty() {
        return false;
    }
    let mut indices = app.selection.clone();
    if indices.is_empty()
        && let Some(selected) = app.selected
    {
        indices.push(selected);
    }
    indices.retain(|index| {
        *index < app.items.len() && app.items[*index].source != icons::ItemSource::Portal
    });
    if indices.is_empty() {
        return false;
    }
    crate::app::remove_items(&indices);
    true
}

pub(super) fn auto_scroll_tick(hwnd: HWND) {
    let app = state();
    let Some(drag) = app.drag else {
        return;
    };
    if !drag.active {
        return;
    }
    let mut point = POINT::default();
    // SAFETY: 输出结构体有效；句柄有效。
    unsafe {
        GetCursorPos(&mut point);
        windows_sys::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point);
    }
    let scale = app.layout.scale;
    let top_zone = content_top(app) + (28.0 * scale) as i32;
    let bottom_zone = app.layout.height - (80.0 * scale) as i32;
    let step = (14.0 * scale) as i32;
    let delta = if point.y < top_zone {
        -step
    } else if point.y > bottom_zone {
        step
    } else {
        0
    };
    if delta == 0 {
        return;
    }
    let next = (app.scroll + delta).clamp(0, max_scroll(app));
    if next == app.scroll {
        return;
    }
    app.scroll = next;
    app.static_dirty = true;
    // 内容滚了、指针没动，目标组可能已经变了；刷新一次，松手时用最新目标。
    refresh_drag_hover(app, drag.from, point.x, point.y);
    // SAFETY: 句柄有效；整屏重画一帧，拖拽 ghost 与目标高亮由 paint 负责。
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 0);
        UpdateWindow(hwnd);
    }
}

pub(super) fn launch_and_hide(index: usize, as_admin: bool) {
    let app = state();
    let Some(item) = app.items.get(index) else {
        return;
    };
    let path = item.path.clone();
    let is_portal = item.source == icons::ItemSource::Portal;
    let is_custom = item.source == icons::ItemSource::Custom;
    if !as_admin && is_portal && path.is_dir() && crate::app::enter_portal_subdir(&path) {
        return;
    }
    if is_custom && !path.exists() {
        crate::app::refresh_custom_availability();
        relayout();
        show_drop_notice(crate::i18n::t("路径暂不可用，请检查设备连接", "Path unavailable. Check the device connection.").into());
        return;
    }
    hide_panel();
    let result = if as_admin {
        icons::launch_as_admin(&path)
    } else {
        icons::launch(&path)
    };
    match result {
        Ok(()) => crate::app::record_launch(&path),
        Err(error) => log::error(&error),
    }
}

pub(super) unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_COMMAND if (wparam >> 16) as u32 == EN_CHANGE && wparam & 0xffff == 2001 => {
            let app = state();
            let edit = lparam as HWND;
            if !edit.is_null() && edit == app.search_edit {
                let capacity = unsafe { GetWindowTextLengthW(edit) } + 1;
                let mut buffer = vec![0; capacity as usize];
                let count = unsafe { GetWindowTextW(edit, buffer.as_mut_ptr(), capacity) };
                app.search = String::from_utf16_lossy(&buffer[..count as usize]);
                apply_search(app);
            }
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_NCHITTEST => HTCLIENT as LRESULT,
        WM_TIMER if wparam == TIMER_GROUP_FOLD => {
            let app = state();
            if let Some(fold) = app.fold.as_ref() {
                let position = fold.openness(Instant::now());
                if position == fold.to {
                    finish_fold(app);
                } else {
                    fold.frame(position, &mut app.layout);
                    clamp_scroll(app);
                }
                app.static_dirty = true;
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            }
            0
        }
        WM_TIMER if wparam == TIMER_SEARCH => {
            let app = state();
            let finished = app
                .search_anim
                .is_some_and(|started| started.elapsed() >= Duration::from_millis(160));
            if finished {
                app.search_anim = None;
                // SAFETY: 句柄有效。
                unsafe { KillTimer(hwnd, TIMER_SEARCH) };
                // SAFETY: 句柄有效；让最终状态整屏落定。
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            } else {
                // SAFETY: 句柄有效；只重画工具条。
                unsafe {
                    let bar = toolbar_rect(app);
                    InvalidateRect(hwnd, &bar, 0);
                    UpdateWindow(hwnd);
                }
            }
            0
        }
        WM_TIMER if wparam == TIMER_AUTOSCROLL => {
            auto_scroll_tick(hwnd);
            0
        }
        WM_TIMER if wparam == TIMER_DROP_NOTICE => {
            let app = state();
            app.drop_notice = None;
            app.static_dirty = true;
            // SAFETY: 句柄有效；提示条在静态帧里，整屏重画一次去掉它。
            unsafe {
                KillTimer(hwnd, TIMER_DROP_NOTICE);
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            0
        }

        WM_MOUSEMOVE => {
            let app = state();
            if let Some(mut marquee) = app.marquee.take() {
                let (x, y) = mouse_xy(lparam);
                marquee.current = (x, y);
                let moved = (x - marquee.start.0).abs() > 4 || (y - marquee.start.1).abs() > 4;
                if moved {
                    marquee.active = true;
                }
                if marquee.active {
                    let top_limit = content_top(app);
                    let start_c = (marquee.start.0, marquee.start.1.max(top_limit) + app.scroll);
                    let cur_c = (marquee.current.0, marquee.current.1.max(top_limit) + app.scroll);
                    let rect = normalized_rect(start_c, cur_c);
                    let mut hits = marquee_hit_items(&app.layout.tiles, app.items.len(), rect);
                    hits.retain(|&idx| visible(app, idx));
                    let mut next = if marquee.additive {
                        marquee.base_selection.clone()
                    } else {
                        Vec::new()
                    };
                    for idx in hits {
                        if !next.contains(&idx) {
                            next.push(idx);
                        }
                    }
                    app.recent_selected = None;
                    app.selected = next.last().copied();
                    app.selection_anchor = next.first().copied();
                    app.selection = next;
                    // SAFETY: 句柄有效；选框覆盖区域需要即时刷新。
                    unsafe {
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                        UpdateWindow(hwnd);
                    }
                }
                app.marquee = Some(marquee);
                return 0;
            }
            // 拖着分组标题走：算插入位置并画指示线（整屏重贴缓存，拖标题不频繁）。
            if let Some(mut header) = app.header_drag {
                let (x, y) = mouse_xy(lparam);
                let moved = (x - header.start.0).abs() > 4 || (y - header.start.1).abs() > 4;
                if moved {
                    let is_portal = app
                        .groups
                        .get(header.from)
                        .is_some_and(|group| group.portal.is_some());
                    if !is_portal {
                        if !header.active {
                            header.active = true;
                            header.target = Some(header.from);
                        }
                        let target = header_insert_target(app, y);
                        let changed = header.target != Some(target);
                        header.target = Some(target);
                        // 先把新状态写回全局，再重绘——paint 读的是 App 里的副本，
                        // 顺序反了就会一直画上一次的位置（表现为“指示线不出现”）。
                        app.header_drag = Some(header);
                        if changed {
                            log::info(&format!("标题拖动：插入位置 {target}"));
                            // SAFETY: 句柄有效；整屏重贴缓存再画指示线。
                            unsafe {
                                InvalidateRect(hwnd, std::ptr::null(), 0);
                                UpdateWindow(hwnd);
                            }
                        }
                    }
                }
                return 0;
            }
            if let Some(mut drag) = app.drag {
                let previous_point = drag.current;
                drag.current = mouse_xy(lparam);
                let moved_x = (drag.current.0 - drag.start.0).abs();
                let moved_y = (drag.current.1 - drag.start.1).abs();
                if !drag.active && (moved_x > 4 || moved_y > 4) {
                    // 入口项是只读视图，不进入拖动（单击仍然可以打开）。
                    if app
                        .items
                        .get(drag.from)
                        .is_some_and(|item| item.source == crate::icons::ItemSource::Portal)
                    {
                        return 0;
                    }
                    drag.active = true;
                    // 拖到上下边缘时自动滚动。
                    // SAFETY: 句柄有效；30ms 一次。
                    unsafe { SetTimer(hwnd, TIMER_AUTOSCROLL, 30, None) };
                    drag.last_paint = None;
                    drag.ghost = None;
                    drag.target = None;
                    // 让源图标从静态帧里消失，避免和跟随图标重影。
                    app.drag_hidden = Some(drag.from);
                    app.static_dirty = true;
                    // SAFETY: 句柄有效；这一帧要把新的静态帧整屏贴上。
                    unsafe {
                        InvalidateRect(hwnd, std::ptr::null(), 0);
                    }
                }
                if drag.active {
                    // 只在鼠标真的移动了才重画。
                    let moved = (drag.current.0 - previous_point.0).abs() > 0
                        || (drag.current.1 - previous_point.1).abs() > 0;
                    let now = Instant::now();
                    let due = drag
                        .last_paint
                        .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(8));
                    if moved && due {
                        drag.last_paint = Some(now);
                        let icon = app.layout.icon;
                        let new_ghost = ghost_rect(drag.current, icon);
                        let target = hit_test(drag.current.0, drag.current.1)
                            .filter(|index| *index < app.items.len());
                        // 落点规则和松手时共用：按组区域算，排除源组、只读入口不收。
                        let hover_header = drag_drop_group(
                            app,
                            drag.from,
                            drag.current.0,
                            drag.current.1,
                        )
                        .and_then(|group| drag_header_inner(app, group).map(|rect| (group, rect)));
                        // 先把缓存刷成当前布局，否则贴回的是旧画面（表现为“涂抹”）。
                        ensure_cache(hwnd);
                        // 直接画，不进 WM_PAINT：先贴回旧区域，再画新的跟随图标/目标框。
                        // SAFETY: 句柄有效；缓存存在时才有意义。
                        unsafe {
                            let hdc = GetDC(hwnd);
                            if !hdc.is_null() && !app.buffer_dc.is_null() {
                                if let Some(old) = drag.ghost {
                                    blit_region(hdc, app.buffer_dc, old);
                                }
                                if let Some(index) = drag.target
                                    && let Some(raw) = app.layout.tiles.get(index).copied()
                                {
                                    blit_region(hdc, app.buffer_dc, view_rect(app, raw));
                                }
                                if let Some(index) = target
                                    && let Some(raw) = app.layout.tiles.get(index).copied()
                                {
                                    draw_target_outline(hdc, view_rect(app, raw), app.layout.scale, app.theme);
                                }
                                // 悬停到目标组：标题行填色，松手就移入。
                                if let Some(old) = drag.header_rect {
                                    blit_region(hdc, app.buffer_dc, old);
                                }
                                if let Some((group, _)) = hover_header {
                                    draw_drag_header_hover(hdc, app, group);
                                }
                                // 缓存里没有工具条，拖动经过时补画，避免被贴成黑块。
                                let full = RECT {
                                    left: 0,
                                    top: 0,
                                    right: app.layout.width,
                                    bottom: app.layout.height,
                                };
                                let font = ui_font(app);
                                if !font.is_null() {
                                    SelectObject(hdc, font as _);
                                }
                                draw_toolbar(hdc, app, &full);
                                draw_search_box(hdc, app, &full);
                                draw_ghost_icon(hdc, app, drag.from, drag.current);
                            }
                            if !hdc.is_null() {
                                ReleaseDC(hwnd, hdc);
                            }
                        }
                        drag.ghost = Some(new_ghost);
                        drag.target = target;
                        if drag.header != hover_header.map(|(group, _)| group) {
                            if let Some((group, _)) = hover_header {
                                let name = app
                                    .groups
                                    .get(group)
                                    .map(|entry| entry.name.clone())
                                    .unwrap_or_default();
                                log::info(&format!("拖动目标组：{name}"));
                            }
                        }
                        drag.header = hover_header.map(|(group, _)| group);
                        drag.header_rect = hover_header.map(|(_, rect)| rect);
                    }
                }
                app.drag = Some(drag);
            }
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = mouse_xy(lparam);
            let target = hit_test(x, y);
            let app = state();
            if let Some(marquee) = app.marquee.take() {
                // SAFETY: 捕获由本窗口持有。
                unsafe { ReleaseCapture() };
                if marquee.active {
                    log::info(&format!("拉框多选：{} 项", app.selection.len()));
                    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
                } else {
                    log::info(&format!("单击空白 ({x},{y})，收起"));
                    hide_panel();
                }
                return 0;
            }
            // 松手结束标题拖动：拖过就是调顺序，没拖就是单击折叠/展开。
            if let Some(header) = app.header_drag.take() {
                // SAFETY: 捕获由本窗口持有。
                unsafe { ReleaseCapture() };
                if header.active {
                    if let Some(target) = header.target {
                        move_group(header.from, target);
                    }
                    // SAFETY: 句柄有效；清掉指示线。
                    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
                } else {
                    toggle_group(header.from);
                }
                return 0;
            }
            if let Some(drag) = app.drag.take() {
                // SAFETY: 捕获由本窗口持有。
                unsafe { ReleaseCapture() };
                // 还原静态帧：源图标回到格子里。
                app.drag_hidden = None;
                app.static_dirty = true;
                if drag.active {
                    // 以松手位置为准重算目标组：最后一次 mousemove 可能被节流，
                    // 自动滚动时指针没动、指针下的组也会变。
                    let group = drag_drop_group(app, drag.from, x, y);
                    if let Some(group) = group {
                        let name = app
                            .groups
                            .get(group)
                            .map(|group| crate::groups::display_name(&group.name))
                            .unwrap_or_default();
                        // 多选时整组移入；只拖一个时与原行为一致。
                        let indices = if app.selection.len() > 1
                            && app.selection.contains(&drag.from)
                        {
                            app.selection.clone()
                        } else {
                            vec![drag.from]
                        };
                        let moved = crate::app::move_items_to_group(&indices, group);
                        if moved > 0 {
                            let notice = if crate::i18n::is_en() {
                                if moved > 1 {
                                    format!("Moved {moved} into \"{name}\"")
                                } else {
                                    format!("Moved into \"{name}\"")
                                }
                            } else if moved > 1 {
                                format!("已移入「{name}」{moved} 项")
                            } else {
                                format!("已移入「{name}」")
                            };
                            show_drop_notice(notice);
                        }
                    } else if let Some(group) = group_under_cursor(app, x, y)
                        && app
                            .groups
                            .get(group)
                            .is_some_and(|entry| entry.portal.is_some())
                    {
                        let name = app
                            .groups
                            .get(group)
                            .map(|group| group.name.clone())
                            .unwrap_or_default();
                        log::info(&format!("拖动落点在只读入口「{name}」上，忽略"));
                        show_drop_notice(
                            crate::i18n::t(
                                "入口是只读视图，请拖到其它分组",
                                "Folder entries are read-only; drop into another group",
                            )
                            .to_string(),
                        );
                    } else if let Some(target) = target
                        && target != drag.from
                        && app.search.is_empty()
                        && target < app.items.len()
                    {
                        crate::app::reorder_items(drag.from, target);
                    }
                    // 松手后整屏重贴一次，清掉跟随图标留下的痕迹。
                    // SAFETY: 句柄有效。
                    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
                } else if visible(app, drag.from) {
                    let name = app
                        .items
                        .get(drag.from)
                        .map(|item| item.name.clone())
                        .unwrap_or_default();
                    log::info(&format!("单击图标 #{} {name}", drag.from));
                    launch_and_hide(drag.from, false);
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            // 面板不抢焦点，靠系统“悬停时滚动非活动窗口”把滚轮送过来。
            let app = state();
            if !app.shown {
                return 0;
            }
            let max = max_scroll(app);
            if max <= 0 {
                return 0;
            }
            let delta = ((wparam >> 16) & 0xffff) as u16 as i16 as i32;
            let step = (app.layout.cell_h / 2).max(48);
            let next = (app.scroll - (delta / 120) * step).clamp(0, max);
            if next != app.scroll {
                app.scroll = next;
                app.static_dirty = true;
                // SAFETY: 句柄有效；内容整体位移，整屏重画一帧。
                unsafe {
                    InvalidateRect(app.overlay, std::ptr::null(), 0);
                    UpdateWindow(app.overlay);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            // 输入框开着时，点面板第一下先关掉输入框。
            if crate::text_input::is_open() {
                crate::text_input::cancel();
                return 0;
            }
            {
                let app = state();
                if app.pending_remove.take().is_some() {
                    app.static_dirty = true;
                }
            }
            if !within_cooldown() && !animating() {
                let (x, y) = mouse_xy(lparam);
                let point = POINT { x, y };
                if !state().onboarded {
                    // 点「知道了」只是收卡片；点别处照常执行，同时把卡片收掉。
                    let button = onboarding_button_rect(state());
                    let in_button = unsafe { PtInRect(&button, point) != 0 };
                    dismiss_onboarding();
                    if in_button {
                        return 0;
                    }
                }
                let (in_settings, in_new_group, in_fold_all, in_chip, in_search) = unsafe {
                    let app = state();
                    (
                        PtInRect(&settings_chip_rect(app), point) != 0,
                        PtInRect(&new_group_chip_rect(app), point) != 0,
                        PtInRect(&fold_all_chip_rect(app), point) != 0,
                        PtInRect(&remove_chip_rect(app), point) != 0,
                        PtInRect(&search_rect(app), point) != 0,
                    )
                };
                if in_settings {
                    log::info("点击左上角「设置」");
                    crate::app::clear_selection();
                    open_settings();
                    return 0;
                }
                if in_new_group {
                    crate::app::clear_selection();
                    create_group();
                    return 0;
                }
                if in_fold_all {
                    crate::app::clear_selection();
                    toggle_all_groups();
                    return 0;
                }
                if in_search {
                    log::info("点击搜索按钮");
                    crate::app::clear_selection();
                    if state().search_edit.is_null() {
                        activate_typing();
                        unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
                    } else {
                        close_search();
                    }
                    return 0;
                }
                if in_chip {
                    crate::app::clear_selection();
                    let app = state();
                    app.remove_mode = !app.remove_mode;
                    app.static_dirty = true;
                    log::info(&format!(
                        "移除模式：{}",
                        if app.remove_mode { "开启" } else { "关闭" }
                    ));
                    // SAFETY: 句柄有效。
                    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
                    return 0;
                }
                // 搜索栏开着时，点空白处先收起搜索；点结果照常打开。
                if state().search_open && hit_test(x, y).is_none() {
                    close_search();
                    return 0;
                }
                if !state().remove_mode {
                    if let Some(index) = hit_test_recent(state(), x, y) {
                        log::info(&format!("点击最近常用项 #{index}"));
                        launch_and_hide(index, false);
                        return 0;
                    }
                    if in_recent_bar(state(), x, y) {
                        return 0;
                    }
                }
                // 整条分组标题都能点：若命中下钻入口的「← 上一级」按钮直接返回上一级；
                // 否则按下先记录，松开没拖动就折叠/展开，拖过就是调顺序。
                if let Some(group) = header_at(x, y) {
                    crate::app::clear_selection();
                    let drilled_up = {
                        let app = state();
                        app.groups
                            .get(group)
                            .and_then(|g| g.portal)
                            .filter(|&p| app.portals.get(p).is_some_and(|portal| portal.is_drilled()))
                            .and_then(|portal_index| {
                                let header_rect = sticky_header(app)
                                    .filter(|(g, _)| *g == group)
                                    .map(|(_, rect)| rect)
                                    .or_else(|| {
                                        app.layout
                                            .headers
                                            .iter()
                                            .find(|h| h.group == group)
                                            .map(|h| view_rect(app, h.rect))
                                    })?;
                                let up = portal_up_rect(header_rect, app.layout.scale);
                                (unsafe { PtInRect(&up, point) != 0 }).then_some(portal_index)
                            })
                    };
                    if let Some(portal_index) = drilled_up {
                        crate::app::portal_step_up(portal_index);
                        return 0;
                    }
                    let app = state();
                    app.pending_remove = None;
                    app.header_drag = Some(crate::state::HeaderDragState {
                        from: group,
                        start: (x, y),
                        active: false,
                        target: None,
                    });
                    // SAFETY: 捕获鼠标，拖出标题行也能收到消息。
                    unsafe { SetCapture(hwnd) };
                    return 0;
                }
                if state().remove_mode {
                    if let Some(index) = hit_test(x, y)
                        && index < state().items.len()
                        && visible(state(), index)
                    {
                        let app = state();
                        let indices = if app.selection.len() > 1
                            && app.selection.contains(&index)
                        {
                            app.selection.clone()
                        } else {
                            vec![index]
                        };
                        crate::app::remove_items(&indices);
                    }
                    return 0;
                }
                match hit_test(x, y) {
                    Some(index) if index == state().items.len() => {
                        log::info("点击「添加图标」");
                        begin_add_icon();
                    }
                    Some(index) if !visible(state(), index) => {
                        // 搜索过滤掉的项不可操作。
                    }
                    Some(index) => {
                        // Ctrl/Shift 点选只改多选，不启动拖动。
                        // SAFETY: 只读取按键状态。
                        let (ctrl, shift) = unsafe {
                            (
                                GetKeyState(i32::from(VK_CONTROL)) < 0,
                                GetKeyState(i32::from(VK_SHIFT)) < 0,
                            )
                        };
                        if ctrl || shift {
                            click_select(index, ctrl, shift);
                            return 0;
                        }
                        // 按下先记录，松开时若没拖动就是“打开”，拖动则是排序。
                        let app = state();
                        // 点在已有多选里的某一项时保留整组，方便整体拖动；
                        // 点在外面则改成只选这一项。
                        if !app.selection.contains(&index) {
                            app.selection.clear();
                            app.selection.push(index);
                            app.selection_anchor = Some(index);
                        }
                        app.selected = Some(index);
                        app.static_dirty = true;
                        app.drag = Some(crate::state::DragState {
                            from: index,
                            start: (x, y),
                            current: (x, y),
                            active: false,
                            last_paint: None,
                            ghost: None,
                            target: None,
                            header: None,
                            header_rect: None,
                        });
                        // SAFETY: 句柄有效；捕获鼠标，拖出格子也能收到消息。
                        unsafe { SetCapture(hwnd) };
                    }
                    None => {
                        let app = state();
                        if y >= content_top(app) && app.search.is_empty() && !app.remove_mode {
                            // SAFETY: 只读取 Ctrl 键状态。
                            let ctrl = unsafe { GetKeyState(i32::from(VK_CONTROL)) < 0 };
                            let base_selection = if ctrl {
                                app.selection.clone()
                            } else {
                                app.selection.clear();
                                app.selection_anchor = None;
                                app.selected = None;
                                app.recent_selected = None;
                                Vec::new()
                            };
                            app.marquee = Some(crate::state::MarqueeState {
                                start: (x, y),
                                current: (x, y),
                                active: false,
                                additive: ctrl,
                                base_selection,
                            });
                            unsafe { SetCapture(hwnd) };
                        } else {
                            log::info(&format!("单击空白 ({x},{y})，收起"));
                            hide_panel();
                        }
                    }
                }
            }
            0
        }
        WM_RBUTTONUP => {
            if !within_cooldown() && !animating() {
                let (x, y) = mouse_xy(lparam);
                let point = POINT { x, y };
                // 吸顶标题优先按它自己固定的位置判定。
                if let Some((group, rect)) = sticky_header(state()) {
                    // SAFETY: 矩形来自本模块布局。
                    if unsafe { PtInRect(&rect, point) != 0 } {
                        crate::app::clear_selection();
                        open_header_menu(group);
                        return 0;
                    }
                }
                let hovered_header = {
                    let app = state();
                    let content = content_point(app, point.x, point.y);
                    app.layout
                        .headers
                        .iter()
                        .find(|header| unsafe { PtInRect(&header.rect, content) != 0 })
                        .map(|header| header.group)
                };
                if let Some(group) = hovered_header {
                    crate::app::clear_selection();
                    open_header_menu(group);
                    return 0;
                }
                if !state().remove_mode
                    && let Some(index) = hit_test_recent(state(), x, y)
                {
                    let app = state();
                    app.selection.clear();
                    app.selection.push(index);
                    app.selection_anchor = Some(index);
                    app.selected = Some(index);
                    app.static_dirty = true;
                    open_group_menu(&[index]);
                    return 0;
                }
                if state().remove_mode {
                    if let Some(index) = hit_test(x, y)
                        && index < state().items.len()
                        && visible(state(), index)
                    {
                        let app = state();
                        let indices = if app.selection.len() > 1
                            && app.selection.contains(&index)
                        {
                            app.selection.clone()
                        } else {
                            vec![index]
                        };
                        crate::app::remove_items(&indices);
                    }
                } else if let Some(index) = hit_test(x, y)
                    && index < state().items.len()
                    && visible(state(), index)
                {
                    let app = state();
                    let in_multi =
                        app.selection.len() > 1 && app.selection.contains(&index);
                    let indices = if in_multi {
                        app.selection.clone()
                    } else {
                        app.selection.clear();
                        app.selection.push(index);
                        app.selection_anchor = Some(index);
                        app.selected = Some(index);
                        app.static_dirty = true;
                        vec![index]
                    };
                    log::info(&format!("右键图标 #{index}：选择分组"));
                    open_group_menu(&indices);
                }
            }
            0
        }
        WM_LBUTTONDBLCLK => 0,
        WM_DPICHANGED => {
            let app = state();
            finish_fold(app);
            app.tile_cache = None;
            let dpi = (wparam >> 16) & 0xffff;
            let dpi = if dpi == 0 { 96 } else { dpi as u32 };
            // 优先锁定所在显示器全屏范围（含负坐标副屏），拿不到再退回系统建议矩形。
            let monitor_rect = unsafe {
                let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                (GetMonitorInfoW(monitor, &mut info) != 0).then_some(info.rcMonitor)
            };
            let suggested = lparam as *const RECT;
            let target_rect = monitor_rect.or_else(|| {
                (!suggested.is_null()).then(|| unsafe { *suggested })
            });
            let mut width = app.layout.width;
            let mut height = app.layout.height;
            if let Some(rect) = target_rect {
                width = rect.right - rect.left;
                height = rect.bottom - rect.top;
                // SAFETY: 句柄有效。
                unsafe {
                    SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        rect.left,
                        rect.top,
                        width,
                        height,
                        SWP_NOACTIVATE,
                    );
                }
            }
            let top_dip = if app.low_integrity { 88.0 } else { 48.0 };
            let layout = compute_layout_full(
                &app.items,
                &app.groups,
                &app.usage,
                app.show_recent,
                width,
                height,
                dpi,
                top_dip,
                app.density.scale_factor(),
            );
            app.layout = layout;
            app.scroll = 0;
            app.static_dirty = true;
            if !app.search_edit.is_null() {
                let rect = search_rect(app);
                let font = ui_font(app);
                unsafe {
                    SetWindowPos(app.search_edit, std::ptr::null_mut(), rect.left, rect.top,
                        rect.right - rect.left, rect.bottom - rect.top, SWP_NOACTIVATE);
                    SendMessageW(app.search_edit, WM_SETFONT, font as usize, 1);
                }
            }
            if !app.search.is_empty() { apply_search(app); }
            log::info(&format!("DPI 变化：{dpi}，面板 {width}x{height}"));
            // SAFETY: 句柄有效。
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            0
        }
        _ => {
            // SAFETY: 默认处理其余消息。
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

