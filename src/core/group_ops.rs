//! Transactional operations on configuration groups.
use super::{
    contracts::AppError,
    layout::RectDip,
    model::{
        Config, FileReference, Group, GroupId, Item, ItemId, MAX_GROUPS, MAX_ITEMS,
        MAX_TOTAL_PATH_UNITS, ValidationError,
    },
};
use std::collections::BTreeSet;

/// Adds a batch of path references to one group; no source file is opened or modified.
/// All size, duplicate and ID checks precede mutation, so an error is atomic.
pub fn add_references(
    config: &mut Config,
    group_id: GroupId,
    targets: Vec<FileReference>,
) -> Result<Vec<ItemId>, AppError> {
    config.validate()?;
    let index = config
        .groups
        .iter()
        .position(|group| group.id == group_id)
        .ok_or(AppError::NotFound)?;
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let existing_count = config
        .groups
        .iter()
        .map(|group| group.items.len())
        .sum::<usize>();
    if existing_count
        .checked_add(targets.len())
        .is_none_or(|count| count > MAX_ITEMS)
    {
        return Err(AppError::InvalidData(ValidationError::TooManyItems));
    }
    let mut paths = BTreeSet::new();
    for item in &config.groups[index].items {
        paths.insert(&item.target.path);
    }
    for target in &targets {
        if !paths.insert(&target.path) {
            return Err(AppError::InvalidData(ValidationError::DuplicateTarget));
        }
    }
    let existing_units = config
        .groups
        .iter()
        .flat_map(|group| &group.items)
        .map(|item| item.target.path.as_utf16().len())
        .sum::<usize>();
    let incoming_units = targets
        .iter()
        .map(|target| target.path.as_utf16().len())
        .try_fold(0_usize, |total, length| total.checked_add(length))
        .ok_or(AppError::InvalidData(ValidationError::PathBudgetExceeded))?;
    if existing_units
        .checked_add(incoming_units)
        .is_none_or(|total| total > MAX_TOTAL_PATH_UNITS)
    {
        return Err(AppError::InvalidData(ValidationError::PathBudgetExceeded));
    }
    let count = u64::try_from(targets.len())
        .map_err(|_| AppError::InvalidData(ValidationError::IdExhausted))?;
    if config.next_item_id.get().checked_add(count).is_none() {
        return Err(AppError::InvalidData(ValidationError::IdExhausted));
    }
    let mut ids = Vec::with_capacity(targets.len());
    for target in targets {
        let id = config
            .allocate_item_id()
            .expect("batch ID range was checked");
        config.groups[index].items.push(Item { id, target });
        ids.push(id);
    }
    Ok(ids)
}

/// Creates a group after validating both the existing configuration and the new values.
/// All fallible checks precede mutation, so errors leave `config` unchanged.
pub fn create_group(
    config: &mut Config,
    title: String,
    rect: RectDip,
) -> Result<GroupId, AppError> {
    config.validate()?;
    validate_title(&title)?;
    rect.validate()
        .map_err(|_| AppError::InvalidData(ValidationError::InvalidLayout))?;
    if config.groups.len() >= MAX_GROUPS {
        return Err(AppError::InvalidData(ValidationError::TooManyGroups));
    }

    let id = config.allocate_group_id()?;
    config.groups.push(Group {
        id,
        title,
        color: [62, 110, 180],
        rect,
        monitor_key_utf16: None,
        collapsed: false,
        items: Vec::new(),
    });
    Ok(id)
}

/// Removes a group without changing either ID allocator.
/// Returns its original index and complete value so callers can restore it.
pub fn delete_group(config: &mut Config, id: GroupId) -> Result<(usize, Group), AppError> {
    config.validate()?;
    let index = config
        .groups
        .iter()
        .position(|group| group.id == id)
        .ok_or(AppError::NotFound)?;
    Ok((index, config.groups.remove(index)))
}

/// Restores a previously removed group at its former position, clamped to the current list.
/// The complete configuration is validated after insertion; validation failures roll back.
pub fn restore_group(config: &mut Config, index: usize, group: &Group) -> Result<(), AppError> {
    config.validate()?;
    let restore_index = index.min(config.groups.len());
    config.groups.insert(restore_index, group.clone());
    if let Err(error) = config.validate() {
        config.groups.remove(restore_index);
        return Err(error.into());
    }
    Ok(())
}

/// Removes one configuration reference from a group without touching its source file.
/// Returns the original index and complete item so callers can restore it on failure.
pub fn remove_reference(
    config: &mut Config,
    group_id: GroupId,
    item_id: ItemId,
) -> Result<(usize, Item), AppError> {
    config.validate()?;
    let group_index = config
        .groups
        .iter()
        .position(|group| group.id == group_id)
        .ok_or(AppError::NotFound)?;
    let item_index = config.groups[group_index]
        .items
        .iter()
        .position(|item| item.id == item_id)
        .ok_or(AppError::NotFound)?;
    Ok((
        item_index,
        config.groups[group_index].items.remove(item_index),
    ))
}

