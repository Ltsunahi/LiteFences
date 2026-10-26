//! Strict, bounded JSON v1 codec for configuration values.
#![forbid(unsafe_code)]
use crate::core::model::{
    Config, FileReference, Group, GroupId, Item, ItemId, NativePath, ReferenceKind, Settings,
};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{fmt, io::Write, marker::PhantomData};

use crate::core::model::ValidationError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    TooLarge,
    Malformed,
    UnsupportedVersion,
    InvalidModel(ValidationError),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => f.write_str("configuration exceeds size limit"),
            Self::Malformed => f.write_str("malformed configuration"),
            Self::UnsupportedVersion => f.write_str("unsupported configuration version"),
            Self::InvalidModel(error) => write!(f, "invalid configuration: {error}"),
        }
    }
}
impl std::error::Error for CodecError {}

fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

// serde's derived structs also accept JSON arrays. These wrappers deliberately
// request a map at every object boundary to enforce the documented JSON shape.
fn object<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    struct ObjectVisitor<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> de::Visitor<'de> for ObjectVisitor<T> {
        type Value = T;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an object")
        }
        fn visit_map<M: de::MapAccess<'de>>(self, map: M) -> Result<T, M::Error> {
            T::deserialize(de::value::MapAccessDeserializer::new(map))
        }
    }
    d.deserialize_map(ObjectVisitor(PhantomData))
}
struct Object<T>(T);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        object(d).map(Self)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigDto {
    schema_version: u32,
    next_group_id: String,
    next_item_id: String,
    #[serde(deserialize_with = "object")]
    settings: SettingsDto,
    groups: Vec<Object<GroupDto>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsDto {
    launch_at_login: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupDto {
    id: String,
    title: String,
    color: [u8; 3],
    #[serde(deserialize_with = "object")]
    rect: RectDto,
    #[serde(deserialize_with = "required_option")]
    monitor_key_utf16: Option<Vec<u16>>,
    collapsed: bool,
    items: Vec<Object<ItemDto>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RectDto {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemDto {
    id: String,
    #[serde(deserialize_with = "object")]
    target: TargetDto,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetDto {
    kind: String,
    path_utf16: Vec<u16>,
}

struct SchemaProbe(Option<u64>);
impl<'de> Deserialize<'de> for SchemaProbe {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ProbeVisitor;
        impl<'de> de::Visitor<'de> for ProbeVisitor {
            type Value = SchemaProbe;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a configuration object")
            }
            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut version = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "schema_version" {
                        if version.is_some() {
                            return Err(de::Error::duplicate_field("schema_version"));
                        }
                        version = Some(map.next_value::<u64>()?);
                    } else {
                        map.next_value::<de::IgnoredAny>()?;
                    }
                }
                Ok(SchemaProbe(version))
            }
        }
        deserializer.deserialize_map(ProbeVisitor)
    }
}

fn malformed<T>(_: T) -> CodecError {
    CodecError::Malformed
}

#[derive(Serialize)]
struct ConfigRef<'a> {
    schema_version: u32,
    next_group_id: String,
    next_item_id: String,
    settings: SettingsDto,
    groups: Vec<GroupRef<'a>>,
}
#[derive(Serialize)]
struct GroupRef<'a> {
    id: String,
    title: &'a str,
    color: [u8; 3],
    rect: RectDto,
    monitor_key_utf16: Option<&'a [u16]>,
    collapsed: bool,
    items: Vec<ItemRef<'a>>,
}
#[derive(Serialize)]
struct ItemRef<'a> {
    id: String,
    target: TargetRef<'a>,
}
#[derive(Serialize)]
struct TargetRef<'a> {
    kind: &'static str,
    path_utf16: &'a [u16],
}

