//! Bounded Shell icon requests. No Shell calls or file reads occur in WM_PAINT.
use super::config_ui::WakeEvent;
use super::open_policy::{OpenAction, OpenFailure};
use litefences::core::model::{FileReference, NativePath, ReferenceKind};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    mem::size_of,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows_sys::Win32::{
    Graphics::Gdi::{BITMAP, DeleteObject, GetObjectW, HBITMAP},
    UI::{
        Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON, SHGetFileInfoW},
        WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO},
    },
};

const MAX_QUEUED: usize = 64;
// Includes queued, in-progress, and completed-but-undrained requests. Bounding
// the input queue alone is insufficient when the UI stops draining results.
const MAX_OUTSTANDING: usize = 64;
const MAX_CACHED: usize = 256;
// Accounted bitmap payload, two retained path copies, and per-entry overhead.
// Native USER/GDI allocator overhead is opaque and is measured separately.
const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;
const ENTRY_OVERHEAD: usize = 256;
const REFRESH_AFTER: Duration = Duration::from_secs(60);

struct Apartment;
impl Apartment {
    fn initialize() -> Option<Self> {
        // SAFETY: executed on the dedicated icon thread (or its test thread).
        // Every successful call, including S_FALSE, is balanced on this thread.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .ok()
                .map(|()| Self)
        }
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: this guard never leaves the thread that initialized COM.
        unsafe { CoUninitialize() }
    }
}

// A Shell-created HICON is owned by this process and can be drawn on the UI
// thread. It is destroyed exactly once, even if a result arrives after exit.
struct OwnedIcon {
    handle: usize,
    bitmap_bytes: usize,
}
impl OwnedIcon {
    fn raw(&self) -> HICON {
        self.handle as HICON
    }
}
impl Drop for OwnedIcon {
    fn drop(&mut self) {
        // SAFETY: SHGetFileInfoW returned this non-null, process-owned icon.
        unsafe {
            DestroyIcon(self.raw());
        }
    }
}

