//! The flat agent-list ORDERING decision, core-owned and shared by rule with the
//! web's `crates/dux-web/web/src/lib/flatList.ts` + `sortSessions.ts`. This is a
//! cross-language twin: the rule lives here in Rust (the source of truth), the TS
//! keeps a hand-written mirror, and the two are pinned by SHARED TEST VECTORS
//! (the cases below are duplicated in `flatList.test.ts` / `sortSessions.test.ts`,
//! in the `agent_search.rs` / `agentSearch.ts` style) so a comparator change in
//! one language that is not mirrored fails a test.
//!
//! The list is a single globally-ordered agent list with no project grouping.
//! Active agents form the MAIN bucket; detached/exited ("inactive"/"quiet") agents
//! form a collapsible TAIL. This module owns only the ORDERING (which indices, in
//! which order); each surface wraps the result into its own list-item type.

use crate::model::{AgentSession, SessionStatus};

/// The display sort applied to the flat list. Mirrors the TUI's `AgentSortMode`
/// and the web's `FlatSortKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatSortMode {
    /// Working / needs-attention agents float to the top (a stable float, not a
    /// re-sort); the inactive tail sorts most-recently-active-first. The default.
    Active,
    /// Most recently updated first (`Reverse(updated_at)`).
    Updated,
    /// Most recently created first (`Reverse(created_at)`).
    Created,
    /// By name (title-or-branch, case-insensitive), A to Z.
    NameAsc,
    /// By name (title-or-branch, case-insensitive), Z to A.
    NameDesc,
    /// The stored global order verbatim (the web's drag-reorder order).
    Manual,
}

/// The ordered result: main (active) indices, then the inactive/quiet tail
/// indices. Indices point into the input `sessions` slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatOrder {
    pub active: Vec<usize>,
    pub inactive: Vec<usize>,
}

