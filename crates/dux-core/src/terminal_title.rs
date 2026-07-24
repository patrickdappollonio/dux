//! Terminal display-title derivation, core-owned and shared by rule with the
//! web's `crates/dux-web/web/src/lib/terminals.ts`. Cross-language twin: the
//! DECISION (foreground normalization + the collision "(#N)" ordinal rule) lives
//! here in Rust, the TS keeps a hand-written mirror, and the two are pinned by
//! SHARED TEST VECTORS (the cases below are duplicated in `terminals.test.ts`, in
//! the `agent_search.rs` / `agentSearch.ts` style).
//!
//! Chosen over a viewmodel `display_title` field because the two TUI read sites
//! (the sidebar row and the Kill-Running overlay) compute this from engine-local
//! terminal state, not from the viewmodel, so they need the function regardless;
//! and the web still needs the nullable normalized foreground on its own (idle
//! vs running decision, the delete dialog), so `terminals.ts` cannot be reduced
//! to a pure field read anyway. One core rule, called by every site.

/// The terminal's NORMALIZED foreground command, or `None` when the shell itself
/// is in the foreground (idle). Trims, strips a leading `"TERM "`/`"term "`
/// prefix off the trimmed string, then discards the result if it is empty/blank.
pub fn terminal_foreground_display(foreground_cmd: Option<&str>) -> Option<String> {
    let trimmed = foreground_cmd?.trim();
    let cmd = trimmed
        .strip_prefix("TERM ")
        .or_else(|| trimmed.strip_prefix("term "))
        .unwrap_or(trimmed);
    if cmd.trim().is_empty() {
        None
    } else {
        Some(cmd.to_string())
    }
}

/// The terminal's number, parsed from the trailing digits of its "Terminal N"
/// label. `None` for a label with no trailing number (never happens for
/// engine-assigned labels, but keeps the helper total).
fn terminal_number(label: &str) -> Option<u64> {
    let digits: String = label
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok()
}

/// The terminal's display title. When an app is running its command name is the
/// most useful label ("vim", "htop"), surfaced alone rather than the redundant
/// "Terminal N" suffix; the stable label returns the moment the app exits. The
/// one exception is COLLISION: when another terminal in `sibling_foreground_cmds`
/// (the RAW foregrounds of the other terminals shown together, owner-scoped)
/// normalizes to the same app, both would read identically, so disambiguate with
/// the terminal's own counter number ("vim (#1)", "vim (#2)"). Deterministic and
/// identical on every surface.
pub fn terminal_title(
    label: &str,
    own_foreground_cmd: Option<&str>,
    sibling_foreground_cmds: &[Option<&str>],
) -> String {
    let Some(cmd) = terminal_foreground_display(own_foreground_cmd) else {
        return label.to_string();
    };
    let collision = sibling_foreground_cmds
        .iter()
        .any(|other| terminal_foreground_display(*other).as_deref() == Some(cmd.as_str()));
    if !collision {
        return cmd;
    }
    match terminal_number(label) {
        Some(n) => format!("{cmd} (#{n})"),
        None => format!("{cmd} ({label})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── SHARED VECTORS with terminals.test.ts `terminalForeground` ─────────────
    #[test]
    fn foreground_normalization() {
        assert_eq!(terminal_foreground_display(None), None);
        assert_eq!(
            terminal_foreground_display(Some("vim")).as_deref(),
            Some("vim")
        );
        assert_eq!(
            terminal_foreground_display(Some("  npm  ")).as_deref(),
            Some("npm")
        );
        assert_eq!(
            terminal_foreground_display(Some("TERM vim")).as_deref(),
            Some("vim")
        );
        assert_eq!(
            terminal_foreground_display(Some("term vim")).as_deref(),
            Some("vim")
        );
        assert_eq!(terminal_foreground_display(Some("")), None);
        assert_eq!(terminal_foreground_display(Some("   ")), None);
        // A bare "TERM " whose trailing space is trimmed away keeps "TERM" (the
        // prefix no longer matches after the trim).
        assert_eq!(
            terminal_foreground_display(Some("TERM ")).as_deref(),
            Some("TERM")
        );
        assert_eq!(
            terminal_foreground_display(Some("  TERM vim  ")).as_deref(),
            Some("vim")
        );
    }

    // ── SHARED VECTORS with terminals.test.ts `terminalTitle` ──────────────────
    // Note: the web passes siblings INCLUDING self and skips self by id; this core
    // fn takes the OTHER siblings' foregrounds (self already excluded by the
    // caller). The RULE is identical: idle -> label, unique -> normalized cmd,
    // collision with another same-app terminal -> "cmd (#N)".
    #[test]
    fn title_idle_running_and_collision() {
        // Idle: just the label.
        assert_eq!(terminal_title("Terminal 1", None, &[]), "Terminal 1");
        // Running and unique (no other siblings): the app name alone.
        assert_eq!(terminal_title("Terminal 1", Some("vim"), &[]), "vim");
        // Normalizes the running name.
        assert_eq!(terminal_title("Terminal 1", Some("TERM htop"), &[]), "htop");
        // A command that normalizes to empty falls back to the label.
        assert_eq!(terminal_title("Terminal 1", Some("   "), &[]), "Terminal 1");
        // Collision: another sibling runs the same app -> "(#N)".
        assert_eq!(
            terminal_title("Terminal 2", Some("vim"), &[Some("vim")]),
            "vim (#2)"
        );
        // Collision is decided against the NORMALIZED sibling foreground.
        assert_eq!(
            terminal_title("Terminal 3", Some("vim"), &[Some("TERM vim")]),
            "vim (#3)"
        );
    }
}
