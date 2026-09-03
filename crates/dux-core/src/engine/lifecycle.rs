//! Engine lifecycle housekeeping shared by all surfaces: detecting and cleaning
//! up PTY child processes (agent providers and companion terminals) that have
//! exited. The TUI has its own richer exit handling (resume-fallback, UI focus);
//! this is the minimal headless-safe cleanup the web server's engine loop calls
//! each tick so exited agents/terminals don't linger in `providers` /
//! `companion_terminals` (and therefore the ViewModel).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::ids::{SessionIdRef, TabId, TabIdRef};
use crate::model::{AgentSession, SessionStatus, TerminalOwner};
use crate::pty::PtyClient;

use super::Engine;

/// How long an agent PTY may stay unpruned while the two facts prune needs, a
/// fully drained terminal buffer and the child's exit STATUS, are still arriving.
///
/// The two signals disagree on purpose, and NEITHER implies the other. `try_wait`
/// reaps the child the instant it becomes waitable; the reader thread sets
/// `is_exited` at `Ok(0)`, EOF on the PTY read side, which is the only moment all
/// of the child's output is guaranteed to be in the terminal buffer. Prune needs
/// both, and each direction of the disagreement loses something real:
///
/// - Reaped but NOT drained: the crash excerpt is captured off a buffer the
///   reader has not finished filling, and since `try_wait` yields the status
///   exactly once there is no second chance later, so the agent gets reported as
///   exited with an EMPTY excerpt, losing exactly the diagnostic the message
///   exists to show.
/// - Drained but NOT reaped: the exit status is `None`, and every decision keyed
///   on it silently takes its unknown branch. `clean_exit_closes_tab_row` cannot
///   fire, so an extra tab whose CLI exited cleanly keeps a dead row the user
///   then has to close by hand. This window is real, not theoretical: the kernel
///   closes a dying task's descriptors before it makes the task waitable, so EOF
///   can genuinely land first, and it is reproduced by construction in
///   `prune_defers_a_drained_child_until_its_exit_status_is_known`.
///
/// So prune waits for BOTH and falls back to whichever clock it has once this
/// grace expires. The fallback cannot be dropped in either direction: a surviving
/// GRANDCHILD holding the PTY slave open means the read side never EOFs, and a
/// child that closes its own descriptors and keeps running is never reaped at
/// all, so waiting on either fact alone would leak that provider forever.
///
/// 250ms is chosen as the smallest value that is unambiguously both:
/// - **invisible in the normal case**, where the child is the only holder of the
///   slave, so EOF lands within microseconds of the exit and prune fires on the
///   very next engine tick (the web loop at 50ms, the TUI at 33ms while any row
///   animates and 100ms otherwise) exactly as before, and the grace is never
///   even consulted; and
/// - **far more than a scheduler needs**, since the deferral is re-evaluated on
///   every tick, so the reader thread gets a few dozen chances to be scheduled
///   and drain a few kilobytes rather than the single chance it had before.
///
/// It is also the ceiling on how long a PTY held open by a grandchild, or by a
/// child that EOFs without ever exiting, lingers, which is why it is not larger.
pub const REAPED_DRAIN_GRACE: Duration = Duration::from_millis(250);

/// Whether an agent PTY whose child is on its way out is ready to be pruned,
/// given whether its exit status is known (the child has been reaped), how long
/// ago its reader thread reached EOF (so the terminal buffer is complete), and
/// how long ago the child was reaped.
///
/// The rule is: prune once we hold BOTH facts, or once either clock has run past
/// [`REAPED_DRAIN_GRACE`]. Holding out for both is what protects the two things
/// prune consumes exactly once and can never re-read, the crash excerpt and the
/// exit status; the grace is the safety valve, so a PTY that will never supply
/// the missing half cannot wedge the prune forever. See [`REAPED_DRAIN_GRACE`]
/// for what each direction loses and why neither arm can be dropped.
///
/// `since_eof` is `Some` exactly when the reader is at end of input, so it
/// carries the old `reader_at_eof` flag as well as the clock that bounds it.
/// Pure, so the policy is testable without a real PTY.
pub fn agent_pty_ready_to_prune(
    exit_status_known: bool,
    since_eof: Option<Duration>,
    since_reap: Option<Duration>,
) -> bool {
    if exit_status_known && since_eof.is_some() {
        return true;
    }
    [since_eof, since_reap]
        .into_iter()
        .flatten()
        .any(|elapsed| elapsed >= REAPED_DRAIN_GRACE)
}

/// Which kind of PTY was pruned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrunedPtyKind {
    Agent,
    Terminal,
}

/// A request to remove an agent's worktree, deferred until that agent's PTY has
/// actually exited — so files are never deleted out from under a still-running
/// process (which would also risk git-lock failures). Carried on the agent's
/// [`TerminatingPty`] and dispatched by `reap_terminating_ptys` once the PTY is
/// reaped. `None` for terminals and for keep-worktree deletes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferredWorktreeRemoval {
    pub session_id: String,
    pub project_path: String,
    /// The managed working copy to remove, captured whole at delete time: the
    /// worktree path, the current branch, the branch the agent was BORN on
    /// (deleted too when it differs, or a drifted agent leaves its birth branch
    /// behind, see `git::remove_worktree`), and where the branch came from
    /// (which decides whether the removal may delete branches at all, because
    /// dux deletes only what it created; see `Engine::do_delete_session`).
    ///
    /// Carried as the whole [`crate::model::ManagedWorkspace`] rather than as
    /// loose fields SO THAT A STANDALONE AGENT CANNOT PRODUCE ONE. A folder
    /// workspace has no value of this type to offer, so no delete of a
    /// standalone agent can construct a removal for the user's folder. That is
    /// the structural spelling of "dux never touches the folder".
    pub managed: crate::model::ManagedWorkspace,
    /// The delete dialog's "also delete the branch" answer, captured when the
    /// delete was requested. `None` for a caller with no dialog behind it,
    /// which keeps the provenance default. See
    /// [`crate::model::BranchProvenance::resolve_branch_deletion`].
    pub delete_branch: Option<bool>,
    /// The Busy status message to show while the removal runs (set when the
    /// worker is finally spawned, after the PTY is reaped).
    pub busy_message: String,
}

/// A PTY that was SIGTERMed by an individual delete/close and is being given a
/// grace period to exit before being force-killed. Held (not dropped) because
/// `PtyClient::drop` hard-kills with no grace; `reap_terminating_ptys` drops it
/// once it exits or its deadline passes. This is the non-blocking, per-PTY
/// counterpart to `shutdown_ptys`'s whole-app blocking wait.
pub struct TerminatingPty {
    pub client: PtyClient,
    pub deadline: Instant,
    pub kind: PrunedPtyKind,
    /// Session id (agent) or terminal id (companion terminal).
    pub id: String,
    pub label: String,
    /// Deferred worktree removal to dispatch once this PTY is reaped (agent
    /// deletes with `delete_worktree` only).
    pub worktree_removal: Option<DeferredWorktreeRemoval>,
}

/// The result of a user-initiated `Engine::kill_tab_runtime` teardown, so a
/// surface can drive its focus/status work off the decision instead of
/// re-deriving it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KillTabRuntimeOutcome {
    /// True when there was a live provider to kill; `false` is an idempotent
    /// no-op (already gone).
    pub killed: bool,
    /// The session that owns the killed tab, resolved through the session's
    /// slot pointer for a first tab and through the tab's own row for an extra
    /// one, or `None` for an unknown tab.
    pub session_id: Option<String>,
    /// True when the kill detached the agent (its last live tab is gone, so the
    /// session is now `Detached` and its auto-reopen intent cleared).
    pub detached: bool,
}

/// A PTY that `prune_exited_ptys` removed because its child process exited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrunedPty {
    pub kind: PrunedPtyKind,
    /// The tab id (for an agent, the id of whichever tab exited) or terminal
    /// id (for a companion terminal).
    pub id: String,
    /// Who owned the pruned PTY: `Session(sid)` for an agent tab or a
    /// session-owned companion terminal, `Project(pid)` for a project terminal,
    /// `None` only for an orphan whose owner could not be resolved.
    pub owner: Option<TerminalOwner>,
    /// True when this exit detached the agent — i.e. it was the agent's LAST live
    /// tab, so the session is now Detached. Surfaces show the workspace-wide
    /// "Agent exited" notice for this; a tab exit that leaves siblings running
    /// (`false`) gets a quiet, scoped "tab exited" notice instead. Always `false`
    /// for a companion terminal.
    pub agent_detached: bool,
    /// A human-facing label: the agent's branch name (session-slot tab), a
    /// "{provider} on {branch}" descriptor (extra tab), or the terminal's label
    /// (companion terminal).
    pub label: String,
    /// True when the exit also CLOSED the tab (deleted its `agent_tabs` row):
    /// a clean exit (code 0) of an extra tab is the user deliberately ending
    /// that conversation, so no dead pill is left in the strip. Nothing of
    /// value is lost — the provider's conversation history lives in the
    /// worktree, not the row. Always `false` for the session-slot tab (it has
    /// no row), for a non-zero/unknown exit (the dormant relaunch screen is
    /// the crash-diagnosis surface), and for a companion terminal.
    pub tab_closed: bool,
    /// The reaped child's exit-success (`Some(true)` clean, `Some(false)`
    /// non-zero, `None` when only EOF was observed without a status). Captured
    /// at reap time because `try_wait` yields the status exactly once; carried
    /// out so a surface can key its exit message on it without a second reap.
    /// Always `None` for a companion terminal (its exit needs no status copy).
    pub exit_success: Option<bool>,
    /// True when the exited agent produced only minimal output (no scrollback,
    /// few visible lines) — the "resume printed a short error and quit" shape
    /// the TUI embeds in its exit message. Captured at reap time, before
    /// `clear_tab_runtime` drops the client. Always `false` for a terminal.
    pub is_minimal: bool,
    /// The visible-text excerpt captured when `is_minimal` is true (empty
    /// otherwise, and always empty for a terminal). The TUI folds this into its
    /// exit-status message and error log; the web ignores it. Captured off the
    /// live client at reap time for the same once-only reason as `exit_success`.
    pub output_excerpt: String,
}

/// Whether an exited agent tab's row should be closed along with the prune:
/// only an EXTRA tab (the slot tab's row stays, so its slot survives) that exited CLEANLY
/// (code 0 — the user deliberately ended the conversation, e.g. /exit). The
/// one shared rule both surfaces' exit paths consult, so the TUI loop and the
/// web's `prune_exited_ptys` cannot drift.
pub fn clean_exit_closes_tab_row(is_session_slot: bool, exit_success: Option<bool>) -> bool {
    !is_session_slot && exit_success == Some(true)
}

/// A deferred worktree removal that must wait for a WHOLE GROUP of an agent's
/// tab PTYs (Main plus every extra tab) to reap before it fires — closing the
/// gap where removing the worktree after only the first tab exits could delete
/// files out from under a still-running sibling tab (a git-lock race). Each of
/// the session's terminating tab entries is listed in `pending_ids`;
/// `reap_terminating_ptys` removes ids as they reap (clean exit OR force-kill)
/// and dispatches `removal` exactly once, when `pending_ids` empties.
pub struct GroupWorktreeRemoval {
    pub pending_ids: std::collections::HashSet<String>,
    pub removal: DeferredWorktreeRemoval,
}

/// Outcome of [`Engine::shutdown_ptys`], so a caller can echo the result to its
/// own surface (e.g. the TUI to its restored terminal) using the same pure
/// formatters this routine logs with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShutdownReport {
    pub agents_total: usize,
    pub terminals_total: usize,
    /// Agents that exited within the grace period (the rest were SIGKILLed).
    pub agents_exited: usize,
    /// Terminals that exited within the grace period (the rest were SIGKILLed).
    pub terminals_exited: usize,
    pub elapsed: std::time::Duration,
    /// True when at least one child had to be force-killed (SIGKILL) because it
    /// had not exited by the time the grace window was up — equivalently,
    /// `agents_exited < agents_total || terminals_exited < terminals_total`. With
    /// a grace of `0` the wait is skipped, so any not-yet-exited child sets this.
    pub timed_out: bool,
}

#[derive(Clone, Copy)]
struct ShutdownTotals {
    agents: usize,
    terminals: usize,
}

impl ShutdownTotals {
    fn is_empty(self) -> bool {
        self.agents == 0 && self.terminals == 0
    }

