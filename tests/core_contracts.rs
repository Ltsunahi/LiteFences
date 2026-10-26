use litefences::core::{contracts::*, layout::RectDip, model::*};
use std::num::NonZeroU64;

fn path(value: &str) -> NativePath {
    NativePath::from_utf16(value.encode_utf16().collect()).unwrap()
}
fn group(config: &mut Config, title: &str) -> Group {
    Group {
        id: config.allocate_group_id().unwrap(),
        title: title.into(),
        color: [60, 110, 180],
        rect: RectDip {
            x: -100,
            y: 40,
            width: 360,
            height: 280,
        },
        monitor_key_utf16: None,
        collapsed: false,
        items: Vec::new(),
    }
}
fn item(config: &mut Config, name: &str) -> Item {
    Item {
        id: config.allocate_item_id().unwrap(),
        target: FileReference {
            kind: ReferenceKind::File,
            path: path(name),
        },
    }
}
fn sample() -> Config {
    let mut config = Config::default();
    let mut first = group(&mut config, "工作");
    first.items.push(item(&mut config, r"C:\demo.txt"));
    config.groups.push(first);
    config
}

#[test]
fn empty_config_is_valid_and_login_launch_is_opt_in() {
    let config = Config::default();
    assert_eq!(config.validate(), Ok(()));
    assert!(!config.settings.launch_at_login);
    assert_eq!(config.next_group_id.get(), 1);
    assert_eq!(config.next_item_id.get(), 1);
}
#[test]
fn stable_ids_roundtrip_beyond_json_float_precision() {
    for value in [1, 9_007_199_254_740_993, u64::MAX] {
        let id = GroupId::new(value).unwrap();
        assert_eq!(GroupId::from_decimal(&id.to_string()), Ok(id));
        let item = ItemId::new(value).unwrap();
        assert_eq!(ItemId::from_decimal(&item.to_string()), Ok(item));
    }
}
#[test]
fn noncanonical_or_overflow_ids_are_rejected() {
    for invalid in [
        "",
        "0",
        "01",
        "+1",
        "-1",
        " 1",
        "1 ",
        "1.0",
        "1e3",
        "１２",
        "18446744073709551616",
    ] {
        assert_eq!(
            GroupId::from_decimal(invalid),
            Err(ValidationError::InvalidId)
        );
        assert_eq!(
            ItemId::from_decimal(invalid),
            Err(ValidationError::InvalidId)
        );
    }
    assert!(GroupId::new(0).is_err());
    assert!(ItemId::new(0).is_err());
}
#[test]
fn exhausted_allocators_do_not_wrap_or_mutate() {
    let mut config = Config {
        next_group_id: GroupId::new(u64::MAX).unwrap(),
        next_item_id: ItemId::new(u64::MAX).unwrap(),
        ..Config::default()
    };
    let previous = config.clone();
    assert_eq!(
        config.allocate_group_id(),
        Err(ValidationError::IdExhausted)
    );
    assert_eq!(config.allocate_item_id(), Err(ValidationError::IdExhausted));
    assert_eq!(config, previous);
}
#[test]
fn deletion_does_not_reuse_ids() {
    let mut config = sample();
    let deleted_group = config.groups[0].id;
    let deleted_item = config.groups[0].items[0].id;
    config.groups.clear();
    assert!(config.allocate_group_id().unwrap() > deleted_group);
    assert!(config.allocate_item_id().unwrap() > deleted_item);
}
#[test]
fn versions_and_stale_next_ids_fail_without_mutation() {
    let mut config = sample();
    config.schema_version = 2;
    let before = config.clone();
    assert_eq!(config.validate(), Err(ValidationError::UnsupportedVersion));
    assert_eq!(config, before);
    config.schema_version = 1;
    config.next_group_id = config.groups[0].id;
    assert_eq!(config.validate(), Err(ValidationError::InvalidNextId));
    config.next_group_id = GroupId::new(2).unwrap();
    config.next_item_id = config.groups[0].items[0].id;
    assert_eq!(config.validate(), Err(ValidationError::InvalidNextId));
}
#[test]
fn duplicate_group_and_item_ids_are_rejected() {
    let mut config = sample();
    config.groups.push(config.groups[0].clone());
    assert_eq!(config.validate(), Err(ValidationError::DuplicateGroupId));
    config.groups[1].id = config.allocate_group_id().unwrap();
    assert_eq!(config.validate(), Err(ValidationError::DuplicateItemId));
}
#[test]
fn same_group_exact_target_duplicates_fail_regardless_of_kind() {
    let mut config = sample();
    let mut duplicate = config.groups[0].items[0].clone();
    duplicate.id = config.allocate_item_id().unwrap();
    duplicate.target.kind = ReferenceKind::Shortcut;
    config.groups[0].items.push(duplicate);
    assert_eq!(config.validate(), Err(ValidationError::DuplicateTarget));
}
#[test]
fn cross_group_same_target_is_allowed_and_case_is_not_folded() {
    let mut config = sample();
    let mut second = group(&mut config, "归档");
    second.items.push(item(&mut config, r"C:\demo.txt"));
    second.items.push(item(&mut config, r"C:\DEMO.txt"));
    config.groups.push(second);
    assert_eq!(config.validate(), Ok(()));
}
#[test]
fn titles_layouts_and_monitor_keys_are_validated() {
    for title in ["".into(), " \t".into(), "x\ny".into(), "好".repeat(129)] {
        let mut config = sample();
        config.groups[0].title = title;
        assert_eq!(config.validate(), Err(ValidationError::InvalidTitle));
    }
    let mut config = sample();
    config.groups[0].title = "好".repeat(128);
    assert_eq!(config.validate(), Ok(()));
    config.groups[0].rect.width = 0;
    assert_eq!(config.validate(), Err(ValidationError::InvalidLayout));
    config.groups[0].rect.width = 240;
    for key in [vec![], vec![0], vec![65; 1025]] {
        config.groups[0].monitor_key_utf16 = Some(key);
        assert_eq!(config.validate(), Err(ValidationError::InvalidMonitorKey));
    }
    config.groups[0].monitor_key_utf16 = Some(vec![65; 1024]);
    assert_eq!(config.validate(), Ok(()));
}
#[test]
fn local_paths_preserve_utf16_and_redact_debug() {
    let mut units: Vec<_> = r"C:\私人资料\".encode_utf16().collect();
    units.extend([0xD800, 0x0061, 0xDC00]);
    let path = NativePath::from_utf16(units.clone()).unwrap();
    assert_eq!(path.as_utf16(), units);
    assert!(!format!("{path:?}").contains("私人资料"));
    #[cfg(windows)]
    {
        assert_eq!(NativePath::from_os_str(&path.to_os_string()).unwrap(), path);
    }
}
#[test]
fn unsupported_ambiguous_and_device_paths_are_rejected() {
    for value in [
        "",
        "C:",
        "C:relative",
        r"\relative",
        r"\\server\share",
        r"\\?\C:\file",
        r"\\.\PhysicalDrive0",
        "https://example.org",
        "C:/file",
        r"C:\a\..\file",
        r"C:\a\.\file",
        r"C:\a\\file",
        "C:\\file\0",
        r"C:\file:stream",
        r"C:\*.txt",
        r"C:\?.txt",
        r"C:\a|b",
        r"C:\a<b",
        r"C:\a>b",
        "C:\\a\"b",
        r"C:\a ",
        r"C:\a.",
        r"C:\dir\",
        r"C:\CON",
        r"C:\Nul.txt",
        r"C:\con .txt",
        r"C:\aux",
        r"C:\PRN\file",
        r"C:\COM1",
        r"C:\LPT9.txt",
        r"C:\com¹",
        r"C:\lpt².txt",
        r"C:\LPT³",
    ] {
        assert_eq!(
            NativePath::from_utf16(value.encode_utf16().collect()),
            Err(ValidationError::InvalidPath),
            "{value}"
        );
    }
    for value in [
        r"C:\",
        r"d:\.gitignore",
        r"E:\中文\😀.txt",
        r"C:\COM10",
        r"C:\LPT0",
        r"C:\conman",
    ] {
        assert!(NativePath::from_utf16(value.encode_utf16().collect()).is_ok());
    }
}
#[test]
fn path_length_limit_is_in_utf16_units() {
    let mut units = vec![65; MAX_PATH_UNITS];
    units[..3].copy_from_slice(&[67, 58, 92]);
    assert!(NativePath::from_utf16(units.clone()).is_ok());
    units.push(65);
    assert_eq!(
        NativePath::from_utf16(units),
        Err(ValidationError::InvalidPath)
    );
}
#[test]
fn group_count_is_bounded() {
    let mut config = Config::default();
    for _ in 0..MAX_GROUPS {
        let group = group(&mut config, "组");
        config.groups.push(group);
    }
    assert_eq!(config.validate(), Ok(()));
    let extra = group(&mut config, "超限");
    config.groups.push(extra);
    assert_eq!(config.validate(), Err(ValidationError::TooManyGroups));
}
#[test]
fn item_limit_and_thousand_item_fixture_validate() {
    let mut config = Config::default();
    let mut group = group(&mut config, "容量测试");
    for index in 0..MAX_ITEMS {
        group
            .items
            .push(item(&mut config, &format!(r"C:\fixture\{index}.txt")));
        if index == 999 {
            config.groups.push(group.clone());
            assert_eq!(config.validate(), Ok(()));
            config.groups.clear();
        }
    }
    config.groups.push(group);
    assert_eq!(config.validate(), Ok(()));
    let extra = item(&mut config, r"C:\extra.txt");
    config.groups[0].items.push(extra);
    assert_eq!(config.validate(), Err(ValidationError::TooManyItems));
}
#[test]
fn aggregate_path_budget_is_bounded() {
    let mut config = Config::default();
    let mut group = group(&mut config, "路径预算");
    for index in 0..31 {
        let mut units = vec![65; MAX_PATH_UNITS];
        units[..4].copy_from_slice(&[67, 58, 92, 0x100 + index]);
        group.items.push(Item {
            id: config.allocate_item_id().unwrap(),
            target: FileReference {
                kind: ReferenceKind::File,
                path: NativePath::from_utf16(units).unwrap(),
            },
        });
    }
    config.groups.push(group);
    assert_eq!(config.validate(), Err(ValidationError::PathBudgetExceeded));
    config.groups[0].items.pop();
    assert_eq!(config.validate(), Ok(()));
}
#[test]
fn stale_cancelled_and_removed_item_results_are_ignored() {
    let mut config = sample();
    let token = RequestToken {
        request_id: NonZeroU64::new(1).unwrap(),
        generation: NonZeroU64::new(1).unwrap(),
        item_id: config.groups[0].items[0].id,
    };
    assert!(token.is_current(Some(token), &config));
    assert!(!token.is_current(None, &config));
    assert!(!token.is_current(
        Some(RequestToken {
            generation: NonZeroU64::new(2).unwrap(),
            ..token
        }),
        &config
    ));
    assert!(!token.is_current(
        Some(RequestToken {
            request_id: NonZeroU64::new(2).unwrap(),
            ..token
        }),
        &config
    ));
    config.groups[0].items.clear();
    assert!(!token.is_current(Some(token), &config));
}
#[test]
fn icons_are_bounded_and_clones_share_payload() {
    assert_eq!(IconPixels::new(0, 1, vec![]), Err(AppError::InvalidIcon));
    assert_eq!(IconPixels::new(257, 1, vec![]), Err(AppError::InvalidIcon));
    assert_eq!(
        IconPixels::new(1, 1, vec![0; 3]),
        Err(AppError::InvalidIcon)
    );
    let icon = IconPixels::new(256, 256, vec![255; 256 * 256 * 4]).unwrap();
    let clone = icon.clone();
    assert_eq!((icon.width(), icon.height()), (256, 256));
    assert!(std::ptr::eq(icon.bgra().as_ptr(), clone.bgra().as_ptr()));
}
#[test]
fn owned_messages_are_send_without_com_or_window_handles() {
    fn assert_send<T: Send>() {}
    assert_send::<AppCommand>();
    assert_send::<AppEvent>();
    assert_send::<Config>();
    assert_send::<LoadedConfig>();
}
