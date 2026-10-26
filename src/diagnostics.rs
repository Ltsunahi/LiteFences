//! Bounded, local-only diagnostic records. Never accepts user text or paths.
#![forbid(unsafe_code)]
use crate::{
    core::{
        contracts::{AppError, LoadSource, LoadedConfig},
        error_text::error_code,
    },
    storage::file_store::FileStore,
};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_LOG_BYTES: u64 = 64 * 1024;
const HEADER: &[u8] = b"LiteFences diagnostics v1\n";
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum LogStatus {
    Unavailable = 0,
    Recording = 1,
    Full = 2,
}
impl LogStatus {
    pub fn from_byte(value: u8) -> Self {
        match value {
            1 => Self::Recording,
            2 => Self::Full,
            _ => Self::Unavailable,
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Self::Recording => "本地诊断已启用（上限 64 KiB，不记录私人路径）。",
            Self::Full => "诊断日志已达上限，停止追加；配置功能不受影响。",
            Self::Unavailable => "诊断日志不可用；配置状态仍以上方提示为准。",
        }
    }
}
pub enum Record<'a> {
    Started,
    Loaded(&'a Result<LoadedConfig, AppError>),
    Saved {
        revision: u64,
        result: &'a Result<(), AppError>,
    },
    Stopped,
}
pub struct DiagnosticLog {
    file: Option<File>,
    length: u64,
    status: LogStatus,
}
impl DiagnosticLog {
    pub fn open(store: &FileStore) -> Self {
        match store.diagnostic_file().and_then(Self::from_file) {
            Ok(log) => log,
            Err(_) => Self::unavailable(),
        }
    }
    pub fn unavailable() -> Self {
        Self {
            file: None,
            length: 0,
            status: LogStatus::Unavailable,
        }
    }
    fn from_file(mut file: File) -> io::Result<Self> {
        let mut length = file.metadata()?.len();
        if length > MAX_LOG_BYTES {
            return Err(io::Error::other("Oversized diagnostic file"));
        }
        if length == 0 {
            file.write_all(HEADER)?;
            length = HEADER.len() as u64;
        } else {
            let mut header = [0; HEADER.len()];
            file.read_exact(&mut header)?;
            if header != HEADER {
                return Err(io::Error::other("Unknown diagnostic file"));
            }
            // A partially written final record is preserved but never extended.
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0];
            file.read_exact(&mut last)?;
            if last != *b"\n" {
                return Err(io::Error::other("Incomplete diagnostic record"));
            }
        }
        file.seek(SeekFrom::End(0))?;
        Ok(Self {
            file: Some(file),
            length,
            status: LogStatus::Recording,
        })
    }
    pub fn status(&self) -> LogStatus {
        self.status
    }
    pub fn record(&mut self, record: Record<'_>) {
        if self.status != LogStatus::Recording {
            return;
        }
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        // All dynamic text is a fixed whitelist; only numeric time/revision/build version vary.
        let details = match record {
            Record::Started => format!("start version={}", env!("CARGO_PKG_VERSION")),
            Record::Loaded(Ok(loaded)) => format!(
                "load source={} writable={}",
                match loaded.source {
                    LoadSource::Empty => "empty",
                    LoadSource::Primary => "primary",
                    LoadSource::Backup => "backup",
                },
                loaded.write_allowed
            ),
            Record::Loaded(Err(error)) => format!("load error={}", error_code(error)),
            Record::Saved { revision, result } => format!(
                "save revision={revision} result={}",
                result
                    .as_ref()
                    .map_or_else(|error| error_code(error), |_| "ok")
            ),
            Record::Stopped => "stop".into(),
        };
        let line = format!("unix_s={seconds} {details}\n");
        if self.length + line.len() as u64 > MAX_LOG_BYTES {
            self.status = LogStatus::Full;
            self.file = None;
            return;
        }
        if self
            .file
            .as_mut()
            .unwrap()
            .write_all(line.as_bytes())
            .is_err()
        {
            self.status = LogStatus::Unavailable;
            self.file = None;
        } else {
            self.length += line.len() as u64;
        }
    }
}