/// Order the sessions for the flat list. `is_hot(index)` reports whether the
/// session is working or needs attention (used only by `Active`'s float).
/// `is_visible(index)` is a symmetric DISPLAY filter: an index that fails it never
/// enters either bucket. `sessions` is never mutated.
///
/// The partition always preserves incoming order first, then each bucket is
/// ordered by `mode`:
/// - `Active`: the main bucket floats `is_hot` indices up (a STABLE float keeping
///   incoming order within each group); the inactive tail sorts by most recently
///   updated (`Reverse(updated_at)`), most-recently-active-first.
/// - `Updated` / `Created` / `NameAsc` / `NameDesc`: the MAIN bucket is sorted by
///   that comparator; the inactive tail stays VERBATIM (only Active reorders the
///   tail, so the two surfaces agree).
/// - `Manual`: both buckets verbatim (the stored global order).
pub fn order_sessions(
    sessions: &[AgentSession],
    mode: FlatSortMode,
    is_hot: &dyn Fn(usize) -> bool,
    is_visible: &dyn Fn(usize) -> bool,
) -> FlatOrder {
    let mut active: Vec<usize> = Vec::new();
    let mut inactive: Vec<usize> = Vec::new();
    for (index, session) in sessions.iter().enumerate() {
        if !is_visible(index) {
            continue;
        }
        match session.status {
            SessionStatus::Detached | SessionStatus::Exited => inactive.push(index),
            _ => active.push(index),
        }
    }

    // Case-insensitive name key: title-or-branch lowercased. Rust's `str::cmp` on
    // this lowercased key matches the web's code-point comparison in
    // `sortSessions.ts` (UTF-8 byte order equals code-point order).
    let name_key = |index: usize| -> String {
        let s = &sessions[index];
        s.display_label().to_lowercase()
    };
    // Comparator-based ordering for the MAIN bucket in the non-Active modes.
    // `sort_by_key` is stable, so equal keys keep incoming order.
    let order_bucket = |bucket: &mut Vec<usize>| match mode {
        FlatSortMode::Updated => bucket.sort_by_key(|&i| std::cmp::Reverse(sessions[i].updated_at)),
        FlatSortMode::Created => bucket.sort_by_key(|&i| std::cmp::Reverse(sessions[i].created_at)),
        FlatSortMode::NameAsc => bucket.sort_by_key(|&i| name_key(i)),
        FlatSortMode::NameDesc => bucket.sort_by_key(|&i| std::cmp::Reverse(name_key(i))),
        FlatSortMode::Active | FlatSortMode::Manual => {}
    };

    match mode {
        FlatSortMode::Active => {
            // Stable float: hot indices rise above the rest, each group keeping
            // incoming order.
            let mut hot: Vec<usize> = Vec::new();
            let mut rest: Vec<usize> = Vec::new();
            for &i in &active {
                if is_hot(i) {
                    hot.push(i);
                } else {
                    rest.push(i);
                }
            }
            active = hot;
            active.extend(rest);
            // The collapsed tail sorts most-recently-active-first.
            inactive.sort_by_key(|&i| std::cmp::Reverse(sessions[i].updated_at));
        }
        FlatSortMode::Manual => {
            // Both buckets stay verbatim (incoming order).
        }
        _ => {
            // Only the MAIN bucket sorts by the comparator; the inactive tail stays
            // verbatim (only Active reorders the tail).
            order_bucket(&mut active);
        }
    }

    FlatOrder { active, inactive }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::sample_session;
    use chrono::{TimeZone, Utc};

    fn session(id: &str, status: SessionStatus, updated_hour: u32, name: &str) -> AgentSession {
        let mut s = sample_session(id, "p1", name);
        s.status = status;
        s.title = Some(name.to_string());
        s.updated_at = Utc
            .with_ymd_and_hms(2026, 7, 17, updated_hour, 0, 0)
            .unwrap();
        s.created_at = Utc
            .with_ymd_and_hms(2026, 7, 17, updated_hour, 0, 0)
            .unwrap();
        s
    }

    // ── SHARED VECTORS with flatList.test.ts ───────────────────────────────────

    #[test]
    fn active_mode_floats_hot_and_recency_sorts_the_tail() {
        // main: zeta(active), alpha(active), hot(active+hot); tail: gone(exited),
        // parked(detached) with distinct updated_at.
        let sessions = vec![
            session("zeta", SessionStatus::Active, 12, "zeta"),
            session("gone", SessionStatus::Exited, 10, "gone"),
            session("alpha", SessionStatus::Active, 12, "alpha"),
            session("hot", SessionStatus::Active, 12, "hot"),
            session("parked", SessionStatus::Detached, 14, "parked"),
        ];
        let is_hot = |i: usize| sessions[i].id == "hot";
        let order = order_sessions(&sessions, FlatSortMode::Active, &is_hot, &|_| true);
        // Hot floats up, the rest keep incoming order.
        assert_eq!(order.active, vec![3, 0, 2]); // hot, zeta, alpha
        // Tail is most-recently-updated first: parked(14) before gone(10).
        assert_eq!(order.inactive, vec![4, 1]); // parked, gone
    }

    #[test]
    fn non_active_modes_leave_the_tail_verbatim() {
        let sessions = vec![
            session("old", SessionStatus::Detached, 10, "old"),
            session("newest", SessionStatus::Exited, 14, "newest"),
            session("mid", SessionStatus::Detached, 12, "mid"),
        ];
        // Name mode: no active rows, tail stays in incoming order (old, newest, mid).
        let order = order_sessions(&sessions, FlatSortMode::NameAsc, &|_| false, &|_| true);
        assert!(order.active.is_empty());
        assert_eq!(order.inactive, vec![0, 1, 2]);
    }

    #[test]
    fn name_mode_sorts_the_main_bucket_only() {
        let sessions = vec![
            session("b", SessionStatus::Active, 12, "bravo"),
            session("a", SessionStatus::Active, 12, "alpha"),
        ];
        let order = order_sessions(&sessions, FlatSortMode::NameAsc, &|_| false, &|_| true);
        assert_eq!(order.active, vec![1, 0]); // alpha, bravo
    }

    #[test]
    fn manual_keeps_both_buckets_verbatim() {
        let sessions = vec![
            session("z", SessionStatus::Active, 12, "z"),
            session("gone", SessionStatus::Exited, 10, "gone"),
            session("a", SessionStatus::Active, 12, "a"),
            session("parked", SessionStatus::Detached, 14, "parked"),
        ];
        let order = order_sessions(&sessions, FlatSortMode::Manual, &|_| false, &|_| true);
        assert_eq!(order.active, vec![0, 2]);
        assert_eq!(order.inactive, vec![1, 3]); // verbatim, NOT recency-sorted
    }

    #[test]
    fn invisible_indices_enter_neither_bucket() {
        let sessions = vec![
            session("a", SessionStatus::Active, 12, "a"),
            session("b", SessionStatus::Active, 12, "b"),
        ];
        let order = order_sessions(&sessions, FlatSortMode::Active, &|_| false, &|i| {
            sessions[i].id == "a"
        });
        assert_eq!(order.active, vec![0]);
    }
}
