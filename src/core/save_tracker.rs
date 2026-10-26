#[derive(Debug)]
pub struct SaveTracker {
    next_revision: u64,
    in_flight: Option<u64>,
    last_saved: Option<u64>,
}

impl Default for SaveTracker {
    fn default() -> Self {
        Self {
            next_revision: 1,
            in_flight: None,
            last_saved: None,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum BeginSaveError {
    Busy,
    Exhausted,
}

impl SaveTracker {
    pub fn begin(&mut self) -> Result<u64, BeginSaveError> {
        if self.in_flight.is_some() {
            return Err(BeginSaveError::Busy);
        }

        let revision = self.next_revision;
        let next_revision = revision.checked_add(1).ok_or(BeginSaveError::Exhausted)?;

        self.next_revision = next_revision;
        self.in_flight = Some(revision);
        Ok(revision)
    }

    pub fn finish(&mut self, revision: u64, success: bool) -> bool {
        if self.in_flight != Some(revision) {
            return false;
        }

        self.in_flight = None;
        if success {
            self.last_saved = Some(revision);
        }
        true
    }

    pub fn is_saving(&self) -> bool {
        self.in_flight.is_some()
    }

    pub fn last_saved(&self) -> Option<u64> {
        self.last_saved
    }
}

#[cfg(test)]
mod tests {
    use super::{BeginSaveError, SaveTracker};

    #[test]
    fn starts_with_empty_state_and_first_revision_one() {
        let mut tracker = SaveTracker::default();

        assert!(!tracker.is_saving());
        assert_eq!(tracker.last_saved(), None);
        assert_eq!(tracker.begin(), Ok(1));
        assert!(tracker.is_saving());
    }

    #[test]
    fn busy_begin_does_not_consume_a_revision() {
        let mut tracker = SaveTracker::default();

        assert_eq!(tracker.begin(), Ok(1));
        assert_eq!(tracker.begin(), Err(BeginSaveError::Busy));
        assert!(tracker.finish(1, false));
        assert_eq!(tracker.begin(), Ok(2));
    }

    #[test]
    fn successful_finish_updates_last_saved() {
        let mut tracker = SaveTracker::default();
        let revision = tracker.begin().unwrap();

        assert!(tracker.finish(revision, true));
        assert!(!tracker.is_saving());
        assert_eq!(tracker.last_saved(), Some(revision));
    }

    #[test]
    fn failed_finish_keeps_last_saved_and_allows_retry() {
        let mut tracker = SaveTracker::default();
        let first = tracker.begin().unwrap();
        assert!(tracker.finish(first, true));

        let failed = tracker.begin().unwrap();
        assert!(tracker.finish(failed, false));
        assert_eq!(tracker.last_saved(), Some(first));
        assert_eq!(tracker.begin(), Ok(failed + 1));
    }

    #[test]
    fn stale_or_repeated_result_cannot_clear_a_new_request() {
        let mut tracker = SaveTracker::default();
        let old = tracker.begin().unwrap();
        assert!(tracker.finish(old, false));
        let current = tracker.begin().unwrap();

        assert!(!tracker.finish(old, true));
        assert!(tracker.is_saving());
        assert_eq!(tracker.last_saved(), None);
        assert!(tracker.finish(current, true));
        assert!(!tracker.finish(current, false));
        assert_eq!(tracker.last_saved(), Some(current));
    }

    #[test]
    fn max_revision_is_rejected_without_mutation() {
        let mut tracker = SaveTracker {
            next_revision: u64::MAX,
            in_flight: None,
            last_saved: None,
        };

        assert_eq!(tracker.begin(), Err(BeginSaveError::Exhausted));
        assert!(!tracker.is_saving());
        assert_eq!(tracker.last_saved(), None);
        assert_eq!(tracker.next_revision, u64::MAX);
    }
}
