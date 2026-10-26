//! Versioned, bounded configuration values. Validation never touches the filesystem.
use super::layout::RectDip;
use std::{collections::BTreeSet, fmt, num::NonZeroU64};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_CONFIG_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_GROUPS: usize = 128;
pub const MAX_ITEMS: usize = 10_000;
pub const MAX_PATH_UNITS: usize = 32_767;
pub const MAX_TOTAL_PATH_UNITS: usize = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationError {
    InvalidId,
    IdExhausted,
    UnsupportedVersion,
    TooManyGroups,
    TooManyItems,
    DuplicateGroupId,
    DuplicateItemId,
    DuplicateTarget,
    InvalidNextId,
    InvalidTitle,
    InvalidLayout,
    InvalidMonitorKey,
    InvalidPath,
    PathBudgetExceeded,
}
impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ValidationError {}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
        pub struct $name(NonZeroU64);
        impl $name {
            pub fn new(value: u64) -> Result<Self, ValidationError> {
                NonZeroU64::new(value)
                    .map(Self)
                    .ok_or(ValidationError::InvalidId)
            }
            pub fn get(self) -> u64 {
                self.0.get()
            }
            pub fn from_decimal(value: &str) -> Result<Self, ValidationError> {
                if value.is_empty()
                    || value.len() > 20
                    || value.starts_with('0')
                    || !value.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(ValidationError::InvalidId);
                }
                Self::new(value.parse().map_err(|_| ValidationError::InvalidId)?)
            }
            fn next(self) -> Result<Self, ValidationError> {
                Self::new(
                    self.get()
                        .checked_add(1)
                        .ok_or(ValidationError::IdExhausted)?,
                )
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.get().fmt(f)
            }
        }
    };
}
id_type!(GroupId);
id_type!(ItemId);

/// Opaque lossless local path. Debug deliberately excludes user path content.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct NativePath(Vec<u16>);
impl fmt::Debug for NativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativePath")
            .field("units", &self.0.len())
            .finish_non_exhaustive()
    }
}
impl NativePath {
    pub fn from_utf16(units: Vec<u16>) -> Result<Self, ValidationError> {
        let invalid = || ValidationError::InvalidPath;
        if units.len() < 3
            || units.len() > MAX_PATH_UNITS
            || !matches!(units[0], 65..=90 | 97..=122)
            || units[1] != 58
            || units[2] != 92
        {
            return Err(invalid());
        }
        if units[3..]
            .iter()
            .any(|u| *u < 32 || matches!(*u, 34 | 42 | 47 | 58 | 60 | 62 | 63 | 124))
        {
            return Err(invalid());
        }
        // A drive root is valid; all other components must be nonempty and normalized.
        if units.len() > 3 {
            for segment in units[3..].split(|u| *u == 92) {
                if segment.is_empty()
                    || matches!(segment.last(), Some(32 | 46))
                    || reserved_device(segment)
                {
                    return Err(invalid());
                }
            }
        }
        Ok(Self(units))
    }
    pub fn as_utf16(&self) -> &[u16] {
        &self.0
    }

    #[cfg(windows)]
    pub fn from_os_str(path: &std::ffi::OsStr) -> Result<Self, ValidationError> {
        use std::os::windows::ffi::OsStrExt;
        Self::from_utf16(path.encode_wide().collect())
    }
    #[cfg(windows)]
    pub fn to_os_string(&self) -> std::ffi::OsString {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&self.0)
    }
}