    fn report(self, tally: ShutdownTally, elapsed: Duration) -> ShutdownReport {
        ShutdownReport {
            agents_total: self.agents,
            terminals_total: self.terminals,
            agents_exited: tally.agents_exited,
            terminals_exited: tally.terminals_exited,
            elapsed,
            timed_out: tally.agents_exited < self.agents || tally.terminals_exited < self.terminals,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct ShutdownTally {
    agents_exited: usize,
    terminals_exited: usize,
}

fn shutdown_aborted(abort: Option<&AtomicBool>) -> bool {
    abort.is_some_and(|flag| flag.load(Ordering::SeqCst))
}

fn pty_has_exited(client: &mut PtyClient) -> bool {
    client.is_exited() || client.try_wait().is_some()
}

fn force_survivors_and_count_exited<'a>(clients: impl Iterator<Item = &'a mut PtyClient>) -> usize {
    let mut exited = 0;
    for client in clients {
        if pty_has_exited(client) {
            exited += 1;
        } else {
            client.force_terminate();
        }
    }
    exited
}

/// `"1 agent"` / `"2 agents"` — pluralize `word` for `n`.
fn pluralize(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// The line logged (and echoed by surfaces) when graceful shutdown begins.
pub fn format_shutdown_start(
    agents: usize,
    terminals: usize,
    grace: std::time::Duration,
) -> String {
    format!(
        "Requesting {} and {} to gracefully shut down, timeout {}s.",
        pluralize(agents, "agent"),
        pluralize(terminals, "terminal"),
        grace.as_secs()
    )
}

/// The final line logged (and echoed) when shutdown finishes: a clean notice
/// when every child exited within the grace period, otherwise the force-closing
/// notice naming how many exited cleanly versus how many are being killed.
pub fn format_shutdown_result(report: &ShutdownReport) -> String {
    if report.timed_out {
        // saturating_sub: exited <= total always holds for a report this engine
        // builds, but the struct is public and constructible, so never risk a
        // usize underflow that would print a giant number into the log.
        let remaining_agents = report.agents_total.saturating_sub(report.agents_exited);
        let remaining_terminals = report
            .terminals_total
            .saturating_sub(report.terminals_exited);
        format!(
            "{} and {} exited successfully. Force-closing {} and {}, then exiting...",
            pluralize(report.agents_exited, "agent"),
            pluralize(report.terminals_exited, "terminal"),
            pluralize(remaining_agents, "agent"),
            pluralize(remaining_terminals, "terminal"),
        )
    } else {
        format!(
            "All {} and {} exited gracefully in {:.1}s.",
            pluralize(report.agents_total, "agent"),
            pluralize(report.terminals_total, "terminal"),
            report.elapsed.as_secs_f64()
        )
    }
}

impl Engine {
    /// Detect agent providers and companion terminals whose child PTY has exited,
    /// remove them from the engine, mark exited agents' sessions `Detached`, and
    /// return what was pruned (so callers can surface a status). Pure engine
    /// state mutation — no UI, no network. Safe to call every tick.
    pub fn prune_exited_ptys(&mut self) -> Vec<PrunedPty> {
        let mut pruned = Vec::new();

        // Agent providers (keyed by TAB id). Capture each exited client's
        // exit-success so a clean exit can clear `desired_running` (matching the
        // TUI), which keeps a deliberately-exited agent from auto-reopening.
        // Capture the minimal-output excerpt in the SAME pass, before
        // `clear_tab_runtime` below drops the client: the TUI's exit-status
        // message needs this data off the returned value, and once the tab is
        // cleared there is no client left to re-read it from.
        //
        // Both of those are once-only reads, and they arrive from two different
        // places in either order: the excerpt is only truthful once the READER
        // thread has reached EOF, and the status only exists once the child has
        // been reaped. So an agent that has supplied one and not the other is
        // left in place and reconsidered on a later tick, up to
        // `REAPED_DRAIN_GRACE` (see it for what each direction loses).
        // `PtyClient::try_wait` memoizes the status, so deferring costs nothing:
        // a reap observed on the first tick is still available on the tick that
        // finally prunes.
        let exited_agents: Vec<(TabId, Option<bool>, bool, String)> = self
            .providers
            .iter_mut()
            .filter_map(|(id, client)| {
                // Poll for the status FIRST on every pass, so a status that
                // arrives during the deferral is picked up by the same pass that
                // acts on it.
                let exit_success = client.try_wait().map(|status| status.success());
                if agent_pty_ready_to_prune(
                    exit_success.is_some(),
                    client.exited_at().map(|at| at.elapsed()),
                    client.reaped_at().map(|at| at.elapsed()),
                ) {
                    let is_minimal = client.has_minimal_output(5);
                    let output_excerpt = if is_minimal {
                        client.visible_text_excerpt(usize::MAX)
                    } else {
                        String::new()
                    };
                    Some((id.clone(), exit_success, is_minimal, output_excerpt))
                } else {
                    None
                }
            })
            .collect();
        for (tab_id, exit_success, is_minimal, output_excerpt) in exited_agents {
            // Resolve the exited PTY's owning session and whether it held the
            // slot. `providers` is keyed by tab id and a tab id names no session,
            // so resolve via the tab index first, or the label falls back to a
            // raw id and the session-state marks silently no-op on the wrong
            // key.
            let owning = self.owning_session_for_tab(tab_id.as_str());
            let is_session_slot = owning
                .as_deref()
                .is_some_and(|sid| self.is_slot_tab_of(SessionIdRef::new(sid), &tab_id));
            // When the AGENT itself exits (its session-slot tab), re-check its PR
            // now: an exit commonly follows a merge, so the badge would otherwise
            // stay stale until the next background sync. This is the shared-exit
            // trigger both surfaces get; the TUI additionally fires it from its own
            // richer exit loop. Rate-limited and in-flight-guarded inside the spawn.
            if is_session_slot && let Some(sid) = owning.clone() {
                self.spawn_pr_check_for_session(&sid, crate::engine::PR_CHECK_MIN_INTERVAL);
            }
            let (owner, label) = match &owning {
                Some(sid) => {
                    let branch = self
                        .sessions
                        .iter()
                        .find(|s| &s.id == sid)
                        .map(|s| s.display_label())
                        .unwrap_or_else(|| sid.clone());
                    let label = if is_session_slot {
                        branch
                    } else {
                        let provider = self
                            .agent_tabs
                            .get(&tab_id)
                            .map(|t| t.provider.as_str().to_string())
                            .unwrap_or_default();
                        format!("{provider} on {branch}")
                    };
                    (Some(TerminalOwner::Session(sid.clone())), label)
                }
                None => (None, tab_id.as_str().to_string()),
            };
            // Clear EVERY runtime map keyed by this tab via the single-source
            // helper — not just providers/activity/input. In particular
            // `running_provider_pins` (set when a live tab is retargeted) would
            // otherwise leak and keep showing the old provider for the now-exited
            // tab; a long-running server would also leak one entry per exited tab.
            self.clear_tab_runtime(&tab_id);
            // No tab is privileged: the agent only detaches once its LAST tab is
            // gone. `clear_tab_runtime` above already dropped this tab from
            // `providers`, so `any_tab_active` reflects the true post-exit state —
            // if a sibling tab is still live/launching the agent stays Active.
            // This exit detaches the agent only when it was the LAST live tab.
            // An explicit match on the owner: only a session-owned prune can
            // detach an agent (an orphan has no session to mark).
            // Resolve "which session, if any, this exit detaches" ONCE, through an
            // exhaustive match, and then act on the answer. Re-testing the owner
            // with a partial pattern below would be a second, silently-extendable
            // ownership decision for the same question.
            let detaching_session: Option<String> =
                match owner.as_ref().map(crate::model::TerminalOwner::as_ref) {
                    Some(crate::model::TerminalOwnerRef::Session(sid)) => {
                        (!self.any_tab_active(sid)).then(|| sid.to_string())
                    }
                    // A project terminal, a standalone terminal and an orphan PTY
                    // all have no session behind them to mark Detached.
                    Some(
                        crate::model::TerminalOwnerRef::Project(_)
                        | crate::model::TerminalOwnerRef::Standalone,
                    )
                    | None => None,
                };
            let agent_detached = detaching_session.is_some();
            if let Some(sid) = &detaching_session {
                // A clean exit of the session-slot tab is the "user quit the
                // agent" signal that cancels auto-reopen; an extra tab exiting
                // (or any crash) leaves the auto-reopen intent untouched.
                if is_session_slot && exit_success == Some(true) {
                    self.mark_session_desired_running(sid, false);
                }
                self.mark_session_status(sid, SessionStatus::Detached);
            }
            // A non-zero exit is the tab's last run ending badly, and it is
            // recorded AFTER `clear_tab_runtime` above (which wipes the flag for
            // every deliberate end) so the verdict survives its own teardown. A
            // clean exit and a bare EOF with no status both leave the slate
            // clean: only a status that actually says "this failed" may stop the
            // next selection from starting the tab. The guard is existence, the
            // same one the launch-failed path uses: an orphan PTY has no tab
            // anything can ever ask about again, so a verdict for it would be one
            // leaked entry per orphan on a long-running server.
            if exit_success == Some(false) && owning.is_some() {
                self.mark_tab_run_failed(&tab_id);
            }
            let tab_closed = clean_exit_closes_tab_row(is_session_slot, exit_success)
                && self.remove_agent_tab_row(tab_id.as_str());
            pruned.push(PrunedPty {
                kind: PrunedPtyKind::Agent,
                id: tab_id.as_str().to_string(),
                owner,
                agent_detached,
                label,
                tab_closed,
                exit_success,
                is_minimal,
                output_excerpt,
            });
        }

        // Companion terminals (keyed by terminal id). Deliberately the simpler
        // "either signal" condition rather than the agents' readiness rule above:
        // a terminal prune reads NEITHER of the two once-only facts that rule
        // protects. It captures no excerpt, and it carries no exit status at all
        // (`exit_success` below is hardcoded `None`, because a terminal exit
        // drives no status-dependent decision anywhere). With nothing to lose by
        // pruning early, deferring one would only keep a dead terminal on screen.
        let exited_terminals: Vec<(String, String)> = self
            .companion_terminals
            .iter_mut()
            .filter_map(|(id, terminal)| {
                if terminal.client.is_exited() || terminal.client.try_wait().is_some() {
                    Some((id.clone(), terminal.label.clone()))
                } else {
                    None
                }
            })
            .collect();
        for (terminal_id, label) in exited_terminals {
            let owner = self
                .companion_terminals
                .get(&terminal_id)
                .map(|t| t.owner.clone());
            self.companion_terminals.remove(&terminal_id);
            self.clear_terminal_runtime(&terminal_id);
            pruned.push(PrunedPty {
                kind: PrunedPtyKind::Terminal,
                id: terminal_id,
                owner,
                agent_detached: false,
                label,
                tab_closed: false,
                // A terminal exit carries no status message, so it needs none of
                // the agent exit-message fields.
                exit_success: None,
                is_minimal: false,
                output_excerpt: String::new(),
            });
        }

        pruned
    }

    /// The grace `Duration` an individual delete/close gives a child to exit
    /// before the background reaper force-kills it. Uses the global top-level
    /// `shutdown_timeout_seconds` (engine-wide; the close/delete handlers are
    /// shared by both surfaces and cannot tell TUI from web). Background, so the
    /// value only bounds force-kill latency, never blocks the UI.
    fn individual_close_grace(&self) -> std::time::Duration {
        crate::config::shutdown_grace(self.config.shutdown_timeout_seconds)
    }

    /// Tear down ONE tab's live provider as a deliberate, user-initiated KILL
    /// (the kill overlay / close-session-slot-tab action), and report what
    /// happened. The single-source teardown decision shared by the wire
    /// `kill_session_pty` and the TUI kill overlay, so both surfaces agree.
    ///
    /// Behavior, in order:
    /// - No live provider for `tab_id` -> `killed: false`, nothing changes
    ///   (idempotent, so a double-tap or a kill racing a natural exit is a
    ///   no-op, not an error).
    /// - Otherwise `clear_tab_runtime` drops the provider (SIGKILL via
    ///   `PtyClient::drop`, the intended semantics of an explicit kill) and
    ///   clears every runtime map keyed by the tab, INCLUDING the in-flight
    ///   `AgentLaunch` key a hand-rolled list used to miss.
    /// - The agent detaches only when this was its LAST live tab
    ///   (`any_tab_active` is in-flight-aware). On detach the session is marked
    ///   `Detached` AND `desired_running` is cleared, because a deliberate kill
    ///   is the "user no longer wants this agent" signal: without clearing it
    ///   the startup auto-reopen pass would relaunch the agent the user just
    ///   killed. A surviving sibling leaves `desired_running` untouched (the
    ///   agent is still wanted running).
    ///
    /// This is distinct from `prune_exited_ptys`, which handles NATURAL exits
    /// and deliberately keeps `desired_running` set on a crash so auto-reopen
    /// can bring the agent back.
    pub fn kill_tab_runtime(&mut self, tab_id: &str) -> KillTabRuntimeOutcome {
        let session_id = self.owning_session_for_tab(tab_id);
        // Transport-facing (a wire command's path segment): named here, at the
        // door, before it touches any tab-keyed map.
        let tab_id = TabIdRef::new(tab_id);
        if !self.providers.contains_key(tab_id) {
            return KillTabRuntimeOutcome {
                killed: false,
                session_id,
                detached: false,
            };
        }
        self.clear_tab_runtime(tab_id);
        let detached = match &session_id {
            Some(sid) if !self.any_tab_active(sid) => {
                self.mark_session_status(sid, SessionStatus::Detached);
                self.mark_session_desired_running(sid, false);
                true
            }
            _ => false,
        };
        KillTabRuntimeOutcome {
            killed: true,
            session_id,
            detached,
        }
    }

    /// SIGTERM a companion terminal and move it into the terminating set for a
    /// non-blocking background reap, instead of dropping it from the map (which
    /// would hard-SIGKILL via `PtyClient::drop`). Returns the terminal's label,
    /// or `None` if it was not found.
    pub fn begin_close_companion_terminal(&mut self, terminal_id: &str) -> Option<String> {
        let term = self.companion_terminals.remove(terminal_id)?;
        self.clear_terminal_runtime(terminal_id);
        let label = term.label.clone();
        term.client.terminate();
        let deadline = Instant::now() + self.individual_close_grace();
        self.terminating_ptys.push(TerminatingPty {
            client: term.client,
            deadline,
            kind: PrunedPtyKind::Terminal,
            id: terminal_id.to_string(),
            label: label.clone(),
            worktree_removal: None,
        });
        Some(label)
    }

    /// SIGTERM an agent provider and move it into the terminating set for a
    /// background reap. `label` is kept for the reap log; `worktree_removal` is
    /// dispatched once the PTY is reaped (agent delete with `delete_worktree`).
    ///
    /// Returns the `worktree_removal` back **unhandled** when the session has no
    /// live provider (the agent already exited or never started): there is no PTY
    /// to wait for, so the caller must dispatch the removal immediately rather
    /// than let it be lost. Returns `None` when it was captured on a terminating
    /// entry (or there was nothing to remove).
    ///
    /// Takes a TAB id, never a session id: `providers` is tab-keyed and
    /// `close_tab` passes an extra tab's id through here.
    #[must_use]
    pub fn begin_close_provider(
        &mut self,
        tab_id: &TabIdRef,
        label: String,
        worktree_removal: Option<DeferredWorktreeRemoval>,
    ) -> Option<DeferredWorktreeRemoval> {
        let Some(client) = self.providers.remove(tab_id) else {
            return worktree_removal;
        };
        client.terminate();
        let deadline = Instant::now() + self.individual_close_grace();
        self.terminating_ptys.push(TerminatingPty {
            client,
            deadline,
            kind: PrunedPtyKind::Agent,
            id: tab_id.as_str().to_string(),
            label,
            worktree_removal,
        });
        None
    }

    /// SIGTERM every companion terminal belonging to a session and move them all
    /// into the terminating set (used when the owning agent is deleted).
    ///
    /// A STANDALONE terminal is deliberately not in scope, and the omission is a
    /// decision rather than something to be tidied up later. It belongs to no
    /// agent, so deleting an agent has nothing to do with it. Nothing closes a
    /// standalone terminal automatically: it ends when the user closes it or dux
    /// shuts down. The same note sits on `begin_close_project_terminals` below,
    /// and the rule itself is on `TerminalOwner::closed_by_session_delete`, whose
    /// exhaustive match is what actually enforces it.
    pub fn begin_close_session_terminals(&mut self, session_id: &str) {
        let ids: Vec<String> = self
            .companion_terminals
            .iter()
            .filter(|(_, t)| t.owner.closed_by_session_delete(session_id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.begin_close_companion_terminal(&id);
        }
    }

    /// SIGTERM every project terminal belonging to a project and move them all
    /// into the terminating set (used when the project is removed).
    ///
    /// A STANDALONE terminal is deliberately not in scope, and the omission is a
    /// decision rather than something to be tidied up later. It belongs to no
    /// project, so removing a project has nothing to do with it. Nothing closes
    /// a standalone terminal automatically: it ends when the user closes it or
    /// dux shuts down. The same note sits on `begin_close_session_terminals`
    /// above, and the rule itself is on
    /// `TerminalOwner::closed_by_project_removal`, whose exhaustive match is
    /// what actually enforces it.
    pub fn begin_close_project_terminals(&mut self, project_id: &str) {
        let ids: Vec<String> = self
            .companion_terminals
            .iter()
            .filter(|(_, t)| t.owner.closed_by_project_removal(project_id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.begin_close_companion_terminal(&id);
        }
    }

    /// Drop every terminating PTY that has exited, and force-kill (then drop) any
    /// whose grace deadline has passed. Called once per engine tick on both
    /// surfaces (alongside `prune_exited_ptys`). Returns the deferred worktree
    /// removals for any reaped agents so the caller can dispatch them; logs each
    /// reap at debug. A no-op when nothing is terminating.
    pub fn reap_terminating_ptys(&mut self) -> Vec<DeferredWorktreeRemoval> {
        if self.terminating_ptys.is_empty() {
            return Vec::new();
        }
        let now = Instant::now();
        let mut dispatch = Vec::new();
        let mut reaped_ids = Vec::new();
        let mut remaining = Vec::with_capacity(self.terminating_ptys.len());
        for mut entry in std::mem::take(&mut self.terminating_ptys) {
            let exited = entry.client.is_exited() || entry.client.try_wait().is_some();
            if exited {
                crate::logger::debug(&format!(
                    "reaped terminating {:?} {} (\"{}\") after a clean exit",
                    entry.kind, entry.id, entry.label
                ));
            } else if now >= entry.deadline {
                entry.client.force_terminate();
                crate::logger::debug(&format!(
                    "force-killed terminating {:?} {} (\"{}\") after the grace period elapsed",
                    entry.kind, entry.id, entry.label
                ));
            } else {
                remaining.push(entry);
                continue;
            }
            // Reaped: hand back any single-PTY deferred worktree removal, then
            // drop the client (its `Drop` SIGKILL is a benign no-op now — already
            // gone). Group removals are resolved below once every member reaps.
            reaped_ids.push(entry.id.clone());
            if let Some(req) = entry.worktree_removal.take() {
                dispatch.push(req);
            }
        }
        self.terminating_ptys = remaining;

        // Group barrier: a multi-tab delete defers its worktree removal until the
        // LAST of the session's tab PTYs has reaped. Drop reaped ids from every
        // pending group; a group whose set is now empty dispatches its removal
        // exactly once.
        if !self.pending_group_removals.is_empty() {
            for id in &reaped_ids {
                for group in &mut self.pending_group_removals {
                    group.pending_ids.remove(id);
                }
            }
            let mut still_pending = Vec::new();
            for group in std::mem::take(&mut self.pending_group_removals) {
                if group.pending_ids.is_empty() {
                    dispatch.push(group.removal);
                } else {
                    still_pending.push(group);
                }
            }
            self.pending_group_removals = still_pending;
        }
        dispatch
    }

    /// Boot-time normalization of persisted session statuses (the headless
    /// counterpart of the TUI's `restore_sessions`): nothing is running yet, so
    /// a session whose worktree still exists is `Detached`; one whose worktree
    /// vanished is `Exited`. Statuses persist via `mark_session_status`. Unlike
    /// the TUI this does not auto-reopen anything — the web resumes on subscribe.
    pub fn normalize_restored_sessions(&mut self) {
        let ids: Vec<(String, bool)> = self
            .sessions
            .iter()
            .map(|s| (s.id.clone(), std::path::Path::new(s.directory()).exists()))
            .collect();
        for (id, exists) in ids {
            let status = if exists {
                SessionStatus::Detached
            } else {
                SessionStatus::Exited
            };
            self.mark_session_status(&id, status);
        }
        // Classify every restored STANDALONE agent's folder now, off-thread, so
        // the first frame already knows whether each one's changes panel works
        // rather than starting at "dux has not looked yet".
        //
        // It matters beyond the panel: an unprobed folder reads as
        // Indeterminate, which fails CLOSED for mutations and for the upload
        // directory's gitignore seed. Waiting for the panel to open would leave
        // a file dropped before then unseeded in a folder git can see.
        self.probe_standalone_folders();
    }

    /// Ask git about every standalone agent's folder, one off-thread probe each.
    ///
    /// A no-op when there are none, which is the ordinary case, so this costs
    /// nothing for a workspace of project agents.
    pub fn probe_standalone_folders(&mut self) {
        let ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|s| s.folder_path().is_some())
            .map(|s| s.id.clone())
            .collect();
        for id in ids {
            self.spawn_folder_repo_probe(&id);
        }
    }

    /// The sessions eligible for a startup auto-reopen relaunch, the CORE-owned
    /// eligibility rule both surfaces apply (the TUI after `restore_sessions`,
    /// the web server after `bootstrap_engine`'s status normalization). A
    /// session qualifies only when EVERY condition holds:
    ///
    /// - the global `ui.auto_reopen_agents` switch is on,
    /// - the session recorded reopen intent (`desired_running`: it was still
    ///   running when dux last exited),
    /// - the per-agent `auto_reopen_enabled` opt-in is on,
    /// - the directory it runs in still exists on disk (a vanished directory
    ///   cannot host a provider),
    /// - for a MANAGED agent, its project has not opted out
    ///   (`project_allows_auto_reopen`), and
    /// - its provider can actually resume a conversation
    ///   (`supports_session_resume`; reopening a provider that starts from
    ///   scratch would silently discard the conversation the intent was about).
    ///
    /// The project consult is a STRUCTURAL switch on the workspace, not a
    /// lookup that happens to miss. `project_allows_auto_reopen` fails OPEN on
    /// an unknown project, so a standalone agent passed through it would sail
    /// past a question nobody ever answered for it; here the question is simply
    /// not asked, and the fail-open helper is unreachable from that arm.
    ///
    /// Only the DECISION lives here; each surface keeps its own launch dispatch
    /// (`build_agent_launch_request` with `AgentLaunchKind::StartupAutoReopen`).
    pub fn auto_reopen_candidates(&self) -> Vec<AgentSession> {
        if !self.config.ui.auto_reopen_agents {
            return Vec::new();
        }
        self.sessions
            .iter()
            .filter(|session| {
                let owner_allows = match &session.workspace {
                    crate::model::AgentWorkspace::Managed(managed) => {
                        self.project_allows_auto_reopen(&managed.project_id)
                    }
                    // No project, so nothing to consult and nobody to veto.
                    crate::model::AgentWorkspace::Folder(_) => true,
                };
                session.desired_running
                    && session.auto_reopen_enabled
                    && std::path::Path::new(session.directory()).exists()
                    && owner_allows
                    && crate::config::provider_config(&self.config, &session.provider)
                        .supports_session_resume()
            })
            .cloned()
            .collect()
    }

    /// Gracefully wind down every running PTY for server shutdown: SIGTERM each
    /// child (agents save state for a later resume), wait up to `grace` for
    /// exits, and mark agent sessions Detached (persisted). `desired_running`
    /// is left untouched — a server shutdown is not the user stopping the
    /// agent. Any child still alive when `grace` elapses is force-killed
    /// (SIGKILL) on the spot so the logged result is truthful; `PtyClient::drop`
    /// remains the backstop. Logs a start and a result line to `dux.log` and
    /// returns a [`ShutdownReport`] so callers can echo the same lines to their
    /// own surface. A grace of `0` skips the wait and force-kills immediately.
    /// With nothing running, it is a silent no-op (no signals, no logs).
    pub fn shutdown_ptys(&mut self, grace: std::time::Duration) -> ShutdownReport {
        self.shutdown_ptys_interruptible(grace, None)
    }

    /// Ends the grace wait early when `abort` is set. Surviving children are
    /// force-killed with the same tally semantics as a deadline timeout.
    pub fn shutdown_ptys_interruptible(
        &mut self,
        grace: std::time::Duration,
        abort: Option<&AtomicBool>,
    ) -> ShutdownReport {
        let totals = ShutdownTotals {
            agents: self.providers.len(),
            terminals: self.companion_terminals.len(),
        };
        if totals.is_empty() {
            return totals.report(ShutdownTally::default(), Duration::ZERO);
        }

        crate::logger::info(&format_shutdown_start(
            totals.agents,
            totals.terminals,
            grace,
        ));
        self.terminate_shutdown_ptys();

        let start = Instant::now();
        self.wait_for_shutdown_ptys(start, grace, abort);
        let tally = self.force_shutdown_survivors();
        let report = totals.report(tally, start.elapsed());
        crate::logger::info(&format_shutdown_result(&report));
        self.detach_shutdown_sessions();

        report
    }

    fn terminate_shutdown_ptys(&self) {
        for client in self.providers.values() {
            client.terminate();
        }
        for terminal in self.companion_terminals.values() {
            terminal.client.terminate();
        }
    }

    fn wait_for_shutdown_ptys(
        &mut self,
        start: Instant,
        grace: Duration,
        abort: Option<&AtomicBool>,
    ) {
        let deadline = start + grace;
        if grace.is_zero() || shutdown_aborted(abort) {
            return;
        }

        loop {
            if self.all_shutdown_ptys_exited()
                || Instant::now() >= deadline
                || shutdown_aborted(abort)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn all_shutdown_ptys_exited(&mut self) -> bool {
        let agents_exited = self.providers.values_mut().all(pty_has_exited);
        let terminals_exited = self
            .companion_terminals
            .values_mut()
            .all(|terminal| pty_has_exited(&mut terminal.client));
        agents_exited && terminals_exited
    }

    fn force_shutdown_survivors(&mut self) -> ShutdownTally {
        let agents_exited = force_survivors_and_count_exited(self.providers.values_mut());
        let terminals_exited = force_survivors_and_count_exited(
            self.companion_terminals
                .values_mut()
                .map(|terminal| &mut terminal.client),
        );
        ShutdownTally {
            agents_exited,
            terminals_exited,
        }
    }

    /// Resolve live tab IDs so sessions owned only by an extra tab are detached.
    fn detach_shutdown_sessions(&mut self) {
        let keys: Vec<TabId> = self.providers.keys().cloned().collect();
        let mut session_ids: Vec<String> = keys
            .iter()
            .filter_map(|id| self.owning_session_for_tab(id.as_str()))
            .collect();
        session_ids.sort();
        session_ids.dedup();
        for id in session_ids {
            self.mark_session_status(&id, SessionStatus::Detached);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ids::{TabId, TabIdRef};
    use std::path::Path;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    use super::PrunedPtyKind;
    use super::TerminatingPty;
    use super::{REAPED_DRAIN_GRACE, agent_pty_ready_to_prune};
    use super::{format_shutdown_result, format_shutdown_start};
    use crate::engine::Engine;
    use crate::engine::test_support::{sample_project, sample_session, sample_tab, test_engine};
    use crate::model::SessionStatus;
    use crate::pty::PtyClient;
    use tempfile::TempDir;

    /// Spawn a real `cat`-backed PtyClient in the given working directory.
    /// `cat` echoes stdin and exits 0 on EOF, and exits on SIGTERM — making it
    /// a safe stand-in for both clean-exit and shutdown tests.
    fn spawn_cat(cwd: &Path) -> PtyClient {
        PtyClient::spawn_with_env("cat", &[], cwd, 24, 80, 1000, &[]).expect("spawn cat")
    }

    /// A clean exit (code 0) of an EXTRA tab is the user deliberately ending
    /// that conversation (e.g. typing /exit): prune closes the tab's row too,
    /// so no dead pill lingers in the strip. Nothing of value is lost — the
    /// provider's conversation history lives in the worktree, not the row. A
    /// non-zero exit keeps the row: the dormant relaunch screen is the
    /// crash-diagnosis surface.
    #[test]
    fn prune_closes_extra_tab_row_on_clean_exit_and_keeps_it_on_crash() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.agent_tabs.insert(
            TabId::new("tab-clean"),
            sample_tab("tab-clean", "s1", "claude", 1),
        );
        engine.agent_tabs.insert(
            TabId::new("tab-crash"),
            sample_tab("tab-crash", "s1", "codex", 2),
        );

        let spawn = |code: &str| {
            PtyClient::spawn_with_env(
                "sh",
                &["-c".to_string(), format!("exit {code}")],
                worktree.path(),
                24,
                80,
                100,
                &[],
            )
            .expect("spawn sh")
        };
        engine.providers.insert(TabId::new("tab-clean"), spawn("0"));
        engine.providers.insert(TabId::new("tab-crash"), spawn("3"));

        // Wait until BOTH facts prune needs are in for both tabs, end of input
        // AND a reaped exit status, then prune ONCE.
        //
        // Waiting on either one alone is what makes this test race, and it has
        // raced in both directions. They are different facts arriving from
        // different places: `try_wait` reaps the child and stamps the reap
        // instant, while `is_exited` is set by the reader thread at EOF on the
        // PTY read side. `agent_pty_ready_to_prune` REFUSES to prune until it
        // holds both (or the grace expires), because each is read exactly once
        // and cannot be recovered later: pruning without the drain captures a
        // crash excerpt off an unfilled buffer, and pruning without the status
        // records `None`, which is what stops a clean exit from closing its row.
        // Break on either arm alone and the single prune below can return
        // nothing, firing the "clean tab pruned" expect. A single prune, rather
        // than a retry loop, is deliberate: with both facts in hand the prune
        // MUST take both tabs, and a loop would stop pinning that.
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let all_done = ["tab-clean", "tab-crash"].iter().all(|id| {
                engine
                    .providers
                    .get_mut(TabIdRef::new(id))
                    .is_some_and(|c| c.is_exited() && c.try_wait().is_some())
            });
            if all_done {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the tab PTYs never reached end of input"
            );
            sleep(Duration::from_millis(20));
        }
        let pruned = engine.prune_exited_ptys();

        let clean = pruned
            .iter()
            .find(|p| p.id == "tab-clean")
            .expect("clean tab pruned");
        assert!(
            clean.tab_closed,
            "a clean extra-tab exit must close the tab row"
        );
        assert!(
            !engine.agent_tabs.contains_key(TabIdRef::new("tab-clean")),
            "the cleanly-exited tab's row must be gone"
        );

        let crash = pruned
            .iter()
            .find(|p| p.id == "tab-crash")
            .expect("crashed tab pruned");
        assert!(
            !crash.tab_closed,
            "a non-zero exit must keep the tab row for diagnosis/relaunch"
        );
        assert!(
            engine.agent_tabs.contains_key(TabIdRef::new("tab-crash")),
            "the crashed tab's dormant row must survive"
        );
        assert!(
            engine.tab_last_run_failed("tab-crash"),
            "a non-zero exit must record the tab's last run as failed, so selecting \
             it shows the diagnosis surface instead of starting it again"
        );
        assert!(
            !engine.tab_last_run_failed("tab-clean"),
            "a clean exit leaves a clean slate"
        );
    }

    /// An explicit stop clears the recorded failure, so the next selection starts
    /// the tab again rather than meeting the diagnosis surface for a run the user
    /// has already dealt with. `clear_tab_runtime` is where every deliberate end
    /// funnels, which is why the clear lives there.
    #[test]
    fn a_deliberate_teardown_clears_a_recorded_failure() {
        let (mut engine, _tmp) = test_engine();
        engine.mark_tab_run_failed(TabIdRef::new("s1-slot"));
        assert!(engine.tab_last_run_failed("s1-slot"));
        engine.clear_tab_runtime(TabIdRef::new("s1-slot"));
        assert!(
            !engine.tab_last_run_failed("s1-slot"),
            "a stop, a force reconnect, a close or a delete all give the tab a clean slate"
        );
    }

    /// An ORPHAN PTY (one whose tab belongs to no session any more) exiting
    /// non-zero records nothing. Nothing can ever ask about that tab again, so
    /// the entry would sit in the map for the life of the process, one per
    /// orphan. Same existence guard the launch-failed path applies.
    #[test]
    fn prune_records_no_failure_for_an_orphan_ptys_non_zero_exit() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        // No session, no `agent_tabs` row: just a live PTY under a tab id.
        engine.providers.insert(
            TabId::new("tab-orphan"),
            PtyClient::spawn_with_env(
                "sh",
                &["-c".to_string(), "exit 3".to_string()],
                worktree.path(),
                24,
                80,
                100,
                &[],
            )
            .expect("spawn sh"),
        );

        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let done = engine
                .providers
                .get_mut(TabIdRef::new("tab-orphan"))
                .is_some_and(|c| c.is_exited() && c.try_wait().is_some());
            if done {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the orphan PTY never reached end of input"
            );
            sleep(Duration::from_millis(20));
        }
        let pruned = engine.prune_exited_ptys();
        assert!(
            pruned.iter().any(|p| p.id == "tab-orphan"),
            "the orphan must actually be pruned for this test to say anything"
        );
        assert!(
            engine.failed_tab_runs.is_empty(),
            "an orphan PTY's bad exit leaves no entry behind"
        );
    }

    /// The slot tab's row survives its own clean exit.
    ///
    /// Every tab is an `agent_tabs` row now, the slot tab included, so the
    /// clean-exit rule that deletes an extra tab's row would delete the row the
    /// session points at and leave the agent with a dangling slot. The slot tab
    /// detaches instead: its process is gone, its row and its slot are not.
    #[test]
    fn prune_keeps_the_slot_tabs_row_when_it_exits_cleanly() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine
            .session_store
            .create_session(&session)
            .expect("persist the agent and its first tab");
        let slot = session.slot_tab_id().to_owned();
        engine.sessions.push(session);
        engine.providers.insert(
            slot.clone(),
            PtyClient::spawn_with_env(
                "sh",
                &["-c".to_string(), "exit 0".to_string()],
                worktree.path(),
                24,
                80,
                100,
                &[],
            )
            .expect("spawn sh"),
        );

        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let done = engine
                .providers
                .get_mut(slot.as_ref_id())
                .is_some_and(|c| c.is_exited() && c.try_wait().is_some());
            if done {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the slot tab's PTY never reached end of input"
            );
            sleep(Duration::from_millis(20));
        }
        let pruned = engine.prune_exited_ptys();

        let entry = pruned
            .iter()
            .find(|p| p.id == slot.as_str())
            .expect("the slot tab pruned");
        assert!(
            !entry.tab_closed,
            "a clean exit of the slot tab must not close its row"
        );
        assert_eq!(
            engine.session_store.count_agent_tabs("s1").unwrap(),
            1,
            "the slot row survives, so the agent still has a first tab to point at"
        );
        assert_eq!(
            engine.sessions[0].status,
            SessionStatus::Detached,
            "a clean exit of the last live tab detaches the agent instead"
        );
    }

    #[test]
    fn prune_carries_exit_success_and_minimal_output_excerpt() {
        // The TUI's exit-status message needs the reaped exit-success plus a
        // minimal-output excerpt, and both must ride out on the PrunedPty (the
        // reap consumes `try_wait` once, so a second surface can't re-read them).
        // A crashing agent that printed a short line is the canonical shape.
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // Print one short line, then exit non-zero: minimal output + a crash.
        let client = PtyClient::spawn_with_env(
            "sh",
            &["-c".to_string(), "printf 'boom\\n'; exit 3".to_string()],
            worktree.path(),
            24,
            80,
            1000,
            &[],
        )
        .expect("spawn sh");
        engine.providers.insert(TabId::new("s1-slot"), client);

        let deadline = Instant::now() + Duration::from_secs(3);
        let pruned = loop {
            let pruned = engine.prune_exited_ptys();
            if pruned.iter().any(|p| p.id == "s1-slot") {
                break pruned;
            }
            assert!(Instant::now() < deadline, "agent provider never exited");
            sleep(Duration::from_millis(50));
        };

        let agent = pruned
            .iter()
            .find(|p| p.id == "s1-slot")
            .expect("the slot tab pruned");
        assert_eq!(
            agent.exit_success,
            Some(false),
            "a non-zero exit must be carried as exit_success = Some(false)"
        );
        assert!(
            agent.is_minimal,
            "a one-line-then-exit agent has minimal output"
        );
        assert!(
            agent.output_excerpt.contains("boom"),
            "the captured excerpt must carry the agent's final output, got {:?}",
            agent.output_excerpt
        );
    }

    /// Engine + project + session `s1` rooted at a scratch worktree, with an
    /// agent PTY in the exact state the excerpt race lives in: the child has
    /// EXITED and is reapable, but a surviving GRANDCHILD still holds the PTY
    /// slave open, so the reader thread never sees EOF and `is_exited()` stays
    /// false. `trap '' HUP` is what makes the grandchild survive: when the
    /// session leader (`sh`) exits, the kernel hangs up the controlling terminal
    /// and SIGHUPs its foreground process group, which would otherwise take the
    /// background job with it. The trailing `:` keeps the shell from
    /// exec-optimizing the subshell away, which would drop the ignored
    /// disposition. This holds the racy state open indefinitely instead of for
    /// the microseconds CI hits, so the tests below can pin it without racing.
    ///
    /// Returns once the child is reaped, and ASSERTS the premise, so a shell
    /// that behaved differently fails loudly rather than passing vacuously.
    fn engine_with_reaped_but_undrained_agent(worktree: &Path) -> (Engine, TempDir) {
        let (mut engine, tmp) = test_engine();
        engine
            .projects
            .push(sample_project("p1", worktree.to_string_lossy().as_ref()));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let client = PtyClient::spawn_with_env(
            "sh",
            &[
                "-c".to_string(),
                // The parent must not exit until the trap is actually installed,
                // or the SIGHUP wins the race and takes the grandchild with it.
                // The ready file is the handshake; it is written from inside the
                // subshell, after the trap.
                "(trap '' HUP; : > .grandchild-ready; sleep 30; :) & \
                 while [ ! -f .grandchild-ready ]; do sleep 0.01; done; exit 3"
                    .to_string(),
            ],
            worktree,
            24,
            80,
            1000,
            &[],
        )
        .expect("spawn sh");
        engine.providers.insert(TabId::new("s1-slot"), client);

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let client = engine
                .providers
                .get_mut(TabIdRef::new("s1-slot"))
                .expect("provider");
            if client.try_wait().is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "the child never exited");
            sleep(Duration::from_millis(5));
        }
        assert!(
            !engine.providers[TabIdRef::new("s1-slot")].is_exited(),
            "premise: the grandchild must still hold the PTY read side open, so \
             the reader has NOT reached EOF; without that this test proves nothing"
        );
        (engine, tmp)
    }

