//! The flat list's ordering decision, core-owned and mirrored by hand in the
//! web's `crates/dux-web/web/src/lib/flatList.ts` and `sortSessions.ts`. The rule
//! lives here; the two are pinned by shared test vectors, the agent-only ones
//! duplicated in `flatList.test.ts` and `sortSessions.test.ts` and the ones with
//! terminal rows in `tests/fixtures/flat_list_cross_language.json`, which both
//! languages read, so an unmirrored comparator change fails a test.
//!
//! The list is a single globally-ordered list with no project grouping: active
//! agents and project and standalone terminals form the main bucket, and
//! detached or exited agents with no terminal of their own running form a
//! collapsible tail. This module owns only the ordering; each surface wraps the
//! result into its own list-item type.

use chrono::{DateTime, Utc};

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

/// The ordered agent result: main (active) indices, then the inactive/quiet
/// tail indices. Indices point into the input `sessions` slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatOrder {
    /// The active agents, in display order.
    pub active: Vec<usize>,
    /// The inactive agents, in display order.
    pub inactive: Vec<usize>,
}

/// One row of the list's main bucket: an agent or a project or standalone
/// terminal. Indices point into the `sessions` and `terminals` inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatRow {
    /// An agent, by its index into the sessions.
    Agent(usize),
    /// A project or standalone terminal, by its index into the terminals.
    Terminal(usize),
}

/// The ordered list: the main bucket of agents and terminals together, then
/// the inactive tail, which holds agents only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatRows {
    /// The active agents and the terminals, in display order.
    pub main: Vec<FlatRow>,
    /// The inactive agents, in display order.
    pub inactive: Vec<usize>,
}

/// What the list sorts a project or standalone terminal row by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRowFacts<'a> {
    /// The name it was opened with, never its live title, so a program
    /// renaming its window does not move the row.
    pub name: &'a str,
    /// When it was opened.
    pub created_at: DateTime<Utc>,
    /// Its last output, or when it was opened before any.
    pub updated_at: DateTime<Utc>,
    /// A running terminal counts as busy, the way a working agent is hot.
    pub busy: bool,
    /// Its manual place: right after this agent in the stored agent order,
    /// first when `None`, last when the agent is gone. Runtime only, so the
    /// stored agent order never names a terminal.
    pub after_agent: Option<&'a str>,
}

/// What a surface knows about each agent beyond the session itself.
pub struct AgentRowFacts<'a> {
    /// Working or needing attention (used only by `Active`'s float).
    pub is_hot: &'a dyn Fn(usize) -> bool,
    /// A symmetric display filter: an index that fails it enters neither
    /// bucket.
    pub is_visible: &'a dyn Fn(usize) -> bool,
    /// Whether one of the agent's own terminals is running.
    pub has_running_terminal: &'a dyn Fn(usize) -> bool,
}

impl std::fmt::Debug for AgentRowFacts<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The facts are closures, which have nothing to print.
        f.debug_struct("AgentRowFacts").finish_non_exhaustive()
    }
}

/// Whether a session is inactive: detached or exited.
///
/// The pull-request poller puts exactly these agents on its slow clock. It
/// judges the agent alone, which is the poller's question; the list's tail asks
/// [`in_inactive_tail`], which also keeps an agent whose own terminals are
/// still running out of the tail.
///
/// The web does NOT share this one. Its mirror is `isQuietSession` in
/// `crates/dux-web/web/src/lib/flatList.ts`, hand-written like the rest of that
/// module's twins of this one, and the two must stay in step: change the rule
/// here and change it there in the same commit.
pub(crate) fn is_inactive(session: &AgentSession) -> bool {
    matches!(
        session.status,
        SessionStatus::Detached | SessionStatus::Exited
    )
}

/// Whether a session belongs in the list's INACTIVE tail: detached or exited,
/// and with none of its own terminals running. An agent stopped with its
/// terminals left going was stopped on purpose and is still in use, so it
/// stays in the main list until its last terminal ends.
///
/// This is the list's question only. The pull-request poller asks
/// [`is_inactive`], which judges the agent alone.
///
/// The web's twin is `inInactiveTail` in `crates/dux-web/web/src/lib/flatList.ts`;
/// both are held to the same answers by `tests/fixtures/flat_list_cross_language.json`.
pub fn in_inactive_tail(session: &AgentSession, own_terminals: OwnTerminals) -> bool {
    is_inactive(session) && own_terminals == OwnTerminals::NoneRunning
}

