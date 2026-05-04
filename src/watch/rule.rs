//! Serializable types describing a single watch rule.

use serde::{Deserialize, Serialize};

/// One watch rule. Loaded from `[[providers.<name>.watch]]` arrays in
/// `config.toml`. Fields default to safe values so partially-specified rules
/// in user configs still load.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchRule {
    /// Regex pattern matched against each row of the agent's visible
    /// terminal viewport. Compiled by the engine at load time; invalid
    /// patterns are reported and the rule is skipped.
    pub pattern: String,
    /// Optional human label used in status messages. If empty, a
    /// truncated copy of `pattern` stands in.
    pub label: String,
    /// Action to take when the pattern matches.
    #[serde(flatten)]
    pub action: WatchAction,
    /// Backoff schedule applied to repeat matches.
    pub backoff: WatchBackoff,
    /// Maximum number of times the rule may fire in one session.
    pub budget: WatchBudget,
    /// If the same rule re-matches within this many milliseconds of its
    /// last action, the engine treats it as the same incident — the
    /// backoff curve is not reset and the budget is not consumed again.
    pub cooldown_ms: u64,
}

impl Default for WatchRule {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            label: String::new(),
            action: WatchAction::default(),
            backoff: WatchBackoff::default(),
            budget: WatchBudget::default(),
            cooldown_ms: 30_000,
        }
    }
}

/// Action variants a rule can dispatch when it fires.
///
/// Serialized as a flat TOML form using `action = "<kind>"` as the
/// discriminator and the variant fields as siblings:
///
/// ```toml
/// [[providers.claude.watch]]
/// pattern = "..."
/// action = "send_text"
/// text = "please continue"
/// append_enter = true
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum WatchAction {
    /// Write `text` (optionally followed by `\r` to simulate Enter) to the
    /// agent's PTY.
    SendText {
        text: String,
        #[serde(default = "default_append_enter")]
        append_enter: bool,
    },
}

impl Default for WatchAction {
    fn default() -> Self {
        Self::SendText {
            text: String::new(),
            append_enter: true,
        }
    }
}

fn default_append_enter() -> bool {
    true
}

/// Exponential backoff parameters with jitter.
///
/// Delay for attempt `n` (0-indexed) is
/// `min(initial_ms * multiplier^n, max_ms) + uniform_random(0, jitter_ms)`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchBackoff {
    pub initial_ms: u64,
    pub max_ms: u64,
    pub multiplier: f64,
    pub jitter_ms: u64,
}

impl Default for WatchBackoff {
    fn default() -> Self {
        Self {
            initial_ms: 60_000,
            max_ms: 600_000,
            multiplier: 2.0,
            jitter_ms: 5_000,
        }
    }
}

impl WatchBackoff {
    /// Compute the deterministic (jitter-free) component of the delay for
    /// the given attempt number. Capped at `max_ms`.
    pub(crate) fn deterministic_delay_ms(&self, attempt: u32) -> u64 {
        let mult = self.multiplier.max(1.0);
        let raw = (self.initial_ms as f64) * mult.powi(attempt as i32);
        if !raw.is_finite() || raw > self.max_ms as f64 {
            self.max_ms
        } else {
            raw as u64
        }
    }
}

/// How many times a rule may fire before disarming itself for the rest of
/// the session.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchBudget {
    pub max_attempts: u32,
}

impl Default for WatchBudget {
    fn default() -> Self {
        Self { max_attempts: 5 }
    }
}