    /// The invariant behind the flaky excerpt test above, pinned without racing.
    /// `try_wait` reaps the child the instant it dies, but the terminal buffer is
    /// only complete once the READER thread reaches EOF on the PTY read side.
    /// Pruning on the reap alone captures `visible_text_excerpt` off a buffer the
    /// reader has not finished filling, and because `try_wait` yields the status
    /// exactly once there is no second chance to capture it later, so a crashed
    /// agent gets reported with an EMPTY excerpt, losing the very diagnostic the
    /// message exists to show.
    #[test]
    fn prune_defers_a_reaped_child_until_its_reader_has_drained() {
        let worktree = tempfile::tempdir().expect("worktree dir");
        let (mut engine, _tmp) = engine_with_reaped_but_undrained_agent(worktree.path());

        // Comfortably inside the drain grace, measured from the reap the fixture
        // already observed: the reader can never reach EOF here, so every pass in
        // this window must hold off.
        let reaped_at = engine.providers[TabIdRef::new("s1-slot")]
            .reaped_at()
            .expect("reaped");
        while reaped_at.elapsed() < REAPED_DRAIN_GRACE / 2 {
            let pruned = engine.prune_exited_ptys();
            assert!(
                pruned.is_empty(),
                "a reaped child whose reader has not reached EOF must not be pruned \
                 yet (its excerpt would be captured off an undrained buffer), got {pruned:?}"
            );
            sleep(Duration::from_millis(10));
        }
    }

