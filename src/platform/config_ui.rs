//! UI-thread session; the worker owns files and shares only an owned wake event.
use crate::app_options::StorageSmoke;
use litefences::core::{
    error_text::{error_code, error_hint},
    group_ops::{
        add_references, create_group, delete_group, move_group, remove_reference, rename_group,
        reorder_reference, resize_group, restore_group, restore_reference, set_group_color,
        toggle_group_collapsed, transfer_reference,
    },
    layout::RectDip,
};
use litefences::{
    core::{
        contracts::LoadSource,
        model::{Config, FileReference, Group, GroupId, Item, ItemId, NativePath, ReferenceKind},
        save_tracker::SaveTracker,
    },
    services::config_worker::{Command, ConfigWorker, DataRoot, Event},
};
use std::{
    io,
    sync::{Arc, mpsc::TryRecvError},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::Threading::{CreateEventW, SetEvent},
};

pub struct WakeEvent(usize);
impl WakeEvent {
    pub fn new() -> io::Result<Arc<Self>> {
        // SAFETY: unnamed auto-reset event, no borrowed security descriptor.
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Arc::new(Self(handle as usize)))
        }
    }
    pub fn handle(&self) -> HANDLE {
        self.0 as HANDLE
    }
    pub(crate) fn signal(&self) {
        // SAFETY: Arc keeps the event handle alive through this call on either thread.
        unsafe {
            SetEvent(self.handle());
        }
    }
}
impl Drop for WakeEvent {
    fn drop(&mut self) {
        // SAFETY: the last owner closes the sole owned handle exactly once.
        unsafe {
            CloseHandle(self.handle());
        }
    }
}

pub struct Session {
    pub wake: Arc<WakeEvent>,
    worker: ConfigWorker,
    config: Option<Arc<Config>>,
    loaded: bool,
    writable: bool,
    tracker: SaveTracker,
    pending_group: Option<(GroupId, GroupId)>,
    pending_rename: Option<(GroupId, String)>,
    pending_move: Option<(GroupId, RectDip)>,
    pending_resize: Option<(GroupId, RectDip)>,
    pending_delete: Option<(usize, Group)>,
    pending_color: Option<(GroupId, [u8; 3])>,
    pending_fold: Option<(GroupId, bool)>,
    pending_add: Option<(GroupId, Vec<ItemId>, ItemId)>,
    pending_remove: Option<(GroupId, usize, Item)>,
    pending_sort: Option<(GroupId, ItemId, usize)>,
    pending_transfer: Option<(GroupId, ItemId, GroupId, usize)>,
    last_removal: Option<UndoRemoval>,
    pending_undo: bool,
    load_source: Option<LoadSource>,
    closing: bool,
    shutdown_sent: bool,
    pub stopped: bool,
    pub status: String,
    pub details: String,
    smoke: Option<StorageSmoke>,
    pub smoke_passed: bool,
}

// One committed removal per session, bounded by the configuration limits.
// The record remains owned here until undo is durably saved (or superseded).
enum UndoRemoval {
    Group(usize, Group),
    Reference(GroupId, usize, Item),
}
pub struct GroupPreview {
    pub id: GroupId,
    pub title: String,
    pub rect: RectDip,
    pub color: [u8; 3],
    pub collapsed: bool,
}
pub struct ItemPreview {
    pub id: ItemId,
    pub kind: ReferenceKind,
    pub label: String,
    pub path: NativePath,
}

const MAX_PREVIEW_ITEMS: usize = 24;

fn preview_name(path: &NativePath) -> String {
    let units = path.as_utf16();
    let basename = units
        .rsplit(|unit| *unit == b'\\' as u16)
        .next()
        .unwrap_or(units);
    let mut chars = char::decode_utf16(basename.iter().copied())
        .map(|value| value.unwrap_or(char::REPLACEMENT_CHARACTER))
        .filter(|character| !character.is_control());
    let mut label: String = chars.by_ref().take(32).collect();
    if chars.next().is_some() {
        label.push('…');
    }
    if label.is_empty() {
        label.push_str("(无名称)");
    }
    label
}

