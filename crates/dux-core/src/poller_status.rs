//! What dux says when one of its background workers falls over or never
//! starts.
//!
//! A poller that dies and comes back, and one that could not be spawned at all,
//! used to be log lines: the user saw a list that stopped updating and had
//! nothing on screen to tell them why. Both are facts about the process rather
//! than about a surface, so they travel the engine's worker lane and both
//! surfaces say them in the same words.
//!
//! The two pollers that answer the same question (the terminal UI's own
//! changed-files sweep and the web's changes service) share a key, so a process
//! running both reports one restart rather than two.

use std::collections::{HashMap, HashSet};

use crate::engine::StatusUpdate;
use crate::statusline::StatusTone;

/// The keyed-status key for one background worker's health. A restart and a
/// failure to start are outcomes for the same worker, so they replace each
/// other rather than stacking.
pub fn key(label: &str) -> String {
    format!("poller:{label}")
}

/// The label the changed-files pollers share. There are two of them, the
/// terminal UI's sweep and the web's changes service, and they keep one list up
/// to date between them, so one restart is one sentence.
pub const CHANGED_FILES_LABEL: &str = "changed-files-poller";

/// What a user loses while the changed-files pollers are down. Named here
/// because both pollers report under it.
pub const CHANGED_FILES_FEATURE: &str = "changed-file updates";

/// A worker died and dux started it again. `feature` is the consumer-facing
/// noun phrase for what it keeps up to date.
pub fn restarted(label: &str, feature: &str) -> StatusUpdate {
    StatusUpdate::keyed(
        key(label),
        StatusTone::Warning,
        format!(
            "dux restarted the background worker behind {feature} after it failed. What it shows \
             may briefly lag; the reason is in dux.log."
        ),
    )
}

// What a user can do about a worker that would not start belongs to the CALL
// SITE: a worker started once at boot is gone for the run, while one spawned on
// demand tries again the next time something asks for it.

/// The remedy for a worker spawned once, at startup, that nothing retries.
pub(crate) const REMEDY_RESTART_DUX: &str = "It stays off until you restart dux.";

/// The remedy for the one-shot probe behind a standalone agent's git status,
/// which every later question about the folder starts again.
pub(crate) const REMEDY_NEXT_LOOK_AT_AGENT: &str =
    "The next look at this agent asks again, so nothing has to be restarted.";

/// The remedy for the on-demand sweep behind the changed-files list, which the
/// next refresh starts again.
pub(crate) const REMEDY_NEXT_CHANGED_FILES_REFRESH: &str =
    "The next refresh of that list asks again, so nothing has to be restarted.";

/// The remedy for the pull-request poller, which is started by a config reload
/// and by dux re-checking whether the GitHub CLI works.
pub(crate) const REMEDY_PR_SYNC: &str = "dux starts it again when you reload your config; otherwise it stays off until you restart \
     dux.";

/// A worker could not be started at all, so the feature behind it is off. What
/// the user can do about that is `remedy`, which the call site chooses from the
/// constants above because only it knows whether anything retries.
pub(crate) fn spawn_failed(label: &str, feature: &str, error: &str, remedy: &str) -> StatusUpdate {
    StatusUpdate::keyed(
        key(label),
        StatusTone::Warning,
        format!("dux could not start the background worker behind {feature}: {error}. {remedy}"),
    )
}

/// A pull request status was fetched but could not be written to SQLite.
///
/// The badge on screen is right, so this is about what survives a restart
/// rather than about what the user is looking at, and the sentence says so
/// instead of implying the status itself is wrong.
pub(crate) fn pr_status_not_saved(
    session_id: &str,
    agent_label: &str,
    error: &str,
) -> StatusUpdate {
    StatusUpdate::keyed(
        key(&format!("pr-status-write:{session_id}")),
        StatusTone::Warning,
        crate::status_text![
            "dux refreshed the pull request status for agent ",
            q(agent_label),
            format!(
                " but could not save \
             it: {}. What you see is correct; after a restart dux has to ask GitHub again.",
                error
            )
        ],
    )
}

