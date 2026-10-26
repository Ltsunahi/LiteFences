//! Single-writer local store. Explicit load is required before each write session.
//! Never opens paths referenced by the configuration. Errors redact private paths.
#![forbid(unsafe_code)]
use super::{
    codec::{self, CodecError},
    native,
};
use crate::core::{
    contracts::{AppError, ConfigStore, LoadSource, LoadedConfig},
    model::{Config, MAX_CONFIG_BYTES},
};
use std::{
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const PRIMARY: &str = "config.json";
const BACKUP: &str = "config.bak.json";
const PENDING: &str = "config.pending";
const BACKUP_PENDING: &str = "backup.pending";
const CORRUPT: &str = "config.corrupt.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    Busy,
    ReadOnly,
    NotLoaded,
    Conflict,
    StaleRevision,
    InvalidDirectory,
    TooLarge,
    Codec(CodecError),
    Io(Option<i32>),
}
impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for StoreError {}
fn io_error(error: io::Error) -> StoreError {
    StoreError::Io(error.raw_os_error())
}
impl From<StoreError> for AppError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Busy => AppError::Busy,
            StoreError::Codec(CodecError::InvalidModel(error)) => AppError::InvalidData(error),
            StoreError::Codec(CodecError::UnsupportedVersion) => AppError::Unsupported,
            StoreError::ReadOnly | StoreError::NotLoaded => AppError::StorageReadOnly,
            StoreError::Conflict => AppError::Conflict,
            StoreError::StaleRevision => AppError::Cancelled,
            StoreError::TooLarge | StoreError::Codec(CodecError::TooLarge) => {
                AppError::ConfigTooLarge
            }
            StoreError::Codec(_) | StoreError::InvalidDirectory => AppError::InvalidConfig,
            StoreError::Io(Some(5)) => AppError::AccessDenied,
            StoreError::Io(_) => AppError::Io,
        }
    }
}

