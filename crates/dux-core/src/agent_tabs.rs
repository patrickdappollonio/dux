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
