//! The sidebar row "state" priority ladder, core-owned and shared by rule with
//! the web's `flatList.ts` `stateWord` and `flatTerminals.ts` `terminalStateWord`.
//!
//! The DECISION (which state a row is in, and its priority) lives here as a typed
//! [`RowState`]; each surface maps that to its own WORD and color. The wording
//! deliberately differs per surface for the busy state (an agent "Working" vs a
//! terminal "Running"), so this returns a typed value rather than a string, per
//! the same split used elsewhere in the codebase. The TS mirror is pinned by
//! shared test vectors (`agent_search.rs` / `agentSearch.ts` style).

use crate::model::SessionStatus;

/// A row's resolved state, in priority order. `NeedsAttention` outranks
/// everything; for a live (active) row `Typing` outranks `Busy` outranks `Idle`;
/// the non-active statuses map to `Detached`/`Exited`. Terminals only ever
/// produce `Typing`/`Busy`/`Idle` (a terminal is a live PTY: it cannot be
/// detached/exited and raises no attention signal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    NeedsAttention,
    Typing,
    Busy,
    Idle,
    Detached,
    Exited,
}

/// The agent-row state ladder: needs-attention wins; then an active agent that is
/// typing outranks working outranks idle; then the non-active statuses.
pub fn agent_row_state(
    status: SessionStatus,
    working: bool,
    typing: bool,
    needs_attention: bool,
) -> RowState {
    if needs_attention {
        return RowState::NeedsAttention;
    }
    match status {
        SessionStatus::Active if typing => RowState::Typing,
        SessionStatus::Active if working => RowState::Busy,
        SessionStatus::Active => RowState::Idle,
        SessionStatus::Detached => RowState::Detached,
        SessionStatus::Exited => RowState::Exited,
    }
}

/// The terminal-row state ladder: typing outranks busy (running) outranks idle.
/// A terminal has no needs-attention/detached/exited states.
pub fn terminal_row_state(working: bool, typing: bool) -> RowState {
    if typing {
        RowState::Typing
    } else if working {
        RowState::Busy
    } else {
        RowState::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── SHARED VECTORS with flatList.test.ts `stateWord` ───────────────────────
    #[test]
    fn agent_row_state_priority_ladder() {
        use SessionStatus::{Active, Detached, Exited};
        // (status, working, typing, needs_attention)
        assert_eq!(agent_row_state(Active, true, false, false), RowState::Busy,);
        assert_eq!(agent_row_state(Active, false, false, false), RowState::Idle,);
        assert_eq!(
            agent_row_state(Detached, false, false, false),
            RowState::Detached,
        );
        assert_eq!(
            agent_row_state(Exited, false, false, false),
            RowState::Exited,
        );
        // Typing outranks working for an active agent.
        assert_eq!(agent_row_state(Active, true, true, false), RowState::Typing);
        // Needs-attention wins over every other state, including working and a
        // non-active status.
        assert_eq!(
            agent_row_state(Active, true, false, true),
            RowState::NeedsAttention,
        );
        assert_eq!(
            agent_row_state(Detached, false, false, true),
            RowState::NeedsAttention,
        );
    }

    // ── SHARED VECTORS with flatTerminals.test.ts `terminalStateWord` ──────────
    #[test]
    fn terminal_row_state_priority_ladder() {
        assert_eq!(terminal_row_state(false, false), RowState::Idle);
        assert_eq!(terminal_row_state(true, false), RowState::Busy);
        assert_eq!(terminal_row_state(false, true), RowState::Typing);
        // Typing outranks running.
        assert_eq!(terminal_row_state(true, true), RowState::Typing);
    }
}