    /// The bound on that deferral: a PTY whose read side is held open by a
    /// surviving grandchild never reaches EOF, so waiting for EOF alone would
    /// leak the provider forever. That is precisely why the reap arm of the
    /// prune condition exists and cannot simply be deleted. Once
    /// `REAPED_DRAIN_GRACE` has elapsed since the reap, prune takes it anyway,
    /// carrying the exit status cached at reap time.
    #[test]
    fn prune_takes_a_never_draining_pty_once_the_drain_grace_expires() {
        let worktree = tempfile::tempdir().expect("worktree dir");
        let (mut engine, _tmp) = engine_with_reaped_but_undrained_agent(worktree.path());
        let reaped_at = engine.providers[TabIdRef::new("s1-slot")]
            .reaped_at()
            .expect("reaped");

        let deadline = Instant::now() + Duration::from_secs(10);
        let pruned = loop {
            let pruned = engine.prune_exited_ptys();
            if let Some(entry) = pruned.into_iter().find(|p| p.id == "s1-slot") {
                break entry;
            }
            assert!(
                Instant::now() < deadline,
                "a PTY held open by a grandchild must still be pruned once the \
                 drain grace expires, or it would linger forever"
            );
            sleep(Duration::from_millis(10));
        };

        assert!(
            reaped_at.elapsed() >= REAPED_DRAIN_GRACE,
            "the prune must have waited out the drain grace, not fired on the reap"
        );
        assert_eq!(
            pruned.exit_success,
            Some(false),
            "the status cached at reap time must survive the deferral (the raw \
             try_wait yields it exactly once)"
        );
        assert!(
            !engine.providers.contains_key(TabIdRef::new("s1-slot")),
            "the never-draining provider must be gone from the engine"
        );
    }

    /// Engine + project + session `s1` with an agent PTY keyed `tab_id` in the
    /// MIRROR IMAGE of `engine_with_reaped_but_undrained_agent`: the reader HAS
    /// reached EOF (so the terminal buffer is complete) while the child is still
    /// ALIVE, therefore unreapable, therefore of UNKNOWN exit status.
    ///
    /// `exec 0<&- 1>&- 2>&-` closes the shell's three PTY-slave descriptors while
    /// it keeps running, and `PtyClient::spawn` already dropped the parent's own
    /// slave handle, so the master read returns end of input immediately even
    /// though nothing has exited. `linger` then decides how long the child stays
    /// alive before exiting 0. That holds the window open for as long as the test
    /// needs, instead of the microseconds a real clean exit leaves between EOF
    /// and reapability, so the tests below pin it by construction rather than by
    /// racing.
    ///
    /// Returns once the reader has reached EOF, and ASSERTS both halves of the
    /// premise, so a shell that behaved differently fails loudly rather than
    /// passing vacuously.
    fn engine_with_drained_but_unreaped_agent(
        worktree: &Path,
        tab_id: &str,
        linger: &str,
    ) -> (Engine, TempDir) {
        let (mut engine, tmp) = test_engine();
        engine
            .projects
            .push(sample_project("p1", worktree.to_string_lossy().as_ref()));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let client = PtyClient::spawn_with_env(
            "sh",
            &[
                "-c".to_string(),
                format!("exec 0<&- 1>&- 2>&-; sleep {linger}; exit 0"),
            ],
            worktree,
            24,
            80,
            1000,
            &[],
        )
        .expect("spawn sh");
        engine.providers.insert(TabId::new(tab_id), client);

        let deadline = Instant::now() + Duration::from_secs(5);
        // A 1ms poll keeps the observation as close to the EOF as possible, so
        // the shortest `linger` a caller can usefully pick stays generous.
        while !engine.providers[TabIdRef::new(tab_id)].is_exited() {
            assert!(
                Instant::now() < deadline,
                "the reader never reached end of input"
            );
            sleep(Duration::from_millis(1));
        }
        assert!(
            engine
                .providers
                .get_mut(TabIdRef::new(tab_id))
                .expect("provider")
                .try_wait()
                .is_none(),
            "premise: the child must still be RUNNING (unreapable, so its exit \
             status is unknown) while its reader is already at end of input; \
             without that this test proves nothing"
        );
        (engine, tmp)
    }

    /// End of input alone must not prune, because it does not carry the exit
    /// STATUS. The reader can EOF before the child is reapable (the kernel closes
    /// the descriptors in `do_exit` before it makes the task waitable), and in
    /// that window `try_wait` is `None`. Pruning there throws away the status
    /// permanently, and every decision keyed on it silently takes its unknown
    /// branch: `clean_exit_closes_tab_row` cannot fire, so a tab that exited
    /// cleanly keeps a dead row.
    #[test]
    fn prune_defers_a_drained_child_until_its_exit_status_is_known() {
        let worktree = tempfile::tempdir().expect("worktree dir");
        let (mut engine, _tmp) =
            engine_with_drained_but_unreaped_agent(worktree.path(), "s1-slot", "5");

        // Comfortably inside the grace, measured from the EOF the fixture already
        // observed: the child cannot exit here, so every pass must hold off.
        let eof_at = engine.providers[TabIdRef::new("s1-slot")]
            .exited_at()
            .expect("EOF stamped");
        while eof_at.elapsed() < REAPED_DRAIN_GRACE / 2 {
            let pruned = engine.prune_exited_ptys();
            assert!(
                pruned.is_empty(),
                "a drained child whose exit status is still unknown must not be \
                 pruned yet (the status would be lost for good), got {pruned:?}"
            );
            sleep(Duration::from_millis(10));
        }
    }

    /// The bound on THAT deferral. A child that closes its descriptors and keeps
    /// running reaches EOF and is never reaped, so waiting for the status alone
    /// would leak the provider forever. The grace is the safety valve in both
    /// directions: once it expires, prune takes the PTY with the status still
    /// unknown.
    #[test]
    fn prune_takes_a_never_reaped_child_once_the_drain_grace_expires() {
        let worktree = tempfile::tempdir().expect("worktree dir");
        let (mut engine, _tmp) =
            engine_with_drained_but_unreaped_agent(worktree.path(), "s1-slot", "30");
        let eof_at = engine.providers[TabIdRef::new("s1-slot")]
            .exited_at()
            .expect("EOF stamped");

        let deadline = Instant::now() + Duration::from_secs(10);
        let pruned = loop {
            let pruned = engine.prune_exited_ptys();
            if let Some(entry) = pruned.into_iter().find(|p| p.id == "s1-slot") {
                break entry;
            }
            assert!(
                Instant::now() < deadline,
                "a PTY whose child EOFs and never exits must still be pruned once \
                 the grace expires, or it would linger forever"
            );
            sleep(Duration::from_millis(10));
        };

        assert!(
            eof_at.elapsed() >= REAPED_DRAIN_GRACE,
            "the prune must have waited out the grace, not fired on the bare EOF"
        );
        assert_eq!(
            pruned.exit_success, None,
            "the status genuinely is unknown here, and the valve reports it as such"
        );
        assert!(
            !engine.providers.contains_key(TabIdRef::new("s1-slot")),
            "the never-reaped provider must be gone from the engine"
        );
    }

