//! Arm/disarm state for pull-request background work, and the single-instance
//! guard for its long-lived poller.
//!
//! This replaces a bare `Arc<AtomicBool>` kill switch, which could not do the
//! job on its own. The poller reads the switch once per
//! [`PR_SYNC_SLICE_SECS`](super::PR_SYNC_SLICE_SECS), so the documented "turn it
//! off and on again" workflow lands inside that window: the running poller never
//! observes the `false`, the enable spawns a second poller, and both then poll
//! `gh` forever. Repeat the workflow and the traffic multiplies again.
//!
//! So arming, disarming, and the poller's own "should I keep going" check all
//! happen under ONE lock, and arming spawns only when no loop is live. Deciding
//! to stop and releasing the slot are the same critical section, which is what
//! closes the opposite race: an enable that arrives while a poller is on its way
//! out either sees the slot still taken (and the poller then sees the re-enable
//! and keeps running) or sees it free (and spawns a replacement). It can never
//! see a live poller that is about to exit and so leave zero.

use std::sync::{Mutex, MutexGuard};

/// Shared control for pull-request background work. Held behind an `Arc` by the
/// engine and by the poller thread.
#[derive(Debug, Default)]
pub struct PrSyncControl {
    state: Mutex<PrSyncState>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PrSyncState {
    /// Whether pull-request work should be running at all. The poller reads it
    /// once per sleep slice; a `false` ends the loop.
    enabled: bool,
    /// Whether a poller loop is live. Only ever true for ONE loop at a time.
    poller_live: bool,
    /// How many poller threads have been started in this process.
    poller_starts: u64,
    /// How many one-shot refreshes have been dispatched in this process.
    refresh_starts: u64,
}

impl PrSyncControl {
    fn lock(&self) -> MutexGuard<'_, PrSyncState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Arm pull-request work, and claim the poller slot if it is free.
    ///
    /// Returns whether the caller must actually start a poller thread. `false`
    /// means one is already live and has just been told to keep going, so a
    /// second must NOT be spawned.
    #[must_use]
    pub fn arm(&self) -> bool {
        let mut state = self.lock();
        state.enabled = true;
        if state.poller_live {
            return false;
        }
        state.poller_live = true;
        state.poller_starts += 1;
        true
    }

    /// Disarm pull-request work. The live poller (if any) ends its loop on its
    /// next slice; the slot is released there, not here, so a re-arm inside that
    /// window is handed the poller that is still running rather than a new one.
    pub fn disarm(&self) {
        self.lock().enabled = false;
    }

    /// The poller's own per-slice check. Returns `false` once, releasing the
    /// slot in the same critical section, when the loop must end.
    pub fn poller_should_continue(&self) -> bool {
        let mut state = self.lock();
        if state.enabled {
            return true;
        }
        state.poller_live = false;
        false
    }

    /// Release the poller slot for a reason that is not the kill switch: the
    /// event receiver was dropped, or the thread never started at all. Without
    /// this the slot would stay claimed and pull-request polling would be dead
    /// for the rest of the process.
    pub fn poller_stopped(&self) {
        self.lock().poller_live = false;
    }

    /// Record that a one-shot refresh was dispatched.
    pub fn note_refresh(&self) {
        self.lock().refresh_starts += 1;
    }

    /// Whether pull-request work is armed.
    pub fn is_armed(&self) -> bool {
        self.lock().enabled
    }

    /// Whether a poller loop currently holds the slot.
    pub fn poller_is_live(&self) -> bool {
        self.lock().poller_live
    }

    /// How many poller threads have been started. The number a lifecycle test
    /// counts: "a second permanent poller was created" is exactly this going up
    /// twice across one enable.
    pub fn poller_starts(&self) -> u64 {
        self.lock().poller_starts
    }

    /// How many one-shot refreshes have been dispatched. Counted for the same
    /// reason: acting on a stale status used to produce two per enable.
    pub fn refresh_starts(&self) -> u64 {
        self.lock().refresh_starts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arming_twice_claims_the_poller_slot_once() {
        let control = PrSyncControl::default();
        assert!(control.arm(), "the first arm must spawn a poller");
        assert!(!control.arm(), "a second arm must not spawn another poller");
        assert_eq!(control.poller_starts(), 1);
        assert!(control.is_armed());
    }

    #[test]
    fn a_re_arm_inside_the_pollers_read_window_neither_duplicates_nor_strands() {
        // The exact sequence a bare AtomicBool got wrong: disarm and re-arm
        // both land before the sleeping poller reads the flag.
        let control = PrSyncControl::default();
        assert!(control.arm());
        control.disarm();
        assert!(
            !control.arm(),
            "the poller that is still live must be reused, not duplicated",
        );
        assert!(
            control.poller_should_continue(),
            "and it must see the re-enable and keep running, not exit into nothing",
        );
        assert_eq!(control.poller_starts(), 1);
    }

    #[test]
    fn a_poller_that_has_ended_releases_the_slot_for_a_replacement() {
        let control = PrSyncControl::default();
        assert!(control.arm());
        control.disarm();
        assert!(!control.poller_should_continue(), "the loop ends");
        assert!(!control.poller_is_live());
        assert!(control.arm(), "a later enable starts a fresh poller");
        assert_eq!(control.poller_starts(), 2);
    }
}