impl Session {
    pub fn can_create(&self) -> bool {
        self.can_save()
    }
    pub fn group_titles(&self, limit: usize) -> Vec<String> {
        self.config.as_ref().map_or_else(Vec::new, |config| {
            config
                .groups
                .iter()
                .take(limit)
                .map(|g| format!("分组 {}：{}", g.id, g.title))
                .collect()
        })
    }
    pub fn group_id_at(&self, index: usize) -> Option<GroupId> {
        self.config
            .as_ref()?
            .groups
            .get(index)
            .map(|group| group.id)
    }
    pub fn group_title_at(&self, index: usize) -> Option<String> {
        self.config
            .as_ref()?
            .groups
            .get(index)
            .map(|group| group.title.clone())
    }
    pub fn group_rect_at(&self, index: usize) -> Option<RectDip> {
        self.config
            .as_ref()?
            .groups
            .get(index)
            .map(|group| group.rect)
    }
    pub fn group_color_at(&self, index: usize) -> Option<[u8; 3]> {
        self.config
            .as_ref()?
            .groups
            .get(index)
            .map(|group| group.color)
    }
    pub fn group_collapsed_at(&self, index: usize) -> Option<bool> {
        self.config
            .as_ref()?
            .groups
            .get(index)
            .map(|group| group.collapsed)
    }
    pub fn group_previews(&self) -> Vec<GroupPreview> {
        self.config.as_ref().map_or_else(Vec::new, |config| {
            config
                .groups
                .iter()
                .map(|group| GroupPreview {
                    id: group.id,
                    title: group.title.clone(),
                    rect: group.rect,
                    color: group.color,
                    collapsed: group.collapsed,
                })
                .collect()
        })
    }
    pub fn preview_items(&self, group_id: GroupId, limit: usize) -> (usize, Vec<ItemPreview>) {
        let Some(group) = self
            .config
            .as_ref()
            .and_then(|config| config.groups.iter().find(|group| group.id == group_id))
        else {
            return (0, Vec::new());
        };
        let visible = group
            .items
            .iter()
            .take(limit.min(MAX_PREVIEW_ITEMS))
            .map(|item| ItemPreview {
                id: item.id,
                kind: item.target.kind,
                label: preview_name(&item.target.path),
                path: item.target.path.clone(),
            })
            .collect();
        (group.items.len(), visible)
    }
    pub fn reference_for_item(&self, group: GroupId, item: ItemId) -> Option<FileReference> {
        self.config
            .as_ref()?
            .groups
            .iter()
            .find(|candidate| candidate.id == group)?
            .items
            .iter()
            .find(|candidate| candidate.id == item)
            .map(|candidate| candidate.target.clone())
    }
    fn refresh_details(&mut self) {
        if let (Some(source), Some(config)) = (self.load_source, self.config.as_ref()) {
            let source = match source {
                LoadSource::Empty => "空配置",
                LoadSource::Primary => "主配置",
                LoadSource::Backup => "备份恢复",
            };
            self.details = format!(
                "来源：{source}  ·  {} 个分组  ·  {} 个引用",
                config.groups.len(),
                config.groups.iter().map(|g| g.items.len()).sum::<usize>()
            );
        }
    }
    fn rollback_group(&mut self) {
        if let Some((id, old_next)) = self.pending_group.take() {
            let config = Arc::make_mut(self.config.as_mut().expect("pending config"));
            if config.groups.last().is_some_and(|g| g.id == id) {
                config.groups.pop();
                config.next_group_id = old_next;
                self.refresh_details();
            }
        }
    }
    fn rollback_rename(&mut self) {
        if let Some((id, old_title)) = self.pending_rename.take() {
            let config = Arc::make_mut(self.config.as_mut().expect("pending config"));
            if let Some(group) = config.groups.iter_mut().find(|group| group.id == id) {
                group.title = old_title;
            }
        }
    }
    fn rollback_move(&mut self) {
        if let Some((id, old_rect)) = self.pending_move.take() {
            let config = Arc::make_mut(self.config.as_mut().expect("pending config"));
            if let Some(group) = config.groups.iter_mut().find(|group| group.id == id) {
                group.rect = old_rect;
            }
        }
    }
    fn rollback_resize(&mut self) {
        if let Some((id, old_rect)) = self.pending_resize.take() {
            let config = Arc::make_mut(self.config.as_mut().expect("pending config"));
            if let Some(group) = config.groups.iter_mut().find(|group| group.id == id) {
                group.rect = old_rect;
            }
        }
    }
    fn rollback_delete(&mut self) {
        if let Some((index, group)) = self.pending_delete.take() {
            let config = Arc::make_mut(self.config.as_mut().expect("pending config"));
            if !config.groups.iter().any(|current| current.id == group.id) {
                config.groups.insert(index.min(config.groups.len()), group);
                self.refresh_details();
            }
        }
    }
    fn rollback_color(&mut self) {
        if let Some((id, old_color)) = self.pending_color.take() {
            let config = Arc::make_mut(self.config.as_mut().expect("pending config"));
            if let Some(group) = config.groups.iter_mut().find(|group| group.id == id) {
                group.color = old_color;
            }
        }
    }
    fn rollback_fold(&mut self) {
        if let Some((id, old_collapsed)) = self.pending_fold.take() {
            let config = Arc::make_mut(self.config.as_mut().expect("pending config"));
            if let Some(group) = config.groups.iter_mut().find(|group| group.id == id) {
                group.collapsed = old_collapsed;
            }
        }
    }
    fn rollback_add(&mut self) {
        if let Some((group_id, ids, old_next)) = self.pending_add.take() {
            let config = Arc::make_mut(self.config.as_mut().expect("pending config"));
            if let Some(group) = config.groups.iter_mut().find(|group| group.id == group_id) {
                let ids: std::collections::BTreeSet<_> = ids.into_iter().collect();
                group.items.retain(|item| !ids.contains(&item.id));
                config.next_item_id = old_next;
                self.refresh_details();
            }
        }
    }
    fn rollback_remove(&mut self) {
        if let Some((group_id, index, item)) = self.pending_remove.take() {
            let config = Arc::make_mut(self.config.as_mut().expect("pending config"));
            if let Some(group) = config.groups.iter_mut().find(|group| group.id == group_id)
                && !group.items.iter().any(|current| current.id == item.id)
            {
                group.items.insert(index.min(group.items.len()), item);
                self.refresh_details();
            }
        }
    }
    fn rollback_undo(&mut self) {
        if !std::mem::take(&mut self.pending_undo) {
            return;
        }
        let config = Arc::make_mut(self.config.as_mut().expect("pending undo config"));
        match self.last_removal.as_ref().expect("pending undo record") {
            UndoRemoval::Group(_, group) => {
                config.groups.retain(|current| current.id != group.id);
            }
            UndoRemoval::Reference(group_id, _, item) => {
                if let Some(group) = config.groups.iter_mut().find(|group| group.id == *group_id) {
                    group.items.retain(|current| current.id != item.id);
                }
            }
        }
        self.refresh_details();
    }
    pub fn can_undo_removal(&self) -> bool {
        self.can_save() && self.last_removal.is_some()
    }
    pub fn undo_removal(&mut self) -> bool {
        if !self.can_undo_removal() {
            return false;
        }
        let config = Arc::make_mut(self.config.as_mut().expect("loaded config"));
        let result = match self.last_removal.as_ref().expect("undo record") {
            UndoRemoval::Group(index, group) => restore_group(config, *index, group),
            UndoRemoval::Reference(group_id, index, item) => {
                restore_reference(config, *group_id, *index, item)
            }
        };
        if let Err(error) = result {
            self.status = format!(
                "无法撤销移除/删除 [{}]：{}",
                error_code(&error),
                error_hint(&error)
            );
            return false;
        }
        self.pending_undo = true;
        self.refresh_details();
        self.save();
        if !self.tracker.is_saving() {
            self.rollback_undo();
            false
        } else {
            true
        }
    }
    pub fn create_group(&mut self) {
        if !self.can_create() {
            return;
        }
        let config = Arc::make_mut(self.config.as_mut().expect("loaded config"));
        let old_next = config.next_group_id;
        let index = config.groups.len();
        let title = format!("新分组 {}", old_next.get());
        let offset = (index % 4) as i32 * 28;
        let rect = RectDip {
            x: 48 + offset,
            y: 130 + offset,
            width: 320,
            height: 220,
        };
        match create_group(config, title, rect) {
            Ok(id) => {
                self.pending_group = Some((id, old_next));
                self.refresh_details();
                self.save();
                if !self.tracker.is_saving() {
                    self.rollback_group();
                }
            }
            Err(error) => {
                self.status = format!(
                    "无法创建分组 [{}]：{}",
                    error_code(&error),
                    error_hint(&error)
                )
            }
        }
    }
    pub fn rename_group(&mut self, id: GroupId, title: String) {
        if !self.can_save() {
            return;
        }
        let config = Arc::make_mut(self.config.as_mut().expect("loaded config"));
        match rename_group(config, id, title) {
            Ok(old_title) => {
                self.pending_rename = Some((id, old_title));
                self.save();
                if !self.tracker.is_saving() {
                    self.rollback_rename();
                }
            }
            Err(error) => {
                self.status = format!(
                    "无法重命名分组 [{}]：{}",
                    error_code(&error),
                    error_hint(&error)
                );
            }
        }
    }
    pub fn move_group(&mut self, id: GroupId, x: i32, y: i32) {
        if !self.can_save() {
            return;
        }
        let config = Arc::make_mut(self.config.as_mut().expect("loaded config"));
        match move_group(config, id, x, y) {
            Ok(old_rect) => {
                self.pending_move = Some((id, old_rect));
                self.save();
                if !self.tracker.is_saving() {
                    self.rollback_move();
                }
            }
            Err(error) => {
                self.status = format!(
                    "无法移动分组 [{}]：{}",
                    error_code(&error),
                    error_hint(&error)
                );
            }
        }
    }
    pub fn resize_group(&mut self, id: GroupId, width: u32, height: u32) {
        if !self.can_save() {
            return;
        }
        let config = Arc::make_mut(self.config.as_mut().expect("loaded config"));
        match resize_group(config, id, width, height) {
            Ok(old_rect) => {
                self.pending_resize = Some((id, old_rect));
                self.save();
                if !self.tracker.is_saving() {
                    self.rollback_resize();
                }
            }
            Err(error) => {
                self.status = format!(
                    "无法缩放分组 [{}]：{}",
                    error_code(&error),
                    error_hint(&error)
                );
            }
        }
    }
    pub fn delete_group(&mut self, id: GroupId) {
        if !self.can_save() {
            return;
        }
        let config = Arc::make_mut(self.config.as_mut().expect("loaded config"));
        match delete_group(config, id) {
            Ok(removed) => {
                self.pending_delete = Some(removed);
                self.refresh_details();
                self.save();
                if !self.tracker.is_saving() {
                    self.rollback_delete();
                }
            }
            Err(error) => {
                self.status = format!(
                    "无法删除分组 [{}]：{}",
                    error_code(&error),
                    error_hint(&error)
                );
            }
        }
    }
    pub fn set_group_color(&mut self, id: GroupId, color: [u8; 3]) {
        if !self.can_save() {
            return;
        }
        let config = Arc::make_mut(self.config.as_mut().expect("loaded config"));
        match set_group_color(config, id, color) {
            Ok(old_color) => {
                self.pending_color = Some((id, old_color));
                self.save();
                if !self.tracker.is_saving() {
                    self.rollback_color();
                }
            }
            Err(error) => {
                self.status = format!(
                    "无法设置分组颜色 [{}]：{}",
                    error_code(&error),
                    error_hint(&error)
                );
            }
        }
    }
    pub fn toggle_group_collapsed(&mut self, id: GroupId) {
        if !self.can_save() {
            return;
        }
        let config = Arc::make_mut(self.config.as_mut().expect("loaded config"));
        match toggle_group_collapsed(config, id) {
            Ok(old_collapsed) => {
                self.pending_fold = Some((id, old_collapsed));
                self.save();
                if !self.tracker.is_saving() {
                    self.rollback_fold();
                }
            }
            Err(error) => {
                self.status = format!(
                    "无法折叠或展开分组 [{}]：{}",
                    error_code(&error),
                    error_hint(&error)
                );
            }
        }
    }
    pub fn contains_reference(&self, group_id: GroupId, path: &NativePath) -> bool {
        self.config.as_ref().is_some_and(|config| {
            config.groups.iter().any(|group| {
                group.id == group_id && group.items.iter().any(|item| &item.target.path == path)
            })
        })
    }
    pub fn contains_reference_id(&self, group_id: GroupId, item_id: ItemId) -> bool {
        self.config.as_ref().is_some_and(|config| {
            config.groups.iter().any(|group| {
                group.id == group_id && group.items.iter().any(|item| item.id == item_id)
            })
        })
    }
    pub fn add_references(&mut self, group_id: GroupId, targets: Vec<FileReference>) -> bool {
        if !self.can_save() {
            return false;
        }
        let config = Arc::make_mut(self.config.as_mut().expect("loaded config"));
        let old_next = config.next_item_id;
        match add_references(config, group_id, targets) {
            Ok(ids) if !ids.is_empty() => {
                self.pending_add = Some((group_id, ids, old_next));
                self.refresh_details();
                self.save();
                if !self.tracker.is_saving() {
                    self.rollback_add();
                    false
                } else {
                    true
                }
            }
            Ok(_) => false,
            Err(error) => {
                self.status = format!(
                    "无法添加引用 [{}]：{}",
                    error_code(&error),
                    error_hint(&error)
                );
                false
            }
        }
    }
    pub fn remove_reference(&mut self, group_id: GroupId, item_id: ItemId) -> bool {
        if !self.can_save() {
            return false;
        }
        let config = Arc::make_mut(self.config.as_mut().expect("loaded config"));
        match remove_reference(config, group_id, item_id) {
            Ok((index, item)) => {
                self.pending_remove = Some((group_id, index, item));
                self.refresh_details();
                self.save();
                if !self.tracker.is_saving() {
                    self.rollback_remove();
                    false
                } else {
                    true
                }
            }
            Err(error) => {
                self.status = format!(
                    "无法移除引用 [{}]：{}",
                    error_code(&error),
                    error_hint(&error)
                );
                false
            }
        }
    }
    pub fn diagnostic_hint(&self) -> &'static str {
        self.worker.log_status().hint()
    }
    pub fn shifted_index(&self, group_id: GroupId, item_id: ItemId, step: isize) -> Option<usize> {
        let group = self
            .config
            .as_ref()?
            .groups
            .iter()
            .find(|g| g.id == group_id)?;
        let index = group.items.iter().position(|item| item.id == item_id)?;
        let next = index.checked_add_signed(step)?;
        (next < group.items.len() && next != index).then_some(next)
    }
    pub fn reorder_reference(
        &mut self,
        group_id: GroupId,
        item_id: ItemId,
        new_index: usize,
    ) -> bool {
        if !self.can_save() {
            return false;
        }
        let config = Arc::make_mut(self.config.as_mut().expect("loaded config"));
        match reorder_reference(config, group_id, item_id, new_index) {
            Ok(old_index) if old_index != new_index => {
                self.pending_sort = Some((group_id, item_id, old_index));
                self.save();
                if !self.tracker.is_saving() {
                    self.rollback_sort();
                    false
                } else {
                    true
                }
            }
            Ok(_) => false,
            Err(error) => {
                self.status = format!(
                    "无法排序引用 [{}]：{}",
                    error_code(&error),
                    error_hint(&error)
                );
                false
            }
        }
    }
    fn rollback_sort(&mut self) {
        if let Some((group_id, item_id, index)) = self.pending_sort.take()
            && let Some(config) = self.config.as_mut()
        {
            // Only one mutation can be pending; the item and original index are
            // unchanged by any other command until this save finishes.
            let _ = reorder_reference(Arc::make_mut(config), group_id, item_id, index);
        }
    }
    pub fn transfer_reference(
        &mut self,
        source: GroupId,
        item: ItemId,
        destination: GroupId,
    ) -> bool {
        if !self.can_save() {
            return false;
        }
        let config = Arc::make_mut(self.config.as_mut().expect("loaded config"));
        match transfer_reference(config, source, item, destination) {
            Ok(index) => {
                self.pending_transfer = Some((source, item, destination, index));
                self.save();
                if !self.tracker.is_saving() {
                    self.rollback_transfer();
                    false
                } else {
                    true
                }
            }
            Err(error) => {
                self.status = format!(
                    "无法移动引用 [{}]：{}",
                    error_code(&error),
                    error_hint(&error)
                );
                false
            }
        }
    }
    fn rollback_transfer(&mut self) {
        if let Some((source, item, destination, index)) = self.pending_transfer.take()
            && let Some(config) = self.config.as_mut()
        {
            // One pending mutation: neither group nor item can change during save.
            let config = Arc::make_mut(config);
            if transfer_reference(config, destination, item, source).is_ok() {
                let _ = reorder_reference(config, source, item, index);
            }
        }
    }
    pub fn start(root: DataRoot, smoke: Option<StorageSmoke>) -> io::Result<Self> {
        let wake = WakeEvent::new()?;
        let signal = wake.clone();
        let worker =
            ConfigWorker::start(root, move || signal.signal()).map_err(io::Error::other)?;
        Ok(Self {
            wake,
            worker,
            config: None,
            loaded: false,
            writable: false,
            tracker: SaveTracker::default(),
            pending_group: None,
            pending_rename: None,
            pending_move: None,
            pending_resize: None,
            pending_delete: None,
            pending_color: None,
            pending_fold: None,
            pending_add: None,
            pending_remove: None,
            pending_sort: None,
            pending_transfer: None,
            last_removal: None,
            pending_undo: false,
            load_source: None,
            closing: false,
            shutdown_sent: false,
            stopped: false,
            status: "正在后台加载配置…".into(),
            details: "尚未读取配置；保存暂不可用。".into(),
            smoke,
            smoke_passed: false,
        })
    }
    pub fn can_save(&self) -> bool {
        self.loaded && self.writable && !self.closing && !self.tracker.is_saving() && !self.stopped
    }
    pub fn save(&mut self) {
        if !self.can_save() {
            return;
        }
        let Ok(revision) = self.tracker.begin() else {
            self.writable = false;
            self.status = "保存序号已耗尽，请重新启动。".into();
            return;
        };
        let config = self
            .config
            .as_ref()
            .expect("loaded writable config")
            .clone();
        match self.worker.submit(Command::Save { config, revision }) {
            Ok(()) => self.status = "正在后台保存；请勿强制结束进程。".into(),
            Err(error) => {
                self.tracker.finish(revision, false);
                self.writable = false;
                self.status = format!(
                    "无法提交保存 [{}]：{}",
                    error_code(&error),
                    error_hint(&error)
                );
            }
        }
    }
    pub fn close(&mut self) {
        self.closing = true;
        self.status = "正在退出：等待后台操作完成…".into();
        self.advance_close();
    }
    fn advance_close(&mut self) {
        if self.closing
            && self.loaded
            && !self.tracker.is_saving()
            && !self.shutdown_sent
            && !self.stopped
        {
            match self.worker.submit(Command::Shutdown) {
                Ok(()) => self.shutdown_sent = true,
                Err(error) => {
                    self.closing = false;
                    self.status = format!(
                        "退出请求失败 [{}]：{}",
                        error_code(&error),
                        error_hint(&error)
                    );
                }
            }
        }
    }
    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.worker.poll() {
                Ok(event) => {
                    changed = true;
                    self.accept(event);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.stopped {
                        changed = true;
                        let had_group = self.pending_group.is_some();
                        let had_rename = self.pending_rename.is_some();
                        let had_move = self.pending_move.is_some();
                        let had_resize = self.pending_resize.is_some();
                        let had_delete = self.pending_delete.is_some();
                        let had_color = self.pending_color.is_some();
                        let had_fold = self.pending_fold.is_some();
                        let had_add = self.pending_add.is_some();
                        let had_remove = self.pending_remove.is_some();
                        let had_sort = self.pending_sort.is_some();
                        let had_transfer = self.pending_transfer.is_some();
                        let had_undo = self.pending_undo;
                        self.rollback_group();
                        self.rollback_rename();
                        self.rollback_move();
                        self.rollback_resize();
                        self.rollback_delete();
                        self.rollback_color();
                        self.rollback_fold();
                        self.rollback_add();
                        self.rollback_remove();
                        self.rollback_sort();
                        self.rollback_transfer();
                        self.rollback_undo();
                        self.writable = false;
                        self.status = if had_group {
                            "后台线程已中断，不能确认保存结果；未确认的新分组已回退，请重新启动。"
                        } else if had_rename {
                            "后台线程已中断，不能确认保存结果；未确认的重命名已回退，请重新启动。"
                        } else if had_move {
                            "后台线程已中断，不能确认保存结果；未确认的位置变更已回退，请重新启动。"
                        } else if had_resize {
                            "后台线程已中断，不能确认保存结果；未确认的尺寸变更已回退，请重新启动。"
                        } else if had_delete {
                            "后台线程已中断，不能确认保存结果；未确认的分组删除已回退，请重新启动。"
                        } else if had_color {
                            "后台线程已中断，不能确认保存结果；未确认的颜色变更已回退，请重新启动。"
                        } else if had_fold {
                            "后台线程已中断，不能确认保存结果；未确认的折叠变更已回退，请重新启动。"
                        } else if had_add {
                            "后台线程已中断，不能确认保存结果；未确认的引用添加已回退，请重新启动。"
                        } else if had_undo {
                            "后台线程已中断，不能确认撤销保存；本次恢复已回退，请重新启动。"
                        } else if had_sort {
                            "后台线程已中断，不能确认保存结果；未确认的排序已回退，请重新启动。"
                        } else if had_transfer {
                            "后台线程已中断，不能确认保存结果；未确认的组间移动已回退，请重新启动。"
                        } else if had_remove {
                            "后台线程已中断，不能确认保存结果；未确认的引用移除已回退，请重新启动。"
                        } else {
                            "后台线程已中断，不能确认保存结果；请重新启动。"
                        }
                        .into();
                        // A disconnected worker has released its store; a later close is safe.
                        self.stopped = true;
                    }
                    break;
                }
            }
        }
        changed
    }
    fn accept(&mut self, event: Event) {
        match event {
            Event::Loaded(result) => {
                self.loaded = true;
                let was_error = result.is_err();
                match result {
                    Ok(loaded) => {
                        self.writable = loaded.write_allowed;
                        self.load_source = Some(loaded.source);
                        self.status = if self.writable {
                            "加载完成，可保存配置。"
                        } else {
                            "只读保护：文件版本、损坏或残留冲突；不会覆盖配置。"
                        }
                        .into();
                        self.config = Some(Arc::new(loaded.config));
                        self.refresh_details();
                    }
                    Err(error) => {
                        self.status =
                            format!("加载失败 [{}]：{}", error_code(&error), error_hint(&error));
                        self.details = "未用空配置替换读取失败的数据。".into();
                    }
                }
                if let Some(expected) = self.smoke
                    && expected != StorageSmoke::UiRename
                    && expected != StorageSmoke::UiMove
                    && expected != StorageSmoke::UiDrag
                    && expected != StorageSmoke::UiResize
                    && expected != StorageSmoke::UiResizeDrag
                    && expected != StorageSmoke::UiDelete
                    && expected != StorageSmoke::UiColor
                    && expected != StorageSmoke::UiFold
                    && expected != StorageSmoke::Add
                    && expected != StorageSmoke::UiAdd
                    && expected != StorageSmoke::UiShortcut
                    && expected != StorageSmoke::UiFolder
                    && expected != StorageSmoke::UiDrop
                    && expected != StorageSmoke::UiDropFolder
                    && expected != StorageSmoke::UiDropMixed
                    && expected != StorageSmoke::UiDropShortcut
                    && expected != StorageSmoke::UiRemove
                    && expected != StorageSmoke::UiUndoRemove
                    && expected != StorageSmoke::UiUndoDelete
                    && expected != StorageSmoke::UiTransfer
                    && expected != StorageSmoke::UiTransferCancel
                    && expected != StorageSmoke::UiTransferOutside
                    && expected != StorageSmoke::UiTransferCapture
                {
                    match expected {
                        StorageSmoke::Writable if !was_error && self.writable => self.save(),
                        StorageSmoke::ReadOnly => self.smoke_passed = !was_error && !self.writable,
                        StorageSmoke::Error => self.smoke_passed = was_error,
                        StorageSmoke::Group if !was_error && self.writable => self.create_group(),
                        StorageSmoke::Rename if !was_error && self.writable => {
                            if let Some(id) =
                                self.config.as_ref().unwrap().groups.first().map(|g| g.id)
                            {
                                self.rename_group(id, "已重命名的分组".into());
                            }
                        }
                        StorageSmoke::Move if !was_error && self.writable => {
                            if let Some(id) =
                                self.config.as_ref().unwrap().groups.first().map(|g| g.id)
                            {
                                self.move_group(id, -120, 72);
                            }
                        }
                        StorageSmoke::Resize if !was_error && self.writable => {
                            if let Some(id) =
                                self.config.as_ref().unwrap().groups.first().map(|g| g.id)
                            {
                                self.resize_group(id, 480, 280);
                            }
                        }
                        StorageSmoke::Delete if !was_error && self.writable => {
                            if let Some(id) =
                                self.config.as_ref().unwrap().groups.first().map(|g| g.id)
                            {
                                self.delete_group(id);
                            }
                        }
                        StorageSmoke::Color if !was_error && self.writable => {
                            if let Some(id) =
                                self.config.as_ref().unwrap().groups.first().map(|g| g.id)
                            {
                                self.set_group_color(id, [16, 128, 240]);
                            }
                        }
                        StorageSmoke::Fold if !was_error && self.writable => {
                            if let Some(id) =
                                self.config.as_ref().unwrap().groups.first().map(|g| g.id)
                            {
                                self.toggle_group_collapsed(id);
                            }
                        }
                        _ => {}
                    }
                    // Exercise close DURING save; only Stopped permits destruction.
                    self.close();
                }
                self.advance_close();
            }
            Event::Saved { revision, result } => {
                if !self.tracker.finish(revision, result.is_ok()) {
                    return;
                }
                match result {
                    Ok(()) => {
                        self.status = format!("配置已保存（本次会话 #{revision}）。");
                        let saved_group = self.pending_group.is_some();
                        let saved_rename = self.pending_rename.is_some();
                        let saved_move = self.pending_move.is_some();
                        let saved_resize = self.pending_resize.is_some();
                        let saved_delete = self.pending_delete.is_some();
                        let saved_color = self.pending_color.is_some();
                        let saved_fold = self.pending_fold.is_some();
                        let saved_add = self.pending_add.is_some();
                        let saved_remove = self.pending_remove.is_some();
                        let saved_sort = self.pending_sort.is_some();
                        let saved_transfer = self.pending_transfer.is_some();
                        let saved_undo = self.pending_undo;
                        if let Some((index, group)) = self.pending_delete.take() {
                            self.last_removal = Some(UndoRemoval::Group(index, group));
                        }
                        if let Some((group_id, index, item)) = self.pending_remove.take() {
                            self.last_removal = Some(UndoRemoval::Reference(group_id, index, item));
                        }
                        self.pending_group = None;
                        self.pending_rename = None;
                        self.pending_move = None;
                        self.pending_resize = None;
                        self.pending_delete = None;
                        self.pending_color = None;
                        self.pending_fold = None;
                        self.pending_add = None;
                        self.pending_remove = None;
                        self.pending_sort = None;
                        self.pending_transfer = None;
                        self.pending_undo = false;
                        if saved_undo {
                            self.last_removal = None;
                            self.status = format!(
                                "配置已保存（本次会话 #{revision}）；已撤销最近一次移除/删除，源文件未改动。"
                            );
                            if matches!(
                                self.smoke,
                                Some(StorageSmoke::UiUndoRemove | StorageSmoke::UiUndoDelete)
                            ) {
                                self.smoke_passed = true;
                            }
                        }
                        if self.smoke == Some(StorageSmoke::Writable) {
                            self.smoke_passed = true;
                        }
                        if self.smoke == Some(StorageSmoke::Group) && saved_group {
                            self.smoke_passed = true;
                        }
                        if matches!(
                            self.smoke,
                            Some(StorageSmoke::Rename | StorageSmoke::UiRename)
                        ) && saved_rename
                        {
                            self.smoke_passed = true;
                        }
                        if matches!(
                            self.smoke,
                            Some(StorageSmoke::Move | StorageSmoke::UiMove | StorageSmoke::UiDrag)
                        ) && saved_move
                        {
                            self.smoke_passed = true;
                        }
                        if matches!(
                            self.smoke,
                            Some(
                                StorageSmoke::Resize
                                    | StorageSmoke::UiResize
                                    | StorageSmoke::UiResizeDrag
                            )
                        ) && saved_resize
                        {
                            self.smoke_passed = true;
                        }
                        if matches!(
                            self.smoke,
                            Some(StorageSmoke::Delete | StorageSmoke::UiDelete)
                        ) && saved_delete
                        {
                            self.smoke_passed = true;
                        }
                        if matches!(
                            self.smoke,
                            Some(StorageSmoke::Color | StorageSmoke::UiColor)
                        ) && saved_color
                        {
                            self.smoke_passed = true;
                        }
                        if matches!(self.smoke, Some(StorageSmoke::Fold | StorageSmoke::UiFold))
                            && saved_fold
                        {
                            self.smoke_passed = true;
                        }
                        if saved_add {
                            if matches!(
                                self.smoke,
                                Some(
                                    StorageSmoke::Add
                                        | StorageSmoke::UiAdd
                                        | StorageSmoke::UiShortcut
                                        | StorageSmoke::UiFolder
                                        | StorageSmoke::UiDrop
                                        | StorageSmoke::UiDropFolder
                                        | StorageSmoke::UiDropMixed
                                        | StorageSmoke::UiDropShortcut
                                )
                            ) {
                                self.smoke_passed = true;
                            }
                            self.status =
                                format!("配置已保存（本次会话 #{revision}）；引用已添加。");
                        }
                        if saved_remove {
                            if self.smoke == Some(StorageSmoke::UiRemove) {
                                self.smoke_passed = true;
                            }
                            self.status = format!(
                                "配置已保存（本次会话 #{revision}）；仅移除了软件引用，源文件未改动。"
                            );
                        }
                        if saved_sort {
                            self.status = format!(
                                "配置已保存（本次会话 #{revision}）；组内顺序已更新，源文件未改动。"
                            );
                        }
                        if saved_transfer {
                            if self.smoke == Some(StorageSmoke::UiTransfer) {
                                self.smoke_passed = true;
                            }
                            self.status = format!(
                                "配置已保存（本次会话 #{revision}）；引用已移动到目标分组，源文件未改动。"
                            );
                        }
                    }
                    Err(error) => {
                        let had_group = self.pending_group.is_some();
                        let had_rename = self.pending_rename.is_some();
                        let had_move = self.pending_move.is_some();
                        let had_resize = self.pending_resize.is_some();
                        let had_delete = self.pending_delete.is_some();
                        let had_color = self.pending_color.is_some();
                        let had_fold = self.pending_fold.is_some();
                        let had_add = self.pending_add.is_some();
                        let had_remove = self.pending_remove.is_some();
                        let had_sort = self.pending_sort.is_some();
                        let had_transfer = self.pending_transfer.is_some();
                        let had_undo = self.pending_undo;
                        self.rollback_group();
                        self.rollback_rename();
                        self.rollback_move();
                        self.rollback_resize();
                        self.rollback_delete();
                        self.rollback_color();
                        self.rollback_fold();
                        self.rollback_add();
                        self.rollback_remove();
                        self.rollback_sort();
                        self.rollback_transfer();
                        self.rollback_undo();
                        self.writable = false;
                        self.closing = false;
                        self.status =
                            format!("保存失败 [{}]：{}", error_code(&error), error_hint(&error));
                        self.details = if had_group {
                            "新分组保存失败，已回退本次创建。"
                        } else if had_rename {
                            "分组重命名保存失败，已恢复旧名称。"
                        } else if had_move {
                            "分组位置保存失败，已恢复原位置。"
                        } else if had_resize {
                            "分组尺寸保存失败，已恢复原尺寸。"
                        } else if had_delete {
                            "分组删除保存失败，已恢复原分组。"
                        } else if had_color {
                            "分组颜色保存失败，已恢复原颜色。"
                        } else if had_fold {
                            "分组折叠保存失败，已恢复原状态。"
                        } else if had_add {
                            "项目引用保存失败，已移除本次新增引用。"
                        } else if had_undo {
                            "撤销保存失败，已回退本次恢复；源文件未改动。"
                        } else if had_sort {
                            "组内排序保存失败，已恢复原顺序。"
                        } else if had_transfer {
                            "组间移动保存失败，已恢复原分组与顺序。"
                        } else if had_remove {
                            "项目引用移除保存失败，已恢复原引用。"
                        } else {
                            "保存已禁用，本次退出已取消；再次关闭可退出。"
                        }
                        .into();
                        if self.smoke.is_some() {
                            self.close();
                        }
                    }
                }
                self.advance_close();
            }
            Event::Stopped => self.stopped = true,
        }
    }
    pub fn should_destroy(&self) -> bool {
        self.stopped && self.closing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    mod transfer_tests {
        include!("config_ui/transfer_tests.rs");
    }
    use litefences::core::contracts::AppError;
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };
    use windows_sys::Win32::{Foundation::WAIT_OBJECT_0, System::Threading::WaitForSingleObject};

    static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    fn fixture() -> (PathBuf, Session) {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("artifacts/storage-tests")
            .join(format!(
                "session-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&dir).unwrap();
        let session = Session::start(DataRoot::Existing(dir.clone()), None).unwrap();
        (dir, session)
    }
    fn source_reference(path: &Path) -> FileReference {
        // Cargo may supply a verbatim manifest path; the model stores canonical drive paths.
        let raw = path.to_string_lossy();
        let drive_path = raw
            .strip_prefix(r"\\?\")
            .unwrap_or(raw.as_ref())
            .replace('/', "\\");
        FileReference {
            kind: litefences::core::model::ReferenceKind::File,
            path: litefences::core::model::NativePath::from_utf16(
                drive_path.encode_utf16().collect(),
            )
            .unwrap_or_else(|error| panic!("invalid fixture reference {drive_path:?}: {error:?}")),
        }
    }
    #[test]
    fn preview_items_expose_bounded_basename_and_kind_without_changing_references() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let group_id = session.config.as_ref().unwrap().groups[0].id;
        let file = source_reference(&dir.join("file.txt"));
        let mut folder = source_reference(&dir.join("folder"));
        folder.kind = ReferenceKind::Directory;
        let mut shortcut = source_reference(&dir.join("source-link.lnk"));
        shortcut.kind = ReferenceKind::Shortcut;
        assert!(session.add_references(group_id, vec![file, folder, shortcut]));
        until(&mut session, |s| !s.tracker.is_saving());
        let (total, items) = session.preview_items(group_id, 24);
        assert_eq!(total, 3);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].label, "file.txt");
        assert_eq!(items[0].id.get(), 1);
        assert_eq!(items[0].kind, ReferenceKind::File);
        assert_eq!(items[1].label, "folder");
        assert_eq!(items[1].kind, ReferenceKind::Directory);
        assert_eq!(items[2].label, "source-link.lnk");
        assert_eq!(items[2].kind, ReferenceKind::Shortcut);
        let (total_without_clones, hidden) = session.preview_items(group_id, 0);
        assert_eq!(total_without_clones, 3);
        assert!(hidden.is_empty());
        assert_eq!(session.config.as_ref().unwrap().groups[0].items.len(), 3);
        session.close();
        until(&mut session, Session::should_destroy);
    }

    #[test]
    fn reference_lookup_uses_stable_ids_beyond_preview_and_works_read_only() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let group_id = session.config.as_ref().unwrap().groups[0].id;
        let targets: Vec<_> = (0..25)
            .map(|index| source_reference(&dir.join(format!("item-{index}.txt"))))
            .collect();
        let expected = targets[24].clone();
        assert!(session.add_references(group_id, targets));
        until(&mut session, |s| !s.tracker.is_saving());

        // The real lookup must not inherit the UI preview's 24-item cap or
        // require writable configuration state.
        session.writable = false;
        assert_eq!(
            session.reference_for_item(group_id, ItemId::new(25).unwrap()),
            Some(expected)
        );
        assert_eq!(
            session.reference_for_item(group_id, ItemId::new(26).unwrap()),
            None
        );
        assert_eq!(
            session.reference_for_item(GroupId::new(999).unwrap(), ItemId::new(25).unwrap()),
            None
        );
        session.close();
        until(&mut session, Session::should_destroy);
    }

    fn until(session: &mut Session, done: impl Fn(&Session) -> bool) {
        // At most four bounded waits; auto-reset events can coalesce queue notifications.
        for _ in 0..4 {
            session.drain();
            if done(session) {
                return;
            }
            // SAFETY: Session owns an Arc keeping the event alive through the wait.
            assert_eq!(
                unsafe { WaitForSingleObject(session.wake.handle(), 5000) },
                WAIT_OBJECT_0
            );
        }
        session.drain();
        assert!(done(session));
    }
    #[test]
    fn close_before_load_waits_and_never_saves_empty_config() {
        let (dir, mut session) = fixture();
        session.close();
        assert!(!session.should_destroy());
        until(&mut session, Session::should_destroy);
        assert!(!dir.join("config.json").exists());
        litefences::storage::file_store::FileStore::open(&dir).unwrap();
    }
    #[test]
    fn duplicate_save_and_stale_receipt_do_not_change_current_request() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.save();
        session.save();
        session.accept(Event::Saved {
            revision: 999,
            result: Err(AppError::Io),
        });
        assert!(session.tracker.is_saving());
        assert!(session.writable);
        session.close();
        until(&mut session, Session::should_destroy);
        assert_eq!(session.tracker.last_saved(), Some(1));
        assert!(dir.join("config.json").exists());
        assert!(!dir.join("config.bak.json").exists());
    }
    #[test]
    fn failed_save_cancels_close_and_keeps_conflicting_data() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        fs::write(dir.join("config.pending"), b"external pending data").unwrap();
        session.save();
        session.close();
        until(&mut session, |s| !s.tracker.is_saving());
        assert!(!session.should_destroy());
        assert!(!session.closing);
        assert!(!session.can_save());
        assert!(session.status.contains("保存失败"));
        assert_eq!(
            fs::read(dir.join("config.pending")).unwrap(),
            b"external pending data"
        );
        assert!(!dir.join("config.json").exists());
        session.close();
        until(&mut session, Session::should_destroy);
    }
    #[test]
    fn load_error_never_enables_save_and_can_close() {
        let (dir, first) = fixture();
        drop(first);
        let mut session = Session::start(DataRoot::Existing(dir.join("missing")), None).unwrap();
        until(&mut session, |s| s.loaded);
        assert!(session.config.is_none());
        assert!(!session.can_save());
        session.save();
        assert!(!session.tracker.is_saving());
        session.close();
        until(&mut session, Session::should_destroy);
    }
    #[test]
    fn create_group_autosaves_and_reopens_with_same_identity() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        assert_eq!(session.group_titles(3), ["分组 1：新分组 1"]);
        assert!(!session.can_create());
        until(&mut session, |s| !s.tracker.is_saving());
        assert!(session.can_create());
        assert_eq!(session.tracker.last_saved(), Some(1));
        session.close();
        until(&mut session, Session::should_destroy);
        let mut reopened = Session::start(DataRoot::Existing(dir), None).unwrap();
        until(&mut reopened, |s| s.loaded);
        assert_eq!(reopened.group_titles(3), ["分组 1：新分组 1"]);
        assert_eq!(reopened.config.as_ref().unwrap().next_group_id.get(), 2);
        reopened.close();
        until(&mut reopened, Session::should_destroy);
    }
    #[test]
    fn failed_autosave_rolls_back_group_and_id_without_overwriting_pending() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        fs::write(dir.join("config.pending"), b"external pending data").unwrap();
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        assert!(session.group_titles(3).is_empty());
        assert_eq!(session.config.as_ref().unwrap().next_group_id.get(), 1);
        assert!(!session.can_create());
        assert!(session.details.contains("已回退"));
        assert!(!dir.join("config.json").exists());
        assert_eq!(
            fs::read(dir.join("config.pending")).unwrap(),
            b"external pending data"
        );
        session.close();
        until(&mut session, Session::should_destroy);
    }
    #[test]
    fn rename_autosaves_and_reopens_without_changing_group_id() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let id = GroupId::new(1).unwrap();
        session.rename_group(id, "工作".into());
        assert_eq!(session.group_titles(3), ["分组 1：工作"]);
        assert_eq!(session.group_title_at(0).as_deref(), Some("工作"));
        assert_eq!(session.group_title_at(1), None);
        until(&mut session, |s| !s.tracker.is_saving());
        assert_eq!(session.tracker.last_saved(), Some(2));
        session.close();
        until(&mut session, Session::should_destroy);
        let mut reopened = Session::start(DataRoot::Existing(dir), None).unwrap();
        until(&mut reopened, |s| s.loaded);
        assert_eq!(reopened.group_titles(3), ["分组 1：工作"]);
        assert_eq!(reopened.group_title_at(0).as_deref(), Some("工作"));
        assert_eq!(reopened.config.as_ref().unwrap().next_group_id.get(), 2);
        reopened.close();
        until(&mut reopened, Session::should_destroy);
    }
    #[test]
    fn failed_rename_autosave_restores_old_title_and_preserves_disk() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let old_json = fs::read(dir.join("config.json")).unwrap();
        fs::write(dir.join("config.pending"), b"external pending data").unwrap();
        session.rename_group(GroupId::new(1).unwrap(), "未保存".into());
        until(&mut session, |s| !s.tracker.is_saving());
        assert_eq!(session.group_titles(3), ["分组 1：新分组 1"]);
        assert!(session.details.contains("已恢复旧名称"));
        assert!(!session.can_save());
        assert_eq!(fs::read(dir.join("config.json")).unwrap(), old_json);
        assert_eq!(
            fs::read(dir.join("config.pending")).unwrap(),
            b"external pending data"
        );
        session.close();
        until(&mut session, Session::should_destroy);
    }
    #[test]
    fn move_autosaves_and_reopens_without_changing_size_or_id() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let before = session.config.as_ref().unwrap().groups[0].clone();
        session.move_group(before.id, -120, 72);
        assert!(!session.can_save());
        until(&mut session, |s| !s.tracker.is_saving());
        assert_eq!(session.tracker.last_saved(), Some(2));
        session.close();
        until(&mut session, Session::should_destroy);
        let mut reopened = Session::start(DataRoot::Existing(dir), None).unwrap();
        until(&mut reopened, |s| s.loaded);
        let moved = &reopened.config.as_ref().unwrap().groups[0];
        assert_eq!(moved.id, before.id);
        assert_eq!(moved.title, before.title);
        assert_eq!(moved.rect.x, -120);
        assert_eq!(moved.rect.y, 72);
        assert_eq!(moved.rect.width, before.rect.width);
        assert_eq!(moved.rect.height, before.rect.height);
        reopened.close();
        until(&mut reopened, Session::should_destroy);
    }
    #[test]
    fn failed_move_autosave_restores_old_rect_and_preserves_disk() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let before = session.config.as_ref().unwrap().groups[0].clone();
        let old_json = fs::read(dir.join("config.json")).unwrap();
        fs::write(dir.join("config.pending"), b"external pending data").unwrap();
        session.move_group(before.id, 999, 888);
        until(&mut session, |s| !s.tracker.is_saving());
        assert_eq!(session.config.as_ref().unwrap().groups[0], before);
        assert!(session.details.contains("已恢复原位置"));
        assert!(!session.can_save());
        assert_eq!(fs::read(dir.join("config.json")).unwrap(), old_json);
        assert_eq!(
            fs::read(dir.join("config.pending")).unwrap(),
            b"external pending data"
        );
        session.close();
        until(&mut session, Session::should_destroy);
    }
    #[test]
    fn resize_autosaves_and_reopens_without_changing_position_or_id() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let before = session.config.as_ref().unwrap().groups[0].clone();
        session.resize_group(before.id, 480, 280);
        assert!(!session.can_save());
        until(&mut session, |s| !s.tracker.is_saving());
        assert_eq!(session.tracker.last_saved(), Some(2));
        session.close();
        until(&mut session, Session::should_destroy);
        let mut reopened = Session::start(DataRoot::Existing(dir), None).unwrap();
        until(&mut reopened, |s| s.loaded);
        let resized = &reopened.config.as_ref().unwrap().groups[0];
        assert_eq!(resized.id, before.id);
        assert_eq!(resized.title, before.title);
        assert_eq!(resized.rect.x, before.rect.x);
        assert_eq!(resized.rect.y, before.rect.y);
        assert_eq!((resized.rect.width, resized.rect.height), (480, 280));
        reopened.close();
        until(&mut reopened, Session::should_destroy);
    }
    #[test]
    fn failed_resize_autosave_restores_old_rect_and_preserves_disk() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let before = session.config.as_ref().unwrap().groups[0].clone();
        let old_json = fs::read(dir.join("config.json")).unwrap();
        fs::write(dir.join("config.pending"), b"external pending data").unwrap();
        session.resize_group(before.id, 480, 280);
        until(&mut session, |s| !s.tracker.is_saving());
        assert_eq!(session.config.as_ref().unwrap().groups[0], before);
        assert!(session.details.contains("已恢复原尺寸"));
        assert!(!session.can_save());
        assert_eq!(fs::read(dir.join("config.json")).unwrap(), old_json);
        assert_eq!(
            fs::read(dir.join("config.pending")).unwrap(),
            b"external pending data"
        );
        session.close();
        until(&mut session, Session::should_destroy);
    }
    #[test]
    fn delete_autosaves_and_reopens_without_reusing_group_id() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let second = session.config.as_ref().unwrap().groups[1].clone();
        session.delete_group(GroupId::new(1).unwrap());
        assert!(!session.can_save());
        until(&mut session, |s| !s.tracker.is_saving());
        assert_eq!(
            session.config.as_ref().unwrap().groups.as_slice(),
            std::slice::from_ref(&second)
        );
        assert_eq!(session.config.as_ref().unwrap().next_group_id.get(), 3);
        session.close();
        until(&mut session, Session::should_destroy);
        let mut reopened = Session::start(DataRoot::Existing(dir), None).unwrap();
        until(&mut reopened, |s| s.loaded);
        assert_eq!(reopened.config.as_ref().unwrap().groups, [second]);
        assert_eq!(reopened.config.as_ref().unwrap().next_group_id.get(), 3);
        reopened.close();
        until(&mut reopened, Session::should_destroy);
    }
    #[test]
    fn failed_delete_autosave_restores_group_and_preserves_disk() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let before = session.config.as_ref().unwrap().clone();
        let old_json = fs::read(dir.join("config.json")).unwrap();
        fs::write(dir.join("config.pending"), b"external pending data").unwrap();
        session.delete_group(GroupId::new(1).unwrap());
        until(&mut session, |s| !s.tracker.is_saving());
        assert_eq!(session.config.as_ref().unwrap().as_ref(), before.as_ref());
        assert!(session.details.contains("已恢复原分组"));
        assert!(!session.can_save());
        assert_eq!(fs::read(dir.join("config.json")).unwrap(), old_json);
        assert_eq!(
            fs::read(dir.join("config.pending")).unwrap(),
            b"external pending data"
        );
        session.close();
        until(&mut session, Session::should_destroy);
    }
    #[test]
    fn color_autosaves_and_reopens_without_changing_other_fields() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let before = session.config.as_ref().unwrap().groups[0].clone();
        session.set_group_color(before.id, [16, 128, 240]);
        until(&mut session, |s| !s.tracker.is_saving());
        session.close();
        until(&mut session, Session::should_destroy);
        let mut reopened = Session::start(DataRoot::Existing(dir), None).unwrap();
        until(&mut reopened, |s| s.loaded);
        let mut expected = before;
        expected.color = [16, 128, 240];
        assert_eq!(reopened.config.as_ref().unwrap().groups[0], expected);
        reopened.close();
        until(&mut reopened, Session::should_destroy);
    }
    #[test]
    fn failed_color_autosave_restores_old_color_and_preserves_disk() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let before = session.config.as_ref().unwrap().clone();
        let old_json = fs::read(dir.join("config.json")).unwrap();
        fs::write(dir.join("config.pending"), b"external pending data").unwrap();
        session.set_group_color(before.groups[0].id, [16, 128, 240]);
        until(&mut session, |s| !s.tracker.is_saving());
        assert_eq!(session.config.as_ref().unwrap().as_ref(), before.as_ref());
        assert!(session.details.contains("已恢复原颜色"));
        assert!(!session.can_save());
        assert_eq!(fs::read(dir.join("config.json")).unwrap(), old_json);
        assert_eq!(
            fs::read(dir.join("config.pending")).unwrap(),
            b"external pending data"
        );
        session.close();
        until(&mut session, Session::should_destroy);
    }
    #[test]
    fn fold_autosaves_and_reopens_without_changing_geometry() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let before = session.config.as_ref().unwrap().groups[0].clone();
        session.toggle_group_collapsed(before.id);
        until(&mut session, |s| !s.tracker.is_saving());
        session.close();
        until(&mut session, Session::should_destroy);
        let mut reopened = Session::start(DataRoot::Existing(dir), None).unwrap();
        until(&mut reopened, |s| s.loaded);
        let mut expected = before;
        expected.collapsed = true;
        assert_eq!(reopened.config.as_ref().unwrap().groups[0], expected);
        reopened.close();
        until(&mut reopened, Session::should_destroy);
    }
    #[test]
    fn failed_fold_autosave_restores_state_and_preserves_disk() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let before = session.config.as_ref().unwrap().clone();
        let old_json = fs::read(dir.join("config.json")).unwrap();
        fs::write(dir.join("config.pending"), b"external pending data").unwrap();
        session.toggle_group_collapsed(before.groups[0].id);
        until(&mut session, |s| !s.tracker.is_saving());
        assert_eq!(session.config.as_ref().unwrap().as_ref(), before.as_ref());
        assert!(session.details.contains("已恢复原状态"));
        assert!(!session.can_save());
        assert_eq!(fs::read(dir.join("config.json")).unwrap(), old_json);
        assert_eq!(
            fs::read(dir.join("config.pending")).unwrap(),
            b"external pending data"
        );
        session.close();
        until(&mut session, Session::should_destroy);
    }
    #[test]
    fn add_reference_autosaves_reopens_and_never_changes_source_bytes() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let source = dir.join("source.txt");
        fs::write(&source, b"keep source bytes").unwrap();
        let target = source_reference(&source);
        session.add_references(GroupId::new(1).unwrap(), vec![target.clone()]);
        until(&mut session, |s| !s.tracker.is_saving());
        assert_eq!(fs::read(&source).unwrap(), b"keep source bytes");
        assert_eq!(session.config.as_ref().unwrap().next_item_id.get(), 2);
        session.close();
        until(&mut session, Session::should_destroy);
        let mut reopened = Session::start(DataRoot::Existing(dir), None).unwrap();
        until(&mut reopened, |s| s.loaded);
        let item = &reopened.config.as_ref().unwrap().groups[0].items[0];
        assert_eq!(item.id.get(), 1);
        assert_eq!(item.target, target);
        reopened.close();
        until(&mut reopened, Session::should_destroy);
    }
    #[test]
    fn failed_add_reference_restores_items_and_allocator_without_touching_source() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let before = session.config.as_ref().unwrap().clone();
        let old_json = fs::read(dir.join("config.json")).unwrap();
        let source = dir.join("source.txt");
        fs::write(&source, b"keep source bytes").unwrap();
        fs::write(dir.join("config.pending"), b"external pending data").unwrap();
        let target = source_reference(&source);
        session.add_references(GroupId::new(1).unwrap(), vec![target]);
        until(&mut session, |s| !s.tracker.is_saving());
        assert_eq!(session.config.as_ref().unwrap().as_ref(), before.as_ref());
        assert!(session.details.contains("已移除本次新增引用"));
        assert!(!session.can_save());
        assert_eq!(fs::read(dir.join("config.json")).unwrap(), old_json);
        assert_eq!(fs::read(&source).unwrap(), b"keep source bytes");
        session.close();
        until(&mut session, Session::should_destroy);
    }
    #[test]
    fn remove_reference_autosaves_and_preserves_source_and_item_allocator() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let source = dir.join("source.txt");
        fs::write(&source, b"keep source bytes").unwrap();
        let group_id = GroupId::new(1).unwrap();
        assert!(session.add_references(group_id, vec![source_reference(&source)]));
        until(&mut session, |s| !s.tracker.is_saving());
        let item_id = ItemId::new(1).unwrap();
        assert!(session.contains_reference_id(group_id, item_id));
        assert!(session.remove_reference(group_id, item_id));
        until(&mut session, |s| !s.tracker.is_saving());
        assert!(!session.contains_reference_id(group_id, item_id));
        assert_eq!(session.config.as_ref().unwrap().next_item_id.get(), 2);
        assert_eq!(fs::read(&source).unwrap(), b"keep source bytes");
        session.close();
        until(&mut session, Session::should_destroy);
        let mut reopened = Session::start(DataRoot::Existing(dir), None).unwrap();
        until(&mut reopened, |s| s.loaded);
        assert!(reopened.config.as_ref().unwrap().groups[0].items.is_empty());
        assert_eq!(reopened.config.as_ref().unwrap().next_item_id.get(), 2);
        reopened.close();
        until(&mut reopened, Session::should_destroy);
    }
    #[test]
    fn undo_reference_and_populated_group_restore_exact_config_and_reopen() {
        for delete_group in [false, true] {
            let (dir, mut session) = fixture();
            until(&mut session, |s| s.loaded);
            assert!(!session.can_undo_removal());
            session.create_group();
            until(&mut session, |s| s.can_save());
            let source = dir.join("undo-source.txt");
            fs::write(&source, b"undo must not touch source").unwrap();
            let group_id = GroupId::new(1).unwrap();
            session.add_references(group_id, vec![source_reference(&source)]);
            until(&mut session, |s| s.can_save());
            let before = session.config.as_ref().unwrap().clone();
            if delete_group {
                session.delete_group(group_id);
            } else {
                session.remove_reference(group_id, ItemId::new(1).unwrap());
            }
            assert!(!session.can_undo_removal());
            until(&mut session, |s| s.can_save());
            assert!(session.can_undo_removal());
            assert!(session.undo_removal());
            assert!(!session.undo_removal());
            session.close(); // Close must wait for undo's durable save.
            until(&mut session, Session::should_destroy);
            assert_eq!(session.config.as_ref().unwrap().as_ref(), before.as_ref());
            assert!(session.last_removal.is_none());
            assert!(session.status.contains("已撤销"));
            assert_eq!(fs::read(&source).unwrap(), b"undo must not touch source");
            let mut reopened = Session::start(DataRoot::Existing(dir), None).unwrap();
            until(&mut reopened, |s| s.loaded);
            assert_eq!(reopened.config.as_ref().unwrap().as_ref(), before.as_ref());
            assert!(!reopened.can_undo_removal()); // History is deliberately session-only.
            reopened.close();
            until(&mut reopened, Session::should_destroy);
        }
    }
    #[test]
    fn failed_undo_save_rolls_back_recovery_and_retains_record_without_source_changes() {
        for delete_group in [false, true] {
            let (dir, mut session) = fixture();
            until(&mut session, |s| s.loaded);
            session.create_group();
            until(&mut session, |s| s.can_save());
            let source = dir.join("undo-source.txt");
            fs::write(&source, b"unchanged").unwrap();
            let group_id = GroupId::new(1).unwrap();
            session.add_references(group_id, vec![source_reference(&source)]);
            until(&mut session, |s| s.can_save());
            if delete_group {
                session.delete_group(group_id);
            } else {
                session.remove_reference(group_id, ItemId::new(1).unwrap());
            }
            until(&mut session, |s| s.can_save());
            let before = session.config.as_ref().unwrap().clone();
            let bytes = fs::read(dir.join("config.json")).unwrap();
            fs::write(dir.join("config.pending"), b"do not overwrite").unwrap();
            assert!(session.undo_removal());
            until(&mut session, |s| !s.tracker.is_saving());
            assert_eq!(session.config.as_ref().unwrap().as_ref(), before.as_ref());
            assert_eq!(fs::read(dir.join("config.json")).unwrap(), bytes);
            assert_eq!(
                fs::read(dir.join("config.pending")).unwrap(),
                b"do not overwrite"
            );
            assert_eq!(fs::read(source).unwrap(), b"unchanged");
            assert!(session.last_removal.is_some());
            assert!(!session.can_undo_removal());
            assert!(session.details.contains("撤销保存失败"));
            session.close();
            until(&mut session, Session::should_destroy);
        }
    }
    #[test]
    fn disconnected_worker_rolls_back_undo_before_submit_and_during_unconfirmed_save() {
        for delete_group in [false, true] {
            for during_save in [false, true] {
                let (dir, mut session) = fixture();
                until(&mut session, |s| s.loaded);
                session.create_group();
                until(&mut session, |s| s.can_save());
                let source = dir.join("undo-disconnect.txt");
                fs::write(&source, b"source stays unchanged").unwrap();
                let group_id = GroupId::new(1).unwrap();
                session.add_references(group_id, vec![source_reference(&source)]);
                until(&mut session, |s| s.can_save());
                if delete_group {
                    session.delete_group(group_id);
                } else {
                    session.remove_reference(group_id, ItemId::new(1).unwrap());
                }
                until(&mut session, |s| s.can_save());
                let before = session.config.as_ref().unwrap().clone();
                let bytes = fs::read(dir.join("config.json")).unwrap();

                // Stop the real worker and consume Stopped outside Session to inject
                // an unexpected disconnect, without timing a save against shutdown.
                session.worker.submit(Command::Shutdown).unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    match session.worker.poll() {
                        Err(TryRecvError::Disconnected) => break,
                        Ok(Event::Stopped) | Err(TryRecvError::Empty) => {}
                        event => panic!("unexpected shutdown event: {event:?}"),
                    }
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                if during_save {
                    // Inject the in-memory state of a recovery awaiting its save
                    // acknowledgement. No command is submitted to the dead worker.
                    let config = Arc::make_mut(session.config.as_mut().unwrap());
                    match session.last_removal.as_ref().unwrap() {
                        UndoRemoval::Group(index, group) => {
                            restore_group(config, *index, group).unwrap();
                        }
                        UndoRemoval::Reference(id, index, item) => {
                            restore_reference(config, *id, *index, item).unwrap();
                        }
                    }
                    session.pending_undo = true;
                    session.tracker.begin().unwrap();
                    assert!(session.drain());
                    assert!(session.status.contains("本次恢复已回退"));
                } else {
                    assert!(!session.undo_removal());
                    assert!(session.status.contains("无法提交保存"));
                    session.drain();
                }
                assert_eq!(session.config.as_ref().unwrap().as_ref(), before.as_ref());
                assert!(!session.pending_undo);
                assert!(session.last_removal.is_some());
                assert!(!session.can_undo_removal());
                assert_eq!(fs::read(dir.join("config.json")).unwrap(), bytes);
                assert_eq!(fs::read(&source).unwrap(), b"source stays unchanged");
                session.close();
                assert!(session.should_destroy());
            }
        }
    }
    #[test]
    fn undo_record_is_single_entry_and_stale_save_cannot_consume_it() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| s.can_save());
        let group_id = GroupId::new(1).unwrap();
        session.add_references(
            group_id,
            vec![
                source_reference(&dir.join("one.txt")),
                source_reference(&dir.join("two.txt")),
            ],
        );
        until(&mut session, |s| s.can_save());
        session.remove_reference(group_id, ItemId::new(1).unwrap());
        until(&mut session, |s| s.can_save());
        session.remove_reference(group_id, ItemId::new(2).unwrap());
        until(&mut session, |s| s.can_save());
        session.rename_group(group_id, "保留非删除修改".into());
        until(&mut session, |s| s.can_save());
        session.accept(Event::Saved {
            revision: 1,
            result: Ok(()),
        });
        assert!(session.can_undo_removal());
        session.undo_removal();
        until(&mut session, |s| s.can_save());
        assert!(!session.contains_reference_id(group_id, ItemId::new(1).unwrap()));
        assert!(session.contains_reference_id(group_id, ItemId::new(2).unwrap()));
        assert_eq!(
            session.config.as_ref().unwrap().groups[0].title,
            "保留非删除修改"
        );
        assert!(!session.can_undo_removal());
        session.close();
        until(&mut session, Session::should_destroy);
    }
    #[test]
    fn undo_duplicate_target_is_atomic_and_keeps_history() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| s.can_save());
        let group_id = GroupId::new(1).unwrap();
        let target = source_reference(&dir.join("one.txt"));
        session.add_references(group_id, vec![target.clone()]);
        until(&mut session, |s| s.can_save());
        session.remove_reference(group_id, ItemId::new(1).unwrap());
        until(&mut session, |s| s.can_save());
        session.add_references(group_id, vec![target]);
        until(&mut session, |s| s.can_save());
        let before = session.config.as_ref().unwrap().clone();
        let bytes = fs::read(dir.join("config.json")).unwrap();
        assert!(!session.undo_removal());
        assert_eq!(session.config.as_ref().unwrap().as_ref(), before.as_ref());
        assert_eq!(fs::read(dir.join("config.json")).unwrap(), bytes);
        assert!(session.status.contains("无法撤销"));
        assert!(session.can_undo_removal());
        session.close();
        until(&mut session, Session::should_destroy);
    }
    #[test]
    fn failed_remove_reference_restores_exact_item_without_touching_source() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| !s.tracker.is_saving());
        let source = dir.join("source.txt");
        fs::write(&source, b"keep source bytes").unwrap();
        let group_id = GroupId::new(1).unwrap();
        assert!(session.add_references(group_id, vec![source_reference(&source)]));
        until(&mut session, |s| !s.tracker.is_saving());
        let before = session.config.as_ref().unwrap().clone();
        let old_json = fs::read(dir.join("config.json")).unwrap();
        fs::write(dir.join("config.pending"), b"external pending data").unwrap();
        assert!(session.remove_reference(group_id, ItemId::new(1).unwrap()));
        until(&mut session, |s| !s.tracker.is_saving());
        assert_eq!(session.config.as_ref().unwrap().as_ref(), before.as_ref());
        assert!(session.details.contains("已恢复原引用"));
        assert!(!session.can_save());
        assert_eq!(fs::read(dir.join("config.json")).unwrap(), old_json);
        assert_eq!(fs::read(&source).unwrap(), b"keep source bytes");
        session.close();
        until(&mut session, Session::should_destroy);
    }
    #[test]
    fn reorder_autosaves_stable_ids_and_reopens_in_new_order() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| s.can_save());
        let group = GroupId::new(1).unwrap();
        let sources: Vec<_> = (0..3).map(|i| dir.join(format!("sort-{i}.txt"))).collect();
        for source in &sources {
            fs::write(source, b"sorting must not change bytes").unwrap();
        }
        session.add_references(group, sources.iter().map(|p| source_reference(p)).collect());
        until(&mut session, |s| s.can_save());
        let item = ItemId::new(1).unwrap();
        assert_eq!(session.shifted_index(group, item, -1), None);
        assert_eq!(session.shifted_index(group, item, 1), Some(1));
        assert!(session.reorder_reference(group, item, 2));
        assert!(!session.reorder_reference(group, item, 0)); // Save in flight.
        until(&mut session, |s| s.can_save());
        let expected = session.config.as_ref().unwrap().clone();
        assert_eq!(
            expected.groups[0]
                .items
                .iter()
                .map(|i| i.id.get())
                .collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
        assert_eq!(expected.next_item_id.get(), 4);
        assert!(session.pending_sort.is_none());
        assert!(!session.reorder_reference(group, item, 2)); // No-op doesn't save.
        session.writable = false;
        assert!(!session.reorder_reference(group, item, 0));
        session.close();
        until(&mut session, Session::should_destroy);
        let mut reopened = Session::start(DataRoot::Existing(dir), None).unwrap();
        until(&mut reopened, |s| s.loaded);
        assert_eq!(
            reopened.config.as_ref().unwrap().as_ref(),
            expected.as_ref()
        );
        for source in &sources {
            assert_eq!(fs::read(source).unwrap(), b"sorting must not change bytes");
        }
        reopened.close();
        until(&mut reopened, Session::should_destroy);
    }
    #[test]
    fn failed_sort_restores_exact_order_and_preserves_prior_undo() {
        let (dir, mut session) = fixture();
        until(&mut session, |s| s.loaded);
        session.create_group();
        until(&mut session, |s| s.can_save());
        let group = GroupId::new(1).unwrap();
        session.add_references(
            group,
            (0..3)
                .map(|i| source_reference(&dir.join(format!("ref-{i}.txt"))))
                .collect(),
        );
        until(&mut session, |s| s.can_save());
        session.remove_reference(group, ItemId::new(3).unwrap());
        until(&mut session, |s| s.can_save());
        let expected = session.config.as_ref().unwrap().clone();
        let saved = fs::read(dir.join("config.json")).unwrap();
        fs::write(dir.join("config.pending"), b"external pending fixture").unwrap();
        assert!(session.reorder_reference(group, ItemId::new(1).unwrap(), 1));
        until(&mut session, |s| !s.tracker.is_saving());
        assert_eq!(session.config.as_ref().unwrap().as_ref(), expected.as_ref());
        assert!(session.pending_sort.is_none());
        assert!(session.last_removal.is_some());
        assert!(session.details.contains("已恢复原顺序"));
        assert_eq!(fs::read(dir.join("config.json")).unwrap(), saved);
        session.close();
        until(&mut session, Session::should_destroy);
    }
    #[test]
    fn disconnected_worker_rolls_back_sort_before_submit_and_during_save() {
        for during_save in [false, true] {
            let (dir, mut session) = fixture();
            until(&mut session, |s| s.loaded);
            session.create_group();
            until(&mut session, |s| s.can_save());
            let group = GroupId::new(1).unwrap();
            let item = ItemId::new(1).unwrap();
            session.add_references(
                group,
                (0..2)
                    .map(|i| source_reference(&dir.join(format!("sort-stop-{i}.txt"))))
                    .collect(),
            );
            until(&mut session, |s| s.can_save());
            let before = session.config.as_ref().unwrap().clone();
            let bytes = fs::read(dir.join("config.json")).unwrap();
            session.worker.submit(Command::Shutdown).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match session.worker.poll() {
                    Err(TryRecvError::Disconnected) => break,
                    Ok(Event::Stopped) | Err(TryRecvError::Empty) => {}
                    event => panic!("unexpected shutdown event: {event:?}"),
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            if during_save {
                reorder_reference(
                    Arc::make_mut(session.config.as_mut().unwrap()),
                    group,
                    item,
                    1,
                )
                .unwrap();
                session.pending_sort = Some((group, item, 0));
                session.tracker.begin().unwrap();
                assert!(session.drain());
                assert!(session.status.contains("排序已回退"));
            } else {
                assert!(!session.reorder_reference(group, item, 1));
                assert!(session.status.contains("无法提交保存"));
                session.drain();
            }
            assert!(session.pending_sort.is_none());
            assert_eq!(session.config.as_ref().unwrap().as_ref(), before.as_ref());
            assert_eq!(fs::read(dir.join("config.json")).unwrap(), bytes);
            session.close();
            assert!(session.should_destroy());
        }
    }
}
