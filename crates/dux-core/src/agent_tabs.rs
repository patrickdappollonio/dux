//! Pure agent-tab derivations shared, by rule, with the web's
//! `crates/dux-web/web/src/lib/agentTabs.ts`. These are cross-language TWINS:
//! the DECISION lives here in Rust (the core-owned source of truth), the TS file
//! keeps a hand-written mirror, and the two are pinned by SHARED TEST VECTORS
//! (the cases in this module's tests are duplicated verbatim in
//! `agentTabs.test.ts`, in the `agent_search.rs` / `agentSearch.ts` style) so a
//! change to one language's rule that is not mirrored fails a test.
//!
//! Keep these functions small and pure; anything needing engine state belongs on
//! `Engine`, not here.

use std::collections::HashMap;

/// The one sentence dux says when closing a tab would leave an agent with no
/// tab at all. Owned here so the engine's refusal, the terminal UI's status
/// line and the browser's disabled menu item are the same words in the same
/// casing; the browser mirrors it in `agentTabs.ts` and a test in each language
/// pins the literal.
pub const ONLY_TAB_CLOSE_REFUSAL: &str = "This is the agent's only tab, so closing it would leave the agent with no tab at all. Detach the agent instead to stop everything it is running, or add another tab first.";

/// The way PROSE names a tab: the strip's label with its first character
/// upper-cased. Pills are lower-case because they are chrome; a sentence names
/// a tab the way a sentence names anything, and the disambiguating suffix rides
/// along, so "Codex 2" in a confirmation is the pill the user is looking at.
/// Mirrors the web's `tabProseLabel`.
pub fn prose_tab_label(strip_label: &str) -> String {
    let mut chars = strip_label.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// Disambiguated tab labels: each provider name is used as-is the first time and
/// suffixed with " 2", " 3", … for repeats, in tab order. The first occurrence
/// stays bare. Mirrors the web's `tabLabels`.
pub fn tab_labels(providers: &[&str]) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::with_capacity(providers.len());
    for p in providers {
        let n = seen.entry(*p).or_insert(0);
        *n += 1;
        if *n == 1 {
            out.push((*p).to_string());
        } else {
            out.push(format!("{p} {n}"));
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── SHARED VECTORS with agentTabs.test.ts `tabLabels` ──────────────────────
    #[test]
    fn tab_labels_disambiguate_duplicate_providers() {
        assert_eq!(tab_labels(&["claude", "codex"]), vec!["claude", "codex"]);
        assert_eq!(
            tab_labels(&["codex", "codex"]),
            vec!["codex".to_string(), "codex 2".to_string()]
        );
        assert_eq!(
            tab_labels(&["claude", "codex", "codex", "claude"]),
            vec![
                "claude".to_string(),
                "codex".to_string(),
                "codex 2".to_string(),
                "claude 2".to_string()
            ]
        );
        assert!(tab_labels(&[]).is_empty());
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
}
