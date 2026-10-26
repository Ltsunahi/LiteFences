//! Owned messages and worker boundaries; no executor, queues or OS side effects yet.
use super::{layout::RectDip, model::*};
use std::{fmt, num::NonZeroU64, sync::Arc};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppError {
    InvalidData(ValidationError),
    NotFound,
    Unsupported,
    AccessDenied,
    Busy,
    Cancelled,
    InvalidIcon,
    Io,
    StorageReadOnly,
    Conflict,
    ConfigTooLarge,
    InvalidConfig,
    Platform(i32),
}
impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for AppError {}
impl From<ValidationError> for AppError {
    fn from(error: ValidationError) -> Self {
        Self::InvalidData(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestToken {
    pub request_id: NonZeroU64,
    pub generation: NonZeroU64,
    pub item_id: ItemId,
}
impl RequestToken {
    pub fn is_current(self, pending: Option<Self>, config: &Config) -> bool {
        pending == Some(self) && config.contains_item(self.item_id)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppCommand {
    CreateGroup {
        title: String,
        rect: RectDip,
    },
    RenameGroup {
        group: GroupId,
        title: String,
    },
    SetLayout {
        group: GroupId,
        rect: RectDip,
        monitor_key_utf16: Option<Vec<u16>>,
    },
    SetColor {
        group: GroupId,
        color: [u8; 3],
    },
    SetCollapsed {
        group: GroupId,
        collapsed: bool,
    },
    RemoveGroup(GroupId),
    AddReferences {
        group: GroupId,
        targets: Vec<FileReference>,
    },
    RemoveReference(ItemId),
    MoveReference {
        item: ItemId,
        destination: GroupId,
        index: usize,
    },
    ReorderGroups(Vec<GroupId>),
    OpenReference(ItemId),
    OpenLocation(ItemId),
    Undo,
    Save,
}

/// 1..=256 pixels per side, tightly packed, top-down premultiplied BGRA8.
/// The provider is responsible for premultiplication; the constructor checks shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IconPixels {
    width: u16,
    height: u16,
    bgra: Arc<[u8]>,
}
impl IconPixels {
    pub fn new(width: u16, height: u16, bgra: Vec<u8>) -> Result<Self, AppError> {
        if !(1..=256).contains(&width)
            || !(1..=256).contains(&height)
            || bgra.len() != usize::from(width) * usize::from(height) * 4
        {
            return Err(AppError::InvalidIcon);
        }
        Ok(Self {
            width,
            height,
            bgra: bgra.into(),
        })
    }
    pub fn width(&self) -> u16 {
        self.width
    }
    pub fn height(&self) -> u16 {
        self.height
    }
    pub fn bgra(&self) -> &[u8] {
        &self.bgra
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppEvent {
    ModelChanged {
        revision: u64,
    },
    OpenCompleted {
        token: RequestToken,
        result: Result<(), AppError>,
    },
    IconCompleted {
        token: RequestToken,
        result: Result<IconPixels, AppError>,
    },
    SaveCompleted {
        revision: u64,
        result: Result<(), AppError>,
    },
    Failed {
        token: Option<RequestToken>,
        error: AppError,
    },
}

/// Worker-owned, not implicitly Send/Sync. Implementations may own STA COM objects.
pub trait ShellService {
    fn resolve(&mut self, path: &NativePath) -> Result<FileReference, AppError>;
    fn open(&mut self, target: &FileReference) -> Result<(), AppError>;
    fn open_location(&mut self, target: &FileReference) -> Result<(), AppError>;
}
pub trait IconProvider {
    fn load(&mut self, target: &FileReference, size_px: u16) -> Result<IconPixels, AppError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadSource {
    Empty,
    Primary,
    Backup,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedConfig {
    pub config: Config,
    pub source: LoadSource,
    pub write_allowed: bool,
}
/// Store must preserve damaged/unknown input before allowing writes and validate both
/// reads and saves. Errors must not be silently converted to an empty writable config.
pub trait ConfigStore {
    fn load(&mut self) -> Result<LoadedConfig, AppError>;
    fn save(&mut self, config: &Config, revision: u64) -> Result<(), AppError>;
}