    /// The user-visible payoff, and the shape of the failure that surfaced this:
    /// an extra tab whose CLI exits cleanly must have its row closed. Its child
    /// reaches EOF a moment before it becomes reapable, so a prune that fires on
    /// the bare EOF records `exit_success: None`, `clean_exit_closes_tab_row`
    /// takes its unknown branch, and the user is left with a dormant tab that
    /// should have closed itself.
    #[test]
    fn prune_waits_for_the_status_so_a_clean_extra_tab_exit_closes_its_row() {
        let worktree = tempfile::tempdir().expect("worktree dir");
        // 100ms: long enough that the fixture reliably observes EOF while the
        // child is still alive (it polls at 1ms), and short enough that the reap
        // lands well inside `REAPED_DRAIN_GRACE`, so this exercises the
        // status-arrives path rather than the expiry valve.
        let (mut engine, _tmp) =
            engine_with_drained_but_unreaped_agent(worktree.path(), "tab-x", "0.1");
        engine
            .agent_tabs
            .insert(TabId::new("tab-x"), sample_tab("tab-x", "s1", "claude", 1));

        let deadline = Instant::now() + Duration::from_secs(5);
        let pruned = loop {
            let pruned = engine.prune_exited_ptys();
            if let Some(entry) = pruned.into_iter().find(|p| p.id == "tab-x") {
                break entry;
            }
            assert!(Instant::now() < deadline, "the tab PTY was never pruned");
            sleep(Duration::from_millis(5));
        };

        assert_eq!(
            pruned.exit_success,
            Some(true),
            "the prune must carry the child's real exit status, not the unknown \
             it has while the child is still being reaped"
        );
        assert!(
            pruned.tab_closed,
            "a clean extra-tab exit must close the tab row"
        );
        assert!(
            !engine.agent_tabs.contains_key(TabIdRef::new("tab-x")),
            "the cleanly-exited tab's row must be gone"
        );
    }

    /// The policy in isolation, with no PTY at all: both facts prune immediately,
    /// either one alone waits, and either clock running past the grace takes the
    /// PTY anyway.
    #[test]
    fn agent_pty_ready_to_prune_wants_both_facts_and_falls_back_to_either_clock() {
        let inside = Some(REAPED_DRAIN_GRACE - Duration::from_millis(1));
        let expired = Some(REAPED_DRAIN_GRACE);

        assert!(
            !agent_pty_ready_to_prune(false, None, None),
            "a live child is not ready to prune"
        );
        assert!(
            agent_pty_ready_to_prune(true, Some(Duration::ZERO), Some(Duration::ZERO)),
            "a drained buffer AND a known status is the whole condition: prune now"
        );
        assert!(
            !agent_pty_ready_to_prune(false, Some(Duration::ZERO), None),
            "end of input without a status must wait: pruning here loses the exit \
             status for good, and a clean exit stops closing its tab row"
        );
        assert!(
            !agent_pty_ready_to_prune(true, None, Some(Duration::ZERO)),
            "a just-reaped child whose reader is still going must wait"
        );
        assert!(
            !agent_pty_ready_to_prune(false, inside, None),
            "still inside the grace, measured from end of input"
        );
        assert!(
            !agent_pty_ready_to_prune(true, None, inside),
            "still inside the grace, measured from the reap"
        );
        assert!(
            agent_pty_ready_to_prune(false, expired, None),
            "the grace bounds a child that EOFs and is never reaped: at it, prune"
        );
        assert!(
            agent_pty_ready_to_prune(true, None, expired),
            "the grace bounds a reader held open by a grandchild: at it, prune"
        );
    }

    #[test]
    fn prune_carries_clean_exit_success_and_terminal_has_no_message_fields() {
        // A clean exit reports Some(true); a companion terminal never carries
        // the agent exit-message fields (its exit has no status copy).
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        // Clean-exiting agent (cat exits 0 on EOF).
        let client = spawn_cat(worktree.path());
        engine.providers.insert(TabId::new("s1-slot"), client);
        engine
            .providers
            .get_mut(TabIdRef::new("s1-slot"))
            .unwrap()
            .write_bytes(b"\x04")
            .unwrap();

        // A companion terminal that will also EOF-exit.
        let (terminal_id, _label) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");
        engine
            .companion_terminals
            .get(&terminal_id)
            .unwrap()
            .client
            .write_bytes(b"\x04")
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut agent_seen = None;
        let mut terminal_seen = None;
        while Instant::now() < deadline && (agent_seen.is_none() || terminal_seen.is_none()) {
            for p in engine.prune_exited_ptys() {
                if p.id == "s1-slot" {
                    agent_seen = Some(p);
                } else if p.id == terminal_id {
                    terminal_seen = Some(p);
                }
            }
            if agent_seen.is_none() || terminal_seen.is_none() {
                sleep(Duration::from_millis(50));
            }
        }

        let agent = agent_seen.expect("agent pruned");
        assert_eq!(
            agent.exit_success,
            Some(true),
            "a clean exit must carry exit_success = Some(true)"
        );
        let terminal = terminal_seen.expect("terminal pruned");
        assert_eq!(terminal.exit_success, None);
        assert!(!terminal.is_minimal);
        assert!(terminal.output_excerpt.is_empty());
    }

    #[test]
    fn prune_removes_exited_companion_terminal() {
        let (mut engine, _tmp) = test_engine();

        // A real worktree directory the PTY can `cwd` into.
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        // `cat` echoes stdin and exits on EOF — a safe stand-in terminal.
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (terminal_id, _label) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");
        assert_eq!(terminal_id, "term-1");
        assert!(engine.companion_terminals.contains_key("term-1"));

        // Ctrl-d (EOF) in canonical mode causes `cat` to exit.
        engine
            .companion_terminals
            .get("term-1")
            .unwrap()
            .client
            .write_bytes(b"\x04")
            .unwrap();

        // Poll until the prune detects the exit (or the terminal is gone).
        let deadline = Instant::now() + Duration::from_secs(3);
        let pruned = loop {
            let pruned = engine.prune_exited_ptys();
            if !pruned.is_empty() || !engine.companion_terminals.contains_key("term-1") {
                break pruned;
            }
            assert!(
                Instant::now() < deadline,
                "companion terminal never reported exit"
            );
            sleep(Duration::from_millis(50));
        };

        assert!(
            pruned
                .iter()
                .any(|p| p.kind == PrunedPtyKind::Terminal && p.id == "term-1"),
            "expected a pruned terminal entry for term-1, got {pruned:?}"
        );
        assert!(
            !engine.companion_terminals.contains_key("term-1"),
            "term-1 should have been removed from companion_terminals"
        );
    }

    #[test]
    fn prune_clears_desired_running_on_clean_exit() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        session.desired_running = true;
        // Store the session so `mark_*` persists cleanly.
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // A clean-exiting agent provider (cat exits 0 on EOF).
        let client = spawn_cat(worktree.path());
        engine.providers.insert(TabId::new("s1-slot"), client);
        // The activity and input stamps must die with the provider — a
        // long-running server would otherwise leak one entry per exited agent.
        engine
            .pty_activity
            .insert("s1-slot".to_string(), Instant::now());
        engine
            .pty_input
            .insert("s1-slot".to_string(), Instant::now());

