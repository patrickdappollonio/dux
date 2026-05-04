//! Per-session state machine that owns compiled regexes, tracks match
//! counts across ticks, and applies the backoff/cooldown/budget logic.
//!
//! See `mod.rs` for the security model. The engine itself is pure — it
//! takes a string snapshot and an `Instant`, and returns a list of
//! [`WatchEffect`]s that the caller is responsible for executing (writing
//! bytes to the PTY, posting status-line messages). This keeps the engine
//! testable without a real PTY.
//!
//! ## Match deduplication
//!
//! A naive "did the snapshot match the pattern" check would re-fire on
//! every tick while the matched line was still visible in the scrollback.
//! Instead, the engine counts pattern occurrences in the snapshot and only
//! treats an *increase* in the count as a new incident:
//!
//! - On match → schedule fire after backoff; record `baseline = matches`.
//! - While `Pending` / `Cooling` → don't re-arm even if the line is still
//!   visible.
//! - When matches scroll out of the snapshot, the baseline ratchets down
//!   so a fresh occurrence is detected later as an increase from 0.
//!
//! This makes the engine robust to slow agent responses where the offending
//! message stays on screen well past the cooldown window.

use std::time::{Duration, Instant};

use rand::Rng;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use regex::RegexBuilder;

use super::rule::{WatchAction, WatchRule};

/// Cap on the size of compiled regex DFAs (in bytes). Patterns that exceed
/// this at compile time are rejected — this is the catastrophic-regex
/// mitigation called out in `SECURITY.md` (T13).
const REGEX_SIZE_LIMIT: usize = 64 * 1024;
/// DFA cache cap during matching.
const REGEX_DFA_SIZE_LIMIT: usize = 64 * 1024;
/// Soft cap on rules per provider. Configs with more rules will load the
/// first N and surface a warning for the rest.
pub const MAX_RULES_PER_PROVIDER: usize = 32;

/// A side effect produced by the engine. The caller (the App tick loop)
/// translates these into PTY writes and status-line messages.
#[derive(Clone, Debug, PartialEq)]
pub enum WatchEffect {
    /// Write `text` (optionally followed by `\r`) to the agent's PTY.
    SendText { text: String, append_enter: bool },
    /// Post an informational status-line message.
    StatusInfo(String),
    /// Post a warning status-line message.
    StatusWarning(String),
}

/// Per-rule runtime state.
#[derive(Debug)]
struct RuleRuntime {
    rule: WatchRule,
    regex: regex::Regex,
    state: RuleState,
    attempts_made: u32,
    /// Number of pattern occurrences observed in the most recent snapshot.
    /// A rule re-arms only when the current count strictly exceeds this.
    /// Ratchets down when matches scroll out so future occurrences are
    /// still detected.
    baseline_match_count: usize,
    /// Resolved label (rule.label, or a truncated copy of the pattern).
    label: String,
}

#[derive(Clone, Copy, Debug)]
enum RuleState {
    /// No fire scheduled. Watching for a fresh match.
    Idle,
    /// Match observed; waiting until `fire_at` to dispatch the action.
    Pending { fire_at: Instant },
    /// Action just fired; suppressing repeats until `until`.
    Cooling { until: Instant },
    /// Budget exhausted (or manual disarm). No further work this session.
    Disarmed,
}

/// Per-session matcher + scheduler. Cheap to construct and tick — designed
/// to run synchronously from the App's render loop.
pub struct WatchEngine {
    /// Session this engine was created for. Surfaced via [`Self::session_id`]
    /// and used in log messages; otherwise unread by the engine itself.
    #[allow(dead_code)]
    session_id: String,
    rules: Vec<RuleRuntime>,
    rng: SmallRng,
}