/// Transfers an existing reference between groups without changing its identity or target.
/// Returns its original source index; all checks precede mutation.
pub fn transfer_reference(
    config: &mut Config,
    source: GroupId,
    item: ItemId,
    destination: GroupId,
) -> Result<usize, AppError> {
    config.validate()?;
    if source == destination {
        return Err(AppError::NotFound);
    }
    let source_index = config
        .groups
        .iter()
        .position(|group| group.id == source)
        .ok_or(AppError::NotFound)?;
    let destination_index = config
        .groups
        .iter()
        .position(|group| group.id == destination)
        .ok_or(AppError::NotFound)?;
    let item_index = config.groups[source_index]
        .items
        .iter()
        .position(|candidate| candidate.id == item)
        .ok_or(AppError::NotFound)?;
    let moving = &config.groups[source_index].items[item_index];
    if config.groups[destination_index]
        .items
        .iter()
        .any(|candidate| candidate.target.path == moving.target.path)
    {
        return Err(AppError::InvalidData(ValidationError::DuplicateTarget));
    }

    let moved = config.groups[source_index].items.remove(item_index);
    config.groups[destination_index].items.push(moved);
    Ok(item_index)
}

/// Moves an existing reference within one group without changing its identity or target.
/// Returns the original index; invalid group, item, or destination leaves the config unchanged.
pub fn reorder_reference(
    config: &mut Config,
    group_id: GroupId,
    item_id: ItemId,
    new_index: usize,
) -> Result<usize, AppError> {
    config.validate()?;
    let group_index = config
        .groups
        .iter()
        .position(|group| group.id == group_id)
        .ok_or(AppError::NotFound)?;
    let items = &config.groups[group_index].items;
    let old_index = items
        .iter()
        .position(|item| item.id == item_id)
        .ok_or(AppError::NotFound)?;
    if new_index >= items.len() {
        return Err(AppError::InvalidData(ValidationError::InvalidId));
    }
    if old_index != new_index {
        let items = &mut config.groups[group_index].items;
        let moved = items.remove(old_index);
        items.insert(new_index, moved);
    }
    Ok(old_index)
}

/// Restores a removed reference within its group, clamping its old position to the current list.
/// The complete configuration is validated after insertion; validation failures roll back.
pub fn restore_reference(
    config: &mut Config,
    group_id: GroupId,
    index: usize,
    item: &Item,
) -> Result<(), AppError> {
    config.validate()?;
    let group_index = config
        .groups
        .iter()
        .position(|group| group.id == group_id)
        .ok_or(AppError::NotFound)?;
    let restore_index = index.min(config.groups[group_index].items.len());
    config.groups[group_index]
        .items
        .insert(restore_index, item.clone());
    if let Err(error) = config.validate() {
        config.groups[group_index].items.remove(restore_index);
        return Err(error.into());
    }
    Ok(())
}

/// Toggles a group's collapsed state and returns the previous value for undo.
pub fn toggle_group_collapsed(config: &mut Config, id: GroupId) -> Result<bool, AppError> {
    config.validate()?;
    let group = config
        .groups
        .iter_mut()
        .find(|group| group.id == id)
        .ok_or(AppError::NotFound)?;
    let previous = group.collapsed;
    group.collapsed = !previous;
    Ok(previous)
}

/// Changes a group's RGB color and returns the previous color for undo.
pub fn set_group_color(
    config: &mut Config,
    id: GroupId,
    color: [u8; 3],
) -> Result<[u8; 3], AppError> {
    config.validate()?;
    let group = config
        .groups
        .iter()
        .find(|group| group.id == id)
        .ok_or(AppError::NotFound)?;
    let previous_color = group.color;
    let group = config
        .groups
        .iter_mut()
        .find(|group| group.id == id)
        .expect("group was checked before mutation");
    group.color = color;
    Ok(previous_color)
}

fn validate_title(title: &str) -> Result<(), AppError> {
    if title.trim().is_empty() || title.chars().count() > 128 || title.chars().any(char::is_control)
    {
        Err(AppError::InvalidData(ValidationError::InvalidTitle))
    } else {
        Ok(())
    }
}

/// Renames a group while preserving its identity and all associated data.
/// Returns the previous title so callers can undo the operation if needed.
pub fn rename_group(config: &mut Config, id: GroupId, title: String) -> Result<String, AppError> {
    config.validate()?;
    validate_title(&title)?;
    let group = config
        .groups
        .iter()
        .find(|group| group.id == id)
        .ok_or(AppError::NotFound)?;
    let previous_title = group.title.clone();
    let group = config
        .groups
        .iter_mut()
        .find(|group| group.id == id)
        .expect("group was checked before mutation");
    group.title = title;
    Ok(previous_title)
}

/// Moves a group while preserving its size and associated data.
/// Returns the previous rectangle so callers can undo the operation if needed.
pub fn move_group(config: &mut Config, id: GroupId, x: i32, y: i32) -> Result<RectDip, AppError> {
    config.validate()?;
    let group = config
        .groups
        .iter()
        .find(|group| group.id == id)
        .ok_or(AppError::NotFound)?;
    let old_rect = group.rect;
    let new_rect = RectDip { x, y, ..old_rect };
    new_rect
        .validate()
        .map_err(|_| AppError::InvalidData(ValidationError::InvalidLayout))?;

    let group = config
        .groups
        .iter_mut()
        .find(|group| group.id == id)
        .expect("group was checked before mutation");
    group.rect.x = new_rect.x;
    group.rect.y = new_rect.y;
    Ok(old_rect)
}

