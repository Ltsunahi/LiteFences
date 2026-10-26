//! One worker owns the store. Bounded queues, no polling, no HWND/borrowed pointers.
use crate::{
    core::{
        contracts::{AppError, LoadedConfig},
        model::Config,
    },
    diagnostics::{DiagnosticLog, LogStatus, Record},
    storage::file_store::FileStore,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
};

#[derive(Clone, Debug)]
pub enum DataRoot {
    User,
    Existing(PathBuf),
}
pub enum Command {
    Save { config: Arc<Config>, revision: u64 },
    Shutdown,
}
#[derive(Debug)]
pub enum Event {
    Loaded(Result<LoadedConfig, AppError>),
    Saved {
        revision: u64,
        result: Result<(), AppError>,
    },
    Stopped,
}
pub struct ConfigWorker {
    log_status: Arc<AtomicU8>,
    commands: SyncSender<Command>,
    events: Receiver<Event>,
    // Dropping a worker disconnects the queues; it never blocks the UI in join().
    // Normal shutdown requires Stopped, emitted only after dropping FileStore.
    _thread: JoinHandle<()>,
}
fn open(root: DataRoot) -> Result<FileStore, AppError> {
    let path = match root {
        DataRoot::Existing(path) => path,
        DataRoot::User => {
            let base = std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .ok_or(AppError::NotFound)?;
            use std::path::{Component, Prefix};
            let disk = matches!(base.components().next(), Some(Component::Prefix(p))
                if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)));
            if !base.is_absolute() || !disk {
                return Err(AppError::InvalidConfig);
            }
            let path = base.join("LiteFences");
            match std::fs::create_dir(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    return Err(AppError::AccessDenied);
                }
                Err(_) => return Err(AppError::Io),
            }
            path
        }
    };
    FileStore::open(&path).map_err(Into::into)
}
impl ConfigWorker {
    /// Wake must be nonblocking and must own any resource it touches, never UI state.
    pub fn start(root: DataRoot, wake: impl Fn() + Send + 'static) -> Result<Self, AppError> {
        let (commands, requests) = mpsc::sync_channel(1);
        let (results, events) = mpsc::sync_channel(2);
        let log_status = Arc::new(AtomicU8::new(LogStatus::Unavailable as u8));
        let status = log_status.clone();
        let worker = thread::Builder::new()
            .name("LiteFences-I/O".into())
            .spawn(move || {
                let emit = |event| {
                    let sent = results.send(event).is_ok();
                    wake();
                    sent
                };
                let (mut store, loaded) = match open(root) {
                    Ok(mut store) => {
                        let loaded = store.load_config().map_err(Into::into);
                        (Some(store), loaded)
                    }
                    Err(error) => (None, Err(error)),
                };
                let mut log = store
                    .as_ref()
                    .map_or_else(DiagnosticLog::unavailable, DiagnosticLog::open);
                log.record(Record::Started);
                log.record(Record::Loaded(&loaded));
                status.store(log.status() as u8, Ordering::Release);
                if !emit(Event::Loaded(loaded)) {
                    return;
                }
                while let Ok(command) = requests.recv() {
                    match command {
                        Command::Save { config, revision } => {
                            let result = match store.as_mut() {
                                Some(store) => {
                                    store.save_config(&config, revision).map_err(Into::into)
                                }
                                None => Err(AppError::StorageReadOnly),
                            };
                            log.record(Record::Saved {
                                revision,
                                result: &result,
                            });
                            status.store(log.status() as u8, Ordering::Release);
                            if !emit(Event::Saved { revision, result }) {
                                break;
                            }
                        }
                        Command::Shutdown => break,
                    }
                }
                log.record(Record::Stopped);
                status.store(log.status() as u8, Ordering::Release);
                drop(log);
                drop(store); // Release files and writer lock before acknowledging shutdown.
                emit(Event::Stopped);
            })
            .map_err(|_| AppError::Io)?;
        Ok(Self {
            log_status,
            commands,
            events,
            _thread: worker,
        })
    }
    pub fn log_status(&self) -> LogStatus {
        LogStatus::from_byte(self.log_status.load(Ordering::Acquire))
    }
    pub fn submit(&self, command: Command) -> Result<(), AppError> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                TrySendError::Full(_) => AppError::Busy,
                TrySendError::Disconnected(_) => AppError::Io,
            })
    }
    pub fn poll(&self) -> Result<Event, TryRecvError> {
        self.events.try_recv()
    }
}