/// Consecutive failures a poller tolerates before it says anything.
///
/// The same number [`crate::changes_status`] uses, and for the same reason: git
/// fails transiently all the time (an index lock held by the agent's own commit,
/// a directory mid-rename), so a sentence per blip is noise and a streak is the
/// signal that something is actually wrong.
pub(crate) const WARN_AFTER_FAILURES: u32 = crate::changes_status::ERROR_WARN_THRESHOLD;

/// Consecutive panic-free iterations after which a loop worker reports its next
/// panic out loud again.
///
/// The mirror of [`FailureStreaks::record_success`], and for the same reason: a
/// body that panics every iteration reports once, because the surfaces have
/// already replaced that sentence and the log keeps every one of them, while a
/// worker that ran cleanly for a streak and then broke for a different reason is
/// news nobody has heard.
pub(crate) const REARM_AFTER_SUCCESSES: u32 = WARN_AFTER_FAILURES;

/// Per-subject consecutive-failure counting for a poller that repeats the same
/// question every cycle.
///
/// Answers on the CROSSING only, and owes a withdrawal only where a warning was
/// actually shown, so a blip stays silent and a standing failure is reported
/// once.
#[derive(Debug, Default)]
pub(crate) struct FailureStreaks {
    streaks: HashMap<String, u32>,
    warned: HashSet<String>,
}

impl FailureStreaks {
    /// Record one failure. `true` exactly once, on the cycle that crosses
    /// [`WARN_AFTER_FAILURES`].
    pub(crate) fn record_failure(&mut self, subject: &str) -> bool {
        let streak = self.streaks.entry(subject.to_string()).or_insert(0);
        *streak += 1;
        if *streak != WARN_AFTER_FAILURES {
            return false;
        }
        self.warned.insert(subject.to_string());
        true
    }

    /// Record one success. `true` only when a warning was actually shown and is
    /// now owed a withdrawal; a blip that never warned has nothing to take down.
    pub(crate) fn record_success(&mut self, subject: &str) -> bool {
        self.streaks.remove(subject);
        self.warned.remove(subject)
    }
}

/// What one outcome of a streak-counting poller owes the surfaces: a warning
/// raised on the crossing, or the withdrawal of a warning that was shown, once
/// the trouble behind it is over.
///
/// A withdrawal is silent on both surfaces. The warning said what was wrong
/// while it was wrong, and the screen working again is the news that it ended.
#[derive(Debug, Clone)]
pub enum PollerNotice {
    /// Raise this warning.
    Warn(StatusUpdate),
    /// Take down the warning standing on this key, with nothing in its place.
    Withdraw(String),
}

impl PollerNotice {
    /// The reaction for a poller whose outcome the draining surface applies
    /// directly.
    pub fn into_reaction(self) -> crate::engine::EventReaction {
        match self {
            Self::Warn(status) => crate::engine::EventReaction::Status(status),
            Self::Withdraw(key) => crate::engine::EventReaction::ClearStatus(key),
        }
    }

    /// The worker-lane event for a poller running off the draining thread, so
    /// whichever surface drains shows (or takes down) the same thing.
    pub fn into_worker_event(self) -> crate::worker::WorkerEvent {
        match self {
            Self::Warn(status) => crate::worker::WorkerEvent::PollerStatus(status),
            Self::Withdraw(key) => crate::worker::WorkerEvent::PollerStatusWithdrawn(key),
        }
    }
}

/// The keyed-status key branch sync warns on for one worktree.
pub(crate) fn branch_sync_key(worktree: &str) -> String {
    key(&format!("branch-sync:{worktree}"))
}