impl WatchEngine {
    /// Build an engine for the given session. Returns the engine plus a
    /// list of human-readable error messages for any rules that failed to
    /// load (invalid regex, oversized pattern, etc.); valid rules in the
    /// same array are still loaded.
    pub fn new(session_id: impl Into<String>, rules: &[WatchRule]) -> (Self, Vec<String>) {
        let session_id = session_id.into();
        let mut runtime = Vec::with_capacity(rules.len().min(MAX_RULES_PER_PROVIDER));
        let mut errors = Vec::new();

        for (idx, rule) in rules.iter().enumerate() {
            if idx >= MAX_RULES_PER_PROVIDER {
                errors.push(format!(
                    "watch: too many rules ({} > {}); ignoring rule #{}",
                    rules.len(),
                    MAX_RULES_PER_PROVIDER,
                    idx
                ));
                break;
            }

            if rule.pattern.trim().is_empty() {
                errors.push(format!("watch: rule #{idx} has an empty pattern; skipping"));
                continue;
            }

            let regex = match RegexBuilder::new(&rule.pattern)
                .size_limit(REGEX_SIZE_LIMIT)
                .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    errors.push(format!(
                        "watch: rule #{idx} pattern rejected ({e}); skipping"
                    ));
                    continue;
                }
            };

            let label = if rule.label.trim().is_empty() {
                truncate_for_label(&rule.pattern)
            } else {
                rule.label.clone()
            };

            runtime.push(RuleRuntime {
                rule: rule.clone(),
                regex,
                state: RuleState::Idle,
                attempts_made: 0,
                baseline_match_count: 0,
                label,
            });
        }

        let engine = Self {
            session_id,
            rules: runtime,
            rng: SmallRng::from_entropy(),
        };
        (engine, errors)
    }

    /// Session id this engine was created for. Useful in log messages.
    #[allow(dead_code)] // Phase 3: palette commands surface this.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Number of loaded rules (after invalid-rule filtering).
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Resolved label for the rule at `idx`, if any.
    #[allow(dead_code)] // Phase 3: palette commands surface this.
    pub fn rule_label(&self, idx: usize) -> Option<&str> {
        self.rules.get(idx).map(|r| r.label.as_str())
    }

    /// Whether the rule at `idx` is currently disarmed (budget exhausted
    /// or manually disarmed).
    #[allow(dead_code)] // Used by tests + Phase 3 palette commands.
    pub fn is_disarmed(&self, idx: usize) -> bool {
        matches!(
            self.rules.get(idx).map(|r| &r.state),
            Some(RuleState::Disarmed)
        )
    }

    /// Manually disarm the rule at `idx`. Returns true if the rule
    /// existed and is now disarmed.
    #[allow(dead_code)] // Phase 3: palette commands wire this up.
    pub fn disarm(&mut self, idx: usize) -> bool {
        match self.rules.get_mut(idx) {
            Some(rule) => {
                rule.state = RuleState::Disarmed;
                true
            }
            None => false,
        }
    }

    /// Manually re-arm a disarmed (or active) rule, resetting its attempt
    /// counter and baseline. Returns true if the rule existed.
    #[allow(dead_code)] // Phase 3: palette commands wire this up.
    pub fn rearm(&mut self, idx: usize) -> bool {
        match self.rules.get_mut(idx) {
            Some(rule) => {
                rule.state = RuleState::Idle;
                rule.attempts_made = 0;
                rule.baseline_match_count = 0;
                true
            }
            None => false,
        }
    }

    /// Drive every rule one tick. `snapshot` is the recent visible content
    /// of the agent's terminal (concatenated rows, separated by `\n`).
    /// `now` is wall-clock for backoff/cooldown comparisons.
    ///
    /// Returns the list of effects to dispatch this tick.
    pub fn observe(&mut self, snapshot: &str, now: Instant) -> Vec<WatchEffect> {
        let mut effects = Vec::new();
        for rule in &mut self.rules {
            rule.tick(snapshot, now, &mut self.rng, &mut effects);
        }
        effects
    }

    /// True if any loaded rule is non-empty. Used to skip the per-tick
    /// snapshot work entirely when no rules apply.
    #[allow(dead_code)] // Phase 3: tick path can short-circuit per-engine.
    pub fn is_active(&self) -> bool {
        self.rules
            .iter()
            .any(|r| !matches!(r.state, RuleState::Disarmed))
    }
}

