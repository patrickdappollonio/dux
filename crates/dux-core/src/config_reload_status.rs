//! What dux says about a config reload, and on which key.
//!
//! The reload is decided on one surface and owed to both. The web layer tells
//! browsers to refetch BEFORE the drainer has adopted anything, because the
//! companion seam is pre-consume, so that sentence is a claim about a step that
//! may still fail. The apply's own answer therefore has to land on the same key
//! and travel the worker lane both surfaces drain, or a failed reload leaves
//! "reloaded" standing in a browser while the terminal UI alone says it broke.

use crate::engine::StatusUpdate;
use crate::statusline::StatusTone;
use crate::wire::status_keys::CONFIG_RELOAD;

/// What the web says the moment the file has been read and browsers are being
/// told to refetch. Keyed, so the apply's answer replaces it rather than
/// stacking under it.
pub const REFRESHING: &str = "Configuration reloaded; connected browsers are refreshing.";

/// The config was read, validated and adopted.
pub fn applied() -> StatusUpdate {
    StatusUpdate::keyed(
        CONFIG_RELOAD,
        StatusTone::Info,
        "Configuration reloaded. New settings are active now.",
    )
}

/// The config validated and was adopted, so its settings are in force, but a
/// step of applying it failed. Every surface and every path says this one
/// sentence: the engine keeps the new config whatever failed, so memory, the
/// writer and the file agree, and saying the old settings still run would be
/// false.
pub fn adopted_but_apply_failed(error: &str) -> StatusUpdate {
    StatusUpdate::keyed(
        CONFIG_RELOAD,
        StatusTone::Error,
        format!(
            "The reloaded config.toml is in force and its settings are active, but one step of \
             applying it failed: {error}. Fix that, then reload the config again to finish."
        ),
    )
}

/// How a config reload ended, as the surface that owns it says it to a client
/// that asked for the reload and waits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigReloadOutcome {
    /// The new settings are in force. `notes` are what the owner says only a
    /// restart applies, each a sentence, in the owner's own words.
    Applied { notes: Vec<String> },
    /// The new config is in force, but a step of applying it failed.
    ApplyFailed(String),
    /// The file could not be taken on, so the running settings are unchanged.
    Refused(String),
}

impl ConfigReloadOutcome {
    /// The state and sentence a record ends on. The sentences are the ones the
    /// status line and the browsers' toasts say for the same outcome.
    pub fn record(&self) -> (crate::operations::OperationState, String) {
        use crate::operations::OperationState;
        match self {
            Self::Applied { notes } => {
                let mut message = applied().message;
                for note in notes {
                    message.push(' ');
                    message.push_str(note);
                }
                (OperationState::Succeeded, message)
            }
            Self::ApplyFailed(error) => (
                OperationState::Partial,
                adopted_but_apply_failed(error).message,
            ),
            Self::Refused(reason) => (
                OperationState::Failed,
                format!("Config reload failed: {reason}"),
            ),
        }
    }
}

/// A deferred config write failed, so whatever preference was last changed is
/// not on disk.
///
/// Its own key, not the reload's: nothing asked for this write and nothing is
/// waiting on its answer, so without a sentence the only sign of it is the
/// preference reverting the next time dux starts.
pub fn lazy_write_failed(error: &str) -> StatusUpdate {
    StatusUpdate::keyed(
        "config-write-deferred",
        StatusTone::Warning,
        format!(
            "dux could not save your preferences to config.toml: {error}. The setting you just \
             changed is active now but will be gone after a restart; check that the file is \
             writable and change it again."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_deferred_write_says_the_change_will_not_survive_a_restart() {
        let status = lazy_write_failed("Permission denied (os error 13)");
        assert_eq!(status.tone, StatusTone::Warning);
        assert!(status.message.contains("Permission denied"));
        assert!(status.message.contains("gone after a restart"));
        assert_ne!(
            status.key.as_deref(),
            Some(CONFIG_RELOAD),
            "a write nobody asked for is not the reload's outcome"
        );
    }

    #[test]
    fn both_outcomes_share_the_reload_key() {
        assert_eq!(applied().key.as_deref(), Some(CONFIG_RELOAD));
        assert_eq!(
            adopted_but_apply_failed("boom").key.as_deref(),
            Some(CONFIG_RELOAD)
        );
    }

    /// A failed apply still adopts the new config, so the sentence says the
    /// new settings are in force, names what failed and how to finish, and
    /// never claims the old settings are still running.
    #[test]
    fn the_failure_names_the_error_and_a_remedy() {
        let status = adopted_but_apply_failed("the session database could not be read");
        assert_eq!(status.tone, StatusTone::Error);
        assert!(
            status
                .message
                .contains("the session database could not be read")
        );
        assert!(status.message.contains("in force"), "{}", status.message);
        assert!(
            status.message.contains("reload the config again"),
            "{}",
            status.message
        );
        assert!(!status.message.contains("unchanged"), "{}", status.message);
    }
}
