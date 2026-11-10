//! Pure agent-tab derivations, core-owned and mirrored by hand in the web's
//! `crates/dux-web/web/src/lib/agentTabs.ts`. The cases in this module's tests
//! are duplicated verbatim in `agentTabs.test.ts`, so an unmirrored rule change
//! fails a test.
//!
//! Keep these functions small and pure; anything needing engine state belongs on
//! `Engine`, not here.

/// The one sentence dux says when closing a tab would leave an agent with no
/// tab at all. Owned here so the engine's refusal, the terminal UI's status
/// line and the browser's disabled menu item are the same words in the same
/// casing; the browser mirrors it in `agentTabs.ts` and a test in each language
/// pins the literal.
pub const ONLY_TAB_CLOSE_REFUSAL: &str = "This is the agent's only tab, so closing it would leave the agent with no tab at all. Detach the agent instead to stop everything it is running, or add another tab first.";

/// The way prose names a tab: the strip's label with its first character
/// upper-cased. Pills stay lower-case because they are chrome, and the
/// disambiguating suffix rides along, so "Codex 2" in a confirmation is the pill
/// the user is looking at. Mirrors the web's `tabProseLabel`.
pub(crate) fn prose_tab_label(strip_label: &str) -> String {
    let mut chars = strip_label.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// Disambiguated tab labels: each provider name is bare on its oldest tab and
/// suffixed " 2", " 3", … on the later ones of the same provider, numbered by
/// CREATION order (`created_ms`, then the strip position for a tie), never by
/// where a tab happens to sit. A name is how a tab is told apart from its
/// siblings in a confirmation and on the command line, so dragging a pill must
/// not rename it. `tabs` is in strip order, as is the answer. Mirrors the web's
/// `tabLabels`.
pub fn tab_labels(tabs: &[(&str, i64)]) -> Vec<String> {
    tabs.iter()
        .enumerate()
        .map(|(i, (provider, created))| {
            let older = tabs
                .iter()
                .enumerate()
                .filter(|(j, (other, other_created))| {
                    other == provider && (*other_created, *j) < (*created, i)
                })
                .count();
            if older == 0 {
                (*provider).to_string()
            } else {
                format!("{provider} {}", older + 1)
            }
        })
        .collect()
}

/// True when an agent's `current` branch has drifted from the `initial` branch it
/// was created on. Guards against an empty `initial` (a legacy row that predates
/// the `initial_branch` column and was never backfilled, or a transient
/// pre-persist state) so an empty original never shows a phantom `(orig: )`
/// drift. Route every drift check through this rather than recomputing
/// `current != initial` inline. Mirrors the web's `branchDrift`.
pub fn branch_drifted(current: &str, initial: &str) -> bool {
    !initial.is_empty() && current != initial
}

/// The tab the view lands on when the one on screen closes, like a browser tab:
/// the tab to its right, or the one to its left when it was the last. `strip` is
/// the strip as it stood BEFORE the close. `None` when the closed tab was the
/// only one, or is not in the strip. Mirrors the web's `tabAfterClose`.
pub fn tab_after_close<'a>(strip: &'a [String], closed: &str) -> Option<&'a str> {
    let index = strip.iter().position(|id| id == closed)?;
    strip
        .get(index + 1)
        .or_else(|| index.checked_sub(1).and_then(|left| strip.get(left)))
        .map(String::as_str)
}

/// Whether an agent's two strips show: its provider tabs' and its terminal
/// dock's, which always show and hide together. Both show while the agent has
/// two or more tabs, or two or more terminals, or the user asked to always see
/// them; otherwise neither does, and a lone tab and a lone terminal stand
/// without one. Mirrors the web's `shouldShowTabStrip`.
pub fn strips_shown(tab_count: usize, terminal_count: usize, setting: StripSetting) -> bool {
    setting == StripSetting::Always || tab_count >= 2 || terminal_count >= 2
}

/// `[ui] always_show_tab_strip`, as [`strips_shown`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripSetting {
    /// The strips show whatever the agent has.
    Always,
    /// The strips show once there is a second tab or a second terminal.
    WhenNeeded,
}

