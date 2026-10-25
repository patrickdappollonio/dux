//! The sidebar row "state" priority ladder, core-owned and shared by rule with
//! the web's `flatList.ts` and `flatTerminals.ts`.
//!
//! The DECISION, which state a row is in and its priority, lives here as a typed
//! [`RowState`]; each surface maps that to its own WORD and color, because the
//! wording deliberately differs per surface for the busy state (an agent
//! "Working" against a terminal "Running"). The TS mirror is pinned by shared
//! test vectors.

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

/// What an agent row's live state is decided from, beside its status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentRowFacts {
    /// The agent is working.
    pub working: bool,
    /// Someone is typing into it.
    pub typing: bool,
    /// It asked for attention.
    pub needs_attention: bool,
}

/// What a terminal row's live state is decided from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalRowFacts {
    /// Output is streaming or a foreground app runs in it.
    pub working: bool,
    /// Someone is typing into it.
    pub typing: bool,
}

/// The agent-row state ladder: needs-attention wins; then an active agent that is
/// typing outranks working outranks idle; then the non-active statuses.
pub fn agent_row_state(status: SessionStatus, facts: AgentRowFacts) -> RowState {
    let AgentRowFacts {
        working,
        typing,
        needs_attention,
    } = facts;
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
pub fn terminal_row_state(facts: TerminalRowFacts) -> RowState {
    let TerminalRowFacts { working, typing } = facts;
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
        assert_eq!(
            agent_row_state(
                Active,
                AgentRowFacts {
                    working: true,
                    typing: false,
                    needs_attention: false
                }
            ),
            RowState::Busy,
        );
        assert_eq!(
            agent_row_state(
                Active,
                AgentRowFacts {
                    working: false,
                    typing: false,
                    needs_attention: false
                }
            ),
            RowState::Idle,
        );
        assert_eq!(
            agent_row_state(
                Detached,
                AgentRowFacts {
                    working: false,
                    typing: false,
                    needs_attention: false
                }
            ),
            RowState::Detached,
        );
        assert_eq!(
            agent_row_state(
                Exited,
                AgentRowFacts {
                    working: false,
                    typing: false,
                    needs_attention: false
                }
            ),
            RowState::Exited,
        );
        // Typing outranks working for an active agent.
        assert_eq!(
            agent_row_state(
                Active,
                AgentRowFacts {
                    working: true,
                    typing: true,
                    needs_attention: false
                }
            ),
            RowState::Typing
        );
        // Needs-attention wins over every other state, including working and a
        // non-active status.
        assert_eq!(
            agent_row_state(
                Active,
                AgentRowFacts {
                    working: true,
                    typing: false,
                    needs_attention: true
                }
            ),
            RowState::NeedsAttention,
        );
        assert_eq!(
            agent_row_state(
                Detached,
                AgentRowFacts {
                    working: false,
                    typing: false,
                    needs_attention: true
                }
            ),
            RowState::NeedsAttention,
        );
    }

    // ── SHARED VECTORS with flatTerminals.test.ts `terminalStateWord` ──────────
    #[test]
    fn terminal_row_state_priority_ladder() {
        assert_eq!(
            terminal_row_state(TerminalRowFacts {
                working: false,
                typing: false
            }),
            RowState::Idle
        );
        assert_eq!(
            terminal_row_state(TerminalRowFacts {
                working: true,
                typing: false
            }),
            RowState::Busy
        );
        assert_eq!(
            terminal_row_state(TerminalRowFacts {
                working: false,
                typing: true
            }),
            RowState::Typing
        );
        // Typing outranks running.
        assert_eq!(
            terminal_row_state(TerminalRowFacts {
                working: true,
                typing: true
            }),
            RowState::Typing
        );
    }
}