fn reserved_device(segment: &[u16]) -> bool {
    let base = segment.split(|u| *u == 46).next().unwrap_or_default();
    let mut end = base.len();
    while end > 0 && base[end - 1] == 32 {
        end -= 1;
    }
    let base = &base[..end];
    let equals = |name: &[u8]| {
        base.len() == name.len()
            && base.iter().zip(name).all(|(a, b)| {
                let upper = if (97..=122).contains(a) { *a - 32 } else { *a };
                upper == u16::from(*b)
            })
    };
    if [b"CON".as_slice(), b"PRN", b"AUX", b"NUL"]
        .iter()
        .any(|s| equals(s))
    {
        return true;
    }
    if base.len() != 4 {
        return false;
    }
    let prefix: [u16; 3] = std::array::from_fn(|i| {
        if (97..=122).contains(&base[i]) {
            base[i] - 32
        } else {
            base[i]
        }
    });
    (prefix == [67, 79, 77] || prefix == [76, 80, 84])
        && matches!(base[3], 49..=57 | 0xB9 | 0xB2 | 0xB3)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceKind {
    File,
    Directory,
    Shortcut,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileReference {
    pub kind: ReferenceKind,
    pub path: NativePath,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Item {
    pub id: ItemId,
    pub target: FileReference,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Group {
    pub id: GroupId,
    pub title: String,
    pub color: [u8; 3],
    pub rect: RectDip,
    pub monitor_key_utf16: Option<Vec<u16>>,
    pub collapsed: bool,
    pub items: Vec<Item>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Settings {
    pub launch_at_login: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub schema_version: u32,
    pub next_group_id: GroupId,
    pub next_item_id: ItemId,
    pub settings: Settings,
    pub groups: Vec<Group>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            next_group_id: GroupId(NonZeroU64::MIN),
            next_item_id: ItemId(NonZeroU64::MIN),
            settings: Settings::default(),
            groups: Vec::new(),
        }
    }
}
impl Config {
    pub fn allocate_group_id(&mut self) -> Result<GroupId, ValidationError> {
        let id = self.next_group_id;
        self.next_group_id = id.next()?;
        Ok(id)
    }
    pub fn allocate_item_id(&mut self) -> Result<ItemId, ValidationError> {
        let id = self.next_item_id;
        self.next_item_id = id.next()?;
        Ok(id)
    }
    pub fn contains_item(&self, id: ItemId) -> bool {
        self.groups
            .iter()
            .any(|g| g.items.iter().any(|i| i.id == id))
    }
    pub fn validate(&self) -> Result<(), ValidationError> {
        use ValidationError::*;
        if self.schema_version != SCHEMA_VERSION {
            return Err(UnsupportedVersion);
        }
        if self.groups.len() > MAX_GROUPS {
            return Err(TooManyGroups);
        }
        let mut group_ids = BTreeSet::new();
        let mut item_ids = BTreeSet::new();
        let mut item_count = 0_usize;
        let mut path_units = 0_usize;
        for group in &self.groups {
            if !group_ids.insert(group.id) {
                return Err(DuplicateGroupId);
            }
            if group.id >= self.next_group_id {
                return Err(InvalidNextId);
            }
            if group.title.trim().is_empty()
                || group.title.chars().count() > 128
                || group.title.chars().any(char::is_control)
            {
                return Err(InvalidTitle);
            }
            group.rect.validate().map_err(|_| InvalidLayout)?;
            if let Some(key) = &group.monitor_key_utf16
                && (key.is_empty() || key.len() > 1024 || key.contains(&0))
            {
                return Err(InvalidMonitorKey);
            }
            item_count = item_count
                .checked_add(group.items.len())
                .ok_or(TooManyItems)?;
            if item_count > MAX_ITEMS {
                return Err(TooManyItems);
            }
            let mut targets = BTreeSet::new();
            for item in &group.items {
                if !item_ids.insert(item.id) {
                    return Err(DuplicateItemId);
                }
                if item.id >= self.next_item_id {
                    return Err(InvalidNextId);
                }
                if !targets.insert(&item.target.path) {
                    return Err(DuplicateTarget);
                }
                path_units = path_units
                    .checked_add(item.target.path.0.len())
                    .ok_or(PathBudgetExceeded)?;
                if path_units > MAX_TOTAL_PATH_UNITS {
                    return Err(PathBudgetExceeded);
                }
            }
        }
        Ok(())
    }
}