/// Resizes a group while preserving its position and associated data.
/// Returns the previous rectangle so callers can undo the operation if needed.
pub fn resize_group(
    config: &mut Config,
    id: GroupId,
    width: u32,
    height: u32,
) -> Result<RectDip, AppError> {
    config.validate()?;
    let group = config
        .groups
        .iter()
        .find(|group| group.id == id)
        .ok_or(AppError::NotFound)?;
    let old_rect = group.rect;
    let new_rect = RectDip {
        width,
        height,
        ..old_rect
    };
    new_rect
        .validate()
        .map_err(|_| AppError::InvalidData(ValidationError::InvalidLayout))?;

    let group = config
        .groups
        .iter_mut()
        .find(|group| group.id == id)
        .expect("group was checked before mutation");
    group.rect.width = new_rect.width;
    group.rect.height = new_rect.height;
    Ok(old_rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Group, MAX_GROUPS};

    fn rect() -> RectDip {
        RectDip {
            x: -20,
            y: 10,
            width: 320,
            height: 240,
        }
    }

    fn group(id: u64) -> Group {
        Group {
            id: GroupId::new(id).unwrap(),
            title: format!("Group {id}"),
            color: [62, 110, 180],
            rect: rect(),
            monitor_key_utf16: None,
            collapsed: false,
            items: Vec::new(),
        }
    }

    fn item(id: u64, path: &str) -> Item {
        Item {
            id: ItemId::new(id).unwrap(),
            target: FileReference {
                kind: crate::core::model::ReferenceKind::File,
                path: crate::core::model::NativePath::from_utf16(path.encode_utf16().collect())
                    .unwrap(),
            },
        }
    }

    fn config_with_transfer_items() -> (Config, GroupId, GroupId, GroupId, Vec<Item>) {
        let mut config = Config::default();
        let source = create_group(&mut config, "Source".into(), rect()).unwrap();
        let destination = create_group(&mut config, "Destination".into(), rect()).unwrap();
        let other = create_group(&mut config, "Other".into(), rect()).unwrap();
        let mut items = vec![
            item(1, "C:\\first.txt"),
            item(2, "C:\\middle.txt"),
            item(3, "C:\\last.txt"),
        ];
        items[1].target.kind = crate::core::model::ReferenceKind::Directory;
        items[2].target.kind = crate::core::model::ReferenceKind::Shortcut;
        config.groups[0].items = items.clone();
        config.groups[1].items.push(item(4, "C:\\existing.txt"));
        config.groups[2].items.push(item(5, "C:\\other.txt"));
        config.next_item_id = ItemId::new(6).unwrap();
        (config, source, destination, other, items)
    }

    #[test]
    fn transfer_reference_preserves_item_and_appends_in_destination_order() {
        let (mut config, source, destination, other, items) = config_with_transfer_items();
        let before_other = config.groups[2].clone();
        let next_group_id = config.next_group_id;
        let next_item_id = config.next_item_id;
        assert_eq!(
            transfer_reference(&mut config, source, items[1].id, destination),
            Ok(1)
        );
        assert_eq!(
            config.groups[0].items,
            vec![items[0].clone(), items[2].clone()]
        );
        assert_eq!(
            config.groups[1].items,
            vec![item(4, "C:\\existing.txt"), items[1].clone()]
        );
        assert_eq!(config.groups[2], before_other);
        assert_eq!(config.groups[2].id, other);
        assert_eq!(config.next_group_id, next_group_id);
        assert_eq!(config.next_item_id, next_item_id);
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn transfer_reference_supports_file_directory_and_shortcut_kinds() {
        let (mut config, source, destination, _, items) = config_with_transfer_items();
        for (index, expected_kind) in [
            crate::core::model::ReferenceKind::File,
            crate::core::model::ReferenceKind::Directory,
            crate::core::model::ReferenceKind::Shortcut,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                transfer_reference(&mut config, source, items[index].id, destination),
                Ok(0)
            );
            assert_eq!(
                config.groups[1].items.last().unwrap().target.kind,
                expected_kind
            );
        }
    }

    #[test]
    fn transfer_reference_missing_wrong_group_and_equal_group_fail_atomically() {
        let (mut config, source, destination, _, items) = config_with_transfer_items();
        let wrong_source = destination;
        let cases = [
            (GroupId::new(99).unwrap(), items[0].id, destination),
            (source, ItemId::new(99).unwrap(), destination),
            (wrong_source, items[0].id, source),
            (source, items[0].id, GroupId::new(99).unwrap()),
            (source, items[0].id, source),
        ];
        for (from, id, to) in cases {
            let before = config.clone();
            assert_eq!(
                transfer_reference(&mut config, from, id, to),
                Err(AppError::NotFound)
            );
            assert_eq!(config, before);
        }
    }

    #[test]
    fn transfer_reference_rejects_duplicate_destination_path_atomically() {
        let (mut config, source, destination, _, items) = config_with_transfer_items();
        config.groups[1].items.push(item(6, "C:\\middle.txt"));
        config.next_item_id = ItemId::new(7).unwrap();
        let before = config.clone();
        assert_eq!(
            transfer_reference(&mut config, source, items[1].id, destination),
            Err(AppError::InvalidData(ValidationError::DuplicateTarget))
        );
        assert_eq!(config, before);
    }

    #[test]
    fn transfer_reference_rejects_invalid_config_atomically() {
        let (mut config, source, destination, _, items) = config_with_transfer_items();
        config.groups[0].title.clear();
        let before = config.clone();
        assert!(transfer_reference(&mut config, source, items[0].id, destination).is_err());
        assert_eq!(config, before);
    }

    #[test]
    fn removes_first_middle_and_last_references_and_returns_original_items() {
        let mut config = Config::default();
        let group_id = create_group(&mut config, "References".into(), rect()).unwrap();
        let original = vec![
            item(1, "C:\\first.txt"),
            item(2, "C:\\middle.txt"),
            item(3, "C:\\last.txt"),
        ];
        config.groups[0].items = original.clone();
        config.next_item_id = ItemId::new(4).unwrap();
        let next_item_id = config.next_item_id;

        assert_eq!(
            remove_reference(&mut config, group_id, original[0].id),
            Ok((0, original[0].clone()))
        );
        assert_eq!(config.groups[0].items, original[1..]);
        assert_eq!(
            remove_reference(&mut config, group_id, original[2].id),
            Ok((1, original[2].clone()))
        );
        assert_eq!(config.groups[0].items, vec![original[1].clone()]);
        assert_eq!(
            remove_reference(&mut config, group_id, original[1].id),
            Ok((0, original[1].clone()))
        );
        assert!(config.groups[0].items.is_empty());
        assert_eq!(config.next_item_id, next_item_id);
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn remove_reference_failures_leave_config_unchanged() {
        let mut config = Config::default();
        let group_id = create_group(&mut config, "References".into(), rect()).unwrap();
        config.groups[0].items.push(item(1, "C:\\target.txt"));
        config.next_item_id = ItemId::new(2).unwrap();

        let before = config.clone();
        assert_eq!(
            remove_reference(
                &mut config,
                GroupId::new(999).unwrap(),
                ItemId::new(1).unwrap()
            ),
            Err(AppError::NotFound)
        );
        assert_eq!(config, before);
        assert_eq!(
            remove_reference(&mut config, group_id, ItemId::new(999).unwrap()),
            Err(AppError::NotFound)
        );
        assert_eq!(config, before);

        let mut invalid = config.clone();
        invalid.groups[0].title.clear();
        let invalid_before = invalid.clone();
        assert!(remove_reference(&mut invalid, group_id, ItemId::new(1).unwrap()).is_err());
        assert_eq!(invalid, invalid_before);
    }

    #[test]
    fn removes_matching_id_only_from_the_requested_group() {
        let mut config = Config::default();
        let first = create_group(&mut config, "First".into(), rect()).unwrap();
        let second = create_group(&mut config, "Second".into(), rect()).unwrap();
        let shared_path = "C:\\same-target.txt";
        let first_item = item(1, shared_path);
        let second_item = item(2, shared_path);
        config.groups[0].items.push(first_item.clone());
        config.groups[1].items.push(second_item.clone());
        config.next_item_id = ItemId::new(3).unwrap();

        assert_eq!(
            remove_reference(&mut config, second, second_item.id),
            Ok((0, second_item.clone()))
        );
        assert_eq!(config.groups[0].items, vec![first_item]);
        assert!(config.groups[1].items.is_empty());
        assert_eq!(config.groups[0].id, first);
        assert_eq!(config.next_item_id, ItemId::new(3).unwrap());
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn creates_groups_with_sequential_ids_and_defaults() {
        let mut config = Config::default();
        let first = create_group(&mut config, "Alpha".into(), rect()).unwrap();
        let second = create_group(&mut config, "Beta".into(), rect()).unwrap();

        assert_eq!((first.get(), second.get()), (1, 2));
        assert_eq!(config.groups[0].color, [62, 110, 180]);
        assert_eq!(config.groups[0].monitor_key_utf16, None);
        assert!(!config.groups[0].collapsed);
        assert!(config.groups[0].items.is_empty());
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn deletes_first_middle_and_last_groups_and_returns_original_values() {
        let mut config = Config::default();
        let ids: Vec<_> = ["First", "Middle", "Last"]
            .into_iter()
            .map(|title| create_group(&mut config, title.into(), rect()).unwrap())
            .collect();
        let originals = config.groups.clone();

        let (index, deleted) = delete_group(&mut config, ids[0]).unwrap();
        assert_eq!((index, deleted), (0, originals[0].clone()));
        assert_eq!(config.groups, originals[1..]);

        let (index, deleted) = delete_group(&mut config, ids[2]).unwrap();
        assert_eq!((index, deleted), (1, originals[2].clone()));
        assert_eq!(config.groups, vec![originals[1].clone()]);

        let (index, deleted) = delete_group(&mut config, ids[1]).unwrap();
        assert_eq!((index, deleted), (0, originals[1].clone()));
        assert!(config.groups.is_empty());
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn deletion_removes_only_config_reference_and_preserves_allocators_and_other_groups() {
        let mut config = Config::default();
        let first = create_group(&mut config, "First".into(), rect()).unwrap();
        let target = create_group(&mut config, "Target".into(), rect()).unwrap();
        let last = create_group(&mut config, "Last".into(), rect()).unwrap();
        config.groups[1].items.push(crate::core::model::Item {
            id: crate::core::model::ItemId::new(1).unwrap(),
            target: crate::core::model::FileReference {
                kind: crate::core::model::ReferenceKind::File,
                path: crate::core::model::NativePath::from_utf16(
                    "C:\\item.txt".encode_utf16().collect(),
                )
                .unwrap(),
            },
        });
        config.next_item_id = crate::core::model::ItemId::new(2).unwrap();
        let first_before = config.groups[0].clone();
        let target_before = config.groups[1].clone();
        let last_before = config.groups[2].clone();
        let next_group_id = config.next_group_id;
        let next_item_id = config.next_item_id;

        let (index, deleted) = delete_group(&mut config, target).unwrap();

        assert_eq!(index, 1);
        assert_eq!(deleted, target_before);
        assert_eq!(config.groups, vec![first_before, last_before]);
        assert_eq!(config.next_group_id, next_group_id);
        assert_eq!(config.next_item_id, next_item_id);
        assert_eq!(first.get(), 1);
        assert_eq!(last.get(), 3);
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn delete_failures_leave_config_unchanged() {
        let mut config = Config::default();
        let id = create_group(&mut config, "Existing".into(), rect()).unwrap();

        let before_missing = config.clone();
        assert_eq!(
            delete_group(&mut config, GroupId::new(999).unwrap()),
            Err(AppError::NotFound)
        );
        assert_eq!(config, before_missing);

        let mut invalid_config = config.clone();
        invalid_config.groups[0].title.clear();
        let before_invalid = invalid_config.clone();
        assert!(delete_group(&mut invalid_config, id).is_err());
        assert_eq!(invalid_config, before_invalid);
    }

    #[test]
    fn restores_groups_at_first_middle_and_last_positions_with_clamping() {
        let mut config = Config::default();
        let originals: Vec<_> = ["First", "Middle", "Last"]
            .into_iter()
            .map(|title| {
                create_group(&mut config, title.into(), rect()).unwrap();
                config.groups.last().unwrap().clone()
            })
            .collect();
        let saved_next_id = config.next_group_id;
        for original in &originals {
            delete_group(&mut config, original.id).unwrap();
        }
        for (index, group) in [
            (0, &originals[0]),
            (1, &originals[1]),
            (usize::MAX, &originals[2]),
        ] {
            restore_group(&mut config, index, group).unwrap();
        }
        assert_eq!(config.groups, originals);
        assert_eq!(config.next_group_id, saved_next_id);
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn restore_group_duplicate_id_and_invalid_group_are_atomic() {
        let mut config = Config::default();
        let id = create_group(&mut config, "Existing".into(), rect()).unwrap();
        let before = config.clone();
        let duplicate = config.groups[0].clone();
        assert!(restore_group(&mut config, 0, &duplicate).is_err());
        assert_eq!(config, before);

        let mut invalid = group(90);
        invalid.title.clear();
        assert!(restore_group(&mut config, 0, &invalid).is_err());
        assert_eq!(config, before);
        assert_eq!(id, config.groups[0].id);
    }

    #[test]
    fn restores_references_at_first_middle_and_last_positions() {
        let mut config = Config::default();
        let group_id = create_group(&mut config, "References".into(), rect()).unwrap();
        let originals = vec![
            item(1, "C:\\first"),
            item(2, "C:\\middle"),
            item(3, "C:\\last"),
        ];
        config.next_item_id = ItemId::new(4).unwrap();
        for (index, original) in [
            (0, &originals[0]),
            (1, &originals[1]),
            (usize::MAX, &originals[2]),
        ] {
            restore_reference(&mut config, group_id, index, original).unwrap();
        }
        assert_eq!(config.groups[0].items, originals);
        assert_eq!(config.next_item_id, ItemId::new(4).unwrap());
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn restore_reference_rejects_same_group_duplicate_id_or_target_atomically() {
        let mut config = Config::default();
        let group_id = create_group(&mut config, "References".into(), rect()).unwrap();
        let existing = item(1, "C:\\same");
        config.groups[0].items.push(existing.clone());
        config.next_item_id = ItemId::new(2).unwrap();
        let before = config.clone();
        assert!(restore_reference(&mut config, group_id, 0, &existing).is_err());
        assert_eq!(config, before);
        assert!(restore_reference(&mut config, group_id, 1, &item(2, "C:\\same")).is_err());
        assert_eq!(config, before);
    }

    #[test]
    fn restore_reference_allows_same_path_in_another_group_and_missing_group_is_atomic() {
        let mut config = Config::default();
        let first = create_group(&mut config, "First".into(), rect()).unwrap();
        let second = create_group(&mut config, "Second".into(), rect()).unwrap();
        let shared = item(1, "C:\\same");
        config.groups[0].items.push(shared.clone());
        config.next_item_id = ItemId::new(3).unwrap();
        let second_item = item(2, "C:\\same");
        restore_reference(&mut config, second, 0, &second_item).unwrap();
        assert_eq!(config.groups[1].items, vec![second_item]);
        let before = config.clone();
        assert_eq!(
            restore_reference(
                &mut config,
                GroupId::new(999).unwrap(),
                0,
                &item(3, "C:\\x")
            ),
            Err(AppError::NotFound)
        );
        assert_eq!(config, before);
        assert_eq!(config.groups[0].id, first);
    }

    #[test]
    fn restore_reference_invalid_item_or_allocator_state_rolls_back() {
        let mut config = Config::default();
        let group_id = create_group(&mut config, "References".into(), rect()).unwrap();
        let invalid_item = item(1, "C:\\invalid");
        let before = config.clone();
        let mut bad_config = config.clone();
        bad_config.groups[0].title.clear();
        let before_bad = bad_config.clone();
        assert!(restore_reference(&mut bad_config, group_id, 0, &invalid_item).is_err());
        assert_eq!(bad_config, before_bad);
        assert_eq!(config, before);

        let mut invalid_allocator = config.clone();
        invalid_allocator.next_item_id = ItemId::new(u64::MAX).unwrap();
        let before_allocator = invalid_allocator.clone();
        assert!(
            restore_reference(
                &mut invalid_allocator,
                group_id,
                0,
                &item(u64::MAX, "C:\\valid")
            )
            .is_err()
        );
        assert_eq!(invalid_allocator, before_allocator);
    }

    #[test]
    fn toggles_group_collapsed_and_preserves_every_other_field() {
        let mut config = Config::default();
        let first = create_group(&mut config, "First".into(), rect()).unwrap();
        let target = create_group(&mut config, "Target".into(), rect()).unwrap();
        let last = create_group(&mut config, "Last".into(), rect()).unwrap();
        config.groups[1].color = [4, 5, 6];
        config.groups[1].monitor_key_utf16 = Some(vec![65, 66]);
        config.groups[1].items.push(crate::core::model::Item {
            id: crate::core::model::ItemId::new(1).unwrap(),
            target: crate::core::model::FileReference {
                kind: crate::core::model::ReferenceKind::File,
                path: crate::core::model::NativePath::from_utf16(
                    "C:\\item.txt".encode_utf16().collect(),
                )
                .unwrap(),
            },
        });
        config.next_item_id = crate::core::model::ItemId::new(2).unwrap();
        let before = config.clone();

        assert_eq!(toggle_group_collapsed(&mut config, target), Ok(false));
        let mut expected = before.clone();
        expected.groups[1].collapsed = true;
        assert_eq!(config, expected);
        assert_eq!(config.groups[0].id, first);
        assert_eq!(config.groups[2].id, last);

        assert_eq!(toggle_group_collapsed(&mut config, target), Ok(true));
        assert_eq!(config, before);
    }

    #[test]
    fn toggle_group_collapsed_failures_leave_config_unchanged() {
        let mut config = Config::default();
        let id = create_group(&mut config, "Existing".into(), rect()).unwrap();

        let before_missing = config.clone();
        assert_eq!(
            toggle_group_collapsed(&mut config, GroupId::new(999).unwrap()),
            Err(AppError::NotFound)
        );
        assert_eq!(config, before_missing);

        let mut invalid_config = config.clone();
        invalid_config.groups[0].title.clear();
        let before_invalid = invalid_config.clone();
        assert!(toggle_group_collapsed(&mut invalid_config, id).is_err());
        assert_eq!(invalid_config, before_invalid);
    }

    #[test]
    fn sets_only_target_group_color_and_returns_previous_color() {
        let mut config = Config::default();
        let first = create_group(&mut config, "First".into(), rect()).unwrap();
        let target = create_group(&mut config, "Target".into(), rect()).unwrap();
        let last = create_group(&mut config, "Last".into(), rect()).unwrap();
        config.groups[0].color = [1, 2, 3];
        config.groups[1].color = [4, 5, 6];
        config.groups[2].color = [7, 8, 9];
        let before = config.clone();

        assert_eq!(
            set_group_color(&mut config, target, [0, 128, 255]),
            Ok([4, 5, 6])
        );

        let mut expected = before;
        expected.groups[1].color = [0, 128, 255];
        assert_eq!(config, expected);
        assert_eq!(config.groups[0].id, first);
        assert_eq!(config.groups[1].id, target);
        assert_eq!(config.groups[2].id, last);
    }

    #[test]
    fn set_group_color_failures_leave_config_unchanged() {
        let mut config = Config::default();
        let id = create_group(&mut config, "Existing".into(), rect()).unwrap();

        let before_missing = config.clone();
        assert_eq!(
            set_group_color(&mut config, GroupId::new(999).unwrap(), [255, 255, 255]),
            Err(AppError::NotFound)
        );
        assert_eq!(config, before_missing);

        let mut invalid_config = config.clone();
        invalid_config.groups[0].title.clear();
        let before_invalid = invalid_config.clone();
        assert!(set_group_color(&mut invalid_config, id, [255, 255, 255]).is_err());
        assert_eq!(invalid_config, before_invalid);
    }

    #[test]
    fn invalid_inputs_leave_config_unchanged() {
        let mut config = Config::default();
        create_group(&mut config, "Existing".into(), rect()).unwrap();
        let before = config.clone();

        assert_eq!(
            create_group(&mut config, " \t".into(), rect()),
            Err(AppError::InvalidData(ValidationError::InvalidTitle))
        );
        assert_eq!(config, before);

        let invalid_rect = RectDip {
            width: 239,
            ..rect()
        };
        assert_eq!(
            create_group(&mut config, "Valid".into(), invalid_rect),
            Err(AppError::InvalidData(ValidationError::InvalidLayout))
        );
        assert_eq!(config, before);

        let mut invalid_config = before.clone();
        invalid_config.groups[0].title.clear();
        let invalid_before = invalid_config.clone();
        assert!(create_group(&mut invalid_config, "Valid".into(), rect()).is_err());
        assert_eq!(invalid_config, invalid_before);
    }

    #[test]
    fn group_limit_and_id_exhaustion_leave_config_unchanged() {
        let mut full = Config {
            groups: (1..=MAX_GROUPS as u64).map(group).collect(),
            next_group_id: GroupId::new(MAX_GROUPS as u64 + 1).unwrap(),
            ..Default::default()
        };
        assert_eq!(full.validate(), Ok(()));
        let before_full = full.clone();
        assert_eq!(
            create_group(&mut full, "Overflow".into(), rect()),
            Err(AppError::InvalidData(ValidationError::TooManyGroups))
        );
        assert_eq!(full, before_full);

        let mut exhausted = Config {
            next_group_id: GroupId::new(u64::MAX).unwrap(),
            ..Default::default()
        };
        assert_eq!(exhausted.validate(), Ok(()));
        let before_exhausted = exhausted.clone();
        assert_eq!(
            create_group(&mut exhausted, "No ID".into(), rect()),
            Err(AppError::InvalidData(ValidationError::IdExhausted))
        );
        assert_eq!(exhausted, before_exhausted);
    }

    #[test]
    fn renames_group_and_returns_old_title_including_same_title() {
        let mut config = Config::default();
        let id = create_group(&mut config, "Before".into(), rect()).unwrap();
        let before_metadata = config.clone();

        assert_eq!(
            rename_group(&mut config, id, "After".into()),
            Ok("Before".into())
        );
        assert_eq!(config.groups[0].title, "After");
        assert_eq!(config.groups[0].id, before_metadata.groups[0].id);
        assert_eq!(config.groups[0].rect, before_metadata.groups[0].rect);
        assert_eq!(config.next_group_id, before_metadata.next_group_id);

        assert_eq!(
            rename_group(&mut config, id, "After".into()),
            Ok("After".into())
        );
        assert_eq!(config.groups[0].title, "After");
    }

    #[test]
    fn rename_failures_leave_config_unchanged() {
        let mut config = Config::default();
        let id = create_group(&mut config, "Existing".into(), rect()).unwrap();

        for invalid_title in [
            String::new(),
            " \t".into(),
            "bad\nname".into(),
            "x".repeat(129),
        ] {
            let before = config.clone();
            assert_eq!(
                rename_group(&mut config, id, invalid_title),
                Err(AppError::InvalidData(ValidationError::InvalidTitle))
            );
            assert_eq!(config, before);
        }

        let before_missing = config.clone();
        assert_eq!(
            rename_group(&mut config, GroupId::new(999).unwrap(), "Valid".into()),
            Err(AppError::NotFound)
        );
        assert_eq!(config, before_missing);

        let mut invalid_config = config.clone();
        invalid_config.groups[0].title.clear();
        let before_invalid_config = invalid_config.clone();
        assert!(rename_group(&mut invalid_config, id, "Valid".into()).is_err());
        assert_eq!(invalid_config, before_invalid_config);
    }

    #[test]
    fn moves_group_with_positive_negative_and_boundary_coordinates() {
        let mut config = Config::default();
        let id = create_group(&mut config, "Movable".into(), rect()).unwrap();
        let original = config.groups[0].rect;

        assert_eq!(move_group(&mut config, id, 50, -75), Ok(original));
        assert_eq!(
            config.groups[0].rect,
            RectDip {
                x: 50,
                y: -75,
                ..original
            }
        );
        assert_eq!(
            move_group(&mut config, id, -1_000_000, 1_000_000),
            Ok(RectDip {
                x: 50,
                y: -75,
                ..original
            })
        );
        assert_eq!(
            config.groups[0].rect,
            RectDip {
                x: -1_000_000,
                y: 1_000_000,
                ..original
            }
        );
    }

    #[test]
    fn move_failures_leave_config_unchanged() {
        let mut config = Config::default();
        let id = create_group(&mut config, "Movable".into(), rect()).unwrap();

        for (x, y) in [
            (1_000_001, 0),
            (0, -1_000_001),
            (i32::MIN, 0),
            (0, i32::MAX),
        ] {
            let before = config.clone();
            assert_eq!(
                move_group(&mut config, id, x, y),
                Err(AppError::InvalidData(ValidationError::InvalidLayout))
            );
            assert_eq!(config, before);
        }

        let before_missing = config.clone();
        assert_eq!(
            move_group(&mut config, GroupId::new(999).unwrap(), 0, 0),
            Err(AppError::NotFound)
        );
        assert_eq!(config, before_missing);

        let mut invalid_config = config.clone();
        invalid_config.groups[0].title.clear();
        let before_invalid = invalid_config.clone();
        assert!(move_group(&mut invalid_config, id, 0, 0).is_err());
        assert_eq!(invalid_config, before_invalid);
    }

    #[test]
    fn moving_preserves_everything_except_target_coordinates() {
        let mut config = Config::default();
        let first = create_group(&mut config, "First".into(), rect()).unwrap();
        let second = create_group(&mut config, "Second".into(), rect()).unwrap();
        config.groups[0].color = [1, 2, 3];
        config.groups[0].monitor_key_utf16 = Some(vec![65, 66]);
        config.groups[0].collapsed = true;
        config.groups[0].items.push(crate::core::model::Item {
            id: crate::core::model::ItemId::new(1).unwrap(),
            target: crate::core::model::FileReference {
                kind: crate::core::model::ReferenceKind::File,
                path: crate::core::model::NativePath::from_utf16(
                    "C:\\item.txt".encode_utf16().collect(),
                )
                .unwrap(),
            },
        });
        config.next_item_id = crate::core::model::ItemId::new(2).unwrap();
        let before = config.clone();

        assert_eq!(
            move_group(&mut config, first, 123, -456),
            Ok(before.groups[0].rect)
        );
        let mut expected = before;
        expected.groups[0].rect.x = 123;
        expected.groups[0].rect.y = -456;
        assert_eq!(config, expected);
        assert_eq!(config.groups[1].id, second);
    }

    #[test]
    fn resizes_group_at_minimum_and_maximum_size_boundaries() {
        let mut config = Config::default();
        let id = create_group(&mut config, "Resizable".into(), rect()).unwrap();
        let original = config.groups[0].rect;

        assert_eq!(resize_group(&mut config, id, 240, 120), Ok(original));
        assert_eq!(
            config.groups[0].rect,
            RectDip {
                width: 240,
                height: 120,
                ..original
            }
        );

        let previous = config.groups[0].rect;
        assert_eq!(resize_group(&mut config, id, 16_384, 16_384), Ok(previous));
        assert_eq!(
            config.groups[0].rect,
            RectDip {
                width: 16_384,
                height: 16_384,
                ..original
            }
        );
    }

    #[test]
    fn resize_failures_leave_config_unchanged() {
        let mut config = Config::default();
        let id = create_group(&mut config, "Resizable".into(), rect()).unwrap();

        for (width, height) in [
            (239, 240),
            (240, 119),
            (16_385, 240),
            (240, 16_385),
            (u32::MAX, 120),
            (240, u32::MAX),
        ] {
            let before = config.clone();
            assert_eq!(
                resize_group(&mut config, id, width, height),
                Err(AppError::InvalidData(ValidationError::InvalidLayout))
            );
            assert_eq!(config, before);
        }

        let before_missing = config.clone();
        assert_eq!(
            resize_group(&mut config, GroupId::new(999).unwrap(), 240, 120),
            Err(AppError::NotFound)
        );
        assert_eq!(config, before_missing);

        let mut invalid_config = config.clone();
        invalid_config.groups[0].title.clear();
        let before_invalid = invalid_config.clone();
        assert!(resize_group(&mut invalid_config, id, 240, 120).is_err());
        assert_eq!(invalid_config, before_invalid);
    }

    #[test]
    fn resizing_preserves_everything_except_target_size() {
        let mut config = Config::default();
        let first = create_group(&mut config, "First".into(), rect()).unwrap();
        let second = create_group(&mut config, "Second".into(), rect()).unwrap();
        config.groups[0].color = [1, 2, 3];
        config.groups[0].monitor_key_utf16 = Some(vec![65, 66]);
        config.groups[0].collapsed = true;
        config.groups[0].items.push(crate::core::model::Item {
            id: crate::core::model::ItemId::new(1).unwrap(),
            target: crate::core::model::FileReference {
                kind: crate::core::model::ReferenceKind::File,
                path: crate::core::model::NativePath::from_utf16(
                    "C:\\item.txt".encode_utf16().collect(),
                )
                .unwrap(),
            },
        });
        config.next_item_id = crate::core::model::ItemId::new(2).unwrap();
        let before = config.clone();

        assert_eq!(
            resize_group(&mut config, first, 800, 600),
            Ok(before.groups[0].rect)
        );
        let mut expected = before;
        expected.groups[0].rect.width = 800;
        expected.groups[0].rect.height = 600;
        assert_eq!(config, expected);
        assert_eq!(config.groups[1].id, second);
    }

    fn config_with_reorder_items() -> (Config, GroupId, GroupId, Vec<Item>) {
        let mut config = Config::default();
        let target = create_group(&mut config, "Target".into(), rect()).unwrap();
        let other = create_group(&mut config, "Other".into(), rect()).unwrap();
        let items = vec![
            item(1, "C:\\first.txt"),
            item(2, "C:\\middle.txt"),
            item(3, "C:\\last.txt"),
        ];
        config.groups[0].items = items.clone();
        config.groups[1].items.push(item(4, "C:\\other.txt"));
        config.next_item_id = ItemId::new(5).unwrap();
        (config, target, other, items)
    }

    #[test]
    fn reorder_reference_moves_first_to_last_and_last_to_first() {
        let (mut config, group_id, _, items) = config_with_reorder_items();
        assert_eq!(
            reorder_reference(&mut config, group_id, items[0].id, 2),
            Ok(0)
        );
        assert_eq!(
            config.groups[0].items,
            vec![items[1].clone(), items[2].clone(), items[0].clone()]
        );
        assert_eq!(
            reorder_reference(&mut config, group_id, items[0].id, 0),
            Ok(2)
        );
        assert_eq!(config.groups[0].items, items);
    }

    #[test]
    fn reorder_reference_moves_middle_and_same_index_is_noop() {
        let (mut config, group_id, _, items) = config_with_reorder_items();
        assert_eq!(
            reorder_reference(&mut config, group_id, items[1].id, 0),
            Ok(1)
        );
        assert_eq!(
            config.groups[0].items,
            vec![items[1].clone(), items[0].clone(), items[2].clone()]
        );
        let before = config.clone();
        assert_eq!(
            reorder_reference(&mut config, group_id, items[1].id, 0),
            Ok(0)
        );
        assert_eq!(config, before);
    }

    #[test]
    fn reorder_reference_failures_are_atomic() {
        let (mut config, group_id, other_group, items) = config_with_reorder_items();
        let before = config.clone();
        assert_eq!(
            reorder_reference(&mut config, other_group, items[0].id, 0),
            Err(AppError::NotFound)
        );
        assert_eq!(config, before);
        assert_eq!(
            reorder_reference(&mut config, group_id, ItemId::new(99).unwrap(), 0),
            Err(AppError::NotFound)
        );
        assert_eq!(config, before);
        assert_eq!(
            reorder_reference(&mut config, group_id, items[0].id, items.len()),
            Err(AppError::InvalidData(ValidationError::InvalidId))
        );
        assert_eq!(config, before);
        assert_eq!(
            reorder_reference(&mut config, GroupId::new(99).unwrap(), items[0].id, 0),
            Err(AppError::NotFound)
        );
        assert_eq!(config, before);
    }

    #[test]
    fn reorder_reference_preserves_paths_ids_other_groups_and_allocators() {
        let (mut config, group_id, _, items) = config_with_reorder_items();
        let next_group_id = config.next_group_id;
        let next_item_id = config.next_item_id;
        let other_group = config.groups[1].clone();
        let paths_and_ids: Vec<_> = items
            .iter()
            .map(|item| (item.id, item.target.clone()))
            .collect();
        assert_eq!(
            reorder_reference(&mut config, group_id, items[2].id, 1),
            Ok(2)
        );
        let after: Vec<_> = config.groups[0]
            .items
            .iter()
            .map(|item| (item.id, item.target.clone()))
            .collect();
        let mut sorted_before = paths_and_ids;
        let mut sorted_after = after;
        sorted_before.sort_by_key(|(id, _)| *id);
        sorted_after.sort_by_key(|(id, _)| *id);
        assert_eq!(sorted_after, sorted_before);
        assert_eq!(config.groups[1], other_group);
        assert_eq!(config.next_group_id, next_group_id);
        assert_eq!(config.next_item_id, next_item_id);
    }
}
