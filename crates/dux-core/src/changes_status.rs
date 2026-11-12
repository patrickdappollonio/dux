//! What dux says when it cannot read an agent's changed files, and how that
//! warning goes away again.
//!
//! Two pollers ask the same question: the terminal UI's own changed-files
//! refresh and the web's changes service. They used to answer differently, the
//! web raising a warning through its own side door and the terminal UI saying
//! nothing at all, so the same failing repository produced two different
//! screens. The streak, the threshold, the key and the sentence live here, and
//! the engine owns the one instance both pollers feed.
//!
//! Feeding one tracker from two pollers is deliberately harmless: the streak
//! simply climbs faster, the warning is emitted on the one crossing, and it is
//! withdrawn once.

use std::collections::{HashMap, HashSet};

use crate::engine::StatusUpdate;
use crate::poller_status::PollerNotice;
use crate::statusline::StatusTone;

/// Consecutive failures before a warning is raised.
///
/// git fails transiently all the time (an index lock held by the agent's own
/// commit, a directory mid-rename), and a toast per blip is noise. A streak is
/// the signal that something is actually wrong.
pub const ERROR_WARN_THRESHOLD: u32 = 3;

/// How one changed-files read ended, as far as the failure streak cares: git's
/// own text goes to dux.log and never reaches the streak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOutcome {
    /// The read failed.
    Failed,
    /// The read answered.
    Succeeded,
}

/// What the standing-failure warning says. git's own text is left to dux.log,
/// where every failed read is written: it is the detail someone debugging
/// needs, and it is wrong to call every failure a busy repository.
pub const CHANGED_FILES_STUCK: &str = "dux cannot list the changed files for this agent right now, so the Changes pane may be out of date. It keeps trying, and the details are in dux.log.";

/// The keyed-status key for one agent's changed-files escalation, which the
/// withdrawal names to take the warning down.
pub fn warn_key(session_id: &str) -> String {
    format!("changes-error:{session_id}")
}

/// The per-agent failure streaks, and which agents have actually been warned
/// about.
#[derive(Debug, Default)]
pub struct ChangedFilesFailures {
    streaks: HashMap<String, u32>,
    warned: HashSet<String>,
}

impl ChangedFilesFailures {
    /// Record one failed read. Answers with the keyed warning on the crossing
    /// and with nothing on every other failure, so a blip stays silent and a
    /// standing failure is reported exactly once.
    pub fn record_failure(&mut self, session_id: &str) -> Option<PollerNotice> {
        let streak = self.streaks.entry(session_id.to_string()).or_insert(0);
        *streak += 1;
        if *streak != ERROR_WARN_THRESHOLD {
            return None;
        }
        self.warned.insert(session_id.to_string());
        Some(PollerNotice::Warn(StatusUpdate::keyed(
            warn_key(session_id),
            StatusTone::Warning,
            CHANGED_FILES_STUCK,
        )))
    }

    /// Record one successful read. Answers with the withdrawal of the warning
    /// ONLY when one was actually shown, so a blip that never raised one has
    /// nothing to take down. The list filling in again is the news that it
    /// ended, so nothing is said in its place.
    pub fn record_success(&mut self, session_id: &str) -> Option<PollerNotice> {
        self.streaks.remove(session_id);
        self.warned
            .remove(session_id)
            .then(|| PollerNotice::Withdraw(warn_key(session_id)))
    }

    /// Forget an agent entirely, for a session that is gone.
    pub fn forget(&mut self, session_id: &str) {
        self.streaks.remove(session_id);
        self.warned.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blip_stays_silent_and_a_streak_warns_once() {
        let mut failures = ChangedFilesFailures::default();
        for _ in 1..ERROR_WARN_THRESHOLD {
            assert!(failures.record_failure("s1").is_none());
        }
        let Some(PollerNotice::Warn(warning)) = failures.record_failure("s1") else {
            panic!("the streak crosses the threshold")
        };
        assert_eq!(warning.tone, StatusTone::Warning);
        assert_eq!(warning.key.as_deref(), Some(warn_key("s1").as_str()));
        assert!(warning.message.contains("dux.log"));
        assert!(
            !warning.message.contains("git") && !warning.message.contains("index.lock"),
            "git's text belongs in dux.log, not in the toast: {}",
            warning.message
        );
        assert!(
            failures.record_failure("s1").is_none(),
            "a standing failure is reported once, not once per cycle"
        );
    }

    #[test]
    fn the_warning_is_withdrawn_only_where_it_was_shown() {
        let mut failures = ChangedFilesFailures::default();
        assert!(failures.record_failure("s1").is_none());
        assert!(
            failures.record_success("s1").is_none(),
            "a blip that never warned has nothing to take down"
        );

        for _ in 0..ERROR_WARN_THRESHOLD {
            failures.record_failure("s2");
        }
        assert!(
            matches!(
                failures.record_success("s2"),
                Some(PollerNotice::Withdraw(key)) if key == warn_key("s2")
            ),
            "a shown warning is taken down quietly, on its own key"
        );
        assert!(failures.record_success("s2").is_none(), "once");
    }

    /// Both pollers feed one tracker, so the streak climbs faster and the
    /// warning still lands exactly once.
    #[test]
    fn two_pollers_feeding_one_tracker_still_warn_once() {
        let mut failures = ChangedFilesFailures::default();
        let mut warnings = 0;
        for _ in 0..(ERROR_WARN_THRESHOLD * 2) {
            if failures.record_failure("s1").is_some() {
                warnings += 1;
            }
        }
        assert_eq!(warnings, 1);
    }
}
