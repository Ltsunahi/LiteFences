use std::cell::Cell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    WaitWorker,
    WaitInput,
    WaitOther,
    SessionChanged,
    IconsChanged,
    Message,
    Paint,
    RefreshTimer,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub wait_worker: u64,
    pub wait_input: u64,
    pub wait_other: u64,
    pub session_changes: u64,
    pub icon_changes: u64,
    pub messages: u64,
    pub paints: u64,
    pub refresh_timers: u64,
    pub cross_thread_messages: u64,
    pub cross_thread_getobject: u64,
}

pub struct Diagnostics {
    enabled: Cell<bool>,
    snapshot: Cell<Snapshot>,
    message_ids: [Cell<u64>; 1024],
    high_message_ids: Cell<u64>,
}

impl Default for Diagnostics {
    fn default() -> Self {
        Self {
            enabled: Cell::new(false),
            snapshot: Cell::new(Snapshot::default()),
            message_ids: std::array::from_fn(|_| Cell::new(0)),
            high_message_ids: Cell::new(0),
        }
    }
}

impl Diagnostics {
    pub fn is_enabled(&self) -> bool {
        self.enabled.get()
    }

    // No sender identity or message parameters are retained. This identifies
    // another thread, not necessarily another process.
    pub fn record_origin(&self, message: u32, cross_thread: bool) {
        if !self.enabled.get() || !cross_thread {
            return;
        }
        let mut snapshot = self.snapshot.get();
        snapshot.cross_thread_messages = snapshot.cross_thread_messages.saturating_add(1);
        if message == 0x003d {
            snapshot.cross_thread_getobject = snapshot.cross_thread_getobject.saturating_add(1);
        }
        self.snapshot.set(snapshot);
    }
    pub fn enable(&self) {
        self.enabled.set(true);
    }

    pub fn record(&self, event: Event) {
        if !self.enabled.get() {
            return;
        }

        let mut snapshot = self.snapshot.get();
        let counter = match event {
            Event::WaitWorker => &mut snapshot.wait_worker,
            Event::WaitInput => &mut snapshot.wait_input,
            Event::WaitOther => &mut snapshot.wait_other,
            Event::SessionChanged => &mut snapshot.session_changes,
            Event::IconsChanged => &mut snapshot.icon_changes,
            Event::Message => &mut snapshot.messages,
            Event::Paint => &mut snapshot.paints,
            Event::RefreshTimer => &mut snapshot.refresh_timers,
        };
        *counter = counter.saturating_add(1);
        self.snapshot.set(snapshot);
    }

    pub fn record_message(&self, message: u32) {
        if !self.enabled.get() {
            return;
        }

        self.record(Event::Message);
        if let Some(counter) = self.message_ids.get(message as usize) {
            counter.set(counter.get().saturating_add(1));
        } else {
            self.high_message_ids
                .set(self.high_message_ids.get().saturating_add(1));
        }
    }

    pub fn message_histogram(&self) -> (Vec<(u32, u64)>, u64) {
        let message_ids = self
            .message_ids
            .iter()
            .enumerate()
            .filter_map(|(message, count)| {
                let count = count.get();
                (count != 0).then_some((message as u32, count))
            })
            .collect();
        (message_ids, self.high_message_ids.get())
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.get()
    }
}

#[cfg(test)]
mod tests {
    use super::{Diagnostics, Event, Snapshot};

    #[test]
    fn disabled_recording_has_no_effect() {
        let diagnostics = Diagnostics::default();

        diagnostics.record(Event::WaitWorker);
        diagnostics.record(Event::Paint);

        assert_eq!(diagnostics.snapshot(), Snapshot::default());
    }

    #[test]
    fn enabled_events_increment_their_counters() {
        let diagnostics = Diagnostics::default();
        diagnostics.enable();

        diagnostics.record(Event::WaitWorker);
        diagnostics.record(Event::WaitInput);
        diagnostics.record(Event::WaitOther);
        diagnostics.record(Event::SessionChanged);
        diagnostics.record(Event::IconsChanged);
        diagnostics.record(Event::Message);
        diagnostics.record(Event::Paint);
        diagnostics.record(Event::RefreshTimer);

        assert_eq!(
            diagnostics.snapshot(),
            Snapshot {
                wait_worker: 1,
                wait_input: 1,
                wait_other: 1,
                session_changes: 1,
                icon_changes: 1,
                messages: 1,
                paints: 1,
                refresh_timers: 1,
                ..Snapshot::default()
            }
        );
    }

    #[test]
    fn counters_saturate_at_u64_max() {
        let diagnostics = Diagnostics::default();
        diagnostics.enable();
        diagnostics.snapshot.set(Snapshot {
            wait_worker: u64::MAX,
            ..Snapshot::default()
        });

        diagnostics.record(Event::WaitWorker);

        assert_eq!(diagnostics.snapshot().wait_worker, u64::MAX);
    }

    #[test]
    fn disabled_message_recording_has_no_effect() {
        let diagnostics = Diagnostics::default();

        diagnostics.record_message(1);
        diagnostics.record_message(1024);

        assert_eq!(diagnostics.snapshot(), Snapshot::default());
        assert_eq!(diagnostics.message_histogram(), (Vec::new(), 0));
    }

    #[test]
    fn message_histogram_tracks_low_and_high_ids() {
        let diagnostics = Diagnostics::default();
        diagnostics.enable();

        diagnostics.record_message(0);
        diagnostics.record_message(1023);
        diagnostics.record_message(1023);
        diagnostics.record_message(1024);
        diagnostics.record_message(u32::MAX);

        let (histogram, high) = diagnostics.message_histogram();
        assert_eq!(histogram, vec![(0, 1), (1023, 2)]);
        assert_eq!(high, 2);
        assert_eq!(
            histogram.iter().map(|(_, count)| count).sum::<u64>() + high,
            diagnostics.snapshot().messages
        );
    }

    #[test]
    fn message_histogram_counters_saturate_at_u64_max() {
        let diagnostics = Diagnostics::default();
        diagnostics.enable();
        diagnostics.message_ids[1023].set(u64::MAX);
        diagnostics.high_message_ids.set(u64::MAX);

        diagnostics.record_message(1023);
        diagnostics.record_message(1024);

        assert_eq!(diagnostics.message_ids[1023].get(), u64::MAX);
        assert_eq!(diagnostics.high_message_ids.get(), u64::MAX);
        assert_eq!(diagnostics.snapshot().messages, 2);
    }

    #[test]
    fn origin_is_disabled_by_default_and_only_counts_other_threads() {
        let diagnostics = Diagnostics::default();
        diagnostics.record_origin(0x003d, true);
        assert_eq!(diagnostics.snapshot(), Snapshot::default());
        diagnostics.enable();
        diagnostics.record_origin(0x003d, false);
        diagnostics.record_origin(0x000f, true);
        diagnostics.record_origin(0x003d, true);
        assert_eq!(diagnostics.snapshot().cross_thread_messages, 2);
        assert_eq!(diagnostics.snapshot().cross_thread_getobject, 1);
        diagnostics.snapshot.set(Snapshot {
            cross_thread_messages: u64::MAX,
            cross_thread_getobject: u64::MAX,
            ..Snapshot::default()
        });
        diagnostics.record_origin(0x003d, true);
        assert_eq!(diagnostics.snapshot().cross_thread_messages, u64::MAX);
        assert_eq!(diagnostics.snapshot().cross_thread_getobject, u64::MAX);
    }
}
