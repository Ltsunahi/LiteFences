//! 浮层模块：menu。

use std::time::{Instant};

use windows_sys::Win32::Foundation::{POINT};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, UpdateWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::{AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, HWND_NOTOPMOST, HWND_TOPMOST, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetForegroundWindow, SetTimer, SetWindowPos, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TPM_TOPALIGN, TrackPopupMenu, WM_NULL};

use crate::icons;
use crate::log;
use crate::state::{state};
use crate::util::{wide};

use super::{TIMER_GROUP_FOLD, hide_panel, relayout, show_drop_notice};
use super::layout::{GroupFold, finish_fold, group_motion_enabled, compute_layout_full};
use super::input::{copy_paths_to_clipboard, launch_and_hide};

/// 把分组 `from` 移到插入位置 `to`（0..=普通分组数），并持久化。
pub(super) fn move_group(from: usize, to: usize) {
    let app = state();
    if from >= app.groups.len() {
        return;
    }
    let limit = app
        .groups
        .iter()
        .position(|group| group.portal.is_some())
        .unwrap_or(app.groups.len());
    let mut target = to.min(limit);
    if target > from {
        target -= 1;
    }
    if target == from || target >= app.groups.len() {
        return;
    }
    crate::app::remember_undo("group");
    let name = app.groups[from].name.clone();
    let group = app.groups.remove(from);
    app.groups.insert(target, group);
    crate::groups::save(&app.groups);
    log::info(&format!(
        "调整分组顺序：「{name}」第 {} 位 → 第 {} 位",
        from + 1,
        target + 1
    ));
    relayout();
}

/// 工具条上的「新建分组」：输入名字后建立一个空组。
pub(super) fn create_group() {
    let Some(name) = crate::text_input::ask(crate::i18n::t("新建分组", "New group"), "") else {
        return;
    };
    let app = state();
    crate::app::remember_undo("group");
    let Some(index) = crate::groups::create(&mut app.groups, &name) else {
        return;
    };
    crate::groups::save(&app.groups);
    log::info(&format!("新建分组「{}」", app.groups[index].name));
    relayout();
}

/// 点标题行：折叠/展开该组并保存。
pub(super) fn toggle_group(index: usize) {
    let app = state();
    let Some(group) = app.groups.get_mut(index) else {
        return;
    };
    group.collapsed = !group.collapsed;
    let collapsed = group.collapsed;
    let portal_index = group.portal;
    let name = group.name.clone();
    if let Some(index) = portal_index {
        if let Some(portal) = app.portals.get_mut(index) {
            portal.collapsed = collapsed;
        }
        crate::portals::save(&app.portals);
    }
    crate::groups::save(&app.groups);
    log::info(&format!("分组「{name}」{}", if collapsed { "折叠" } else { "展开" }));
    if !group_motion_enabled(app) || index >= app.layout.headers.len() {
        relayout();
        return;
    }
    let now = Instant::now();
    let to = if collapsed { 0.0 } else { 1.0 };
    if let Some(fold) = app.fold.as_mut().filter(|fold| fold.group == index) {
        fold.from = fold.openness(now);
        fold.to = to;
        fold.start = now;
    } else {
        finish_fold(app);
        let current = &app.layout;
        let top = if app.low_integrity { 88.0 } else { 48.0 };
        let next = compute_layout_full(
            &app.items,
            &app.groups,
            &app.usage,
            app.show_recent,
            current.width,
            current.height,
            current.dpi,
            top,
            app.density.scale_factor(),
        );
        let (expanded, collapsed_layout) = if collapsed { (current.clone(), next) } else { (next, current.clone()) };
        app.fold = Some(GroupFold {
            group: index, expanded, collapsed: collapsed_layout,
            start: now, from: 1.0 - to, to,
            timing: std::env::var_os("LITEFENCES_FOLD_TIMING").map(|_| Vec::with_capacity(64)),
        });
    }
    if unsafe { SetTimer(app.overlay, TIMER_GROUP_FOLD, 10, None) } == 0 {
        finish_fold(app);
        unsafe { InvalidateRect(app.overlay, std::ptr::null(), 0) };
    }
}