impl From<bool> for StripSetting {
    /// The setting `always_show_tab_strip`'s value stands for.
    fn from(always_show: bool) -> Self {
        if always_show {
            Self::Always
        } else {
            Self::WhenNeeded
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_string()).collect()
    }

    #[test]
    fn closing_a_tab_lands_on_the_one_to_its_right_or_the_left_when_it_was_last() {
        let tabs = strip(&["a", "b", "c"]);
        assert_eq!(tab_after_close(&tabs, "a"), Some("b"));
        assert_eq!(tab_after_close(&tabs, "b"), Some("c"));
        assert_eq!(tab_after_close(&tabs, "c"), Some("b"));
    }

    #[test]
    fn closing_the_only_tab_or_one_not_in_the_strip_lands_nowhere() {
        assert_eq!(tab_after_close(&strip(&["a"]), "a"), None);
        assert_eq!(tab_after_close(&strip(&["a", "b"]), "z"), None);
    }

    // ── SHARED VECTORS with agentTabs.test.ts `tabLabels` ──────────────────────
    #[test]
    fn tab_labels_disambiguate_duplicate_providers() {
        assert_eq!(
            tab_labels(&[("claude", 1), ("codex", 2)]),
            vec!["claude", "codex"]
        );
        assert_eq!(
            tab_labels(&[("codex", 1), ("codex", 2)]),
            vec!["codex".to_string(), "codex 2".to_string()]
        );
        assert_eq!(
            tab_labels(&[("claude", 1), ("codex", 2), ("codex", 3), ("claude", 4)]),
            vec![
                "claude".to_string(),
                "codex".to_string(),
                "codex 2".to_string(),
                "claude 2".to_string()
            ]
        );
        assert_eq!(tab_labels(&[]), Vec::<String>::new());
    }

    #[test]
    fn a_tab_keeps_its_number_wherever_it_is_dragged() {
        // Created in the order a, b, c; the strip shows c, a, b.
        assert_eq!(
            tab_labels(&[("claude", 30), ("claude", 10), ("claude", 20)]),
            vec!["claude 3", "claude", "claude 2"]
        );
        // The same creation times with no clock to tell them apart fall back to
        // the strip order.
        assert_eq!(
            tab_labels(&[("claude", 0), ("claude", 0)]),
            vec!["claude", "claude 2"]
        );
    }

    // ── SHARED VECTORS with agentTabs.test.ts `tabProseLabel` ──────────────────
    #[test]
    fn prose_tab_label_upper_cases_the_first_character_only() {
        assert_eq!(prose_tab_label("codex"), "Codex");
        assert_eq!(prose_tab_label("codex 2"), "Codex 2");
        assert_eq!(prose_tab_label("opencode"), "Opencode");
        assert_eq!(prose_tab_label(""), "");
    }

    /// The refusal is one sentence in one casing, mirrored verbatim in
    /// `agentTabs.ts`. Pinned as a literal so a reword in one language that is
    /// not mirrored in the other fails here.
    #[test]
    fn only_tab_close_refusal_is_a_proper_sentence() {
        assert_eq!(
            ONLY_TAB_CLOSE_REFUSAL,
            "This is the agent's only tab, so closing it would leave the agent with no tab at \
             all. Detach the agent instead to stop everything it is running, or add another tab \
             first."
        );
    }

    // ── SHARED VECTORS with agentTabs.test.ts `branchDrift` ────────────────────
    #[test]
    fn branch_drifted_guards_empty_initial() {
        // Real drift: current differs from a non-empty initial.
        assert!(branch_drifted("agent-tabs", "server-mode"));
        // No drift when equal.
        assert!(!branch_drifted("main", "main"));
        // Empty initial (legacy/never-backfilled row) is NEVER drift, even though
        // current != "". This is the phantom "(orig: )" guard.
        assert!(!branch_drifted("main", ""));
        assert!(!branch_drifted("", ""));
    }

    #[test]
    fn the_strips_show_together_for_a_second_tab_a_second_terminal_or_the_setting() {
        let auto = StripSetting::WhenNeeded;
        assert!(!strips_shown(1, 0, auto));
        assert!(!strips_shown(1, 1, auto));
        assert!(strips_shown(2, 0, auto));
        assert!(strips_shown(1, 2, auto));
        assert!(strips_shown(1, 1, StripSetting::Always));
    }
}