impl RuleRuntime {
    fn tick(
        &mut self,
        snapshot: &str,
        now: Instant,
        rng: &mut SmallRng,
        out: &mut Vec<WatchEffect>,
    ) {
        if matches!(self.state, RuleState::Disarmed) {
            return;
        }

        let matches = self.regex.find_iter(snapshot).count();

        // Ratchet baseline down when matches scroll out, so a future
        // occurrence is detected as an increase even if the count drops to
        // zero in between. Increases below promote Idle → Pending.
        if matches < self.baseline_match_count {
            self.baseline_match_count = matches;
        }

        // Phase 1: cooldown elapsed → re-arm.
        if let RuleState::Cooling { until } = self.state
            && now >= until
        {
            self.state = RuleState::Idle;
        }

        // Phase 2: pending fire-time reached → dispatch action.
        if let RuleState::Pending { fire_at } = self.state
            && now >= fire_at
        {
            let WatchAction::SendText { text, append_enter } = self.rule.action.clone();
            out.push(WatchEffect::SendText { text, append_enter });
            self.attempts_made = self.attempts_made.saturating_add(1);
            let cooldown = Duration::from_millis(self.rule.cooldown_ms);
            self.state = RuleState::Cooling {
                until: now + cooldown,
            };
            out.push(WatchEffect::StatusInfo(format!(
                "watch rule \"{}\": fired (attempt {}/{})",
                self.label, self.attempts_made, self.rule.budget.max_attempts
            )));
        }

        // Phase 3: fresh match while idle → schedule fire (or disarm if
        // budget exhausted).
        if matches!(self.state, RuleState::Idle) && matches > self.baseline_match_count {
            if self.attempts_made >= self.rule.budget.max_attempts {
                self.state = RuleState::Disarmed;
                out.push(WatchEffect::StatusWarning(format!(
                    "watch rule \"{}\": budget ({} attempts) exhausted; disarming",
                    self.label, self.rule.budget.max_attempts
                )));
                return;
            }
            let delay_ms = self.rule.backoff.deterministic_delay_ms(self.attempts_made);
            let jitter_ms = if self.rule.backoff.jitter_ms > 0 {
                rng.gen_range(0..=self.rule.backoff.jitter_ms)
            } else {
                0
            };
            let total = Duration::from_millis(delay_ms.saturating_add(jitter_ms));
            self.state = RuleState::Pending {
                fire_at: now + total,
            };
            self.baseline_match_count = matches;
        }
    }
}

