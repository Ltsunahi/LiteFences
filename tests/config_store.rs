#![cfg(all(windows, feature = "config-store"))]
use litefences::{
    core::{
        contracts::LoadSource,
        model::{Config, MAX_CONFIG_BYTES},
    },
    storage::{
        codec::{decode, encode},
        file_store::{FileStore, StoreError},
    },
};
use std::{
    fs::{self, File, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
fn fixture(label: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join("storage-tests");
    fs::create_dir_all(&root).unwrap();
    let dir = root.join(format!(
        "{label}-{}-{}-{}",
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
fn edited() -> Config {
    let mut c = Config::default();
    c.settings.launch_at_login = true;
    c
}
fn put(dir: &std::path::Path, name: &str, config: &Config) {
    fs::write(dir.join(name), encode(config).unwrap()).unwrap();
}

#[test]
fn empty_store_requires_load_and_reopens_saved_config() {
    let dir = fixture("empty");
    let mut store = FileStore::open(&dir).unwrap();
    assert_eq!(store.save_config(&edited(), 1), Err(StoreError::NotLoaded));
    let loaded = store.load_config().unwrap();
    assert_eq!(loaded.source, LoadSource::Empty);
    assert!(loaded.write_allowed);
    store.save_config(&edited(), 1).unwrap();
    drop(store);
    let loaded = FileStore::open(&dir).unwrap().load_config().unwrap();
    assert_eq!(loaded.source, LoadSource::Primary);
    assert_eq!(loaded.config, edited());
    assert!(!dir.join("config.pending").exists());
}
#[test]
fn saves_rotate_previous_valid_primary_and_reject_stale_revisions() {
    let dir = fixture("rotate");
    let mut store = FileStore::open(&dir).unwrap();
    store.load_config().unwrap();
    store.save_config(&Config::default(), 1).unwrap();
    store.save_config(&edited(), 2).unwrap();
    assert_eq!(
        decode(&fs::read(dir.join("config.bak.json")).unwrap()).unwrap(),
        Config::default()
    );
    assert_eq!(
        store.save_config(&Config::default(), 2),
        Err(StoreError::StaleRevision)
    );
    assert_eq!(
        store.save_config(&Config::default(), 1),
        Err(StoreError::StaleRevision)
    );
    store.load_config().unwrap();
    assert_eq!(
        store.save_config(&Config::default(), 1),
        Err(StoreError::StaleRevision)
    );
    store.save_config(&Config::default(), 3).unwrap();
    assert_eq!(
        decode(&fs::read(dir.join("config.bak.json")).unwrap()).unwrap(),
        edited()
    );
    assert!(!dir.join("backup.pending").exists());
}
#[test]
fn corrupt_primary_falls_back_and_preserves_exact_bad_bytes_before_save() {
    let dir = fixture("corrupt");
    let broken = b"{truncated-private-value";
    fs::write(dir.join("config.json"), broken).unwrap();
    put(&dir, "config.bak.json", &edited());
    let mut store = FileStore::open(&dir).unwrap();
    let loaded = store.load_config().unwrap();
    assert_eq!(loaded.source, LoadSource::Backup);
    assert_eq!(loaded.config, edited());
    assert!(loaded.write_allowed);
    store.save_config(&loaded.config, 1).unwrap();
    assert_eq!(fs::read(dir.join("config.corrupt.json")).unwrap(), broken);
    assert_eq!(
        decode(&fs::read(dir.join("config.bak.json")).unwrap()).unwrap(),
        edited()
    );
    assert_eq!(store.load_config().unwrap().source, LoadSource::Primary);
}
#[test]
fn missing_primary_recovers_backup_without_rotating_it() {
    let dir = fixture("missing");
    put(&dir, "config.bak.json", &edited());
    let mut store = FileStore::open(&dir).unwrap();
    let loaded = store.load_config().unwrap();
    assert_eq!(loaded.source, LoadSource::Backup);
    store.save_config(&loaded.config, 1).unwrap();
    assert_eq!(
        decode(&fs::read(dir.join("config.bak.json")).unwrap()).unwrap(),
        edited()
    );
}
#[test]
fn future_schema_is_read_only_even_when_valid_backup_exists() {
    let dir = fixture("future");
    let bytes = br#"{"schema_version":2,"new":true}"#;
    fs::write(dir.join("config.json"), bytes).unwrap();
    put(&dir, "config.bak.json", &Config::default());
    let mut store = FileStore::open(&dir).unwrap();
    let loaded = store.load_config().unwrap();
    assert_eq!(loaded.source, LoadSource::Backup);
    assert!(!loaded.write_allowed);
    assert_eq!(store.save_config(&edited(), 1), Err(StoreError::ReadOnly));
    assert_eq!(fs::read(dir.join("config.json")).unwrap(), bytes);
}
#[test]
fn both_bad_files_yield_read_only_empty_layout_without_overwriting() {
    let dir = fixture("bothbad");
    fs::write(dir.join("config.json"), b"bad1").unwrap();
    fs::write(dir.join("config.bak.json"), b"bad2").unwrap();
    let mut store = FileStore::open(&dir).unwrap();
    let loaded = store.load_config().unwrap();
    assert_eq!(loaded.source, LoadSource::Empty);
    assert!(!loaded.write_allowed);
    assert_eq!(
        store.save_config(&Config::default(), 1),
        Err(StoreError::ReadOnly)
    );
    assert_eq!(fs::read(dir.join("config.json")).unwrap(), b"bad1");
    assert_eq!(fs::read(dir.join("config.bak.json")).unwrap(), b"bad2");
}
#[test]
fn invalid_or_future_backup_is_never_silently_overwritten() {
    for bytes in [b"bad".as_slice(), br#"{"schema_version":2}"#] {
        let dir = fixture("backupbad");
        put(&dir, "config.json", &Config::default());
        fs::write(dir.join("config.bak.json"), bytes).unwrap();
        let mut store = FileStore::open(&dir).unwrap();
        let loaded = store.load_config().unwrap();
        assert_eq!(loaded.source, LoadSource::Primary);
        assert!(!loaded.write_allowed);
        assert_eq!(store.save_config(&edited(), 1), Err(StoreError::ReadOnly));
        assert_eq!(fs::read(dir.join("config.bak.json")).unwrap(), bytes);
    }
}
#[test]
fn orphan_pending_files_are_preserved_and_never_auto_promoted() {
    for name in ["config.pending", "backup.pending"] {
        let dir = fixture("orphan");
        put(&dir, "config.json", &Config::default());
        put(&dir, name, &edited());
        let mut store = FileStore::open(&dir).unwrap();
        let loaded = store.load_config().unwrap();
        assert_eq!(loaded.config, Config::default());
        assert!(!loaded.write_allowed);
        assert_eq!(store.save_config(&edited(), 1), Err(StoreError::ReadOnly));
        assert_eq!(
            decode(&fs::read(dir.join(name)).unwrap()).unwrap(),
            edited()
        );
    }
}
#[test]
fn existing_corrupt_archive_cannot_be_overwritten_by_second_recovery() {
    let dir = fixture("archive");
    fs::write(dir.join("config.json"), b"broken").unwrap();
    fs::write(dir.join("config.corrupt.json"), b"older-broken").unwrap();
    put(&dir, "config.bak.json", &Config::default());
    let mut store = FileStore::open(&dir).unwrap();
    assert!(!store.load_config().unwrap().write_allowed);
    assert_eq!(store.save_config(&edited(), 1), Err(StoreError::ReadOnly));
    assert_eq!(
        fs::read(dir.join("config.corrupt.json")).unwrap(),
        b"older-broken"
    );
}
#[test]
fn external_edits_are_detected_without_overwrite() {
    for name in ["config.json", "config.bak.json"] {
        let dir = fixture("conflict");
        put(&dir, "config.json", &Config::default());
        let mut store = FileStore::open(&dir).unwrap();
        store.load_config().unwrap();
        fs::write(dir.join(name), b"external").unwrap();
        assert_eq!(store.save_config(&edited(), 1), Err(StoreError::Conflict));
        assert_eq!(fs::read(dir.join(name)).unwrap(), b"external");
        assert!(!dir.join("config.pending").exists());
    }
}
#[test]
fn validation_failure_does_not_write_or_consume_revision() {
    let dir = fixture("invalid");
    let mut store = FileStore::open(&dir).unwrap();
    store.load_config().unwrap();
    let config = Config {
        schema_version: 2,
        ..Config::default()
    };
    assert!(matches!(
        store.save_config(&config, 1),
        Err(StoreError::Codec(_))
    ));
    assert!(!dir.join("config.json").exists());
    assert!(!dir.join("config.pending").exists());
    store.save_config(&Config::default(), 1).unwrap();
}
#[test]
fn replacement_failure_preserves_previous_config_backup_and_pending() {
    let dir = fixture("sharingfailure");
    put(&dir, "config.json", &Config::default());
    let mut store = FileStore::open(&dir).unwrap();
    store.load_config().unwrap();
    // Allow reads but deny DELETE/WRITE: actual Windows replacement must fail.
    let blocker = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(dir.join("config.json"))
        .unwrap();
    assert!(matches!(
        store.save_config(&edited(), 1),
        Err(StoreError::Io(_))
    ));
    drop(blocker);
    assert_eq!(
        decode(&fs::read(dir.join("config.json")).unwrap()).unwrap(),
        Config::default()
    );
    assert_eq!(
        decode(&fs::read(dir.join("config.bak.json")).unwrap()).unwrap(),
        Config::default()
    );
    assert_eq!(
        decode(&fs::read(dir.join("config.pending")).unwrap()).unwrap(),
        edited()
    );
    assert_eq!(store.save_config(&edited(), 2), Err(StoreError::ReadOnly));
    drop(store);
    assert!(
        !FileStore::open(&dir)
            .unwrap()
            .load_config()
            .unwrap()
            .write_allowed
    );
}
#[test]
fn oversized_or_inaccessible_input_does_not_become_writable_empty() {
    let dir = fixture("oversize");
    File::create(dir.join("config.json"))
        .unwrap()
        .set_len(MAX_CONFIG_BYTES as u64 + 1)
        .unwrap();
    let mut store = FileStore::open(&dir).unwrap();
    assert_eq!(store.load_config(), Err(StoreError::TooLarge));
    assert_eq!(store.save_config(&edited(), 1), Err(StoreError::ReadOnly));
    let dir = fixture("unreadable");
    put(&dir, "config.json", &edited());
    let _blocked = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(dir.join("config.json"))
        .unwrap();
    let mut store = FileStore::open(&dir).unwrap();
    assert!(matches!(store.load_config(), Err(StoreError::Io(_))));
    assert_eq!(
        store.save_config(&Config::default(), 1),
        Err(StoreError::ReadOnly)
    );
}
#[test]
fn directory_in_config_slot_is_rejected() {
    let dir = fixture("directory");
    fs::create_dir(dir.join("config.json")).unwrap();
    let mut store = FileStore::open(&dir).unwrap();
    assert!(store.load_config().is_err());
    assert_eq!(store.save_config(&edited(), 1), Err(StoreError::ReadOnly));
    assert!(dir.join("config.json").is_dir());
}

#[test]
fn save_and_recovery_do_not_modify_referenced_source_file() {
    use litefences::core::{
        layout::RectDip,
        model::{FileReference, Group, Item, NativePath, ReferenceKind},
    };
    let dir = fixture("source-safety");
    let source = dir.join("source-example.txt");
    fs::write(&source, b"source data must remain unchanged").unwrap();
    let before = fs::read(&source).unwrap();
    let mut config = Config::default();
    let group_id = config.allocate_group_id().unwrap();
    let item_id = config.allocate_item_id().unwrap();
    config.groups.push(Group {
        id: group_id,
        title: "原始".into(),
        color: [1, 2, 3],
        rect: RectDip {
            x: 0,
            y: 0,
            width: 360,
            height: 240,
        },
        monitor_key_utf16: None,
        collapsed: false,
        items: vec![Item {
            id: item_id,
            target: FileReference {
                kind: ReferenceKind::File,
                path: NativePath::from_os_str(source.as_os_str()).unwrap(),
            },
        }],
    });
    let mut store = FileStore::open(&dir).unwrap();
    store.load_config().unwrap();
    store.save_config(&config, 1).unwrap();
    let original = config.clone();
    config.groups[0].title = "更改".into();
    store.save_config(&config, 2).unwrap();
    drop(store);
    fs::write(dir.join("config.json"), b"truncated").unwrap();
    let mut store = FileStore::open(&dir).unwrap();
    let loaded = store.load_config().unwrap();
    assert_eq!(loaded.config, original);
    store.save_config(&loaded.config, 1).unwrap();
    assert_eq!(fs::read(source).unwrap(), before);
}

#[test]
fn failed_backup_replacement_does_not_commit_new_primary() {
    let dir = fixture("backup-failure");
    put(&dir, "config.json", &edited());
    put(&dir, "config.bak.json", &Config::default());
    let mut store = FileStore::open(&dir).unwrap();
    store.load_config().unwrap();
    let _blocker = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(dir.join("config.bak.json"))
        .unwrap();
    assert!(matches!(
        store.save_config(&Config::default(), 1),
        Err(StoreError::Io(_))
    ));
    assert_eq!(
        decode(&fs::read(dir.join("config.json")).unwrap()).unwrap(),
        edited()
    );
    assert_eq!(
        decode(&fs::read(dir.join("config.bak.json")).unwrap()).unwrap(),
        Config::default()
    );
    assert!(dir.join("backup.pending").exists());
    assert!(dir.join("config.pending").exists());
}

// The test executable doubles as an isolated lock-holder. No user process is killed.
#[test]
fn child_lock_holder() {
    let Some(dir) = std::env::var_os("LITEFENCES_TEST_LOCK_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let _store = FileStore::open(&dir).unwrap();
    fs::write(dir.join("child.ready"), b"locked").unwrap();
    loop {
        std::thread::sleep(Duration::from_millis(100));
    }
}
#[test]
fn lock_excludes_other_process_and_is_released_after_forced_exit() {
    let dir = fixture("processlock");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_lock_holder", "--nocapture"])
        .env("LITEFENCES_TEST_LOCK_DIR", &dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !dir.join("child.ready").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let ready = dir.join("child.ready").exists();
    let busy = matches!(FileStore::open(&dir), Err(StoreError::Busy));
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(ready && busy);
    let mut store = FileStore::open(&dir).unwrap();
    assert!(store.load_config().unwrap().write_allowed);
    store.save_config(&Config::default(), 1).unwrap();
}
