#![cfg(feature = "config-store")]

use litefences::core::{
    layout::RectDip,
    model::{
        Config, FileReference, Group, GroupId, Item, ItemId, NativePath, ReferenceKind, Settings,
    },
};
use litefences::storage::codec::{CodecError, decode, encode};

fn sample() -> Config {
    let gid = GroupId::new(1).unwrap();
    let iid = ItemId::new(1).unwrap();
    Config {
        schema_version: 1,
        next_group_id: GroupId::new(2).unwrap(),
        next_item_id: ItemId::new(2).unwrap(),
        settings: Settings {
            launch_at_login: true,
        },
        groups: vec![Group {
            id: gid,
            title: "Work".into(),
            color: [4, 5, 6],
            rect: RectDip {
                x: -20,
                y: 30,
                width: 360,
                height: 240,
            },
            monitor_key_utf16: Some(vec![0xd800, 65]),
            collapsed: false,
            items: vec![Item {
                id: iid,
                target: FileReference {
                    kind: ReferenceKind::File,
                    path: NativePath::from_utf16("C:\\demo.txt".encode_utf16().collect()).unwrap(),
                },
            }],
        }],
    }
}

#[test]
fn roundtrip_preserves_model_and_unpaired_utf16() {
    let mut config = sample();
    let mut units = vec![67, 58, 92];
    units.extend([0xD800, 65, 0xDC00]);
    config.groups[0].items[0].target.path = NativePath::from_utf16(units).unwrap();
    assert_eq!(decode(&encode(&config).unwrap()), Ok(config));
}

#[test]
fn every_required_field_and_unknown_field_is_checked_at_each_level() {
    let value: serde_json::Value = serde_json::from_slice(&encode(&sample()).unwrap()).unwrap();
    for pointer in [
        "",
        "/settings",
        "/groups/0",
        "/groups/0/rect",
        "/groups/0/items/0",
        "/groups/0/items/0/target",
    ] {
        let fields: Vec<_> = value
            .pointer(pointer)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        for field in fields {
            let mut invalid = value.clone();
            invalid
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(&field);
            assert_eq!(
                decode(&serde_json::to_vec(&invalid).unwrap()),
                Err(CodecError::Malformed),
                "{pointer}/{field}"
            );
        }
        let mut invalid = value.clone();
        invalid
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown_field".into(), true.into());
        assert_eq!(
            decode(&serde_json::to_vec(&invalid).unwrap()),
            Err(CodecError::Malformed),
            "{pointer}"
        );
    }
}
#[test]
fn duplicate_fields_are_rejected_at_each_level_even_when_null_or_escaped() {
    let base = String::from_utf8(encode(&sample()).unwrap()).unwrap();
    for (old, new) in [
        (
            "\"next_item_id\":\"2\"",
            "\"next_item_id\":\"2\",\"next_item_id\":\"2\"",
        ),
        (
            "\"launch_at_login\":true",
            "\"launch_at_login\":true,\"launch_at_login\":true",
        ),
        (
            "\"title\":\"Work\"",
            "\"title\":\"Work\",\"title\":\"Work\"",
        ),
        ("\"x\":-20", "\"x\":-20,\"x\":-20"),
        ("\"id\":\"1\"", "\"id\":\"1\",\"id\":\"1\""),
        ("\"kind\":\"file\"", "\"kind\":\"file\",\"kind\":\"file\""),
        (
            "\"monitor_key_utf16\":[55296,65]",
            "\"monitor_key_utf16\":null,\"monitor_key_utf16\":null",
        ),
        (
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_vers\\u0069on\":1",
        ),
    ] {
        assert!(base.contains(old));
        assert_eq!(
            decode(base.replace(old, new).as_bytes()),
            Err(CodecError::Malformed)
        );
    }
}
#[test]
fn nested_objects_cannot_be_replaced_with_positional_arrays() {
    let base: serde_json::Value = serde_json::from_slice(&encode(&sample()).unwrap()).unwrap();
    for (pointer, fields) in [
        ("/settings", vec!["launch_at_login"]),
        (
            "/groups/0",
            vec![
                "id",
                "title",
                "color",
                "rect",
                "monitor_key_utf16",
                "collapsed",
                "items",
            ],
        ),
        ("/groups/0/rect", vec!["x", "y", "width", "height"]),
        ("/groups/0/items/0", vec!["id", "target"]),
        ("/groups/0/items/0/target", vec!["kind", "path_utf16"]),
    ] {
        let mut bad = base.clone();
        let object = bad.pointer_mut(pointer).unwrap();
        let array = fields.iter().map(|field| object[*field].clone()).collect();
        *object = serde_json::Value::Array(array);
        assert_eq!(
            decode(&serde_json::to_vec(&bad).unwrap()),
            Err(CodecError::Malformed),
            "{pointer}"
        );
    }
}
#[test]
fn integer_ranges_utf16_and_reference_kind_are_strict() {
    let base = String::from_utf8(encode(&sample()).unwrap()).unwrap();
    for (old, new) in [
        ("\"width\":360", "\"width\":360.0"),
        ("\"width\":360", "\"width\":-1"),
        ("\"x\":-20", "\"x\":2147483648"),
        ("\"kind\":\"file\"", "\"kind\":\"virtual\""),
        ("[55296,65]", "[65536]"),
        ("[55296,65]", "[65.0]"),
        ("\"next_item_id\":\"2\"", "\"next_item_id\":2"),
    ] {
        assert_eq!(
            decode(base.replace(old, new).as_bytes()),
            Err(CodecError::Malformed)
        );
    }
    let bad = base.replace("\"color\":[4,5,6]", "\"color\":[256,5,6]");
    assert_eq!(decode(bad.as_bytes()), Err(CodecError::Malformed));
}
#[test]
fn decoder_revalidates_model_invariants_and_reports_no_input_values() {
    let base = String::from_utf8(encode(&sample()).unwrap()).unwrap();
    for bad in [
        base.replace("\"width\":360", "\"width\":1"),
        base.replace("\"title\":\"Work\"", "\"title\":\"\""),
        base.replace("\"next_item_id\":\"2\"", "\"next_item_id\":\"1\""),
    ] {
        assert!(matches!(
            decode(bad.as_bytes()),
            Err(CodecError::InvalidModel(_))
        ));
    }
    let bad = base.replace("\"kind\":\"file\"", "\"kind\":\"PRIVATE_PATH_SECRET\"");
    assert!(!format!("{:?}", decode(bad.as_bytes()).unwrap_err()).contains("PRIVATE_PATH_SECRET"));
}