fn truncate_for_label(pattern: &str) -> String {
    const MAX: usize = 40;
    let chars: Vec<char> = pattern.chars().collect();
    if chars.len() <= MAX {
        pattern.to_string()
    } else {
        let head: String = chars.iter().take(MAX).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch::rule::{WatchAction, WatchBackoff, WatchBudget, WatchRule};

    fn rule(pattern: &str) -> WatchRule {
        WatchRule {
            pattern: pattern.to_string(),
            label: String::new(),
            action: WatchAction::SendText {
                text: "please continue".to_string(),
                append_enter: true,
            },
            backoff: WatchBackoff {
                initial_ms: 1_000,
                max_ms: 10_000,
                multiplier: 2.0,
                jitter_ms: 0,
            },
            budget: WatchBudget { max_attempts: 3 },
            cooldown_ms: 500,
        }
    }

    #[test]
    fn matches_pattern_fires_send_text() {
        let (mut engine, errors) = WatchEngine::new("s1", &[rule("rate.*limited")]);
        assert!(errors.is_empty(), "load errors: {errors:?}");

        let t0 = Instant::now();
        // First tick: snapshot contains the match. Engine schedules fire
        // for t0 + 1s (initial_ms with no jitter).
        let effects = engine.observe("hello\nyou are rate limited\nbye", t0);
        assert!(
            effects.is_empty(),
            "should not fire immediately: {effects:?}"
        );

        // Just before fire_at — still pending.
        let effects = engine.observe("you are rate limited", t0 + Duration::from_millis(999));
        assert!(effects.is_empty());

        // At fire_at — sends text + status info.
        let effects = engine.observe("you are rate limited", t0 + Duration::from_millis(1_000));
        assert_eq!(effects.len(), 2);
        assert!(matches!(
            effects[0],
            WatchEffect::SendText { ref text, append_enter: true } if text == "please continue"
        ));
        assert!(matches!(effects[1], WatchEffect::StatusInfo(_)));
    }

    #[test]
    fn cooldown_window_treats_repeat_as_same_failure() {
        let (mut engine, _) = WatchEngine::new("s1", &[rule("rate")]);
        let t0 = Instant::now();
        // Match → schedule.
        engine.observe("rate", t0);
        // Fire.
        let effects = engine.observe("rate", t0 + Duration::from_millis(1_000));
        assert_eq!(effects.len(), 2);
        assert_eq!(engine.rules[0].attempts_made, 1);

        // Within cooldown_ms (500ms) — even if pattern still matches,
        // attempts_made stays at 1.
        let effects = engine.observe("rate", t0 + Duration::from_millis(1_200));
        assert!(
            effects.is_empty(),
            "should suppress in cooldown: {effects:?}"
        );
        assert_eq!(engine.rules[0].attempts_made, 1);
    }

    #[test]
    fn exponential_backoff_with_jitter_bounds() {
        let mut r = rule("rate");
        r.backoff = WatchBackoff {
            initial_ms: 100,
            max_ms: 10_000,
            multiplier: 2.0,
            jitter_ms: 50,
        };
        r.cooldown_ms = 0;
        r.budget = WatchBudget { max_attempts: 5 };

        let (mut engine, _) = WatchEngine::new("s1", &[r]);
        let mut now = Instant::now();

        for attempt in 0..4_u32 {
            // Each iteration adds another occurrence so the match count
            // increases — which is what triggers Idle → Pending.
            let snapshot = "rate ".repeat((attempt + 1) as usize);
            engine.observe(&snapshot, now);
            let fire_at = match engine.rules[0].state {
                RuleState::Pending { fire_at } => fire_at,
                other => panic!("attempt {attempt}: expected Pending, got {other:?}"),
            };
            // Deterministic floor: initial * 2^attempt; jitter adds at
            // most jitter_ms.
            let det = 100 * 2_u64.pow(attempt);
            let elapsed = fire_at.duration_since(now).as_millis() as u64;
            assert!(
                (det..=det + 50).contains(&elapsed),
                "attempt {attempt}: expected delay in [{det}, {}], got {elapsed}",
                det + 50
            );
            // Advance to fire_at, drive the fire (Pending → Cooling),
            // then advance past cooldown=0 so the next iteration's
            // observe transitions Cooling → Idle and schedules Pending.
            now = fire_at;
            engine.observe(&snapshot, now);
            now += Duration::from_millis(1);
        }
        assert_eq!(engine.rules[0].attempts_made, 4);
    }

    #[test]
    fn budget_exhausted_disarms_and_emits_warning() {
        let mut r = rule("rate");
        r.budget = WatchBudget { max_attempts: 2 };
        r.cooldown_ms = 0;
        r.backoff.initial_ms = 1;
        r.backoff.multiplier = 1.0;
        r.backoff.jitter_ms = 0;

        let (mut engine, _) = WatchEngine::new("s1", &[r]);
        let mut now = Instant::now();

        // First fire: 1 occurrence.
        engine.observe("rate", now); // schedule
        now += Duration::from_millis(2);
        engine.observe("rate", now); // fire → Cooling (cooldown=0)
        assert_eq!(engine.rules[0].attempts_made, 1);

        // Second fire: a fresh occurrence (count goes 1 → 2). The single
        // tick combines Cooling → Idle and Idle → Pending in this run.
        now += Duration::from_millis(1);
        engine.observe("rate\nrate", now); // schedule
        now += Duration::from_millis(2);
        engine.observe("rate\nrate", now); // fire → Cooling
        assert_eq!(engine.rules[0].attempts_made, 2);

        // Third occurrence (count goes 2 → 3). Budget is exhausted, so
        // the rule should disarm and emit a warning instead of scheduling.
        now += Duration::from_millis(1);
        let effects = engine.observe("rate\nrate\nrate", now);
        assert!(engine.is_disarmed(0));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, WatchEffect::StatusWarning(_)))
        );

        // Subsequent ticks are silent.
        let effects = engine.observe("rate\nrate\nrate", now + Duration::from_secs(60));
        assert!(effects.is_empty());
    }

    #[test]
    fn manual_rearm_resets_attempt() {
        let (mut engine, _) = WatchEngine::new("s1", &[rule("rate")]);
        let t0 = Instant::now();
        engine.observe("rate", t0);
        engine.observe("rate", t0 + Duration::from_secs(1));
        assert_eq!(engine.rules[0].attempts_made, 1);

        engine.disarm(0);
        assert!(engine.is_disarmed(0));

        engine.rearm(0);
        assert!(!engine.is_disarmed(0));
        assert_eq!(engine.rules[0].attempts_made, 0);
        assert_eq!(engine.rules[0].baseline_match_count, 0);
    }

    #[test]
    fn match_count_dedup_suppresses_stale_match() {
        // After a match, the same match still being on screen shouldn't
        // re-fire after cooldown — only a *new* occurrence should.
        let mut r = rule("rate");
        r.cooldown_ms = 100;
        let (mut engine, _) = WatchEngine::new("s1", &[r]);
        let t0 = Instant::now();

        // Match, schedule, fire, cool down.
        engine.observe("rate", t0);
        engine.observe("rate", t0 + Duration::from_millis(1_000));
        assert_eq!(engine.rules[0].attempts_made, 1);
        // Cooldown ends. Same single match still visible.
        let effects = engine.observe("rate", t0 + Duration::from_millis(1_200));
        assert!(
            effects.is_empty(),
            "stale match should not re-fire: {effects:?}"
        );
        assert_eq!(engine.rules[0].attempts_made, 1);

        // A second occurrence of the pattern (count goes from 1 → 2)
        // *should* re-fire after backoff.
        engine.observe("rate\nrate", t0 + Duration::from_millis(1_300));
        assert!(matches!(engine.rules[0].state, RuleState::Pending { .. }));
    }

    #[test]
    fn match_count_ratchets_down_when_match_scrolls_out() {
        let (mut engine, _) = WatchEngine::new("s1", &[rule("rate")]);
        let t0 = Instant::now();

        engine.observe("rate", t0);
        engine.observe("rate", t0 + Duration::from_millis(1_000));
        // Match scrolls out; baseline should ratchet to 0.
        engine.observe("nothing here", t0 + Duration::from_millis(2_000));
        assert_eq!(engine.rules[0].baseline_match_count, 0);
        // A new occurrence is detected as an increase from 0.
        engine.observe("rate", t0 + Duration::from_millis(3_000));
        assert!(matches!(engine.rules[0].state, RuleState::Pending { .. }));
    }

    #[test]
    fn malformed_regex_rejected_at_load() {
        let mut bad = rule("rate");
        bad.pattern = "(unclosed".to_string();
        let good = rule("limited");

        let (engine, errors) = WatchEngine::new("s1", &[bad, good]);
        assert_eq!(engine.rule_count(), 1, "bad rule should be skipped");
        assert!(!errors.is_empty(), "errors should be reported");
        assert!(errors[0].contains("rule #0"));
    }

    #[test]
    fn empty_pattern_rejected_at_load() {
        let mut empty = rule("");
        empty.pattern = "   ".to_string();
        let (engine, errors) = WatchEngine::new("s1", &[empty]);
        assert_eq!(engine.rule_count(), 0);
        assert!(errors[0].contains("empty pattern"));
    }

    #[test]
    fn regex_size_limit_caps_oversized_patterns() {
        // A long literal pattern produces a Program proportional to its
        // length, which busts our 64KB size_limit cap. This is the
        // catastrophic-regex mitigation from SECURITY.md (T13).
        let huge_pattern = "a".repeat(200_000);
        let r = WatchRule {
            pattern: huge_pattern,
            ..rule("rate")
        };
        let (engine, errors) = WatchEngine::new("s1", &[r]);
        assert_eq!(
            engine.rule_count(),
            0,
            "oversized pattern should be rejected"
        );
        assert!(!errors.is_empty());
    }

    #[test]
    fn rule_cap_enforced() {
        let rules: Vec<_> = (0..MAX_RULES_PER_PROVIDER + 5)
            .map(|i| rule(&format!("p{i}")))
            .collect();
        let (engine, errors) = WatchEngine::new("s1", &rules);
        assert_eq!(engine.rule_count(), MAX_RULES_PER_PROVIDER);
        assert!(errors.iter().any(|e| e.contains("too many rules")));
    }
}
