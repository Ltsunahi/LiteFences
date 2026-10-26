use litefences::core::{
    contracts::AppError,
    group_ops::add_references,
    layout::RectDip,
    model::{
        Config, FileReference, Group, GroupId, Item, ItemId, MAX_ITEMS, MAX_PATH_UNITS,
        MAX_TOTAL_PATH_UNITS, NativePath, ReferenceKind, ValidationError,
    },
};

fn path(value: &str) -> NativePath {
    NativePath::from_utf16(value.encode_utf16().collect()).unwrap()
}

fn reference(value: &str) -> FileReference {
    FileReference {
        kind: ReferenceKind::File,
        path: path(value),
    }
}

fn group(config: &mut Config, title: &str) -> GroupId {
    let id = config.allocate_group_id().unwrap();
    config.groups.push(Group {
        id,
        title: title.into(),
        color: [62, 110, 180],
        rect: RectDip {
            x: 0,
            y: 0,
            width: 320,
            height: 240,
        },
        monitor_key_utf16: None,
        collapsed: false,
        items: Vec::new(),
    });
    id
}

fn add_existing(config: &mut Config, group: GroupId, target: FileReference) {
    let id = config.allocate_item_id().unwrap();
    config
        .groups
        .iter_mut()
        .find(|candidate| candidate.id == group)
        .unwrap()
        .items
        .push(Item { id, target });
}

fn long_path(units: usize, marker: u16) -> NativePath {
    assert!((4..=MAX_PATH_UNITS).contains(&units));
    let mut value = vec![65; units];
    value[..3].copy_from_slice(&[b'C' as u16, b':' as u16, b'\\' as u16]);
    value[3] = marker;
    NativePath::from_utf16(value).unwrap()
}

#[test]
fn adds_two_local_references_with_monotonically_allocated_ids() {
    let mut config = Config::default();
    let target = group(&mut config, "目标");
    let first = reference(r"C:\资料\one.txt");
    let second = reference(r"D:\archive\two.txt");

    let ids = add_references(&mut config, target, vec![first.clone(), second.clone()]).unwrap();

    assert_eq!(
        ids.iter().map(|id| id.get()).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(config.next_item_id.get(), 3);
    assert_eq!(config.groups[0].items.len(), 2);
    assert_eq!(
        config.groups[0].items[0],
        Item {
            id: ids[0],
            target: first
        }
    );
    assert_eq!(
        config.groups[0].items[1],
        Item {
            id: ids[1],
            target: second
        }
    );
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn rejects_a_path_already_in_the_group_without_mutating_config() {
    let mut config = Config::default();
    let target = group(&mut config, "目标");
    let duplicate = reference(r"C:\existing.txt");
    add_existing(&mut config, target, duplicate.clone());
    let before = config.clone();

    let result = add_references(
        &mut config,
        target,
        vec![reference(r"D:\new.txt"), duplicate],
    );

    assert!(matches!(
        result,
        Err(AppError::InvalidData(ValidationError::DuplicateTarget))
    ));
    assert_eq!(config, before);
}

#[test]
fn rejects_duplicate_paths_within_batch_without_mutating_config() {
    let mut config = Config::default();
    let target = group(&mut config, "目标");
    let before = config.clone();

    let result = add_references(
        &mut config,
        target,
        vec![reference(r"C:\same.txt"), reference(r"C:\same.txt")],
    );

    assert!(matches!(
        result,
        Err(AppError::InvalidData(ValidationError::DuplicateTarget))
    ));
    assert_eq!(config, before);
}

#[test]
fn allows_the_same_path_in_different_groups() {
    let mut config = Config::default();
    let first = group(&mut config, "第一组");
    let second = group(&mut config, "第二组");
    let same = reference(r"C:\shared.txt");
    add_existing(&mut config, first, same.clone());

    let ids = add_references(&mut config, second, vec![same.clone()]).unwrap();

    assert_eq!(ids.len(), 1);
    assert_eq!(config.groups[0].items[0].target, same);
    assert_eq!(
        config.groups[1].items,
        vec![Item {
            id: ids[0],
            target: reference(r"C:\shared.txt")
        }]
    );
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn missing_group_leaves_config_unchanged() {
    let mut config = Config::default();
    group(&mut config, "Одна");
    let missing = GroupId::new(config.next_group_id.get() + 10).unwrap();
    let before = config.clone();

    assert_eq!(
        add_references(&mut config, missing, vec![reference(r"C:\new.txt")]),
        Err(AppError::NotFound)
    );
    assert_eq!(config, before);
}

#[test]
fn empty_batch_returns_no_ids_and_leaves_config_unchanged() {
    let mut config = Config::default();
    let target = group(&mut config, "目标");
    let before = config.clone();

    assert_eq!(
        add_references(&mut config, target, Vec::new()),
        Ok(Vec::new())
    );
    assert_eq!(config, before);
}

#[test]
fn rejects_batch_that_would_exceed_item_limit_without_mutating_config() {
    let mut config = Config::default();
    let target = group(&mut config, "容量");
    for index in 0..MAX_ITEMS {
        add_existing(
            &mut config,
            target,
            reference(&format!(r"C:\items\{index}.txt")),
        );
    }
    let before = config.clone();

    let result = add_references(&mut config, target, vec![reference(r"C:\overflow.txt")]);

    assert!(matches!(
        result,
        Err(AppError::InvalidData(ValidationError::TooManyItems))
    ));
    assert_eq!(config, before);
}

#[test]
fn rejects_batch_that_would_exceed_aggregate_path_budget_without_mutating_config() {
    let mut config = Config::default();
    let target = group(&mut config, "路径预算");
    let max_path = long_path(MAX_PATH_UNITS, 0x200);
    let existing_count = MAX_TOTAL_PATH_UNITS / MAX_PATH_UNITS;
    for index in 0..existing_count {
        add_existing(
            &mut config,
            target,
            FileReference {
                kind: ReferenceKind::File,
                path: long_path(MAX_PATH_UNITS, 0x100 + index as u16),
            },
        );
    }
    assert!(MAX_TOTAL_PATH_UNITS - existing_count * MAX_PATH_UNITS < MAX_PATH_UNITS);
    let before = config.clone();

    let result = add_references(
        &mut config,
        target,
        vec![FileReference {
            kind: ReferenceKind::File,
            path: max_path,
        }],
    );

    assert!(
        matches!(
            result,
            Err(AppError::InvalidData(ValidationError::PathBudgetExceeded))
        ),
        "actual result: {result:?}"
    );
    assert_eq!(config, before);
}

#[test]
fn rejects_batch_that_would_overflow_item_ids_without_mutating_config() {
    let mut config = Config::default();
    let target = group(&mut config, "ID 溢出");
    config.next_item_id = ItemId::new(u64::MAX).unwrap();
    let before = config.clone();

    let result = add_references(&mut config, target, vec![reference(r"C:\last.txt")]);

    assert!(matches!(
        result,
        Err(AppError::InvalidData(ValidationError::IdExhausted))
    ));
    assert_eq!(config, before);
}
