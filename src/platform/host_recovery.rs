//! Pure state machine for recovering the host window identity.
//!
//! This module deliberately has no platform, thread, timer, or I/O concerns. The caller
//! owns scheduling probes and retry timers and feeds their results back into `Recovery`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostIdentity {
    pub hwnd: usize,
    pub process_id: u32,
}

impl HostIdentity {
    fn is_valid(self) -> bool {
        self.hwnd != 0 && self.process_id != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryAction {
    Probe {
        attempt: u8,
        generation: u64,
    },
    /// A validated identity was accepted, not evidence that SetParent succeeded.
    Attached(HostIdentity),
    Waiting,
    Detached,
    Exhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Detached,
    Probing,
    Waiting,
    Attached,
    Exhausted,
}

pub struct Recovery {
    max_attempts: u8,
    attempts: u8,
    generation: u64,
    state: State,
    current: Option<HostIdentity>,
}

impl Recovery {
    pub fn new(max_attempts: u8) -> Self {
        Self {
            max_attempts: max_attempts.max(1),
            attempts: 0,
            generation: 0,
            state: State::Detached,
            current: None,
        }
    }

    /// Invalidates the current host and starts a fresh probe budget.
    pub fn on_host_lost(&mut self) -> RecoveryAction {
        self.start_recovery()
    }

    /// Accepts a result only from the currently active probe generation.
    pub fn on_probe_result(
        &mut self,
        generation: u64,
        identity: Option<HostIdentity>,
    ) -> RecoveryAction {
        if generation != self.generation || self.state != State::Probing {
            return RecoveryAction::Detached;
        }

        if let Some(identity) = identity.filter(|identity| identity.is_valid()) {
            self.current = Some(identity);
            self.state = State::Attached;
            return RecoveryAction::Attached(identity);
        }

        if self.attempts < self.max_attempts {
            self.state = State::Waiting;
            RecoveryAction::Waiting
        } else {
            self.state = State::Exhausted;
            RecoveryAction::Exhausted
        }
    }

    /// Issues exactly one next probe, and only after a failed probe entered `Waiting`.
    pub fn on_retry_timer(&mut self) -> RecoveryAction {
        if self.state != State::Waiting {
            return RecoveryAction::Detached;
        }

        self.attempts += 1;
        // Each request gets its own token, including retries in the same recovery.
        // Otherwise a duplicated/late result from attempt 1 could satisfy attempt 2.
        self.generation = self.generation.wrapping_add(1);
        self.state = State::Probing;
        RecoveryAction::Probe {
            attempt: self.attempts,
            generation: self.generation,
        }
    }

    /// A shell hint invalidates the old host; the hint itself never attaches a host.
    pub fn on_shell_hint(&mut self) -> RecoveryAction {
        self.start_recovery()
    }

    pub fn current(&self) -> Option<HostIdentity> {
        self.current
    }

    fn start_recovery(&mut self) -> RecoveryAction {
        self.current = None;
        self.attempts = 1;
        self.generation = self.generation.wrapping_add(1);
        self.state = State::Probing;
        RecoveryAction::Probe {
            attempt: 1,
            generation: self.generation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(hwnd: usize, process_id: u32) -> HostIdentity {
        HostIdentity { hwnd, process_id }
    }

    #[test]
    fn late_generation_is_ignored() {
        let mut recovery = Recovery::new(2);
        let first = recovery.on_host_lost();
        let first_generation = match first {
            RecoveryAction::Probe { generation, .. } => generation,
            action => panic!("unexpected action: {action:?}"),
        };
        let second = recovery.on_shell_hint();
        let second_generation = match second {
            RecoveryAction::Probe { generation, .. } => generation,
            action => panic!("unexpected action: {action:?}"),
        };

        assert_ne!(first_generation, second_generation);
        assert_eq!(
            recovery.on_probe_result(first_generation, Some(identity(1, 2))),
            RecoveryAction::Detached
        );
        assert_eq!(recovery.current(), None);
        assert_eq!(
            recovery.on_probe_result(second_generation, Some(identity(3, 4))),
            RecoveryAction::Attached(identity(3, 4))
        );
    }

    #[test]
    fn three_attempts_exhaust() {
        let mut recovery = Recovery::new(3);
        let mut action = recovery.on_host_lost();
        for attempt in 1..=3 {
            assert_eq!(
                action,
                RecoveryAction::Probe {
                    attempt,
                    generation: match action {
                        RecoveryAction::Probe { generation, .. } => generation,
                        _ => unreachable!(),
                    },
                }
            );
            let generation = match action {
                RecoveryAction::Probe { generation, .. } => generation,
                _ => unreachable!(),
            };
            action = recovery.on_probe_result(generation, None);
            if attempt < 3 {
                assert_eq!(action, RecoveryAction::Waiting);
                action = recovery.on_retry_timer();
            } else {
                assert_eq!(action, RecoveryAction::Exhausted);
            }
        }
        assert_eq!(recovery.current(), None);
    }

    #[test]
    fn restart_replaces_identity() {
        let mut recovery = Recovery::new(1);
        let first_generation = match recovery.on_host_lost() {
            RecoveryAction::Probe { generation, .. } => generation,
            _ => unreachable!(),
        };
        assert_eq!(
            recovery.on_probe_result(first_generation, Some(identity(10, 20))),
            RecoveryAction::Attached(identity(10, 20))
        );

        let second_generation = match recovery.on_host_lost() {
            RecoveryAction::Probe { generation, .. } => generation,
            _ => unreachable!(),
        };
        assert_eq!(recovery.current(), None);
        assert_eq!(
            recovery.on_probe_result(second_generation, Some(identity(30, 40))),
            RecoveryAction::Attached(identity(30, 40))
        );
        assert_eq!(recovery.current(), Some(identity(30, 40)));
    }

    #[test]
    fn repeated_timer_does_not_increase_budget() {
        let mut recovery = Recovery::new(2);
        let generation = match recovery.on_host_lost() {
            RecoveryAction::Probe { generation, .. } => generation,
            _ => unreachable!(),
        };
        assert_eq!(recovery.on_retry_timer(), RecoveryAction::Detached);
        assert_eq!(
            recovery.on_probe_result(generation, None),
            RecoveryAction::Waiting
        );
        assert_eq!(
            recovery.on_retry_timer(),
            RecoveryAction::Probe {
                attempt: 2,
                generation: generation + 1
            }
        );
        assert_eq!(recovery.on_retry_timer(), RecoveryAction::Detached);
    }

    #[test]
    fn zero_identity_is_rejected() {
        let mut recovery = Recovery::new(1);
        let generation = match recovery.on_host_lost() {
            RecoveryAction::Probe { generation, .. } => generation,
            _ => unreachable!(),
        };
        assert_eq!(
            recovery.on_probe_result(generation, Some(identity(0, 7))),
            RecoveryAction::Exhausted
        );
        assert_eq!(recovery.current(), None);
    }

    #[test]
    fn zero_budget_is_one_attempt() {
        let mut recovery = Recovery::new(0);
        let generation = match recovery.on_host_lost() {
            RecoveryAction::Probe {
                attempt,
                generation,
            } => {
                assert_eq!(attempt, 1);
                generation
            }
            _ => unreachable!(),
        };
        assert_eq!(
            recovery.on_probe_result(generation, None),
            RecoveryAction::Exhausted
        );
    }

    #[test]
    fn previous_attempt_cannot_satisfy_retry() {
        let mut recovery = Recovery::new(2);
        let RecoveryAction::Probe {
            generation: old, ..
        } = recovery.on_host_lost()
        else {
            panic!()
        };
        assert_eq!(recovery.on_probe_result(old, None), RecoveryAction::Waiting);
        let RecoveryAction::Probe {
            generation: fresh, ..
        } = recovery.on_retry_timer()
        else {
            panic!()
        };
        assert_ne!(old, fresh);
        assert_eq!(
            recovery.on_probe_result(old, Some(identity(1, 2))),
            RecoveryAction::Detached
        );
        assert_eq!(recovery.current(), None);
        assert_eq!(
            recovery.on_probe_result(fresh, Some(identity(3, 4))),
            RecoveryAction::Attached(identity(3, 4))
        );
        assert_eq!(
            recovery.on_probe_result(old, None),
            RecoveryAction::Detached
        );
        assert_eq!(recovery.current(), Some(identity(3, 4)));
    }

    #[test]
    fn zero_process_is_rejected() {
        let mut recovery = Recovery::new(1);
        let RecoveryAction::Probe { generation, .. } = recovery.on_host_lost() else {
            panic!()
        };
        assert_eq!(
            recovery.on_probe_result(generation, Some(identity(7, 0))),
            RecoveryAction::Exhausted
        );
    }
}