#[test]
fn ids_preserve_large_decimal_values() {
    let mut config = sample();
    config.groups[0].id = GroupId::new(u64::MAX - 1).unwrap();
    config.next_group_id = GroupId::new(u64::MAX).unwrap();
    config.groups[0].items[0].id = ItemId::new(u64::MAX - 1).unwrap();
    config.next_item_id = ItemId::new(u64::MAX).unwrap();
    assert_eq!(decode(&encode(&config).unwrap()), Ok(config));
}

#[test]
fn future_version_wins_over_unknown_fields() {
    assert_eq!(
        decode(br#"{"schema_version":2,"new":true}"#),
        Err(CodecError::UnsupportedVersion)
    );
}

#[test]
fn rejects_trailing_garbage_and_non_integer_values() {
    let valid = String::from_utf8(encode(&Config::default()).unwrap()).unwrap();
    assert_eq!(
        decode(format!("{valid} trailing").as_bytes()),
        Err(CodecError::Malformed)
    );
    assert_eq!(
        decode(
            valid
                .replace("\"schema_version\":1", "\"schema_version\":1.0")
                .as_bytes()
        ),
        Err(CodecError::Malformed)
    );
}

#[test]
fn enforces_input_and_output_limits() {
    assert_eq!(
        decode(&vec![b' '; 8 * 1024 * 1024 + 1]),
        Err(CodecError::TooLarge)
    );
    let config = Config::default();
    assert!(encode(&config).unwrap().len() <= 8 * 1024 * 1024);
}

#[test]
fn rejects_unknown_duplicate_and_missing_fields_at_each_object_level() {
    let base = String::from_utf8(encode(&sample()).unwrap()).unwrap();
    for bad in [
        base.replace("\"settings\":", "\"extra\":0,\"settings\":"),
        base.replacen(
            "\"schema_version\":1,",
            "\"schema_version\":1,\"schema_version\":1,",
            1,
        ),
        base.replace("\"monitor_key_utf16\":[55296,65],", ""),
        base.replace(
            "\"launch_at_login\":true",
            "\"launch_at_login\":true,\"x\":0",
        ),
    ] {
        assert_eq!(decode(bad.as_bytes()), Err(CodecError::Malformed));
    }
}

#[test]
fn rejects_invalid_model_and_bad_id_strings() {
    let mut bad = String::from_utf8(encode(&sample()).unwrap())
        .unwrap()
        .replace("\"id\":\"1\"", "\"id\":\"01\"");
    assert!(matches!(
        decode(bad.as_bytes()),
        Err(CodecError::InvalidModel(_))
    ));
    bad = String::from_utf8(encode(&sample()).unwrap())
        .unwrap()
        .replace("\"width\":360", "\"width\":1.5");
    assert_eq!(decode(bad.as_bytes()), Err(CodecError::Malformed));
}