struct Snapshot {
    primary: Option<Vec<u8>>,
    backup: Option<Vec<u8>>,
}
enum State {
    NotLoaded,
    ReadOnly,
    Writable {
        snapshot: Snapshot,
        rotate_primary: bool,
        preserve_corrupt: bool,
    },
}
pub struct FileStore {
    root: PathBuf,
    _lock: File,
    _directory: File,
    state: State,
    last_revision: Option<u64>,
}
impl FileStore {
    /// The store's directory guard and single-writer lock remain alive while logging.
    pub(crate) fn diagnostic_file(&self) -> io::Result<File> {
        native::diagnostic(&self.root.join("diagnostics.log"))
    }
    /// Opens an existing dedicated directory. No user profile discovery or directory
    /// creation. Exclusive share mode is released by the OS even if a process crashes.
    pub fn open(root: &Path) -> Result<Self, StoreError> {
        use std::path::{Component, Prefix};
        let drive_path = matches!(root.components().next(), Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)));
        if !root.is_absolute() || !drive_path {
            return Err(StoreError::InvalidDirectory);
        }
        let directory = native::directory(root).map_err(io_error)?;
        let root = fs::canonicalize(root).map_err(io_error)?;
        let lock = native::lock(&root.join(".writer.lock")).map_err(|e| {
            if e.raw_os_error() == Some(32) {
                StoreError::Busy
            } else {
                io_error(e)
            }
        })?;
        Ok(Self {
            root,
            _lock: lock,
            _directory: directory,
            state: State::NotLoaded,
            last_revision: None,
        })
    }
    fn read_bytes(&self, name: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let file = match native::read(&self.root.join(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        };
        if file.metadata().map_err(io_error)?.len() > MAX_CONFIG_BYTES as u64 {
            return Err(StoreError::TooLarge);
        }
        let mut bytes = Vec::new();
        file.take(MAX_CONFIG_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err(StoreError::TooLarge);
        }
        Ok(Some(bytes))
    }
    fn exists(&self, name: &str) -> Result<bool, StoreError> {
        match fs::symlink_metadata(self.root.join(name)) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(io_error(e)),
        }
    }
    pub fn load_config(&mut self) -> Result<LoadedConfig, StoreError> {
        self.state = State::ReadOnly;
        let primary = self.read_bytes(PRIMARY)?;
        let backup = self.read_bytes(BACKUP)?;
        let p = primary.as_deref().map(codec::decode);
        let b = backup.as_deref().map(codec::decode);
        let future = matches!(p, Some(Err(CodecError::UnsupportedVersion)))
            || matches!(b, Some(Err(CodecError::UnsupportedVersion)));
        let pending = self.exists(PENDING)? || self.exists(BACKUP_PENDING)?;
        let (config, source, rotate, preserve, recoverable) = match (p, b) {
            (Some(Ok(config)), b) => (
                config,
                LoadSource::Primary,
                true,
                false,
                !matches!(b, Some(Err(_))),
            ),
            (p, Some(Ok(config))) => (config, LoadSource::Backup, false, p.is_some(), true),
            (None, None) => (Config::default(), LoadSource::Empty, false, false, true),
            _ => (Config::default(), LoadSource::Empty, false, false, false),
        };
        let write_allowed =
            recoverable && !future && !pending && !(preserve && self.exists(CORRUPT)?);
        if write_allowed {
            self.state = State::Writable {
                snapshot: Snapshot { primary, backup },
                rotate_primary: rotate,
                preserve_corrupt: preserve,
            };
        }
        Ok(LoadedConfig {
            config,
            source,
            write_allowed,
        })
    }
    fn write_new(&self, name: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let mut file = native::create(&self.root.join(name)).map_err(io_error)?;
        file.write_all(bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        // Drop closes the exclusive handle before ReplaceFileW opens it.
        Ok(())
    }
    pub fn save_config(&mut self, config: &Config, revision: u64) -> Result<(), StoreError> {
        match self.state {
            State::NotLoaded => return Err(StoreError::NotLoaded),
            State::ReadOnly => return Err(StoreError::ReadOnly),
            State::Writable { .. } => {}
        }
        if self.last_revision.is_some_and(|last| revision <= last) {
            return Err(StoreError::StaleRevision);
        }
        let bytes = codec::encode(config).map_err(StoreError::Codec)?;
        let State::Writable {
            snapshot,
            rotate_primary,
            preserve_corrupt,
        } = std::mem::replace(&mut self.state, State::ReadOnly)
        else {
            unreachable!()
        };
        // Exact byte comparison detects external edits since load/save, without a
        // lossy hash. This is not a filesystem CAS against uncooperative writers.
        if self.read_bytes(PRIMARY)? != snapshot.primary
            || self.read_bytes(BACKUP)? != snapshot.backup
        {
            return Err(StoreError::Conflict);
        }
        self.write_new(PENDING, &bytes)?;
        if preserve_corrupt {
            self.write_new(
                CORRUPT,
                snapshot.primary.as_deref().ok_or(StoreError::Conflict)?,
            )?;
        }
        if rotate_primary {
            let previous = snapshot.primary.as_deref().ok_or(StoreError::Conflict)?;
            self.write_new(BACKUP_PENDING, previous)?;
            native::commit(
                &self.root.join(BACKUP_PENDING),
                &self.root.join(BACKUP),
                snapshot.backup.is_some(),
            )
            .map_err(io_error)?;
        }
        // Backup/corrupt preservation is complete before committing the new primary.
        if self.read_bytes(PRIMARY)? != snapshot.primary {
            return Err(StoreError::Conflict);
        }
        native::commit(
            &self.root.join(PENDING),
            &self.root.join(PRIMARY),
            snapshot.primary.is_some(),
        )
        .map_err(io_error)?;
        let backup = if rotate_primary {
            snapshot.primary
        } else {
            snapshot.backup
        };
        self.state = State::Writable {
            snapshot: Snapshot {
                primary: Some(bytes),
                backup,
            },
            rotate_primary: true,
            preserve_corrupt: false,
        };
        self.last_revision = Some(revision);
        Ok(())
    }
}
impl ConfigStore for FileStore {
    fn load(&mut self) -> Result<LoadedConfig, AppError> {
        self.load_config().map_err(Into::into)
    }
    fn save(&mut self, config: &Config, revision: u64) -> Result<(), AppError> {
        self.save_config(config, revision).map_err(Into::into)
    }
}
