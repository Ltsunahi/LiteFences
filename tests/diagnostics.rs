#![cfg(all(windows, feature = "config-store"))]
use litefences::{
    core::{contracts::AppError, model::Config},
    diagnostics::{DiagnosticLog, LogStatus, MAX_LOG_BYTES, Record},
    storage::file_store::FileStore,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
fn fixture() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts/storage-tests")
        .join(format!(
            "diagnostics-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
    fs::create_dir_all(&path).unwrap();
    path
}
#[test]
fn fixed_codes_build_and_lifecycle_append_across_sessions() {
    let path = fixture();
    for _ in 0..2 {
        let mut store = FileStore::open(&path).unwrap();
        let loaded = store.load_config().map_err(Into::into);
        let mut log = DiagnosticLog::open(&store);
        log.record(Record::Started);
        log.record(Record::Loaded(&loaded));
        log.record(Record::Saved {
            revision: 7,
            result: &Err(AppError::Platform(123456789)),
        });
        log.record(Record::Stopped);
        assert_eq!(log.status(), LogStatus::Recording);
    }
    let text = fs::read_to_string(path.join("diagnostics.log")).unwrap();
    assert_eq!(text.matches("diagnostics v1").count(), 1);
    assert_eq!(text.matches("start version=0.1.0").count(), 2);
    assert_eq!(text.matches("save revision=7 result=platform").count(), 2);
    assert!(!text.contains("123456789"));
    assert!(!text.contains(&path.to_string_lossy().to_string()));
    assert!(text.ends_with("stop\n"));
}
#[test]
fn full_log_stops_growing_without_preventing_config_save_or_reopen() {
    let path = fixture();
    let mut store = FileStore::open(&path).unwrap();
    store.load_config().unwrap();
    let mut log = DiagnosticLog::open(&store);
    for _ in 0..4000 {
        log.record(Record::Started);
    }
    assert_eq!(log.status(), LogStatus::Full);
    let before = fs::read(path.join("diagnostics.log")).unwrap();
    assert!(before.len() as u64 <= MAX_LOG_BYTES);
    for _ in 0..100 {
        log.record(Record::Started);
    }
    assert_eq!(before, fs::read(path.join("diagnostics.log")).unwrap());
    store.save_config(&Config::default(), 1).unwrap();
    drop(log);
    let mut reopened = DiagnosticLog::open(&store);
    reopened.record(Record::Started);
    assert_eq!(reopened.status(), LogStatus::Full);
    assert_eq!(before, fs::read(path.join("diagnostics.log")).unwrap());
}
#[test]
fn unknown_oversized_and_incomplete_logs_are_preserved() {
    for bytes in [
        b"private unrelated file".to_vec(),
        vec![b'x'; MAX_LOG_BYTES as usize + 1],
        b"LiteFences diagnostics v1\npartial".to_vec(),
    ] {
        let path = fixture();
        fs::write(path.join("diagnostics.log"), &bytes).unwrap();
        let store = FileStore::open(&path).unwrap();
        let mut log = DiagnosticLog::open(&store);
        assert_eq!(log.status(), LogStatus::Unavailable);
        log.record(Record::Started);
        assert_eq!(fs::read(path.join("diagnostics.log")).unwrap(), bytes);
    }
}
#[test]
fn hard_link_log_never_modifies_its_target() {
    let path = fixture();
    let target = path.join("reference.txt");
    let bytes = b"LiteFences diagnostics v1\n";
    fs::write(&target, bytes).unwrap();
    fs::hard_link(&target, path.join("diagnostics.log")).unwrap();
    let store = FileStore::open(&path).unwrap();
    let mut log = DiagnosticLog::open(&store);
    assert_eq!(log.status(), LogStatus::Unavailable);
    log.record(Record::Started);
    assert_eq!(fs::read(target).unwrap(), bytes);
}
#[test]
fn directory_and_sharing_conflict_disable_only_logging() {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};
    let path = fixture();
    fs::create_dir(path.join("diagnostics.log")).unwrap();
    let store = FileStore::open(&path).unwrap();
    assert_eq!(DiagnosticLog::open(&store).status(), LogStatus::Unavailable);
    let other = fixture();
    let _occupied = OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(other.join("diagnostics.log"))
        .unwrap();
    let mut store = FileStore::open(&other).unwrap();
    store.load_config().unwrap();
    assert_eq!(DiagnosticLog::open(&store).status(), LogStatus::Unavailable);
    store.save_config(&Config::default(), 1).unwrap();
    assert!(other.join("config.json").exists());
}