/// 右键图标：选择要移入的分组、新建分组，或做文件操作。多选时动作作用于整组。
pub(super) fn open_group_menu(indices: &[usize]) {
    let app = state();
    let Some(&anchor) = indices.first() else {
        return;
    };
    let Some(anchor_item) = app.items.get(anchor) else {
        return;
    };
    if anchor_item.source == icons::ItemSource::Portal {
        log::info("入口是只读视图：请右键入口标题管理（重命名/折叠/移除）");
        return;
    }
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for &index in indices {
        let Some(item) = app.items.get(index) else {
            continue;
        };
        if item.source == icons::ItemSource::Portal {
            continue;
        }
        if paths.iter().any(|known| icons::same_path(known, &item.path)) {
            continue;
        }
        paths.push(item.path.clone());
    }
    if paths.is_empty() {
        return;
    }
    let multi = paths.len() > 1;
    let current = if multi {
        None
    } else {
        crate::groups::group_of(&app.groups, &paths[0])
    };
    let labels: Vec<_> = app
        .groups
        .iter()
        .map(|group| {
            wide(&if crate::i18n::is_en() {
                format!("Move to \"{}\"", crate::groups::display_name(&group.name))
            } else {
                format!("移到「{}」", crate::groups::display_name(&group.name))
            })
        })
        .collect();
    // SAFETY: 菜单句柄在函数末尾释放；字符串在调用期间保持存活。
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        if multi {
            let header = wide(&if crate::i18n::is_en() {
                format!("{} items selected", paths.len())
            } else {
                format!("已选 {} 项", paths.len())
            });
            AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, header.as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        } else {
            let rename_label = wide(crate::i18n::t("重命名…", "Rename…"));
            let icon_label = wide(crate::i18n::t("更换图标…", "Change icon…"));
            let reset_icon_label = wide(crate::i18n::t("恢复默认图标", "Reset icon"));
            AppendMenuW(menu, MF_STRING, 2, rename_label.as_ptr());
            if app
                .items
                .get(anchor)
                .and_then(|item| item.custom_icon.as_ref())
                .is_some()
            {
                AppendMenuW(menu, MF_STRING, 4, reset_icon_label.as_ptr());
            }
            AppendMenuW(menu, MF_STRING, 3, icon_label.as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        }
        let reveal_label = wide(crate::i18n::t("打开所在位置", "Show in Explorer"));
        let properties_label = wide(crate::i18n::t("属性", "Properties"));
        let admin_label = wide(crate::i18n::t("以管理员身份运行", "Run as administrator"));
        if !multi && app.items.get(anchor).is_some_and(|item| !item.offline) {
            AppendMenuW(menu, MF_STRING, 29, admin_label.as_ptr());
        }
        let copy_label = wide(&if multi {
            if crate::i18n::is_en() {
                format!("Copy {} paths", paths.len())
            } else {
                format!("复制 {} 条路径", paths.len())
            }
        } else {
            crate::i18n::t("复制路径", "Copy path").to_string()
        });
        AppendMenuW(menu, MF_STRING, 30, reveal_label.as_ptr());
        AppendMenuW(menu, MF_STRING, 31, properties_label.as_ptr());
        AppendMenuW(menu, MF_STRING, 32, copy_label.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        let remove_label = wide(&if multi {
            if crate::i18n::is_en() {
                format!("Remove {} items from panel", paths.len())
            } else {
                format!("从面板移除所选 {} 项", paths.len())
            }
        } else {
            crate::i18n::t("从面板移除", "Remove from panel").to_string()
        });
        AppendMenuW(menu, MF_STRING, 33, remove_label.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        for (index, label) in labels.iter().enumerate() {
            let flags = if current == Some(index) {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            AppendMenuW(menu, flags, 200 + index, label.as_ptr());
        }
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        let new_label = wide(crate::i18n::t("新建分组并移入…", "New group and move…"));
        AppendMenuW(menu, MF_STRING, 1, new_label.as_ptr());

        let overlay = app.overlay;
        // 面板是全屏置顶，先放下来菜单才看得见。
        SetWindowPos(
            overlay,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let mut point = POINT::default();
        GetCursorPos(&mut point);
        if let Some(work) = crate::util::monitor_work_area_at(point) {
            point = crate::util::clamp_point_in_rect(point, work);
        }
        SetForegroundWindow(overlay);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            overlay,
            std::ptr::null(),
        );
        SetWindowPos(
            overlay,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        DestroyMenu(menu);
        PostMessageW(overlay, WM_NULL, 0, 0);
        apply_group_command(command as usize, &paths, anchor);
    }
}

/// 右键分组标题：重命名 / 删除。
pub(super) fn open_header_menu(group_index: usize) {
    let app = state();
    let Some(group) = app.groups.get(group_index) else {
        return;
    };
    let is_default = group.name == crate::groups::DEFAULT_GROUP;
    let collapsed = group.collapsed;
    // SAFETY: 菜单在函数末尾销毁；字符串在调用期间存活。
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        let portal_info = group
            .portal
            .and_then(|index| app.portals.get(index).map(|p| (index, p.sort, p.is_drilled())));
        let rename = wide(crate::i18n::t("重命名分组…", "Rename group…"));
        let toggle = wide(if collapsed {
            crate::i18n::t("展开该组", "Expand")
        } else {
            crate::i18n::t("折叠该组", "Collapse")
        });
        let all_collapsed = app.groups.iter().all(|g| g.collapsed);
        let toggle_all = wide(if all_collapsed {
            crate::i18n::t("全部展开", "Expand all")
        } else {
            crate::i18n::t("全部折叠", "Collapse all")
        });
        let rules_label = wide(&if group.rules.is_empty() {
            crate::i18n::t("自动归组规则…", "Auto-group rules…").to_string()
        } else if crate::i18n::is_en() {
            format!("Auto-group rules… ({})", group.rules.len())
        } else {
            format!("自动归组规则…（{} 条）", group.rules.len())
        });
        let delete = wide(if portal_info.is_some() {
            crate::i18n::t("移除入口（目录内容不动）", "Remove folder entry (files untouched)")
        } else {
            crate::i18n::t(
                "删除分组（图标回到「其它」）",
                "Delete group (items go to \"Others\")",
            )
        });
        AppendMenuW(menu, MF_STRING, 10, rename.as_ptr());
        if portal_info.is_none() && !is_default {
            AppendMenuW(menu, MF_STRING, 15, rules_label.as_ptr());
        }
        if portal_info.is_none() {
            let color_sub = CreatePopupMenu();
            if !color_sub.is_null() {
                let current_color = group.color.as_deref();
                let none_label = wide(crate::i18n::t("无（默认）", "None (default)"));
                let none_flags = if current_color.is_none() {
                    MF_STRING | MF_CHECKED
                } else {
                    MF_STRING
                };
                AppendMenuW(color_sub, none_flags, 30, none_label.as_ptr());
                AppendMenuW(color_sub, MF_SEPARATOR, 0, std::ptr::null());
                let labels: Vec<_> = crate::theme::GROUP_COLORS
                    .iter()
                    .map(|c| wide(c.display_name()))
                    .collect();
                for ((idx, entry), label) in crate::theme::GROUP_COLORS.iter().enumerate().zip(labels.iter()) {
                    let flags = if current_color.is_some_and(|k| k.eq_ignore_ascii_case(entry.key)) {
                        MF_STRING | MF_CHECKED
                    } else {
                        MF_STRING
                    };
                    AppendMenuW(color_sub, flags, 31 + idx, label.as_ptr());
                }
                let sub_title = wide(crate::i18n::t("分组色标", "Group color"));
                if AppendMenuW(menu, MF_POPUP, color_sub as usize, sub_title.as_ptr()) == 0 {
                    DestroyMenu(color_sub);
                }
            }
            let sort_sub = CreatePopupMenu();
            if !sort_sub.is_null() {
                let by_name = wide(crate::i18n::t("按名称", "By name"));
                let by_ext = wide(crate::i18n::t("按类型（扩展名）", "By type (extension)"));
                let by_usage = wide(crate::i18n::t("按常用频次", "By most used"));
                AppendMenuW(sort_sub, MF_STRING, 40, by_name.as_ptr());
                AppendMenuW(sort_sub, MF_STRING, 41, by_ext.as_ptr());
                AppendMenuW(sort_sub, MF_STRING, 42, by_usage.as_ptr());
                let sort_title = wide(crate::i18n::t("组内排序", "Sort items"));
                if AppendMenuW(menu, MF_POPUP, sort_sub as usize, sort_title.as_ptr()) == 0 {
                    DestroyMenu(sort_sub);
                }
            }
        }
        AppendMenuW(menu, MF_STRING, 11, toggle.as_ptr());
        AppendMenuW(menu, MF_STRING, 14, toggle_all.as_ptr());
        let open_folder = wide(crate::i18n::t("在资源管理器中打开", "Open in Explorer"));
        let refresh_entry = wide(crate::i18n::t("刷新该入口", "Refresh entry"));
        let step_up = wide(crate::i18n::t("返回上一级", "Up one level"));
        let reset_root = wide(crate::i18n::t("回到入口根目录", "Back to entry root"));
        if let Some((_, _, drilled)) = portal_info {
            AppendMenuW(menu, MF_STRING, 16, refresh_entry.as_ptr());
            if drilled {
                AppendMenuW(menu, MF_STRING, 17, step_up.as_ptr());
                AppendMenuW(menu, MF_STRING, 18, reset_root.as_ptr());
            }
            AppendMenuW(menu, MF_STRING, 13, open_folder.as_ptr());
        }
        // 入口分组多一份排序方式，当前项打勾。
        if let Some((_, current, _)) = portal_info {
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            let modes = [
                (20, crate::portals::SortMode::FolderFirst),
                (21, crate::portals::SortMode::Name),
                (22, crate::portals::SortMode::Time),
            ];
            let labels: Vec<_> = modes
                .iter()
                .map(|(_, mode)| wide(&format!("{} {}", crate::i18n::t("排序：", "Sort:"), mode.label())))
                .collect();
            for ((id, mode), label) in modes.iter().zip(labels.iter()) {
                let flags = if current == *mode {
                    MF_STRING | MF_CHECKED
                } else {
                    MF_STRING
                };
                AppendMenuW(menu, flags, *id, label.as_ptr());
            }
        }
        if !is_default {
            AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            AppendMenuW(menu, MF_STRING, 12, delete.as_ptr());
        }
        let overlay = app.overlay;
        SetWindowPos(
            overlay,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let mut point = POINT::default();
        GetCursorPos(&mut point);
        if let Some(work) = crate::util::monitor_work_area_at(point) {
            point = crate::util::clamp_point_in_rect(point, work);
        }
        SetForegroundWindow(overlay);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            overlay,
            std::ptr::null(),
        );
        SetWindowPos(
            overlay,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        DestroyMenu(menu);
        PostMessageW(overlay, WM_NULL, 0, 0);
        match command as usize {
            10 => rename_group(group_index),
            11 => toggle_group(group_index),
            14 => toggle_all_groups(),
            15 => edit_group_rules(group_index),
            16 => crate::app::refresh_portal(group_index),
            17 => {
                if let Some((portal_index, _, _)) = portal_info {
                    crate::app::portal_step_up(portal_index);
                }
            }
            18 => {
                if let Some((portal_index, _, _)) = portal_info {
                    crate::app::portal_reset_root(portal_index);
                }
            }
            12 => delete_group(group_index),
            13 => {
                let dir = state()
                    .groups
                    .get(group_index)
                    .and_then(|group| group.portal)
                    .and_then(|index| state().portals.get(index))
                    .map(|portal| portal.active_dir().to_path_buf());
                if let Some(dir) = dir {
                    hide_panel();
                    if let Err(error) = crate::file_actions::reveal(&dir) {
                        log::error(&format!("打开入口目录失败：{error}"));
                    }
                }
            }
            20 => crate::app::set_portal_sort(group_index, crate::portals::SortMode::FolderFirst),
            21 => crate::app::set_portal_sort(group_index, crate::portals::SortMode::Name),
            22 => crate::app::set_portal_sort(group_index, crate::portals::SortMode::Time),
            30 => crate::app::set_group_color(group_index, None),
            31..=36 => {
                if let Some(entry) = crate::theme::GROUP_COLORS.get((command as usize) - 31) {
                    crate::app::set_group_color(group_index, Some(entry.key));
                }
            }
            40 => crate::app::sort_group_items(group_index, crate::app::GroupSortMode::Name),
            41 => crate::app::sort_group_items(group_index, crate::app::GroupSortMode::Extension),
            42 => crate::app::sort_group_items(group_index, crate::app::GroupSortMode::Usage),
            _ => {}
        }
    }
}

pub(super) fn rename_group(index: usize) {
    let app = state();
    let Some(group) = app.groups.get(index) else { return };
    let current = group.portal.and_then(|index| app.portals.get(index))
        .map_or_else(|| group.name.clone(), |portal| portal.name.clone());
    if current.is_empty() {
        return;
    }
    let Some(name) = crate::text_input::ask(crate::i18n::t("重命名分组", "Rename group"), &current) else {
        return;
    };
    let name = name.trim().to_string();
    if name.is_empty() || name == current {
        return;
    }
    crate::app::remember_group_undo(index);
    let app = state();
    let Some(group) = app.groups.get_mut(index) else {
        return;
    };
    if let Some(index) = group.portal {
        if let Some(portal) = app.portals.get_mut(index) {
            portal.name = name.clone();
            group.name = crate::app::portal_group_title(portal);
        } else {
            group.name = format!("{}{name}", crate::i18n::t("入口：", "Entry: "));
        }
        crate::portals::save(&app.portals);
    } else {
        group.name = name.clone();
        crate::groups::save(&app.groups);
    }
    log::info(&format!("分组改名：{current} → {name}"));
    relayout();
}

pub(super) fn edit_group_rules(index: usize) {
    let app = state();
    let Some(group) = app.groups.get(index) else { return };
    if group.portal.is_some() || group.name == crate::groups::DEFAULT_GROUP {
        return;
    }
    let current = crate::groups::format_rules(&group.rules);
    let title = crate::i18n::t(
        "自动归组规则（如 .pdf, 会议, .xlsx，用逗号或空格分隔）",
        "Auto-group rules (e.g. .pdf, report, .xlsx)",
    );
    let Some(input) = crate::text_input::ask(title, &current) else {
        return;
    };
    if let Some(moved) = crate::app::set_group_rules(index, &input)
        && state().shown
    {
        let notice = if crate::i18n::is_en() {
            format!("Rules saved ({moved} item(s) grouped)")
        } else {
            format!("规则已保存（归入 {moved} 项）")
        };
        show_drop_notice(notice);
    }
}

pub(super) fn delete_group(index: usize) {
    let app = state();
    let Some(group) = app.groups.get(index) else {
        return;
    };
    if group.name == crate::groups::DEFAULT_GROUP {
        return;
    }
    let name = group.name.clone();
    crate::app::remember_group_undo(index);
    if let Some(portal_index) = group.portal {
        // 只读入口：只摘掉入口本身，不碰目录里的文件。
        if portal_index < app.portals.len() {
            app.portals.remove(portal_index);
        }
        crate::portals::save(&app.portals);
        log::info(&format!("移除文件夹入口「{name}」（目录内容不动）"));
        crate::app::rebuild_portal_items();
        relayout();
        return;
    }
    app.groups.remove(index);
    crate::groups::save(&app.groups);
    log::info(&format!("删除分组「{name}」，图标回到其它"));
    relayout();
}

pub(super) fn indices_for_paths(app: &crate::state::App, paths: &[std::path::PathBuf]) -> Vec<usize> {
    app.items
        .iter()
        .enumerate()
        .filter(|(_, item)| paths.iter().any(|path| icons::same_path(path, &item.path)))
        .map(|(index, _)| index)
        .collect()
}

pub(super) fn apply_group_command(
    command: usize,
    paths: &[std::path::PathBuf],
    anchor: usize,
) {
    if command == 0 || paths.is_empty() {
        return;
    }
    let app = state();
    match command {
        1 => {
            if let Some(name) = crate::text_input::ask(crate::i18n::t("新建分组", "New group"), "")
            {
                crate::app::remember_undo("group");
                let Some(index) = crate::groups::create(&mut app.groups, &name) else {
                    return;
                };
                for path in paths {
                    crate::groups::assign(&mut app.groups, path, Some(index));
                }
                crate::groups::save(&app.groups);
                log::info(&format!("新建分组「{name}」并移入 {} 项", paths.len()));
                crate::app::persist_layout();
                relayout();
            }
        }
        2 => {
            let current = app
                .items
                .get(anchor)
                .map(|item| item.name.clone())
                .unwrap_or_default();
            if let Some(name) =
                crate::text_input::ask(crate::i18n::t("重命名图标", "Rename item"), &current)
            {
                crate::app::rename_item(anchor, &name);
            }
        }
        3 => {
            let host = app.host;
            if let Some(icon) = crate::picker::pick_icon_file(host)
                && !crate::app::set_item_icon(anchor, &icon)
            {
                log::warn("图标文件无法使用（只支持 .ico）");
            }
        }
        4 => crate::app::clear_item_icon(anchor),
        29 => launch_and_hide(anchor, true),
        30 => {
            let path = paths[0].clone();
            // 资源管理器需要到前台，先把全屏置顶面板收起来。
            hide_panel();
            if let Err(error) = crate::file_actions::reveal(&path) {
                log::error(&format!("打开所在位置失败：{error}"));
            }
        }
        31 => {
            let path = paths[0].clone();
            let hwnd = app.overlay;
            // 属性对话框否则会被置顶面板盖住。
            hide_panel();
            if let Err(error) = crate::file_actions::properties(hwnd, &path) {
                log::error(&format!("打开属性失败：{error}"));
            }
        }
        32 => {
            let _ = copy_paths_to_clipboard(app.overlay, paths);
        }
        33 => {
            let indices = indices_for_paths(app, paths);
            crate::app::remove_items(&indices);
        }
        other if other >= 200 => {
            let target = other - 200;
            let indices = indices_for_paths(app, paths);
            let moved = crate::app::move_items_to_group(&indices, target);
            if moved > 0 {
                let name = app
                    .groups
                    .get(target)
                    .map(|group| crate::groups::display_name(&group.name))
                    .unwrap_or_default();
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
        }
        _ => {}
    }
}

pub(super) fn dismiss_onboarding() {
    let app = state();
    if app.onboarded {
        return;
    }
    app.onboarded = true;
    crate::store::set_onboarded(true);
    // SAFETY: 句柄有效。
    unsafe {
        InvalidateRect(app.overlay, std::ptr::null(), 0);
        UpdateWindow(app.overlay);
    }
    log::info("首次引导已关闭");
}

/// 打开设置菜单：先把面板从置顶拿下来，菜单关掉后再恢复。
pub(super) fn open_settings() {
    let app = state();
    let overlay = app.overlay;
    // SAFETY: 句柄有效。
    unsafe {
        SetWindowPos(
            overlay,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    crate::app::open_settings_menu();
    // SAFETY: 同上。
    unsafe {
        SetWindowPos(
            overlay,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// 点「添加图标」：面板不关，只是暂时取消置顶，让系统选择框显示在它上面。
pub(super) fn begin_add_icon() {
    let app = state();
    let host = app.host;
    let overlay = app.overlay;
    // SAFETY: 句柄有效；临时取消置顶，选完再恢复。
    unsafe {
        SetWindowPos(
            overlay,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    let picked = crate::picker::pick_icon_source(host);
    // SAFETY: 同上。
    unsafe {
        SetWindowPos(
            overlay,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    if let Some(path) = picked {
        crate::app::add_custom(path);
    }
}

/// Ctrl+Shift+C / Ctrl+Shift+E 或点击顶部按钮：一键全部折叠或全部展开。
pub fn set_all_groups_collapsed(collapsed: bool) {
    let app = state();
    if app.groups.is_empty() {
        return;
    }
    finish_fold(app);
    let mut changed = false;
    for group in &mut app.groups {
        if group.collapsed != collapsed {
            group.collapsed = collapsed;
            changed = true;
        }
        if let Some(portal_index) = group.portal
            && let Some(portal) = app.portals.get_mut(portal_index)
        {
            portal.collapsed = collapsed;
        }
    }
    if changed {
        crate::groups::save(&app.groups);
        crate::portals::save(&app.portals);
        app.tile_cache = None;
        relayout();
    }
    log::info(if collapsed { "分组全部折叠" } else { "分组全部展开" });
    if state().shown {
        show_drop_notice(if collapsed {
            crate::i18n::t("已折叠全部分组", "Collapsed all groups").into()
        } else {
            crate::i18n::t("已展开全部分组", "Expanded all groups").into()
        });
    }
}

pub(super) fn toggle_all_groups() {
    let all_collapsed = state().groups.iter().all(|g| g.collapsed);
    set_all_groups_collapsed(!all_collapsed);
}