struct ResultIcon {
    path: NativePath,
    icon: Option<OwnedIcon>,
    availability: Availability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Availability {
    Pending,
    File,
    Directory,
    Missing,
    Unknown,
}

#[derive(Clone, Copy)]
pub struct IconView {
    pub handle: Option<HICON>,
    pub availability: Availability,
}

struct CachedIcon {
    icon: Option<OwnedIcon>,
    availability: Availability,
    checked_at: Instant,
}

impl CachedIcon {
    fn accounted_bytes(&self, path: &NativePath) -> usize {
        std::mem::size_of_val(path.as_utf16()) * 2
            + ENTRY_OVERHEAD
            + self.icon.as_ref().map_or(0, |icon| icon.bitmap_bytes)
    }
}

pub struct IconWorker {
    requests: SyncSender<WorkItem>,
    results: Receiver<ResultIcon>,
    pending: BTreeSet<NativePath>,
    cached: BTreeMap<NativePath, CachedIcon>,
    age: VecDeque<NativePath>,
    open: OpenState,
}

#[derive(Debug, PartialEq)]
enum WorkItem {
    Icon(NativePath),
    Open(FileReference, OpenAction),
}

struct OpenState {
    results: Receiver<Result<(), OpenFailure>>,
    pending: bool,
    action: Option<OpenAction>,
    outcome: Option<Result<(), OpenFailure>>,
    stopped: Arc<AtomicBool>,
}

impl Drop for OpenState {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

#[cfg(test)]
impl Default for OpenState {
    fn default() -> Self {
        let (_, results) = mpsc::sync_channel(1);
        Self {
            results,
            pending: false,
            action: None,
            outcome: None,
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenQueueError {
    Busy,
    Unavailable,
}

impl IconWorker {
    pub fn start(wake: Arc<WakeEvent>) -> io::Result<Self> {
        Self::start_with_opener(wake, super::shell_open::open)
    }

    fn start_with_opener(
        wake: Arc<WakeEvent>,
        opener: fn(&FileReference, OpenAction, &AtomicBool) -> Result<(), OpenFailure>,
    ) -> io::Result<Self> {
        let (requests, requests_rx) = mpsc::sync_channel(MAX_QUEUED);
        let (results_tx, results) = mpsc::sync_channel(MAX_OUTSTANDING);
        let (open_results_tx, open_results) = mpsc::sync_channel(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = stopped.clone();
        thread::Builder::new()
            .name("litefences-icons".into())
            .spawn(move || {
                let apartment = Apartment::initialize();
                while let Ok(request) = requests_rx.recv() {
                    if worker_stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let path = match request {
                        WorkItem::Icon(path) => path,
                        WorkItem::Open(reference, action) => {
                            let outcome = if apartment.is_some() {
                                opener(&reference, action, &worker_stopped)
                            } else {
                                Err(OpenFailure::ShellUnavailable)
                            };
                            if open_results_tx.send(outcome).is_err() {
                                break;
                            }
                            wake.signal();
                            continue;
                        }
                    };
                    let (icon, availability) = if apartment.is_some() {
                        load_icon(&path)
                    } else {
                        // Never call Shell without an initialized apartment.
                        (None, Availability::Unknown)
                    };
                    if results_tx
                        .send(ResultIcon {
                            path,
                            icon,
                            availability,
                        })
                        .is_err()
                    {
                        break;
                    }
                    wake.signal();
                }
            })?;
        Ok(Self {
            requests,
            results,
            pending: BTreeSet::new(),
            cached: BTreeMap::new(),
            age: VecDeque::new(),
            open: OpenState {
                results: open_results,
                pending: false,
                action: None,
                outcome: None,
                stopped,
            },
        })
    }

    /// At most one activation may be queued/in-flight/unconsumed. No UI wait.
    pub fn request_open(
        &mut self,
        reference: FileReference,
        action: OpenAction,
    ) -> Result<(), OpenQueueError> {
        if self.open.stopped.load(Ordering::Acquire) {
            return Err(OpenQueueError::Unavailable);
        }
        if self.open.pending {
            return Err(OpenQueueError::Busy);
        }
        self.requests
            .try_send(WorkItem::Open(reference, action))
            .map_err(|error| match error {
                TrySendError::Full(_) => OpenQueueError::Busy,
                TrySendError::Disconnected(_) => OpenQueueError::Unavailable,
            })?;
        self.open.pending = true;
        self.open.action = Some(action);
        Ok(())
    }

    pub fn take_open_outcome(&mut self) -> Option<(OpenAction, Result<(), OpenFailure>)> {
        let outcome = self.open.outcome.take()?;
        let action = self
            .open
            .action
            .take()
            .expect("outcome has a pending action");
        self.open.pending = false;
        Some((action, outcome))
    }

    pub fn cancel_pending_open(&mut self) {
        // An already-entered Shell call cannot be forcibly cancelled safely.
        self.open.stopped.store(true, Ordering::Release);
    }

    /// Returns a borrowed process icon handle, or queues one nonblocking request.
    pub fn get_or_request(&mut self, path: &NativePath) -> IconView {
        if let Some(icon) = self.cached.get(path) {
            let view = IconView {
                handle: icon.icon.as_ref().map(OwnedIcon::raw),
                availability: icon.availability,
            };
            if icon.checked_at.elapsed() >= REFRESH_AFTER {
                self.queue(path);
            }
            return view;
        }
        self.queue(path);
        IconView::pending()
    }

    fn queue(&mut self, path: &NativePath) {
        if self.pending.contains(path) || self.pending.len() >= MAX_OUTSTANDING {
            return;
        }
        match self.requests.try_send(WorkItem::Icon(path.clone())) {
            Ok(()) => {
                self.pending.insert(path.clone());
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Runs on the UI thread after the shared wake event fires.
    pub fn drain(&mut self) -> bool {
        if self.open.pending && self.open.outcome.is_none() {
            match self.open.results.try_recv() {
                Ok(outcome) => self.open.outcome = Some(outcome),
                Err(TryRecvError::Disconnected) => {
                    self.open.outcome = Some(Err(OpenFailure::ShellUnavailable))
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let mut changed = false;
        loop {
            let result = match self.results.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            };
            self.pending.remove(&result.path);
            // A path may have left the visible set while extraction was pending.
            // Keep a small FIFO cache so moving back does not immediately requery Shell.
            if !self.cached.contains_key(&result.path) {
                self.age.push_back(result.path.clone());
            }
            self.cached.insert(
                result.path,
                CachedIcon {
                    icon: result.icon,
                    availability: result.availability,
                    checked_at: Instant::now(),
                },
            );
            while self.age.len() > MAX_CACHED || self.accounted_cache_bytes() > MAX_CACHE_BYTES {
                if let Some(old) = self.age.pop_front() {
                    self.cached.remove(&old);
                }
            }
            changed = true;
        }
        changed
    }

    fn accounted_cache_bytes(&self) -> usize {
        // At most 256 entries; no filesystem or native calls on this path.
        self.cached
            .iter()
            .map(|(path, icon)| icon.accounted_bytes(path))
            .sum()
    }
}

impl Drop for IconWorker {
    fn drop(&mut self) {
        self.cancel_pending_open();
    }
}

impl IconView {
    pub fn pending() -> Self {
        Self {
            handle: None,
            availability: Availability::Pending,
        }
    }

    pub fn is_invalid(self, kind: ReferenceKind) -> bool {
        matches!(
            (kind, self.availability),
            (_, Availability::Missing)
                | (ReferenceKind::Directory, Availability::File)
                | (
                    ReferenceKind::File | ReferenceKind::Shortcut,
                    Availability::Directory
                )
        )
    }
}

fn load_icon(path: &NativePath) -> (Option<OwnedIcon>, Availability) {
    let native = PathBuf::from(std::ffi::OsString::from_wide(path.as_utf16()));
    let availability = match std::fs::metadata(&native) {
        Ok(metadata) if metadata.is_file() => Availability::File,
        Ok(metadata) if metadata.is_dir() => Availability::Directory,
        Ok(_) => Availability::Unknown,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Availability::Missing,
        Err(_) => Availability::Unknown,
    };
    if availability == Availability::Missing {
        return (None, availability);
    }
    // The string form of SHGetFileInfoW is documented only up to MAX_PATH.
    // Preserve long-path references and their availability, but use the type
    // placeholder until a PIDL-based icon provider is available.
    if path.as_utf16().len() >= 260 {
        return (None, availability);
    }
    let mut units = path.as_utf16().to_vec();
    units.push(0);
    let mut info = SHFILEINFOW::default();
    // SAFETY: units is NUL-terminated and lives through the call. The returned
    // hIcon is process-owned and transferred to OwnedIcon for one DestroyIcon.
    let result = unsafe {
        SHGetFileInfoW(
            units.as_ptr(),
            0,
            &mut info,
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_SMALLICON,
        )
    };
    if info.hIcon.is_null() {
        (None, availability)
    } else {
        let mut icon = OwnedIcon {
            handle: info.hIcon as usize,
            bitmap_bytes: 0,
        };
        if result == 0 {
            // Retain ownership even if Shell supplied a handle on failure.
            return (None, availability);
        }
        match icon_bitmap_bytes(icon.raw()) {
            Some(bytes) if bytes <= MAX_CACHE_BYTES => {
                icon.bitmap_bytes = bytes;
                (Some(icon), availability)
            }
            // Unaccountable or over-budget handles are released, not cached.
            _ => (None, availability),
        }
    }
}

fn bitmap_payload_bytes(stride: i32, height: i32) -> Option<usize> {
    if stride <= 0 || height <= 0 {
        return None;
    }
    (stride as usize).checked_mul(height as usize)
}

fn icon_bitmap_bytes(icon: HICON) -> Option<usize> {
    let mut info = ICONINFO::default();
    // SAFETY: icon is a live worker-owned HICON. GetIconInfo creates separate
    // mask/color bitmap copies, each deleted below even if inspection fails.
    if unsafe { GetIconInfo(icon, &mut info) } == 0 {
        return None;
    }
    let inspect = |bitmap: HBITMAP| {
        if bitmap.is_null() {
            return Some(0);
        }
        let mut object = BITMAP::default();
        // SAFETY: bitmap belongs to this GetIconInfo result; object is aligned
        // and its exact size is supplied. It is never selected into a DC.
        let count = unsafe {
            GetObjectW(
                bitmap,
                size_of::<BITMAP>() as i32,
                (&mut object as *mut BITMAP).cast(),
            )
        };
        (count == size_of::<BITMAP>() as i32)
            .then(|| bitmap_payload_bytes(object.bmWidthBytes, object.bmHeight))
            .flatten()
    };
    let bytes = inspect(info.hbmMask)
        .zip(inspect(info.hbmColor))
        .and_then(|(mask, color)| mask.checked_add(color))
        .filter(|bytes| *bytes > 0);
    // SAFETY: non-null bitmap handles are fresh GetIconInfo copies, not the
    // bitmaps owned by HICON. Delete each exactly once before returning.
    unsafe {
        if !info.hbmMask.is_null() {
            DeleteObject(info.hbmMask);
        }
        if !info.hbmColor.is_null() {
            DeleteObject(info.hbmColor);
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(number: usize) -> NativePath {
        NativePath::from_utf16(
            format!("C:\\fixture\\item-{number}.txt")
                .encode_utf16()
                .collect(),
        )
        .expect("fixture path")
    }

    #[test]
    fn open_requests_share_the_worker_and_stay_single_until_outcome_consumed() {
        fn fake_open(_: &FileReference, _: OpenAction, _: &AtomicBool) -> Result<(), OpenFailure> {
            Err(OpenFailure::Missing)
        }
        let wake = WakeEvent::new().unwrap();
        let mut worker = IconWorker::start_with_opener(wake.clone(), fake_open).unwrap();
        let reference = FileReference {
            kind: ReferenceKind::File,
            path: path(999),
        };
        assert_eq!(
            worker.request_open(reference.clone(), OpenAction::Open),
            Ok(())
        );
        assert_eq!(
            worker.request_open(reference.clone(), OpenAction::Reveal),
            Err(OpenQueueError::Busy)
        );
        // This is only a bounded wait in a test, never in the UI input path.
        for _ in 0..4 {
            worker.drain();
            if worker.open.outcome.is_some() {
                break;
            }
            // SAFETY: this test owns the Arc that keeps the event alive.
            unsafe {
                windows_sys::Win32::System::Threading::WaitForSingleObject(wake.handle(), 5000);
            }
        }
        worker.drain();
        assert_eq!(
            worker.request_open(reference.clone(), OpenAction::Reveal),
            Err(OpenQueueError::Busy)
        );
        assert_eq!(
            worker.take_open_outcome(),
            Some((OpenAction::Open, Err(OpenFailure::Missing)))
        );
        assert_eq!(worker.request_open(reference, OpenAction::Reveal), Ok(()));
    }

    #[test]
    fn shutdown_refuses_new_open_requests() {
        fn fake_open(_: &FileReference, _: OpenAction, _: &AtomicBool) -> Result<(), OpenFailure> {
            Ok(())
        }
        let mut worker =
            IconWorker::start_with_opener(WakeEvent::new().unwrap(), fake_open).unwrap();
        worker.cancel_pending_open();
        assert_eq!(
            worker.request_open(
                FileReference {
                    kind: ReferenceKind::File,
                    path: path(0)
                },
                OpenAction::Reveal
            ),
            Err(OpenQueueError::Unavailable)
        );
    }

    #[test]
    fn repeated_visible_request_is_queued_once_until_result() {
        let (requests, request_rx) = mpsc::sync_channel(2);
        let (result_tx, results) = mpsc::channel();
        let mut worker = IconWorker {
            requests,
            results,
            pending: BTreeSet::new(),
            cached: BTreeMap::new(),
            age: VecDeque::new(),
            open: OpenState::default(),
        };
        let target = path(1);
        assert_eq!(
            worker.get_or_request(&target).availability,
            Availability::Pending
        );
        assert_eq!(
            worker.get_or_request(&target).availability,
            Availability::Pending
        );
        assert_eq!(
            request_rx.try_recv().unwrap(),
            WorkItem::Icon(target.clone())
        );
        assert!(request_rx.try_recv().is_err());
        result_tx
            .send(ResultIcon {
                path: target.clone(),
                icon: None,
                availability: Availability::Missing,
            })
            .unwrap();
        assert!(worker.drain());
        assert!(!worker.drain());
        assert_eq!(
            worker.get_or_request(&target).availability,
            Availability::Missing
        );
        assert!(request_rx.try_recv().is_err());
    }

    #[test]
    fn reveal_action_and_exact_reference_reach_worker_without_becoming_open() {
        fn fake_open(
            reference: &FileReference,
            action: OpenAction,
            _: &AtomicBool,
        ) -> Result<(), OpenFailure> {
            assert_eq!(action, OpenAction::Reveal);
            assert_eq!(reference.kind, ReferenceKind::Shortcut);
            assert_eq!(reference.path, path(7));
            Err(OpenFailure::Missing)
        }
        let wake = WakeEvent::new().unwrap();
        let mut worker = IconWorker::start_with_opener(wake.clone(), fake_open).unwrap();
        worker
            .request_open(
                FileReference {
                    kind: ReferenceKind::Shortcut,
                    path: path(7),
                },
                OpenAction::Reveal,
            )
            .unwrap();
        for _ in 0..4 {
            worker.drain();
            if worker.open.outcome.is_some() {
                break;
            }
            // SAFETY: the live Arc keeps this test-owned event valid.
            unsafe {
                windows_sys::Win32::System::Threading::WaitForSingleObject(wake.handle(), 5000);
            }
        }
        worker.drain();
        assert_eq!(
            worker.take_open_outcome(),
            Some((OpenAction::Reveal, Err(OpenFailure::Missing)))
        );
        assert!(!worker.open.pending);
        assert!(worker.open.action.is_none());
    }

    #[test]
    fn completed_icons_are_bounded_to_the_cache_limit() {
        let (requests, _request_rx) = mpsc::sync_channel(1);
        let (result_tx, results) = mpsc::channel();
        let mut worker = IconWorker {
            requests,
            results,
            pending: BTreeSet::new(),
            cached: BTreeMap::new(),
            age: VecDeque::new(),
            open: OpenState::default(),
        };
        for number in 0..=MAX_CACHED {
            result_tx
                .send(ResultIcon {
                    path: path(number),
                    icon: None,
                    availability: Availability::Unknown,
                })
                .unwrap();
        }
        assert!(worker.drain());
        assert_eq!(worker.cached.len(), MAX_CACHED);
        assert!(!worker.cached.contains_key(&path(0)));
        assert!(worker.cached.contains_key(&path(MAX_CACHED)));
    }

    #[test]
    fn outstanding_limit_includes_results_waiting_for_the_ui() {
        let (requests, request_rx) = mpsc::sync_channel(MAX_QUEUED);
        let (result_tx, results) = mpsc::sync_channel(MAX_OUTSTANDING);
        let mut worker = IconWorker {
            requests,
            results,
            pending: BTreeSet::new(),
            cached: BTreeMap::new(),
            age: VecDeque::new(),
            open: OpenState::default(),
        };
        for number in 0..MAX_OUTSTANDING {
            let target = path(number);
            worker.get_or_request(&target);
            let WorkItem::Icon(requested) = request_rx.try_recv().unwrap() else {
                panic!("expected icon request");
            };
            result_tx
                .try_send(ResultIcon {
                    path: requested,
                    icon: None,
                    availability: Availability::File,
                })
                .unwrap();
        }
        // The input queue is empty, but all completed work is still outstanding.
        let extra = path(MAX_OUTSTANDING);
        worker.get_or_request(&extra);
        assert_eq!(worker.pending.len(), MAX_OUTSTANDING);
        assert!(request_rx.try_recv().is_err());
        assert!(worker.drain());
        assert!(worker.pending.is_empty());
        worker.get_or_request(&extra);
        assert_eq!(request_rx.try_recv().unwrap(), WorkItem::Icon(extra));
    }

    #[test]
    fn full_request_queue_can_retry_after_capacity_returns() {
        let (requests, request_rx) = mpsc::sync_channel(1);
        let (_result_tx, results) = mpsc::sync_channel(1);
        let mut worker = IconWorker {
            requests,
            results,
            pending: BTreeSet::new(),
            cached: BTreeMap::new(),
            age: VecDeque::new(),
            open: OpenState::default(),
        };
        worker.get_or_request(&path(0));
        worker.get_or_request(&path(1));
        assert!(!worker.pending.contains(&path(1)));
        assert_eq!(request_rx.try_recv().unwrap(), WorkItem::Icon(path(0)));
        worker.get_or_request(&path(1));
        assert_eq!(request_rx.try_recv().unwrap(), WorkItem::Icon(path(1)));
        assert!(worker.pending.contains(&path(1)));
        drop(request_rx);
        worker.get_or_request(&path(2));
        assert!(!worker.pending.contains(&path(2)));
    }

    #[test]
    fn stale_visible_entry_rechecks_once_and_preserves_old_view_until_result() {
        let (requests, request_rx) = mpsc::sync_channel(2);
        let (result_tx, results) = mpsc::channel();
        let mut worker = IconWorker {
            requests,
            results,
            pending: BTreeSet::new(),
            cached: BTreeMap::new(),
            age: VecDeque::new(),
            open: OpenState::default(),
        };
        let target = path(9);
        worker.cached.insert(
            target.clone(),
            CachedIcon {
                icon: None,
                availability: Availability::File,
                checked_at: Instant::now() - REFRESH_AFTER,
            },
        );
        worker.age.push_back(target.clone());
        assert_eq!(
            worker.get_or_request(&target).availability,
            Availability::File
        );
        assert_eq!(
            worker.get_or_request(&target).availability,
            Availability::File
        );
        assert_eq!(
            request_rx.try_recv().unwrap(),
            WorkItem::Icon(target.clone())
        );
        assert!(request_rx.try_recv().is_err());
        result_tx
            .send(ResultIcon {
                path: target.clone(),
                icon: None,
                availability: Availability::Missing,
            })
            .unwrap();
        assert!(worker.drain());
        assert_eq!(
            worker.get_or_request(&target).availability,
            Availability::Missing
        );
        assert!(request_rx.try_recv().is_err());
    }

    #[test]
    fn missing_path_is_classified_without_an_icon() {
        let missing = path(usize::MAX);
        let (icon, availability) = load_icon(&missing);
        assert!(icon.is_none());
        assert_eq!(availability, Availability::Missing);
    }

    #[test]
    fn bitmap_accounting_uses_stride_and_checks_invalid_dimensions() {
        assert_eq!(bitmap_payload_bytes(64, 16), Some(1024));
        assert_eq!(bitmap_payload_bytes(2, 32), Some(64));
        assert_eq!(bitmap_payload_bytes(0, 16), None);
        assert_eq!(bitmap_payload_bytes(64, -1), None);
    }

    #[test]
    fn real_shell_icon_has_accountable_mask_and_color_bitmaps() {
        let _apartment = Apartment::initialize().expect("test STA initialization");
        let executable = std::env::current_exe().unwrap();
        let target = NativePath::from_os_str(executable.as_os_str()).unwrap();
        let (icon, availability) = load_icon(&target);
        assert_eq!(availability, Availability::File);
        let icon = icon.expect("system executable icon");
        assert!(icon.bitmap_bytes > 0 && icon.bitmap_bytes <= MAX_CACHE_BYTES);
        for _ in 0..32 {
            assert_eq!(icon_bitmap_bytes(icon.raw()), Some(icon.bitmap_bytes));
        }
    }

    #[test]
    fn cache_byte_limit_evicts_long_paths_before_entry_limit() {
        let (requests, _request_rx) = mpsc::sync_channel(1);
        let (result_tx, results) = mpsc::channel();
        let mut worker = IconWorker {
            requests,
            results,
            pending: BTreeSet::new(),
            cached: BTreeMap::new(),
            age: VecDeque::new(),
            open: OpenState::default(),
        };
        let target = |number| {
            NativePath::from_utf16(
                format!("C:\\{}-{number}.txt", "x".repeat(32_000))
                    .encode_utf16()
                    .collect(),
            )
            .unwrap()
        };
        for number in 0..100 {
            result_tx
                .send(ResultIcon {
                    path: target(number),
                    icon: None,
                    availability: Availability::Unknown,
                })
                .unwrap();
        }
        assert!(worker.drain());
        assert!(worker.cached.len() < 100);
        assert!(worker.accounted_cache_bytes() <= MAX_CACHE_BYTES);
        assert!(!worker.cached.contains_key(&target(0)));
        assert!(worker.cached.contains_key(&target(99)));
        let bytes = worker.accounted_cache_bytes();
        let count = worker.age.len();
        result_tx
            .send(ResultIcon {
                path: target(99),
                icon: None,
                availability: Availability::Missing,
            })
            .unwrap();
        worker.drain();
        assert_eq!(worker.accounted_cache_bytes(), bytes);
        assert_eq!(worker.age.len(), count);
    }

    #[test]
    fn missing_and_type_changed_targets_are_invalid_but_pending_is_not() {
        let view = |availability| IconView {
            handle: None,
            availability,
        };
        assert!(view(Availability::Missing).is_invalid(ReferenceKind::File));
        assert!(view(Availability::Directory).is_invalid(ReferenceKind::Shortcut));
        assert!(view(Availability::File).is_invalid(ReferenceKind::Directory));
        assert!(!view(Availability::Pending).is_invalid(ReferenceKind::Directory));
        assert!(!view(Availability::Unknown).is_invalid(ReferenceKind::File));
    }
}
