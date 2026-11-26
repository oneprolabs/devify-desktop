use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const RECHECK_INTERVAL: Duration = Duration::from_secs(30);
const RECHECK_WINDOW: Duration = Duration::from_secs(240);

#[derive(Clone, Copy)]
enum WakeCheck {
    Initial,
    FollowUp { next: Instant, deadline: Instant },
}

#[derive(Clone, Copy)]
struct ActiveCheck {
    deadline: Option<Instant>,
    found_messages: bool,
}

#[derive(Clone, Copy)]
pub(super) struct WakeScan {
    pending: Option<WakeCheck>,
    active: Option<ActiveCheck>,
}

impl WakeScan {
    pub(super) fn pending() -> Self {
        Self {
            pending: Some(WakeCheck::Initial),
            active: None,
        }
    }

    pub(super) fn rearm(&mut self) {
        self.pending = Some(WakeCheck::Initial);
    }

    pub(super) fn is_due(&self, now: Instant) -> bool {
        match self.pending {
            Some(WakeCheck::Initial) => true,
            Some(WakeCheck::FollowUp { next, deadline }) => next <= now && now < deadline,
            None => false,
        }
    }

    pub(super) fn keep(&self, now: Instant) -> bool {
        self.active.is_some()
            || match self.pending {
                Some(WakeCheck::Initial) => true,
                Some(WakeCheck::FollowUp { deadline, .. }) => now < deadline,
                None => false,
            }
    }

    pub(super) fn start(&mut self, now: Instant, regular_due: bool) {
        self.active = match self.pending {
            Some(WakeCheck::Initial) => Some(ActiveCheck {
                deadline: None,
                found_messages: false,
            }),
            Some(WakeCheck::FollowUp { deadline, .. }) if !regular_due && self.is_due(now) => {
                Some(ActiveCheck {
                    deadline: Some(deadline),
                    found_messages: false,
                })
            }
            _ => None,
        };
        if self.active.is_some() || regular_due {
            self.pending = None;
        }
    }

    pub(super) fn is_scanning(&self) -> bool {
        self.active.is_some()
    }

    pub(super) fn updates_regular_schedule(&self) -> bool {
        self.active.is_none_or(|scan| scan.deadline.is_none())
    }

    pub(super) fn commit_chunk(&mut self, now: Instant, complete: bool, found_messages: bool) {
        let Some(mut scan) = self.active.take() else {
            return;
        };
        scan.found_messages |= found_messages;
        if !complete {
            self.active = Some(scan);
            return;
        }
        if self.pending.is_some() || scan.found_messages {
            return;
        }
        let deadline = scan.deadline.unwrap_or(now + RECHECK_WINDOW);
        let next = now + RECHECK_INTERVAL;
        if next < deadline {
            self.pending = Some(WakeCheck::FollowUp { next, deadline });
        }
    }
}

static WOKE: AtomicBool = AtomicBool::new(false);

pub(super) fn take() -> bool {
    WOKE.swap(false, Ordering::Relaxed)
}

#[cfg(target_os = "macos")]
pub(super) fn observe() {
    use block2::RcBlock;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};

    let workspace = NSWorkspace::sharedWorkspace();
    let center = workspace.notificationCenter();
    let callback = RcBlock::new(|_| {
        WOKE.store(true, Ordering::Relaxed);
        super::TASK_WAKE.notify_one();
    });
    // The center owns the observer for the app's lifetime. The callback accesses only
    // thread-safe statics and may run on the notification's posting thread.
    unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceDidWakeNotification),
            None,
            None,
            &callback,
        );
    }
}

#[cfg(test)]
mod schedule_tests {
    use super::*;

    #[test]
    fn empty_wake_scan_rechecks_for_late_messages_without_moving_the_regular_schedule() {
        let start = Instant::now();
        let mut wake = WakeScan::pending();

        assert!(wake.is_due(start));
        wake.start(start, false);
        assert!(wake.updates_regular_schedule());
        wake.commit_chunk(start, true, false);
        assert!(!wake.is_due(start + Duration::from_secs(29)));

        // The reported WeChat write arrived more than two minutes after wake.
        for seconds in [30, 60, 90, 120] {
            let now = start + Duration::from_secs(seconds);
            assert!(wake.is_due(now));
            wake.start(now, false);
            assert!(!wake.updates_regular_schedule());
            wake.commit_chunk(now, true, false);
        }
        let now = start + Duration::from_secs(150);
        assert!(wake.is_due(now));
        wake.start(now, false);
        wake.commit_chunk(now, false, true);
        assert!(!wake.updates_regular_schedule());
        wake.commit_chunk(now, true, false);
        assert!(!wake.is_due(start + Duration::from_secs(180)));
    }

    #[test]
    fn follow_up_expires_and_yields_to_a_regular_run_that_is_already_due() {
        let start = Instant::now();
        let mut wake = WakeScan::pending();
        wake.start(start, false);
        wake.commit_chunk(start, true, false);

        assert!(!wake.is_due(start + Duration::from_secs(240)));
        let regular_due = start + Duration::from_secs(60);
        assert!(wake.is_due(regular_due));
        wake.start(regular_due, true);
        assert!(wake.updates_regular_schedule());
        wake.commit_chunk(regular_due, true, false);
        assert!(!wake.is_due(regular_due + Duration::from_secs(30)));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};

    #[test]
    fn workspace_wake_notification_is_consumed_once() {
        observe();
        let workspace = NSWorkspace::sharedWorkspace();
        let center = workspace.notificationCenter();

        // Exercise the registered AppKit callback without putting the host to sleep.
        unsafe {
            center.postNotificationName_object(NSWorkspaceDidWakeNotification, Some(&workspace));
        }
        assert!(take());
        assert!(!take());
    }
}