#[derive(Default)]
struct BoundedWriter {
    bytes: Vec<u8>,
    exceeded: bool,
}
impl Write for BoundedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > crate::core::model::MAX_CONFIG_BYTES - self.bytes.len() {
            self.exceeded = true;
            return Err(std::io::Error::other("configuration size limit"));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn encode(config: &Config) -> Result<Vec<u8>, CodecError> {
    config.validate().map_err(CodecError::InvalidModel)?;
    let dto = ConfigRef {
        schema_version: config.schema_version,
        next_group_id: config.next_group_id.to_string(),
        next_item_id: config.next_item_id.to_string(),
        settings: SettingsDto {
            launch_at_login: config.settings.launch_at_login,
        },
        groups: config
            .groups
            .iter()
            .map(|g| GroupRef {
                id: g.id.to_string(),
                title: &g.title,
                color: g.color,
                rect: RectDto {
                    x: g.rect.x,
                    y: g.rect.y,
                    width: g.rect.width,
                    height: g.rect.height,
                },
                monitor_key_utf16: g.monitor_key_utf16.as_deref(),
                collapsed: g.collapsed,
                items: g
                    .items
                    .iter()
                    .map(|i| ItemRef {
                        id: i.id.to_string(),
                        target: TargetRef {
                            kind: match i.target.kind {
                                ReferenceKind::File => "file",
                                ReferenceKind::Directory => "directory",
                                ReferenceKind::Shortcut => "shortcut",
                            },
                            path_utf16: i.target.path.as_utf16(),
                        },
                    })
                    .collect(),
            })
            .collect(),
    };
    let mut writer = BoundedWriter::default();
    if serde_json::to_writer(&mut writer, &dto).is_err() {
        return Err(if writer.exceeded {
            CodecError::TooLarge
        } else {
            CodecError::Malformed
        });
    }
    Ok(writer.bytes)
}

pub fn decode(bytes: &[u8]) -> Result<Config, CodecError> {
    if bytes.len() > crate::core::model::MAX_CONFIG_BYTES {
        return Err(CodecError::TooLarge);
    }
    let probe: SchemaProbe = serde_json::from_slice(bytes).map_err(malformed)?;
    let version = probe.0.ok_or(CodecError::Malformed)?;
    if version != 1 {
        return Err(CodecError::UnsupportedVersion);
    }
    let dto: ConfigDto = serde_json::from_slice(bytes).map_err(malformed)?;
    let next_group_id =
        GroupId::from_decimal(&dto.next_group_id).map_err(CodecError::InvalidModel)?;
    let next_item_id = ItemId::from_decimal(&dto.next_item_id).map_err(CodecError::InvalidModel)?;
    let groups = dto
        .groups
        .into_iter()
        .map(|Object(g)| {
            let id = GroupId::from_decimal(&g.id).map_err(CodecError::InvalidModel)?;
            let items = g
                .items
                .into_iter()
                .map(|Object(i)| {
                    let id = ItemId::from_decimal(&i.id).map_err(CodecError::InvalidModel)?;
                    let kind = match i.target.kind.as_str() {
                        "file" => ReferenceKind::File,
                        "directory" => ReferenceKind::Directory,
                        "shortcut" => ReferenceKind::Shortcut,
                        _ => return Err(CodecError::Malformed),
                    };
                    let path = NativePath::from_utf16(i.target.path_utf16)
                        .map_err(CodecError::InvalidModel)?;
                    Ok(Item {
                        id,
                        target: FileReference { kind, path },
                    })
                })
                .collect::<Result<Vec<_>, CodecError>>()?;
            Ok(Group {
                id,
                title: g.title,
                color: g.color,
                rect: crate::core::layout::RectDip {
                    x: g.rect.x,
                    y: g.rect.y,
                    width: g.rect.width,
                    height: g.rect.height,
                },
                monitor_key_utf16: g.monitor_key_utf16,
                collapsed: g.collapsed,
                items,
            })
        })
        .collect::<Result<Vec<_>, CodecError>>()?;
    let config = Config {
        schema_version: dto.schema_version,
        next_group_id,
        next_item_id,
        settings: Settings {
            launch_at_login: dto.settings.launch_at_login,
        },
        groups,
    };
    config.validate().map_err(CodecError::InvalidModel)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_limit_stops_before_extending_the_buffer() {
        let mut writer = BoundedWriter::default();
        writer
            .write_all(&vec![0; crate::core::model::MAX_CONFIG_BYTES])
            .unwrap();
        assert!(writer.write_all(&[1]).is_err());
        assert!(writer.exceeded);
        assert_eq!(writer.bytes.len(), crate::core::model::MAX_CONFIG_BYTES);
    }
}