        // Ctrl-d (EOF) makes cat exit with status 0.
        engine
            .providers
            .get_mut(TabIdRef::new("s1-slot"))
            .unwrap()
            .write_bytes(b"\x04")
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let pruned = engine.prune_exited_ptys();
            if pruned.iter().any(|p| p.id == "s1-slot") {
                break;
            }
            assert!(Instant::now() < deadline, "agent provider never exited");
            sleep(Duration::from_millis(50));
        }

        let session = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert!(
            !session.desired_running,
            "a clean exit should clear desired_running"
        );
        assert_eq!(session.status, SessionStatus::Detached);
        assert!(
            !engine.pty_activity.contains_key("s1-slot"),
            "pruning an exited agent must clear its activity stamp"
        );
        assert!(
            !engine.pty_input.contains_key("s1-slot"),
            "pruning an exited agent must clear its input stamp"
        );
    }

    #[test]
    fn prune_fires_a_pr_recheck_when_the_agent_exits() {
        // The shared exit handling (used by the web) must re-check the agent's PR
        // the moment it exits, because an exit commonly follows a merge and the
        // badge would otherwise stay stale until the next background sync.
        let (mut engine, _tmp) = test_engine();
        engine.github_integration_enabled = true;
        engine.gh_status = crate::model::GhStatus::Available;

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let client = spawn_cat(worktree.path());
        engine.providers.insert(TabId::new("s1-slot"), client);
        engine
            .providers
            .get_mut(TabIdRef::new("s1-slot"))
            .unwrap()
            .write_bytes(b"\x04")
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let pruned = engine.prune_exited_ptys();
            if pruned.iter().any(|p| p.id == "s1-slot") {
                break;
            }
            assert!(Instant::now() < deadline, "agent provider never exited");
            sleep(Duration::from_millis(50));
        }

        assert!(
            engine.pr_last_checked.contains_key("s1"),
            "an agent exit must trigger a PR re-check (the debounce stamp proves it fired)",
        );
    }

    #[test]
    fn prune_keeps_desired_running_on_nonclean_exit() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        session.desired_running = true;
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // A provider that exits non-zero immediately.
        let client = PtyClient::spawn_with_env(
            "sh",
            &["-c".to_string(), "exit 1".to_string()],
            worktree.path(),
            24,
            80,
            1000,
            &[],
        )
        .expect("spawn sh");
        engine.providers.insert(TabId::new("s1-slot"), client);

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let pruned = engine.prune_exited_ptys();
            if pruned.iter().any(|p| p.id == "s1-slot") {
                break;
            }
            assert!(Instant::now() < deadline, "agent provider never exited");
            sleep(Duration::from_millis(50));
        }

        let session = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert!(
            session.desired_running,
            "a non-clean exit should leave desired_running set"
        );
        assert_eq!(session.status, SessionStatus::Detached);
    }

    /// Engine with the global auto-reopen switch ON and one session that meets
    /// EVERY eligibility condition: desired_running, per-agent auto-reopen on,
    /// a worktree that exists on disk (the tempdir), a project with no opt-out,
    /// and a resume-capable provider (claude). Each matrix test flips exactly
    /// one condition off and asserts the candidate disappears.
    fn auto_reopen_fixture() -> (Engine, TempDir, tempfile::TempDir) {
        let (mut engine, tmp) = test_engine();
        engine.config.ui.auto_reopen_agents = true;
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "b1");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        session.desired_running = true;
        session.auto_reopen_enabled = true;
        engine.sessions.push(session);
        (engine, tmp, worktree)
    }

    fn candidate_ids(engine: &Engine) -> Vec<String> {
        engine
            .auto_reopen_candidates()
            .into_iter()
            .map(|s| s.id)
            .collect()
    }

    #[test]
    fn auto_reopen_candidates_returns_a_fully_eligible_session() {
        let (engine, _tmp, _worktree) = auto_reopen_fixture();
        assert_eq!(candidate_ids(&engine), vec!["s1".to_string()]);
    }

    #[test]
    fn auto_reopen_candidates_empty_when_the_global_switch_is_off() {
        let (mut engine, _tmp, _worktree) = auto_reopen_fixture();
        engine.config.ui.auto_reopen_agents = false;
        assert!(candidate_ids(&engine).is_empty());
    }

    #[test]
    fn auto_reopen_candidates_skip_a_session_without_desired_running() {
        let (mut engine, _tmp, _worktree) = auto_reopen_fixture();
        engine.sessions[0].desired_running = false;
        assert!(candidate_ids(&engine).is_empty());
    }

    #[test]
    fn auto_reopen_candidates_skip_a_session_that_opted_out() {
        let (mut engine, _tmp, _worktree) = auto_reopen_fixture();
        engine.sessions[0].auto_reopen_enabled = false;
        assert!(candidate_ids(&engine).is_empty());
    }

    #[test]
    fn auto_reopen_candidates_skip_a_session_whose_worktree_vanished() {
        let (mut engine, _tmp, worktree) = auto_reopen_fixture();
        engine.sessions[0]
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree
            .path()
            .join("does-not-exist")
            .to_string_lossy()
            .to_string();
        assert!(candidate_ids(&engine).is_empty());
    }

    #[test]
    fn auto_reopen_candidates_skip_a_project_that_opted_out() {
        let (mut engine, _tmp, _worktree) = auto_reopen_fixture();
        engine.projects[0].auto_reopen_agents = Some(false);
        assert!(candidate_ids(&engine).is_empty());
    }

    #[test]
    fn auto_reopen_candidates_skip_a_provider_without_session_resume() {
        let (mut engine, _tmp, _worktree) = auto_reopen_fixture();
        // Copilot ships with `resume_args: None` (excluded from resume by
        // design, see the tabs tenet), so it can never auto-reopen.
        engine.sessions[0].provider = crate::model::ProviderKind::new("copilot");
        assert!(candidate_ids(&engine).is_empty());
    }

    /// A restored standalone agent's folder is classified at STARTUP, not only
    /// when its changes panel opens.
    ///
    /// The reason is not the panel: an unprobed folder reads as Indeterminate,
    /// which fails CLOSED for mutations and for the upload directory's
    /// gitignore seed. Waiting for the panel would leave a file dropped before
    /// then unseeded in a folder git can see, and dux never goes back to clean
    /// a folder up.
    #[test]
    fn restoring_sessions_classifies_every_standalone_folder() {
        let (mut engine, _tmp) = test_engine();
        let repo = tempfile::tempdir().expect("repo");
        init_repo_at(repo.path());
        let plain = tempfile::tempdir().expect("plain");
        engine
            .sessions
            .push(crate::engine::test_support::sample_standalone_session(
                "sa-repo",
                repo.path().to_string_lossy().as_ref(),
            ));
        engine
            .sessions
            .push(crate::engine::test_support::sample_standalone_session(
                "sa-plain",
                plain.path().to_string_lossy().as_ref(),
            ));
        // A managed agent is never probed: its worktree is a repository by
        // construction, so there is nothing to ask git about.
        engine.sessions.push(sample_session("s1", "p1", "b1"));

        engine.normalize_restored_sessions();

        let mut seen = 0;
        let deadline = Instant::now() + Duration::from_secs(20);
        while seen < 2 && Instant::now() < deadline {
            let Ok(event) = engine.worker_rx.recv_timeout(Duration::from_millis(500)) else {
                continue;
            };
            if matches!(
                event,
                crate::worker::WorkerEvent::FolderRepoStatusReady { .. }
            ) {
                seen += 1;
            }
            let _ = engine.process_worker_event(event);
        }
        assert_eq!(seen, 2, "one probe per standalone agent, and no others");
        assert_eq!(
            engine.folder_repo_status("sa-repo"),
            crate::git::FolderRepoStatus::WorkingRepo
        );
        assert_eq!(
            engine.folder_repo_status("sa-plain"),
            crate::git::FolderRepoStatus::NoRepo
        );
    }

    fn init_repo_at(path: &std::path::Path) {
        let out = crate::git::test_support::git_command()
            .args(["init"])
            .current_dir(path)
            .output()
            .expect("git init");
        assert!(out.status.success(), "git init failed");
    }

    /// A standalone agent belongs to no project, so the project consult must
    /// not happen at all for it. `project_allows_auto_reopen` fails OPEN on an
    /// unknown project, so a faked empty project id would sail through and hide
    /// the fact that the question was never answered deliberately. The
    /// structural switch is what this pins: folder exists, provider can resume,
    /// no project consult.
    fn standalone_auto_reopen_fixture() -> (Engine, TempDir, tempfile::TempDir) {
        let (mut engine, tmp) = test_engine();
        engine.config.ui.auto_reopen_agents = true;
        let folder = tempfile::tempdir().expect("folder");
        let mut session = crate::engine::test_support::sample_standalone_session(
            "sa1",
            &folder.path().to_string_lossy(),
        );
        session.desired_running = true;
        session.auto_reopen_enabled = true;
        engine.sessions.push(session);
        (engine, tmp, folder)
    }

    #[test]
    fn a_standalone_agent_auto_reopens_without_consulting_any_project() {
        let (engine, _tmp, _folder) = standalone_auto_reopen_fixture();
        assert_eq!(candidate_ids(&engine), vec!["sa1".to_string()]);
    }

    /// The one project-shaped opt-out that could plausibly leak in: a project
    /// that opted out must not silence an agent that has nothing to do with it.
    #[test]
    fn a_project_opt_out_cannot_silence_a_standalone_agent() {
        let (mut engine, _tmp, folder) = standalone_auto_reopen_fixture();
        engine.projects.push(sample_project(
            "p1",
            folder.path().to_string_lossy().as_ref(),
        ));
        engine.projects[0].auto_reopen_agents = Some(false);
        assert_eq!(candidate_ids(&engine), vec!["sa1".to_string()]);
    }

    #[test]
    fn a_standalone_agent_whose_folder_vanished_is_not_a_candidate() {
        let (mut engine, _tmp, folder) = standalone_auto_reopen_fixture();
        let gone = folder.path().join("no-such-directory");
        engine.sessions[0].workspace =
            crate::model::AgentWorkspace::Folder(crate::model::FolderWorkspace {
                folder_path: gone.to_string_lossy().to_string(),
            });
        assert!(candidate_ids(&engine).is_empty());
    }

    #[test]
    fn a_standalone_agent_on_a_provider_that_cannot_resume_is_not_a_candidate() {
        let (mut engine, _tmp, _folder) = standalone_auto_reopen_fixture();
        engine.sessions[0].provider = crate::model::ProviderKind::new("copilot");
        assert!(candidate_ids(&engine).is_empty());
    }

    /// The user-initiated kill teardown must clear `desired_running` when the
    /// kill detaches the agent (its last live tab is gone), so the startup
    /// auto-reopen pass does NOT relaunch an agent the user deliberately
    /// killed. Tied to the `auto_reopen_candidates` predicate: the killed
    /// agent must not appear among the candidates afterward.
    #[test]
    fn kill_tab_runtime_clears_desired_running_and_drops_the_auto_reopen_candidate() {
        let (mut engine, _tmp, _worktree) = auto_reopen_fixture();
        // Give the eligible session a live provider on its session-slot tab so
        // there is something to kill.
        let worktree = engine.sessions[0].directory().to_string();
        let client = spawn_cat(Path::new(&worktree));
        engine.providers.insert(TabId::new("s1-slot"), client);
        assert_eq!(candidate_ids(&engine), vec!["s1".to_string()]);

        let outcome = engine.kill_tab_runtime("s1-slot");
        assert!(outcome.killed, "the live provider was killed");
        assert!(
            outcome.detached,
            "the last live tab is gone, so it detached"
        );
        let session = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(session.status, SessionStatus::Detached);
        assert!(
            !session.desired_running,
            "a deliberate kill must clear the auto-reopen intent"
        );
        assert!(
            candidate_ids(&engine).is_empty(),
            "a killed agent must not be an auto-reopen candidate"
        );
        // Full teardown ran (the single-source clear), not just the provider drop.
        assert!(!engine.providers.contains_key(TabIdRef::new("s1-slot")));
    }

    /// Killing one of several live tabs does not detach the agent (a sibling
    /// stays live), so `desired_running` is left ALONE: the agent is still
    /// wanted running.
    #[test]
    fn kill_tab_runtime_keeps_desired_running_when_a_sibling_stays_live() {
        let (mut engine, _tmp, _worktree) = auto_reopen_fixture();
        let worktree = engine.sessions[0].directory().to_string();
        engine
            .providers
            .insert(TabId::new("s1-slot"), spawn_cat(Path::new(&worktree)));
        // An extra tab, also live.
        engine
            .agent_tabs
            .insert(TabId::new("tab-2"), sample_tab("tab-2", "s1", "claude", 1));
        engine
            .providers
            .insert(TabId::new("tab-2"), spawn_cat(Path::new(&worktree)));

        let outcome = engine.kill_tab_runtime("s1-slot");
        assert!(outcome.killed);
        assert!(!outcome.detached, "a live sibling keeps the agent attached");
        let session = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert!(
            session.desired_running,
            "an agent with a live tab is still wanted running"
        );
    }

    /// Closing an extra tab while a SIBLING has an in-flight launch must report
    /// `detached: false`. The core outcome uses `any_tab_active`, which is
    /// in-flight-aware; a `has_live_process` (`providers`-only) derivation
    /// misses the in-flight launch and wrongly reports the agent detached.
    #[test]
    fn close_tab_reports_not_detached_when_a_sibling_launch_is_in_flight() {
        let (mut engine, _tmp, _worktree) = auto_reopen_fixture();
        // Active so a wrongful detach is observable (sample_session defaults to
        // Detached, which would mask the mark).
        engine.sessions[0].status = SessionStatus::Active;
        // The extra tab we close.
        engine
            .agent_tabs
            .insert(TabId::new("tab-2"), sample_tab("tab-2", "s1", "claude", 1));
        engine
            .session_store
            .insert_agent_tab(engine.agent_tabs.get(TabIdRef::new("tab-2")).unwrap())
            .unwrap();
        // A sibling (the session-slot tab) is mid-launch: no provider yet, but an
        // in-flight AgentLaunch key. `has_live_process` would miss this.
        engine.mark_in_flight(crate::engine::InFlightKey::AgentLaunch(TabId::new(
            "s1-slot",
        )));

        let outcome = engine.close_tab("s1", "tab-2").expect("close ok");
        assert!(
            !outcome.detached,
            "a sibling with an in-flight launch keeps the agent attached"
        );
        assert_ne!(
            engine
                .sessions
                .iter()
                .find(|s| s.id == "s1")
                .unwrap()
                .status,
            SessionStatus::Detached,
            "the agent must not be marked detached while a launch is in flight"
        );
    }

    #[test]
    fn close_tab_reports_detached_when_it_was_the_last_live_tab() {
        let (mut engine, _tmp, _worktree) = auto_reopen_fixture();
        engine
            .agent_tabs
            .insert(TabId::new("tab-2"), sample_tab("tab-2", "s1", "claude", 1));
        engine
            .session_store
            .insert_agent_tab(engine.agent_tabs.get(TabIdRef::new("tab-2")).unwrap())
            .unwrap();
        // Nothing else of s1 is live (no provider, no in-flight): closing this
        // tab leaves the agent with no live tab.
        let outcome = engine.close_tab("s1", "tab-2").expect("close ok");
        assert!(
            outcome.detached,
            "no live tab remains, so the agent detached"
        );
    }

    #[test]
    fn kill_tab_runtime_reports_not_killed_for_a_tab_with_no_live_provider() {
        let (mut engine, _tmp, _worktree) = auto_reopen_fixture();
        let outcome = engine.kill_tab_runtime("s1-slot");
        assert!(!outcome.killed, "no live provider means nothing was killed");
        assert!(!outcome.detached);
    }

    #[test]
    fn normalize_restored_sessions_marks_detached_and_exited() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));

        let mut present = sample_session("present", "p1", "here");
        present
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        present.status = SessionStatus::Active;
        engine.session_store.upsert_session(&present).unwrap();
        engine.sessions.push(present);

        let mut gone = sample_session("gone", "p1", "gone");
        gone.workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree
            .path()
            .join("does-not-exist")
            .to_string_lossy()
            .to_string();
        gone.status = SessionStatus::Active;
        engine.session_store.upsert_session(&gone).unwrap();
        engine.sessions.push(gone);

        engine.normalize_restored_sessions();

        let present = engine.sessions.iter().find(|s| s.id == "present").unwrap();
        assert_eq!(present.status, SessionStatus::Detached);
        let gone = engine.sessions.iter().find(|s| s.id == "gone").unwrap();
        assert_eq!(gone.status, SessionStatus::Exited);
    }

    #[test]
    fn shutdown_ptys_terminates_children() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // A provider that does not exit on its own — it must be SIGTERMed.
        let client = spawn_cat(worktree.path());
        engine.providers.insert(TabId::new("s1-slot"), client);

        engine.shutdown_ptys(Duration::from_secs(2));

        let client = engine.providers.get_mut(TabIdRef::new("s1-slot")).unwrap();
        assert!(
            client.is_exited() || client.try_wait().is_some(),
            "cat should have exited after SIGTERM"
        );
        let session = engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(session.status, SessionStatus::Detached);
    }

    #[test]
    fn shutdown_ptys_terminates_companion_terminal() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        // A `cat`-backed companion terminal that won't exit on its own; it must
        // be SIGTERMed by shutdown_ptys, just like an agent provider.
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (terminal_id, _label) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");
        assert!(engine.companion_terminals.contains_key(&terminal_id));

        engine.shutdown_ptys(Duration::from_secs(2));

        let terminal = engine.companion_terminals.get_mut(&terminal_id).unwrap();
        assert!(
            terminal.client.is_exited() || terminal.client.try_wait().is_some(),
            "the companion terminal's cat should have exited after SIGTERM"
        );
    }

    /// A child that ignores the whole graceful-shutdown salvo (SIGTERM and
    /// SIGHUP, which `terminate()` sends back to back) so it must be SIGKILLed,
    /// and never exits on its own. The `trap` makes the shell ignore both; the
    /// `echo` then emits a marker AFTER the trap is installed, so a caller can
    /// poll `has_output()` to know the trap is live before signalling —
    /// otherwise a signal that lands during shell startup (before `trap` runs)
    /// would kill it by default and the test would not exercise the force-kill
    /// path. The busy loop keeps it alive.
    fn spawn_sigterm_ignorer(cwd: &Path) -> PtyClient {
        PtyClient::spawn_with_env(
            "sh",
            &[
                "-c".to_string(),
                "trap '' TERM HUP; echo ready; while true; do :; done".to_string(),
            ],
            cwd,
            24,
            80,
            1000,
            &[],
        )
        .expect("spawn sigterm-ignorer")
    }

    /// Block until the SIGTERM-ignorer has printed its readiness marker (proof
    /// the `trap` is installed) or a timeout elapses.
    fn wait_until_ready(engine: &crate::engine::Engine, id: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !engine
            .providers
            .get(TabIdRef::new(id))
            .expect("provider present")
            .has_output()
        {
            assert!(
                Instant::now() < deadline,
                "sigterm-ignorer never signalled ready (trap not installed)"
            );
            sleep(Duration::from_millis(20));
        }
    }

    fn wait_until_terminal_ready(engine: &crate::engine::Engine, id: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !engine
            .companion_terminals
            .get(id)
            .expect("terminal present")
            .client
            .has_output()
        {
            assert!(
                Instant::now() < deadline,
                "terminal sigterm-ignorer never signalled ready"
            );
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn shutdown_ptys_reports_clean_exit() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // `cat` exits promptly on SIGTERM, so the grace window is never hit.
        engine
            .providers
            .insert(TabId::new("s1-slot"), spawn_cat(worktree.path()));

        let report = engine.shutdown_ptys(Duration::from_secs(5));

        assert_eq!(report.agents_total, 1);
        assert_eq!(report.terminals_total, 0);
        assert_eq!(report.agents_exited, 1, "cat should exit on SIGTERM");
        assert!(
            !report.timed_out,
            "a SIGTERM-respecting child is not a timeout"
        );
        assert!(
            report.elapsed < Duration::from_secs(5),
            "a clean exit should return well before the deadline"
        );
    }

    #[test]
    fn shutdown_ptys_force_kills_stragglers_and_reports_timeout() {
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // An agent that ignores SIGTERM: it must be force-killed at the deadline.
        engine.providers.insert(
            TabId::new("s1-slot"),
            spawn_sigterm_ignorer(worktree.path()),
        );
        // Ensure the trap is installed before signalling, so SIGTERM doesn't kill
        // the shell during startup and bypass the force-kill path under test.
        wait_until_ready(&engine, "s1-slot");

        let report = engine.shutdown_ptys(Duration::from_millis(300));

        assert!(report.timed_out, "a SIGTERM-ignoring child must time out");
        assert_eq!(report.agents_total, 1);
        assert_eq!(
            report.agents_exited, 0,
            "the straggler had not exited cleanly"
        );
        assert!(
            report.elapsed >= Duration::from_millis(300),
            "a timeout must wait out the full grace period"
        );

        // force_terminate sent SIGKILL to the group; the child must now die.
        let client = engine.providers.get_mut(TabIdRef::new("s1-slot")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !(client.is_exited() || client.try_wait().is_some()) {
            assert!(
                Instant::now() < deadline,
                "force_terminate's SIGKILL should have reaped the straggler"
            );
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn shutdown_ptys_interruptible_aborts_the_wait_early() {
        // The second-signal escape hatch: an abort flip during the grace wait cuts
        // it short and force-kills, instead of waiting out the full (here 30s)
        // timeout behind a SIGTERM-ignoring child.
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        engine.providers.insert(
            TabId::new("s1-slot"),
            spawn_sigterm_ignorer(worktree.path()),
        );
        wait_until_ready(&engine, "s1-slot");

        // Flip the abort ~200ms into the (30s) wait from another thread.
        let abort = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let abort_setter = std::sync::Arc::clone(&abort);
        std::thread::spawn(move || {
            sleep(Duration::from_millis(200));
            abort_setter.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        let report = engine.shutdown_ptys_interruptible(Duration::from_secs(30), Some(&abort));

        assert!(report.timed_out, "an aborted wait force-kills survivors");
        assert!(
            report.elapsed < Duration::from_secs(5),
            "the abort must cut the 30s wait short, took {:?}",
            report.elapsed
        );

        // The straggler is still reaped by the force-kill.
        let client = engine.providers.get_mut(TabIdRef::new("s1-slot")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !(client.is_exited() || client.try_wait().is_some()) {
            assert!(
                Instant::now() < deadline,
                "the aborted shutdown should still SIGKILL the straggler"
            );
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn shutdown_ptys_no_children_is_a_silent_noop() {
        let (mut engine, _tmp) = test_engine();
        let report = engine.shutdown_ptys(Duration::from_secs(30));
        assert_eq!(report.agents_total, 0);
        assert_eq!(report.terminals_total, 0);
        assert!(!report.timed_out);
        assert_eq!(report.elapsed, Duration::ZERO);
    }

    #[test]
    fn shutdown_ptys_force_kills_straggler_terminal() {
        // The terminals tally/force-kill block is coded separately from the
        // agents one; exercise it directly so a copy-paste slip there is caught.
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        // A companion terminal backed by the SIGTERM-ignorer instead of `cat`.
        engine.config.terminal.command = "sh".to_string();
        engine.config.terminal.args = vec![
            "-c".to_string(),
            "trap '' TERM HUP; echo ready; while true; do :; done".to_string(),
        ];
        let (terminal_id, _label) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");

        // Wait until the trap is installed (marker printed) before signalling.
        let deadline = Instant::now() + Duration::from_secs(3);
        while !engine
            .companion_terminals
            .get(&terminal_id)
            .unwrap()
            .client
            .has_output()
        {
            assert!(Instant::now() < deadline, "terminal ignorer never readied");
            sleep(Duration::from_millis(20));
        }

        let report = engine.shutdown_ptys(Duration::from_millis(300));

        assert!(
            report.timed_out,
            "a SIGTERM-ignoring terminal must time out"
        );
        assert_eq!(report.terminals_total, 1);
        assert_eq!(report.agents_total, 0);
        assert_eq!(
            report.terminals_exited, 0,
            "the straggler terminal had not exited cleanly"
        );

        // force_terminate's SIGKILL must reap it.
        let term = engine.companion_terminals.get_mut(&terminal_id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !(term.client.is_exited() || term.client.try_wait().is_some()) {
            assert!(
                Instant::now() < deadline,
                "force_terminate should have reaped the straggler terminal"
            );
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn shutdown_ptys_mixed_clean_and_straggler() {
        // One agent exits cleanly on SIGTERM, the other ignores it: the report
        // must count them separately and still flag timed_out, proving the
        // wait-loop's `.all(...)` aggregation does not short-circuit on the first
        // exited child.
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        for id in ["clean", "straggler"] {
            let mut session = sample_session(id, "p1", "feat");
            session
                .workspace
                .as_managed_mut()
                .expect("managed test session")
                .worktree_path = worktree.path().to_string_lossy().to_string();
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
        }

        engine
            .providers
            .insert(TabId::new("clean"), spawn_cat(worktree.path()));
        engine.providers.insert(
            TabId::new("straggler"),
            spawn_sigterm_ignorer(worktree.path()),
        );
        wait_until_ready(&engine, "straggler");

        let report = engine.shutdown_ptys(Duration::from_millis(300));

        assert_eq!(report.agents_total, 2);
        assert_eq!(
            report.agents_exited, 1,
            "only the cat agent exits on SIGTERM"
        );
        assert!(report.timed_out, "the straggler forces a timeout");
        assert!(
            report.elapsed >= Duration::from_millis(300),
            "the loop must wait the full grace for the straggler, not stop early"
        );
    }

    #[test]
    fn shutdown_ptys_grace_zero_force_kills_without_waiting() {
        // grace == 0 means "force immediately": the wait loop is skipped, so a
        // straggler is SIGKILLed at once and reported timed_out, with near-zero
        // elapsed.
        let (mut engine, _tmp) = test_engine();

        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        engine.providers.insert(
            TabId::new("s1-slot"),
            spawn_sigterm_ignorer(worktree.path()),
        );
        wait_until_ready(&engine, "s1-slot");

        let report = engine.shutdown_ptys(Duration::ZERO);

        assert!(
            report.timed_out,
            "grace 0 with a live child is a forced close"
        );
        assert_eq!(report.agents_exited, 0);
        assert!(
            report.elapsed < Duration::from_millis(50),
            "grace 0 must not enter the 50ms poll loop, got {:?}",
            report.elapsed
        );

        // The child must still be reaped by the immediate SIGKILL.
        let client = engine.providers.get_mut(TabIdRef::new("s1-slot")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !(client.is_exited() || client.try_wait().is_some()) {
            assert!(
                Instant::now() < deadline,
                "grace-0 force_terminate should have reaped the child"
            );
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn shutdown_ptys_zero_grace_tallies_agent_and_terminal_as_forced() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        engine.providers.insert(
            TabId::new("s1-slot"),
            spawn_sigterm_ignorer(worktree.path()),
        );
        wait_until_ready(&engine, "s1-slot");
        engine.config.terminal.command = "sh".to_string();
        engine.config.terminal.args = vec![
            "-c".to_string(),
            "trap '' TERM HUP; echo ready; while true; do :; done".to_string(),
        ];
        let (terminal_id, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");
        wait_until_terminal_ready(&engine, &terminal_id);

        let report = engine.shutdown_ptys(Duration::ZERO);

        assert_eq!(report.agents_total, 1);
        assert_eq!(report.terminals_total, 1);
        assert_eq!(report.agents_exited, 0);
        assert_eq!(report.terminals_exited, 0);
        assert!(report.timed_out);
        assert!(report.elapsed < Duration::from_millis(50));
    }

    #[test]
    fn shutdown_ptys_pre_set_abort_skips_wait_and_force_kills() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.providers.insert(
            TabId::new("straggler"),
            spawn_sigterm_ignorer(worktree.path()),
        );
        wait_until_ready(&engine, "straggler");
        let abort = std::sync::atomic::AtomicBool::new(true);

        let report = engine.shutdown_ptys_interruptible(Duration::from_secs(30), Some(&abort));

        assert_eq!(report.agents_total, 1);
        assert_eq!(report.agents_exited, 0);
        assert!(report.timed_out);
        assert!(report.elapsed < Duration::from_millis(50));
    }

    #[test]
    fn shutdown_ptys_tallies_clean_and_forced_agents_and_terminals() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        engine
            .providers
            .insert(TabId::new("clean-agent"), spawn_cat(worktree.path()));
        engine.providers.insert(
            TabId::new("forced-agent"),
            spawn_sigterm_ignorer(worktree.path()),
        );
        wait_until_ready(&engine, "forced-agent");

        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args.clear();
        engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create clean terminal");
        engine.config.terminal.command = "sh".to_string();
        engine.config.terminal.args = vec![
            "-c".to_string(),
            "trap '' TERM HUP; echo ready; while true; do :; done".to_string(),
        ];
        let (forced_terminal_id, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create forced terminal");
        wait_until_terminal_ready(&engine, &forced_terminal_id);

        let report = engine.shutdown_ptys(Duration::from_millis(300));

        assert_eq!(report.agents_total, 2);
        assert_eq!(report.terminals_total, 2);
        assert_eq!(report.agents_exited, 1);
        assert_eq!(report.terminals_exited, 1);
        assert!(report.timed_out);
        assert!(report.elapsed >= Duration::from_millis(300));
    }

    #[test]
    fn shutdown_ptys_detaches_and_persists_session_owned_only_by_extra_tab() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        session.status = SessionStatus::Active;
        session.desired_running = true;
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        engine
            .agent_tabs
            .insert(TabId::new("tab-2"), sample_tab("tab-2", "s1", "codex", 1));
        engine
            .providers
            .insert(TabId::new("tab-2"), spawn_cat(worktree.path()));

        let report = engine.shutdown_ptys(Duration::from_secs(2));

        assert_eq!(report.agents_exited, 1);
        let session = engine.sessions.iter().find(|item| item.id == "s1").unwrap();
        assert_eq!(session.status, SessionStatus::Detached);
        assert!(session.desired_running);
        let stored = engine
            .session_store
            .load_sessions()
            .unwrap()
            .into_iter()
            .find(|item| item.id == "s1")
            .expect("persisted session");
        assert_eq!(stored.status, SessionStatus::Detached);
        assert!(stored.desired_running);
    }

    #[test]
    fn begin_close_companion_terminal_moves_to_terminating_and_reaps_on_exit() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (tid, _label) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");
        assert!(engine.companion_terminals.contains_key(&tid));

        // Graceful close: out of the live map, into terminating, SIGTERM sent.
        let label = engine.begin_close_companion_terminal(&tid);
        assert!(label.is_some(), "returns the closed terminal's label");
        assert!(
            !engine.companion_terminals.contains_key(&tid),
            "the terminal leaves the live map immediately (UI updates now)"
        );
        assert_eq!(engine.terminating_ptys.len(), 1);
        assert_eq!(engine.terminating_ptys[0].kind, PrunedPtyKind::Terminal);
        assert!(engine.terminating_ptys[0].worktree_removal.is_none());

        // `cat` exits on SIGTERM, so the reaper drops it well before any deadline.
        let deadline = Instant::now() + Duration::from_secs(3);
        while !engine.terminating_ptys.is_empty() {
            let dispatched = engine.reap_terminating_ptys();
            assert!(dispatched.is_empty(), "a terminal has no deferred worktree");
            assert!(Instant::now() < deadline, "terminal was never reaped");
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn poll_pty_activity_stamps_a_terminals_activity() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        // `cat` echoes stdin as a visible grid change, which sets `received_data`.
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (tid, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");

        // The reader loop suppresses the post-resize redraw burst for 500ms, so
        // let that window lapse and drain any pre-window activity before relying
        // on the write below to be the stamping change.
        sleep(Duration::from_millis(600));
        engine.poll_pty_activity();
        engine.pty_activity.remove(&tid);

        engine
            .companion_terminals
            .get(&tid)
            .expect("terminal is live")
            .client
            .write_bytes(b"hello\n")
            .expect("write to terminal");

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            engine.poll_pty_activity();
            if engine.pty_activity.contains_key(&tid) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "poll_pty_activity never stamped the terminal's activity"
            );
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn closing_a_terminal_clears_its_activity_and_input_entries() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (tid, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("create companion terminal");

        // A recycled `term-N` id must not inherit stale activity/typing/pointer
        // state, so every entry must be gone once the terminal leaves the live
        // map. `pty_pointer` is asserted here and not only documented: its field
        // doc claims it is cleared wherever `pty_activity` is, and a claim
        // nothing pins is a claim that quietly stops being true.
        engine.pty_activity.insert(tid.clone(), Instant::now());
        engine.pty_input.insert(tid.clone(), Instant::now());
        engine.note_pty_pointer(&tid, crate::pty::PointerReport::Wheel);
        assert!(engine.pty_pointer.contains_key(&tid), "armed for the test");

        engine.begin_close_companion_terminal(&tid);

        assert!(
            !engine.pty_activity.contains_key(&tid),
            "teardown must clear the terminal's activity entry"
        );
        assert!(
            !engine.pty_input.contains_key(&tid),
            "teardown must clear the terminal's input entry"
        );
        assert!(
            !engine.pty_pointer.contains_key(&tid),
            "teardown must clear the terminal's pointer entry too"
        );
    }

    #[test]
    fn begin_close_session_terminals_leaves_project_terminals() {
        let (mut engine, _tmp) = test_engine();
        let repo = tempfile::tempdir().expect("project dir");
        engine
            .projects
            .push(sample_project("p1", repo.path().to_string_lossy().as_ref()));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = repo.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (session_tid, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("session terminal");
        let (project_tid, _) = engine
            .create_project_terminal("p1", 24, 80)
            .expect("project terminal");

        engine.begin_close_session_terminals("s1");

        assert!(
            !engine.companion_terminals.contains_key(&session_tid),
            "the session's terminal is closed"
        );
        assert!(
            engine.companion_terminals.contains_key(&project_tid),
            "an over-broad close must not take the project terminal with it"
        );
    }

    #[test]
    fn begin_close_project_terminals_leaves_session_terminals() {
        let (mut engine, _tmp) = test_engine();
        let repo = tempfile::tempdir().expect("project dir");
        engine
            .projects
            .push(sample_project("p1", repo.path().to_string_lossy().as_ref()));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = repo.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (session_tid, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("session terminal");
        let (project_tid, _) = engine
            .create_project_terminal("p1", 24, 80)
            .expect("project terminal");

        engine.begin_close_project_terminals("p1");

        assert!(
            !engine.companion_terminals.contains_key(&project_tid),
            "the project's terminal is closed"
        );
        assert!(
            engine.companion_terminals.contains_key(&session_tid),
            "the session terminal must be untouched"
        );
    }

    #[test]
    fn nothing_closes_a_standalone_terminal_but_the_user() {
        // The journey: a user keeps a standalone terminal open while they delete
        // an agent and remove a project. Both of those close their OWN terminals.
        // The standalone one is still there afterwards, because it belongs to
        // neither and nothing closes it automatically.
        let (mut engine, _tmp) = test_engine();
        let repo = tempfile::tempdir().expect("project dir");
        engine
            .projects
            .push(sample_project("p1", repo.path().to_string_lossy().as_ref()));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = repo.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];

        let (session_tid, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("session terminal");
        let (project_tid, _) = engine
            .create_project_terminal("p1", 24, 80)
            .expect("project terminal");
        let (standalone_tid, _) = engine
            .create_standalone_terminal(24, 80)
            .expect("standalone terminal");

        engine.begin_close_session_terminals("s1");
        assert!(
            !engine.companion_terminals.contains_key(&session_tid),
            "deleting the agent closes the agent's own terminal"
        );
        assert!(
            engine.companion_terminals.contains_key(&standalone_tid),
            "deleting an agent must leave a terminal that belongs to no agent"
        );

        engine.begin_close_project_terminals("p1");
        assert!(
            !engine.companion_terminals.contains_key(&project_tid),
            "removing the project closes the project's own terminal"
        );
        assert!(
            engine.companion_terminals.contains_key(&standalone_tid),
            "removing a project must leave a terminal that belongs to no project"
        );
    }

    #[test]
    fn pruned_project_terminal_carries_project_owner_and_never_detaches_agent() {
        let (mut engine, _tmp) = test_engine();
        let repo = tempfile::tempdir().expect("project dir");
        engine
            .projects
            .push(sample_project("p1", repo.path().to_string_lossy().as_ref()));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = repo.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        engine.mark_session_status("s1", SessionStatus::Active);

        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (terminal_id, _label) = engine
            .create_project_terminal("p1", 24, 80)
            .expect("create project terminal");

        // Ctrl-d (EOF) makes cat exit.
        engine
            .companion_terminals
            .get(&terminal_id)
            .unwrap()
            .client
            .write_bytes(b"\x04")
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        let pruned = loop {
            let pruned = engine.prune_exited_ptys();
            if !pruned.is_empty() {
                break pruned;
            }
            assert!(Instant::now() < deadline, "project terminal never exited");
            sleep(Duration::from_millis(50));
        };

        let p = pruned
            .iter()
            .find(|p| p.id == terminal_id)
            .expect("project terminal pruned");
        assert_eq!(p.kind, PrunedPtyKind::Terminal);
        assert_eq!(
            p.owner,
            Some(crate::model::TerminalOwner::Project("p1".to_string())),
            "the prune must carry the project owner, not an empty-string orphan"
        );
        assert!(
            !p.agent_detached,
            "a project terminal exit never detaches an agent"
        );
        let status = engine
            .sessions
            .iter()
            .find(|s| s.id == "s1")
            .map(|s| s.status);
        assert_eq!(
            status,
            Some(SessionStatus::Active),
            "the agent's session status is untouched by a project terminal exit"
        );
    }

    #[test]
    fn reap_force_kills_a_straggler_past_its_deadline() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        let client = spawn_sigterm_ignorer(worktree.path());
        // Wait until the trap is installed (marker printed) before relying on the
        // force-kill: a SIGTERM during shell startup would kill it for the wrong
        // reason and the test wouldn't exercise force_terminate.
        let ready_by = Instant::now() + Duration::from_secs(3);
        while !client.has_output() {
            assert!(Instant::now() < ready_by, "ignorer never readied");
            sleep(Duration::from_millis(20));
        }
        // Push it as already past its deadline, so one reap must force-kill it.
        engine.terminating_ptys.push(TerminatingPty {
            client,
            deadline: Instant::now() - Duration::from_millis(1),
            kind: PrunedPtyKind::Terminal,
            id: "t1".to_string(),
            label: "scratch".to_string(),
            worktree_removal: None,
        });

        let dispatched = engine.reap_terminating_ptys();
        assert!(dispatched.is_empty());
        assert!(
            engine.terminating_ptys.is_empty(),
            "a past-deadline straggler is force-killed and removed in one reap"
        );
    }

    #[test]
    fn reap_terminating_ptys_is_a_noop_when_empty() {
        let (mut engine, _tmp) = test_engine();
        assert!(engine.reap_terminating_ptys().is_empty());
    }

    #[test]
    fn begin_delete_session_gracefully_closes_agent_and_defers_worktree() {
        use crate::engine::BeginDeleteSessionOutcome;
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        // A live agent PTY that exits on SIGTERM.
        engine
            .providers
            .insert(TabId::new("s1-slot"), spawn_cat(worktree.path()));

        let outcome = engine.begin_delete_session("s1", true, None);
        assert!(
            matches!(outcome, BeginDeleteSessionOutcome::AsyncStarted { .. }),
            "a worktree-removing delete returns the deferred (AsyncStarted) outcome"
        );
        // The agent PTY is gracefully closed: out of `providers`, into the
        // terminating set, with the worktree removal captured for after it exits.
        assert!(!engine.providers.contains_key(TabIdRef::new("s1-slot")));
        assert_eq!(engine.terminating_ptys.len(), 1);
        assert_eq!(engine.terminating_ptys[0].kind, PrunedPtyKind::Agent);
        let req = engine.terminating_ptys[0]
            .worktree_removal
            .as_ref()
            .expect("worktree removal deferred onto the terminating agent");
        assert_eq!(req.session_id, "s1");
        assert_eq!(req.managed.worktree_path, worktree.path().to_string_lossy());

        // Once the agent exits (SIGTERM), the reaper hands the removal back to be
        // dispatched — never before.
        let deadline = Instant::now() + Duration::from_secs(3);
        let removals = loop {
            let r = engine.reap_terminating_ptys();
            if !r.is_empty() {
                break r;
            }
            assert!(Instant::now() < deadline, "agent never reaped");
            sleep(Duration::from_millis(20));
        };
        assert_eq!(removals.len(), 1);
        assert_eq!(removals[0].session_id, "s1");
    }

    #[test]
    fn begin_delete_session_removes_worktree_immediately_when_no_live_agent() {
        use crate::engine::BeginDeleteSessionOutcome;
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        // No provider inserted: the agent already exited.

        let outcome = engine.begin_delete_session("s1", true, None);
        assert!(matches!(
            outcome,
            BeginDeleteSessionOutcome::AsyncStarted { .. }
        ));
        // Nothing to reap (no PTY), and the removal was dispatched right away
        // rather than lost — the in-flight guard proves the worker was spawned.
        assert!(engine.terminating_ptys.is_empty());
        assert!(
            engine.pending_deletions.contains("s1"),
            "the worktree removal is dispatched immediately when there is no PTY"
        );
    }

    #[test]
    fn prune_extra_tab_exit_is_quiet_and_keeps_session_active() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.mark_session_status("s1", crate::model::SessionStatus::Active);
        // The session-slot tab is live (keyed by its own tab id) AND an extra tab
        // of s1 (agent_tabs row + a tab-keyed provider). Only the extra tab exits,
        // so the agent must stay Active — no tab is privileged, but a live sibling
        // keeps the session up.
        engine
            .providers
            .insert(TabId::new("s1-slot"), spawn_cat(worktree.path()));
        engine
            .agent_tabs
            .insert(TabId::new("tab-2"), sample_tab("tab-2", "s1", "codex", 1));
        engine
            .providers
            .insert(TabId::new("tab-2"), spawn_cat(worktree.path()));

        // Make the extra tab's PTY exit (Ctrl-d EOF).
        engine
            .providers
            .get(TabIdRef::new("tab-2"))
            .unwrap()
            .write_bytes(b"\x04")
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        let pruned = loop {
            let pruned = engine.prune_exited_ptys();
            if !pruned.is_empty() || !engine.providers.contains_key(TabIdRef::new("tab-2")) {
                break pruned;
            }
            assert!(Instant::now() < deadline, "extra tab never reported exit");
            sleep(Duration::from_millis(50));
        };

        let p = pruned
            .iter()
            .find(|p| p.id == "tab-2")
            .expect("extra tab pruned");
        assert_eq!(p.kind, PrunedPtyKind::Agent);
        assert_eq!(
            p.owner,
            Some(crate::model::TerminalOwner::Session("s1".to_string())),
            "resolves the owning session"
        );
        // The label names the AGENT, which is its title when it has one, not
        // the branch underneath. A standalone agent has no branch to fall back
        // to, so every label site now goes through the one display rule.
        assert!(
            p.label.contains("s1-title") && p.label.contains("codex"),
            "label names the agent + provider, not a raw UUID: {}",
            p.label
        );
        // The session-slot tab is still live, so the agent stays Active.
        let status = engine
            .sessions
            .iter()
            .find(|s| s.id == "s1")
            .map(|s| s.status);
        assert_eq!(
            status,
            Some(crate::model::SessionStatus::Active),
            "an extra-tab exit must not detach an agent whose session-slot tab is still live"
        );
    }

    #[test]
    fn prune_last_live_tab_exit_detaches_even_when_it_is_an_extra_tab() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine.mark_session_status("s1", crate::model::SessionStatus::Active);
        // Only an extra tab is live (the session-slot tab is dormant). When it
        // exits it is the agent's LAST live tab, so the agent detaches.
        engine
            .agent_tabs
            .insert(TabId::new("tab-2"), sample_tab("tab-2", "s1", "codex", 1));
        engine
            .providers
            .insert(TabId::new("tab-2"), spawn_cat(worktree.path()));
        engine
            .providers
            .get(TabIdRef::new("tab-2"))
            .unwrap()
            .write_bytes(b"\x04")
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let pruned = engine.prune_exited_ptys();
            if !pruned.is_empty() || !engine.providers.contains_key(TabIdRef::new("tab-2")) {
                break;
            }
            assert!(Instant::now() < deadline, "extra tab never reported exit");
            sleep(Duration::from_millis(50));
        }

        let status = engine
            .sessions
            .iter()
            .find(|s| s.id == "s1")
            .map(|s| s.status);
        assert_eq!(
            status,
            Some(crate::model::SessionStatus::Detached),
            "the last live tab exiting detaches the agent, even an extra one"
        );
    }

    #[test]
    fn prune_exited_extra_tab_clears_the_running_provider_pin() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine
            .agent_tabs
            .insert(TabId::new("tab-2"), sample_tab("tab-2", "s1", "codex", 1));
        engine
            .providers
            .insert(TabId::new("tab-2"), spawn_cat(worktree.path()));
        // A retarget-while-live pinned the OLD provider so the UI kept showing it.
        // When the tab exits on its own, that pin must be cleared (else the dormant
        // tab shows the wrong provider forever + the map leaks).
        engine.running_provider_pins.insert(
            TabId::new("tab-2"),
            crate::model::ProviderKind::new("claude"),
        );

        engine
            .providers
            .get(TabIdRef::new("tab-2"))
            .unwrap()
            .write_bytes(b"\x04")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            engine.prune_exited_ptys();
            if !engine.providers.contains_key(TabIdRef::new("tab-2")) {
                break;
            }
            assert!(Instant::now() < deadline, "extra tab never reported exit");
            sleep(Duration::from_millis(50));
        }
        assert!(
            !engine
                .running_provider_pins
                .contains_key(TabIdRef::new("tab-2")),
            "the exited tab's stale provider pin must be cleared"
        );
    }

    #[test]
    fn begin_delete_session_parks_a_group_barrier_over_every_live_tab() {
        use crate::engine::BeginDeleteSessionOutcome;
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine
            .agent_tabs
            .insert(TabId::new("tab-2"), sample_tab("tab-2", "s1", "codex", 1));
        // Both the session-slot tab and the extra tab have a live PTY.
        engine
            .providers
            .insert(TabId::new("s1-slot"), spawn_cat(worktree.path()));
        engine
            .providers
            .insert(TabId::new("tab-2"), spawn_cat(worktree.path()));

        let outcome = engine.begin_delete_session("s1", true, None);
        assert!(matches!(
            outcome,
            BeginDeleteSessionOutcome::AsyncStarted { .. }
        ));
        // Every live tab PTY (Main + Support) is now terminating, and the worktree
        // removal is parked on ONE group barrier over both — not on any single
        // entry (which would fire when the first, not the last, tab reaps).
        assert_eq!(engine.terminating_ptys.len(), 2);
        assert!(
            engine
                .terminating_ptys
                .iter()
                .all(|e| e.worktree_removal.is_none()),
            "a multi-tab delete carries no per-entry removal"
        );
        assert_eq!(engine.pending_group_removals.len(), 1);
        let group = &engine.pending_group_removals[0];
        assert!(group.pending_ids.contains("s1-slot") && group.pending_ids.contains("tab-2"));
        assert_eq!(group.removal.session_id, "s1");
    }

    #[test]
    fn begin_delete_session_waits_for_a_straggler_already_in_terminating_ptys() {
        // `begin_delete_session`'s `live_tabs` barrier must fold in a tab that
        // is already out of `providers` but still parked in `terminating_ptys`
        // under its own SIGTERM grace period: that process is still alive and
        // still using the worktree as its cwd, so the barrier must not clear
        // until it does too.
        use crate::engine::BeginDeleteSessionOutcome;
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        engine
            .agent_tabs
            .insert(TabId::new("tab-2"), sample_tab("tab-2", "s1", "codex", 1));

        // The session-slot tab is still live; "tab-2" was closed moments earlier and is
        // already a straggler in `terminating_ptys` (not in `providers` at all).
        engine
            .providers
            .insert(TabId::new("s1-slot"), spawn_cat(worktree.path()));
        let far = Instant::now() + Duration::from_secs(60);
        engine.terminating_ptys.push(TerminatingPty {
            client: spawn_cat(worktree.path()),
            deadline: far,
            kind: PrunedPtyKind::Agent,
            id: "tab-2".to_string(),
            label: "feat".to_string(),
            worktree_removal: None,
        });

        let outcome = engine.begin_delete_session("s1", true, None);
        assert!(matches!(
            outcome,
            BeginDeleteSessionOutcome::AsyncStarted { .. }
        ));
        // The slot tab gets its own new terminating entry; "tab-2" keeps its existing one
        // (begin_delete_session must NOT re-issue a close for an already-closing
        // straggler). Both must be listed on ONE group barrier.
        assert_eq!(engine.terminating_ptys.len(), 2);
        assert_eq!(engine.pending_group_removals.len(), 1);
        let group = &engine.pending_group_removals[0];
        assert!(
            group.pending_ids.contains("s1-slot") && group.pending_ids.contains("tab-2"),
            "the group barrier must wait for the already-terminating straggler too: {:?}",
            group.pending_ids
        );
    }

    #[test]
    fn group_barrier_dispatches_worktree_removal_only_after_the_last_tab_reaps() {
        use super::{DeferredWorktreeRemoval, GroupWorktreeRemoval};
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");

        // Two terminating tab PTYs under one group barrier. The "main" cat is
        // SIGTERMed (it will reap); the "support" cat is left running with a far
        // deadline so it stays until we expire it by hand — a deterministic stand-in
        // for a sibling tab that outlives the first.
        let main = spawn_cat(worktree.path());
        main.terminate();
        let support = spawn_cat(worktree.path());
        let far = Instant::now() + Duration::from_secs(60);
        for (id, client) in [("s1", main), ("tab-2", support)] {
            engine.terminating_ptys.push(TerminatingPty {
                client,
                deadline: far,
                kind: PrunedPtyKind::Agent,
                id: id.to_string(),
                label: "feat".to_string(),
                worktree_removal: None,
            });
        }
        engine.pending_group_removals.push(GroupWorktreeRemoval {
            pending_ids: ["s1".to_string(), "tab-2".to_string()]
                .into_iter()
                .collect(),
            removal: DeferredWorktreeRemoval {
                delete_branch: None,
                session_id: "s1".to_string(),
                project_path: "/tmp/p".to_string(),
                managed: crate::model::ManagedWorkspace {
                    project_id: "p1".to_string(),
                    project_path: None,
                    source_branch: "main".to_string(),
                    branch_name: "feat".to_string(),
                    initial_branch: "feat".to_string(),
                    branch_provenance: crate::model::BranchProvenance::CreatedByDux,
                    worktree_path: worktree.path().to_string_lossy().to_string(),
                },
                busy_message: "removing".to_string(),
            },
        });

        // Reap until the SIGTERMed "main" cat is gone. The "support" cat is still
        // alive (blocked on stdin, far deadline), so the group must NOT dispatch.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let dispatched = engine.reap_terminating_ptys();
            assert!(
                dispatched.is_empty(),
                "removal must not fire while a sibling tab is still terminating"
            );
            if engine.terminating_ptys.len() == 1 {
                break;
            }
            assert!(Instant::now() < deadline, "session-slot tab never reaped");
            sleep(Duration::from_millis(20));
        }
        assert_eq!(engine.pending_group_removals.len(), 1);
        assert!(
            engine.pending_group_removals[0]
                .pending_ids
                .contains("tab-2")
        );

        // Expire the survivor's deadline: the next reap force-kills it, empties the
        // group, and dispatches the worktree removal EXACTLY ONCE.
        engine.terminating_ptys[0].deadline = Instant::now() - Duration::from_millis(1);
        let dispatched = engine.reap_terminating_ptys();
        assert_eq!(
            dispatched.len(),
            1,
            "removal dispatched once, only after the last tab reaped"
        );
        assert_eq!(dispatched[0].session_id, "s1");
        assert!(engine.pending_group_removals.is_empty());
    }

    #[test]
    fn format_shutdown_start_pluralizes() {
        assert_eq!(
            format_shutdown_start(1, 1, Duration::from_secs(30)),
            "Requesting 1 agent and 1 terminal to gracefully shut down, timeout 30s."
        );
        assert_eq!(
            format_shutdown_start(2, 0, Duration::from_secs(5)),
            "Requesting 2 agents and 0 terminals to gracefully shut down, timeout 5s."
        );
    }

    #[test]
    fn format_shutdown_result_clean_and_forced() {
        let clean = super::ShutdownReport {
            agents_total: 2,
            terminals_total: 1,
            agents_exited: 2,
            terminals_exited: 1,
            elapsed: Duration::from_millis(340),
            timed_out: false,
        };
        assert_eq!(
            format_shutdown_result(&clean),
            "All 2 agents and 1 terminal exited gracefully in 0.3s."
        );

        let forced = super::ShutdownReport {
            agents_total: 3,
            terminals_total: 2,
            agents_exited: 1,
            terminals_exited: 2,
            elapsed: Duration::from_secs(30),
            timed_out: true,
        };
        assert_eq!(
            format_shutdown_result(&forced),
            "1 agent and 2 terminals exited successfully. \
             Force-closing 2 agents and 0 terminals, then exiting..."
        );
    }
}