/// Branch sync has failed to read git's current branch in one worktree for a
/// whole streak, so the branch dux shows for that agent may have drifted.
///
/// git's own text is left to dux.log, where every failure is written: it is
/// the detail someone debugging needs and noise for everyone else.
pub(crate) fn branch_sync_stuck(worktree: &str) -> StatusUpdate {
    StatusUpdate::keyed(
        branch_sync_key(worktree),
        StatusTone::Warning,
        crate::status_text![
            "dux cannot read the current branch in the agent's folder ",
            q(worktree),
            ", so the branch it shows for that agent may be out of date. It keeps trying, and \
             the details are in dux.log."
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blip_stays_silent_and_a_streak_speaks_once() {
        let mut streaks = FailureStreaks::default();
        for _ in 1..WARN_AFTER_FAILURES {
            assert!(!streaks.record_failure("/w"));
        }
        assert!(streaks.record_failure("/w"), "the crossing speaks");
        assert!(!streaks.record_failure("/w"), "and only the crossing");
        assert!(
            streaks.record_success("/w"),
            "a warned subject is withdrawn"
        );
        assert!(!streaks.record_success("/w"), "once");
    }

    #[test]
    fn a_withdrawal_is_owed_only_where_a_warning_was_shown() {
        let mut streaks = FailureStreaks::default();
        streaks.record_failure("/w");
        assert!(!streaks.record_success("/w"));
    }

    #[test]
    fn a_stuck_branch_read_names_the_folder_and_points_at_the_log() {
        let stuck = branch_sync_stuck("/tmp/wt");
        assert_eq!(stuck.tone, StatusTone::Warning);
        assert_eq!(
            stuck.key.as_deref(),
            Some(branch_sync_key("/tmp/wt").as_str())
        );
        assert_ne!(stuck.key, branch_sync_stuck("/tmp/other").key);
        assert_eq!(
            stuck.message,
            "dux cannot read the current branch in the agent's folder \"/tmp/wt\", so the branch \
             it shows for that agent may be out of date. It keeps trying, and the details are in \
             dux.log."
        );
    }

    #[test]
    fn an_unsaved_pr_status_says_the_badge_is_still_right() {
        let status = pr_status_not_saved("s1", "feat-login", "database is locked");
        assert_eq!(status.tone, StatusTone::Warning);
        assert!(status.message.contains("feat-login"));
        assert!(status.message.contains("database is locked"));
        assert!(status.message.contains("What you see is correct"));
        assert_ne!(
            status.key,
            pr_status_not_saved("s2", "feat-login", "x").key,
            "one agent's failure is not another's"
        );
    }

    #[test]
    fn both_outcomes_for_one_worker_share_its_key() {
        assert_eq!(
            restarted("pr-sync", "pull request status updates").key,
            spawn_failed(
                "pr-sync",
                "pull request status updates",
                "boom",
                REMEDY_RESTART_DUX
            )
            .key
        );
        assert_ne!(
            restarted("pr-sync", "a").key,
            restarted("branch-sync", "a").key
        );
    }

    #[test]
    fn a_restart_names_the_feature_and_where_the_reason_is() {
        let status = restarted(CHANGED_FILES_LABEL, CHANGED_FILES_FEATURE);
        assert_eq!(status.tone, StatusTone::Warning);
        assert!(status.message.contains(CHANGED_FILES_FEATURE));
        assert!(status.message.contains("dux.log"));
    }

    #[test]
    fn a_failure_to_start_says_the_feature_is_off_until_a_restart() {
        let status = spawn_failed(
            "branch-sync",
            "branch status updates",
            "too many threads",
            REMEDY_RESTART_DUX,
        );
        assert!(status.message.contains("branch status updates"));
        assert!(status.message.contains("too many threads"));
        assert!(status.message.contains("until you restart dux"));
    }

    #[test]
    fn a_worker_that_retries_does_not_tell_the_user_to_restart_dux() {
        let status = spawn_failed(
            "folder-repo-probe:sa1",
            "this agent's git status",
            "too many threads",
            REMEDY_NEXT_LOOK_AT_AGENT,
        );
        assert!(
            status
                .message
                .contains("The next look at this agent asks again")
        );
        assert!(
            !status.message.contains("restart dux"),
            "a probe the next ask starts again must not send the user for a restart: {}",
            status.message
        );
    }
}