/// Whether any of an agent's own terminals is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnTerminals {
    /// At least one is.
    Running,
    /// None is.
    NoneRunning,
}

impl From<bool> for OwnTerminals {
    /// The answer for whether one is running.
    fn from(running: bool) -> Self {
        if running {
            Self::Running
        } else {
            Self::NoneRunning
        }
    }
}

/// Order the agents alone for the flat list. See [`order_rows`] for the rules;
/// this is that order with no terminal rows in it.
pub fn order_sessions(
    sessions: &[AgentSession],
    mode: FlatSortMode,
    facts: &AgentRowFacts<'_>,
) -> FlatOrder {
    let rows = order_rows(sessions, &[], mode, facts, &|_| true);
    FlatOrder {
        active: rows
            .main
            .into_iter()
            .filter_map(|row| match row {
                FlatRow::Agent(index) => Some(index),
                FlatRow::Terminal(_) => None,
            })
            .collect(),
        inactive: rows.inactive,
    }
}

/// Order the agents and the project and standalone terminals for the flat
/// list. `sessions` and `terminals` are never mutated.
///
/// The partition always preserves incoming order first: agents in stored
/// order, each terminal placed by its `after_agent`. Each bucket is then
/// ordered by `mode`:
/// - `Active`: the main bucket floats hot agents and busy terminals up, stably,
///   keeping incoming order within each group; the inactive tail sorts by
///   `Reverse(updated_at)`, most-recently-active first.
/// - `Updated` / `Created` / `NameAsc` / `NameDesc`: the main bucket is sorted by
///   that comparator over agents and terminals together, and the inactive tail
///   stays verbatim, since only `Active` reorders the tail.
/// - `Manual`: both buckets verbatim.
///
/// Terminals are never in the tail.
pub fn order_rows(
    sessions: &[AgentSession],
    terminals: &[TerminalRowFacts<'_>],
    mode: FlatSortMode,
    facts: &AgentRowFacts<'_>,
    terminal_visible: &dyn Fn(usize) -> bool,
) -> FlatRows {
    let mut active: Vec<usize> = Vec::new();
    let mut inactive: Vec<usize> = Vec::new();
    for (index, session) in sessions.iter().enumerate() {
        if !(facts.is_visible)(index) {
            continue;
        }
        if in_inactive_tail(
            session,
            OwnTerminals::from((facts.has_running_terminal)(index)),
        ) {
            inactive.push(index);
        } else {
            active.push(index);
        }
    }

    let mut main = interleave_terminals(sessions, &active, terminals, terminal_visible);

    // Case-insensitive name key: an agent's title-or-branch, a terminal's
    // opened-with name, lowercased. Rust's `str::cmp` on this key matches the
    // web's code-point comparison (UTF-8 byte order equals code-point order).
    let name_key = |row: &FlatRow| -> String {
        match *row {
            FlatRow::Agent(index) => sessions[index].display_label().to_lowercase(),
            FlatRow::Terminal(index) => terminals[index].name.to_lowercase(),
        }
    };
    let updated = |row: &FlatRow| match *row {
        FlatRow::Agent(index) => sessions[index].updated_at,
        FlatRow::Terminal(index) => terminals[index].updated_at,
    };
    let created = |row: &FlatRow| match *row {
        FlatRow::Agent(index) => sessions[index].created_at,
        FlatRow::Terminal(index) => terminals[index].created_at,
    };
    let hot = |row: &FlatRow| match *row {
        FlatRow::Agent(index) => (facts.is_hot)(index),
        FlatRow::Terminal(index) => terminals[index].busy,
    };

    match mode {
        FlatSortMode::Active => {
            // Stable float: hot rows rise above the rest, each group keeping
            // incoming order.
            let (hot_rows, rest): (Vec<FlatRow>, Vec<FlatRow>) = main.into_iter().partition(hot);
            main = hot_rows;
            main.extend(rest);
            // The collapsed tail sorts most-recently-active-first.
            inactive.sort_by_key(|&i| std::cmp::Reverse(sessions[i].updated_at));
        }
        // Both buckets stay verbatim (incoming order).
        FlatSortMode::Manual => {}
        // Only the MAIN bucket sorts by the comparator; the inactive tail stays
        // verbatim (only Active reorders the tail). `sort_by_key` is stable, so
        // equal keys keep incoming order.
        FlatSortMode::Updated => main.sort_by_key(|row| std::cmp::Reverse(updated(row))),
        FlatSortMode::Created => main.sort_by_key(|row| std::cmp::Reverse(created(row))),
        FlatSortMode::NameAsc => main.sort_by_key(name_key),
        FlatSortMode::NameDesc => main.sort_by_key(|row| std::cmp::Reverse(name_key(row))),
    }

    FlatRows { main, inactive }
}

/// Every agent and terminal row in stored order, the inactive agents left where
/// the stored order has them: the order a hand-placement is made against.
pub fn manual_rows(sessions: &[AgentSession], terminals: &[TerminalRowFacts<'_>]) -> Vec<FlatRow> {
    let all: Vec<usize> = (0..sessions.len()).collect();
    interleave_terminals(sessions, &all, terminals, &|_| true)
}

/// The main bucket in incoming order: the active agents in stored order, each
/// visible terminal right after the agent it names, terminals that name the
/// same agent in their input order, a terminal naming no agent first, and one
/// naming an agent that is gone last.
fn interleave_terminals(
    sessions: &[AgentSession],
    active: &[usize],
    terminals: &[TerminalRowFacts<'_>],
    terminal_visible: &dyn Fn(usize) -> bool,
) -> Vec<FlatRow> {
    let visible: Vec<usize> = (0..terminals.len())
        .filter(|&t| terminal_visible(t))
        .collect();
    let after = |t: usize, id: Option<&str>| terminals[t].after_agent == id;
    let mut placed = vec![false; terminals.len()];
    let mut rows: Vec<FlatRow> = Vec::with_capacity(active.len() + visible.len());
    let place = |rows: &mut Vec<FlatRow>, placed: &mut Vec<bool>, id: Option<&str>| {
        for &t in &visible {
            if !placed[t] && after(t, id) {
                placed[t] = true;
                rows.push(FlatRow::Terminal(t));
            }
        }
    };
    place(&mut rows, &mut placed, None);
    // Anchored to the STORED order, so a terminal keeps its place among the
    // agents whether its anchor is on screen, filtered out or in the tail.
    let active_set: std::collections::HashSet<usize> = active.iter().copied().collect();
    for (index, session) in sessions.iter().enumerate() {
        if active_set.contains(&index) {
            rows.push(FlatRow::Agent(index));
        }
        place(&mut rows, &mut placed, Some(session.id.as_str()));
    }
    for &t in &visible {
        if !placed[t] {
            rows.push(FlatRow::Terminal(t));
        }
    }
    rows
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
        let order = order_sessions(
            &sessions,
            FlatSortMode::Active,
            &AgentRowFacts {
                is_hot: &is_hot,
                is_visible: &|_| true,
                has_running_terminal: &|_| false,
            },
        );
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
        let order = order_sessions(
            &sessions,
            FlatSortMode::NameAsc,
            &AgentRowFacts {
                is_hot: &|_| false,
                is_visible: &|_| true,
                has_running_terminal: &|_| false,
            },
        );
        assert_eq!(order.active, Vec::<usize>::new());
        assert_eq!(order.inactive, vec![0, 1, 2]);
    }

    /// A standalone agent sorts by the same key as any other: its display label.
    ///
    /// The key is `display_label`, which for a standalone agent with no title
    /// falls back to its folder's own name rather than through a branch it does
    /// not have. Pinned because the flat list is a single global order with no
    /// project grouping, so an agent with no project is not a special case here
    /// and must not become one.
    #[test]
    fn a_standalone_agent_sorts_by_its_label_like_any_other() {
        let mut untitled = session("sa1", SessionStatus::Active, 12, "ignored");
        untitled.title = None;
        untitled.workspace = crate::model::AgentWorkspace::Folder(crate::model::FolderWorkspace {
            folder_path: "/home/someone/beta".to_string(),
        });
        let sessions = vec![
            session("z", SessionStatus::Active, 12, "zulu"),
            untitled,
            session("a", SessionStatus::Active, 12, "alpha"),
        ];
        let order = order_sessions(
            &sessions,
            FlatSortMode::NameAsc,
            &AgentRowFacts {
                is_hot: &|_| false,
                is_visible: &|_| true,
                has_running_terminal: &|_| false,
            },
        );
        // alpha, beta (the folder's name), zulu.
        assert_eq!(order.active, vec![2, 1, 0]);

        let order = order_sessions(
            &sessions,
            FlatSortMode::NameDesc,
            &AgentRowFacts {
                is_hot: &|_| false,
                is_visible: &|_| true,
                has_running_terminal: &|_| false,
            },
        );
        assert_eq!(order.active, vec![0, 1, 2]);
    }

    #[test]
    fn name_mode_sorts_the_main_bucket_only() {
        let sessions = vec![
            session("b", SessionStatus::Active, 12, "bravo"),
            session("a", SessionStatus::Active, 12, "alpha"),
        ];
        let order = order_sessions(
            &sessions,
            FlatSortMode::NameAsc,
            &AgentRowFacts {
                is_hot: &|_| false,
                is_visible: &|_| true,
                has_running_terminal: &|_| false,
            },
        );
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
        let order = order_sessions(
            &sessions,
            FlatSortMode::Manual,
            &AgentRowFacts {
                is_hot: &|_| false,
                is_visible: &|_| true,
                has_running_terminal: &|_| false,
            },
        );
        assert_eq!(order.active, vec![0, 2]);
        assert_eq!(order.inactive, vec![1, 3]); // verbatim, NOT recency-sorted
    }

    #[test]
    fn invisible_indices_enter_neither_bucket() {
        let sessions = vec![
            session("a", SessionStatus::Active, 12, "a"),
            session("b", SessionStatus::Active, 12, "b"),
        ];
        let order = order_sessions(
            &sessions,
            FlatSortMode::Active,
            &AgentRowFacts {
                is_hot: &|_| false,
                is_visible: &|i| sessions[i].id == "a",
                has_running_terminal: &|_| false,
            },
        );
        assert_eq!(order.active, vec![0]);
    }

    /// The ordering the browser runs over the same rows: agents and project and
    /// standalone terminals together, under every sort mode, with the tail rule.
    #[test]
    fn the_shared_flat_list_fixture_holds() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/flat_list_cross_language.json");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let parsed: serde_json::Value = serde_json::from_str(&source).expect("valid JSON");
        let cases = parsed["cases"].as_array().expect("a cases array");
        assert!(!cases.is_empty(), "the fixture has lost its cases");
        let time = |value: &serde_json::Value| -> DateTime<Utc> {
            value
                .as_str()
                .expect("a time")
                .parse()
                .expect("an RFC 3339 time")
        };
        let ids = |value: &serde_json::Value| -> Vec<String> {
            value
                .as_array()
                .expect("an id list")
                .iter()
                .map(|id| id.as_str().expect("an id").to_string())
                .collect()
        };
        for case in cases {
            let what = case["what"].as_str().expect("what");
            let agents = case["agents"].as_array().expect("agents");
            let sessions: Vec<AgentSession> = agents
                .iter()
                .map(|agent| {
                    let mut session = sample_session(
                        agent["id"].as_str().expect("id"),
                        "p1",
                        agent["name"].as_str().expect("name"),
                    );
                    session.title = agent["name"].as_str().map(str::to_string);
                    session.status = match agent["status"].as_str().expect("status") {
                        "active" => SessionStatus::Active,
                        "detached" => SessionStatus::Detached,
                        "exited" => SessionStatus::Exited,
                        other => panic!("{what}: unknown status {other}"),
                    };
                    session.created_at = time(&agent["created"]);
                    session.updated_at = time(&agent["updated"]);
                    session
                })
                .collect();
            let terminal_cases = case["terminals"].as_array().expect("terminals");
            let terminals: Vec<TerminalRowFacts<'_>> = terminal_cases
                .iter()
                .map(|terminal| TerminalRowFacts {
                    name: terminal["name"].as_str().expect("name"),
                    created_at: time(&terminal["created"]),
                    updated_at: time(&terminal["updated"]),
                    busy: terminal["busy"].as_bool().expect("busy"),
                    after_agent: terminal["after"].as_str(),
                })
                .collect();
            let flag = |index: usize, key: &str| agents[index][key].as_bool().expect(key);
            let rows = order_rows(
                &sessions,
                &terminals,
                crate::config_effective::effective_agent_sort(case["mode"].as_str().expect("mode")),
                &AgentRowFacts {
                    is_hot: &|i| flag(i, "hot"),
                    is_visible: &|_| true,
                    has_running_terminal: &|i| flag(i, "running_terminal"),
                },
                &|_| true,
            );
            let main: Vec<String> = rows
                .main
                .iter()
                .map(|row| match *row {
                    FlatRow::Agent(i) => sessions[i].id.clone(),
                    FlatRow::Terminal(i) => {
                        terminal_cases[i]["id"].as_str().expect("id").to_string()
                    }
                })
                .collect();
            let inactive: Vec<String> = rows
                .inactive
                .iter()
                .map(|&i| sessions[i].id.clone())
                .collect();
            assert_eq!(main, ids(&case["main"]), "{what}");
            assert_eq!(inactive, ids(&case["inactive"]), "{what}");
        }
    }
}
