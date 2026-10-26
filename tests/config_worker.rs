#![cfg(all(windows, feature = "config-store"))]

use litefences::{
    core::{
        contracts::{AppError, LoadSource},
        model::Config,
    },
    services::config_worker::{Command, ConfigWorker, DataRoot, Event},
    storage::file_store::FileStore,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn fixture(label: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join("storage-tests");
    fs::create_dir_all(&root).unwrap();
    let dir = root.join(format!(
        "worker-{label}-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    dir
}

fn start(dir: &Path) -> (ConfigWorker, Receiver<()>) {
    let (wake_tx, wake_rx) = mpsc::channel();
    let worker = ConfigWorker::start(DataRoot::Existing(dir.to_path_buf()), move || {
        let _ = wake_tx.send(());
    })
    .unwrap();
    (worker, wake_rx)
}

fn event(worker: &ConfigWorker, wake: &Receiver<()>) -> Event {
    wake.recv_timeout(Duration::from_secs(5)).unwrap();
    worker.poll().unwrap()
}

fn edited() -> Config {
    let mut config = Config::default();
    config.settings.launch_at_login = true;
    config
}

#[test]
fn loads_empty_saves_reopens_and_shutdown_releases_lock() {
    let dir = fixture("roundtrip");
    let (worker, wake) = start(&dir);
    let Event::Loaded(Ok(loaded)) = event(&worker, &wake) else {
        panic!("expected loaded config")
    };
    assert_eq!(loaded.source, LoadSource::Empty);
    assert!(loaded.write_allowed);
    assert_eq!(
        worker.log_status(),
        litefences::diagnostics::LogStatus::Recording
    );
    worker
        .submit(Command::Save {
            config: Arc::new(edited()),
            revision: 1,
        })
        .unwrap();
    assert!(matches!(
        event(&worker, &wake),
        Event::Saved {
            revision: 1,
            result: Ok(())
        }
    ));
    worker.submit(Command::Shutdown).unwrap();
    assert!(matches!(event(&worker, &wake), Event::Stopped));
    let log = fs::read_to_string(dir.join("diagnostics.log")).unwrap();
    assert!(log.contains("load source=empty writable=true"));
    assert!(log.contains("save revision=1 result=ok"));
    assert!(log.ends_with("stop\n"));
    drop(worker);

    let (reopened, reopen_wake) = start(&dir);
    let Event::Loaded(Ok(loaded)) = event(&reopened, &reopen_wake) else {
        panic!("expected reopened config")
    };
    assert_eq!(loaded.source, LoadSource::Primary);
    assert_eq!(loaded.config, edited());
    reopened.submit(Command::Shutdown).unwrap();
    assert!(matches!(event(&reopened, &reopen_wake), Event::Stopped));
    drop(reopened);
    FileStore::open(&dir).unwrap();
}

#[test]
fn log_failure_is_visible_but_config_save_still_succeeds() {
    let dir = fixture("log-unavailable");
    let original = b"unrelated original content";
    fs::write(dir.join("diagnostics.log"), original).unwrap();
    let (worker, wake) = start(&dir);
    assert!(matches!(event(&worker, &wake), Event::Loaded(Ok(_))));
    assert_eq!(
        worker.log_status(),
        litefences::diagnostics::LogStatus::Unavailable
    );
    worker
        .submit(Command::Save {
            config: Arc::new(edited()),
            revision: 1,
        })
        .unwrap();
    assert!(matches!(
        event(&worker, &wake),
        Event::Saved { result: Ok(()), .. }
    ));
    assert_eq!(fs::read(dir.join("diagnostics.log")).unwrap(), original);
    worker.submit(Command::Shutdown).unwrap();
    assert!(matches!(event(&worker, &wake), Event::Stopped));
}

#[test]
fn missing_existing_directory_fails_without_creating_it() {
    let dir = fixture("missing").join("absent");
    let (worker, wake) = start(&dir);
    assert!(matches!(event(&worker, &wake), Event::Loaded(Err(_))));
    assert!(!dir.exists());
}

#[test]
fn future_config_is_read_only_and_save_preserves_its_bytes() {
    let dir = fixture("future");
    let bytes = br#"{"schema_version":2,"new":true}"#;
    fs::write(dir.join("config.json"), bytes).unwrap();
    let (worker, wake) = start(&dir);
    let Event::Loaded(Ok(loaded)) = event(&worker, &wake) else {
        panic!("expected loaded config")
    };
    assert!(!loaded.write_allowed);
    worker
        .submit(Command::Save {
            config: Arc::new(edited()),
            revision: 1,
        })
        .unwrap();
    assert!(matches!(
        event(&worker, &wake),
        Event::Saved {
            revision: 1,
            result: Err(AppError::StorageReadOnly)
        }
    ));
    assert_eq!(fs::read(dir.join("config.json")).unwrap(), bytes);
}

#[test]
fn busy_directory_lock_is_reported_as_load_error() {
    let dir = fixture("busy");
    let _holder = FileStore::open(&dir).unwrap();
    let (worker, wake) = start(&dir);
    assert!(matches!(
        event(&worker, &wake),
        Event::Loaded(Err(AppError::Busy))
    ));
}

#[test]
fn dropping_worker_eventually_releases_directory_lock() {
    let dir = fixture("drop");
    let (worker, wake) = start(&dir);
    assert!(matches!(event(&worker, &wake), Event::Loaded(Ok(_))));
    assert!(matches!(
        FileStore::open(&dir),
        Err(litefences::storage::file_store::StoreError::Busy)
    ));
    drop(worker);
    wake.recv_timeout(Duration::from_secs(5)).unwrap();
    FileStore::open(&dir).unwrap();
}
