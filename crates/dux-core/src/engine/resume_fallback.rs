//! `Engine::retry_resume_fallback`, the engine-owned resume-fallback retry.
//! One method both retry paths (exit-driven and timeout-driven) call, so the
//! provider, candidate and pin removal and the re-dispatch happen atomically
//! inside a single `&mut self` call, closing the window where a session has
//! neither its old nor its new provider.

use std::time::Duration;

use crate::engine::events::EventReaction;
use crate::engine::{Command, Engine, InFlightKey};
use crate::ids::{TabId, TabIdRef};
use crate::model::{AgentSession, ProviderKind, SessionStatus};
use crate::worker::{AgentLaunchKind, AgentLaunchRequest};

/// Visible-line threshold below which a resumed provider's output counts as
/// minimal, meaning no real conversation: a `--continue` that found nothing
/// prints a short error and exits. Shared by both detection windows.
pub const RESUME_MINIMAL_OUTPUT_LINES: usize = 5;

/// What the resume-fallback sweep should do with one resume candidate, decided
/// purely from its observable state, so both detection windows (`--continue`
/// exits empty, and a resume hangs past its timeout) live in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeFallbackDecision {
    /// The resumed provider EXITED with only minimal output: the resume found no
    /// prior conversation, so relaunch fresh (window a).
    RetryExitedMinimal,
    /// The resumed provider exited with real output, so it ran a conversation
    /// that ended normally: drop the candidate and let the exit-prune path
    /// detach it, never relaunch.
    DropNonMinimalExit,
    /// The resumed provider is still running but produced no visible output
    /// past its `resume_wait_timeout_ms` window: hung, so relaunch fresh.
    RetryHungTimeout,
    /// Healthy (or not yet decidable): leave the candidate alone.
    Wait,
}

/// The pure resume-fallback decision. `exited`, `minimal` and `has_output` are
/// the provider's observable flags, `timeout_ms` is its configured
/// `resume_wait_timeout_ms` (`None` or `0` disables the hung window) and
/// `elapsed` is how long the resume has been running.
pub(crate) fn resume_fallback_decision(
    exited: bool,
    minimal: bool,
    has_output: bool,
    timeout_ms: Option<u64>,
    elapsed: Duration,
) -> ResumeFallbackDecision {
    if exited {
        return if minimal {
            ResumeFallbackDecision::RetryExitedMinimal
        } else {
            ResumeFallbackDecision::DropNonMinimalExit
        };
    }
    match timeout_ms {
        Some(ms) if ms > 0 && elapsed >= Duration::from_millis(ms) && !has_output => {
            ResumeFallbackDecision::RetryHungTimeout
        }
        _ => ResumeFallbackDecision::Wait,
    }
}

/// Outcome of an attempted resume-fallback retry. Three states because the
/// caller must react differently to each: collapsing any two corrupts state.
pub enum ResumeFallbackOutcome {
    /// Engine removed the candidate, provider and pin and dispatched a fresh
    /// `resume:false` launch. `reaction` is the `DispatchAgentLaunchView`
    /// follow-up the caller must apply. Treat the session as handled: skip the
    /// normal exit and Detached cleanup and the post-exit UI and PR follow-ups.
    ///
    /// An OS thread-spawn failure is the only `launched:false` cause reachable
    /// here, since the in-flight pre-check already passed; the engine has then
    /// already marked the session `Detached` and `reaction` carries the spawn
    /// error. It is still `Retried`.
    ///
    /// `reaction` is boxed because `EventReaction` is large next to the unit
    /// variants beside it, and clippy's `large_enum_variant` is a CI gate.
    Retried { reaction: Box<EventReaction> },
    /// A launch is already in flight for this session, so the engine left the
    /// candidate, provider and pin untouched. Treat the session as protected:
    /// skip the destructive exit cleanup and the post-exit UI and PR follow-ups
    /// exactly as if it had been retried; the in-flight launch resolves it.
    InFlight,
    /// The session is no longer an eligible resume candidate and the engine has
    /// removed any stale entry. The caller falls through to normal exit
    /// handling and the Detached path.
    NotCandidate,
}

impl Engine {
    /// Record that a tab's run just started as a RESUME, in the two places that
    /// care, so they cannot disagree about one launch.
    ///
    /// The candidacy is what the fallback sweep watches and retires; the run
    /// fact outlives that decision and is what the exit path reads to tell a
    /// refused resume from an ordinary exit. Both are cleared together by
    /// `Engine::clear_tab_runtime`.
    pub(crate) fn note_resume_launch(&mut self, tab_id: &TabId) {
        self.resume_fallback_candidates
            .insert(tab_id.clone(), std::time::Instant::now());
        self.resumed_tab_runs.insert(tab_id.clone());
    }

    /// Build an `AgentLaunchRequest` from engine state. `pty_size` is the only
    /// front-end-sourced input; provider config, resolved env and scrollback all
    /// come from engine state. Every surface delegates here, so request
    /// construction has one source of truth.
    pub fn build_agent_launch_request(
        &self,
        session: AgentSession,
        resume: bool,
        pty_size: (u16, u16),
        kind: AgentLaunchKind,
    ) -> AgentLaunchRequest {
        // Session-slot launch: the slot tab id, provider == session.provider.
        let tab_id = session.slot_tab_id().to_owned();
        self.build_tab_launch_request(tab_id, None, session, resume, pty_size, kind)
    }

    /// Tab-aware launch-request builder. `tab_provider = None` uses the
    /// session's own provider, the session-slot tab; `Some(provider)` launches
    /// that provider for an extra tab.
    ///
    /// Resume eligibility is dynamic, not positional: a tab launches with the
    /// provider's `--continue` flag only when no other tab of this session has a
    /// live provider or an in-flight launch. `--continue` is directory-scoped
    /// and always grabs the most recent conversation, so at most one tab may
    /// resume and every tab launched beside a live one starts fresh.
    pub fn build_tab_launch_request(
        &self,
        tab_id: TabId,
        tab_provider: Option<ProviderKind>,
        session: AgentSession,
        resume: bool,
        pty_size: (u16, u16),
        kind: AgentLaunchKind,
    ) -> AgentLaunchRequest {
        let provider = tab_provider.unwrap_or_else(|| session.provider.clone());
        // Resume is decided per-provider in one place; see `tab_resume_decision`.
        let resume = self.tab_resume_decision(&session, &tab_id, &provider, resume);
        let provider_config = crate::config::provider_config(&self.config, &provider);
        // A standalone agent has no project to overlay, so it gets the global
        // environment, not the empty one a missed project lookup falls to.
        let env = self.session_env(&session);
        AgentLaunchRequest {
            session,
            tab_id,
            provider,
            provider_config,
            env,
            identity: self.resolved_identity(),
            resume,
            pty_size,
            scrollback_lines: self.config.ui.agent_scrollback_lines,
            kind,
            // Landing is minimized by default; a fullscreen-seeking gesture
            // flips this on the returned request.
            wants_fullscreen: false,
            // Loud by default: a caller that quiets its completion says so on
            // the returned request.
            status_quiet: crate::statusline::QuietSurfaces::LOUD,
        }
    }

    /// Build the launch request for reopening a dormant extra tab, a tab with a
    /// row but no live process. The single source both surfaces call, so the
    /// resolution, the resume decision and the fresh-versus-resumed wording
    /// cannot drift. Returns `None` for an unknown tab or one whose owning
    /// session is gone; the caller dispatches through its own launch path, and
    /// the core dispatch chokepoint re-gates resume and refuses a closing
    /// session, so no surface needs a guard of its own.
    ///
    /// Resume is decided per provider by `tab_resume_decision`: reopening
    /// resumes that provider's conversation only when this is the sole live or
    /// launching tab of its provider.
    pub fn dormant_tab_launch_request(
        &self,
        tab_id: &str,
        pty_size: (u16, u16),
    ) -> Option<AgentLaunchRequest> {
        // Transport-facing (the web relaunches a dormant tab by id): named here.
        let tab_id = TabIdRef::new(tab_id);
        let tab = self.agent_tabs.get(tab_id)?;
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == tab.session_id)?
            .clone();
        let provider = tab.provider.clone();
        let resume = self.tab_resume_decision(&session, tab_id, &provider, true);
        let status_message = if resume {
            format!(
                "Resumed the {} conversation in this tab.",
                provider.as_str()
            )
        } else {
            format!(
                "Started a fresh {} conversation in this tab.",
                provider.as_str()
            )
        };
        // A resumed tab relaunches and paints the prior conversation, so nothing
        // is owed. A fresh one comes up empty and the sentence says why.
        Some(
            self.build_tab_launch_request(
                tab_id.to_owned(),
                Some(provider),
                session,
                resume,
                pty_size,
                AgentLaunchKind::Tab {
                    is_fresh: false,
                    status_message,
                },
            )
            .quiet_status_on(if resume {
                crate::statusline::QuietSurfaces::BOTH
            } else {
                crate::statusline::QuietSurfaces::LOUD
            }),
        )
    }

    /// Attempt a resume-fallback retry for `session_id`. Synchronous: every
    /// state transition happens inside this one `&mut self` call, so no other
    /// `drain_events` tick observes a half-applied state. See
    /// `ResumeFallbackOutcome` for how the caller must treat each result.
    pub fn retry_resume_fallback(
        &mut self,
        tab_id: &str,
        pty_size: (u16, u16),
        status_message: String,
    ) -> ResumeFallbackOutcome {
        // Transport-facing entry point: named here, at the door.
        let tab_id = TabIdRef::new(tab_id);
        // 1. A launch already in flight for THIS tab: protect it, touch nothing.
        if self.is_in_flight(&InFlightKey::AgentLaunch(tab_id.to_owned())) {
            return ResumeFallbackOutcome::InFlight;
        }
        // 2. Not (any longer) a candidate: nothing to retry.
        if !self.resume_fallback_candidates.contains_key(tab_id) {
            return ResumeFallbackOutcome::NotCandidate;
        }
        // 3. Owning session gone: drop the stale candidate and fall through.
        //    Candidates are keyed by tab id, so the owning session is resolved.
        let Some(session_id) = self.owning_session_for_tab(tab_id.as_str()) else {
            self.resume_fallback_candidates.remove(tab_id);
            return ResumeFallbackOutcome::NotCandidate;
        };
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
            self.resume_fallback_candidates.remove(tab_id);
            return ResumeFallbackOutcome::NotCandidate;
        };
        // Capture the provider that was resuming (the exited tab's own provider)
        // BEFORE tearing down the pin, so the fresh relaunch reuses it.
        let is_session_slot = self.is_slot_tab(&session, tab_id);
        let provider = self.tab_running_provider(&session, tab_id);
        // 4. Tear down the stale resume attempt through the one function that
        //    knows every map keyed by a tab id, so a map added there later does
        //    not have to remember this site too. Safe to call whole: the
        //    in-flight pre-check at step 1 already returned, so clearing the
        //    `AgentLaunch` key is a no-op, and the running provider was
        //    captured above.
        self.clear_tab_runtime(tab_id);
        // 5. Build a fresh, non-resume launch request. An extra tab rebuilds its
        //    own provider as a Tab launch so the ready and failed handlers stay
        //    tab-scoped and never flip session state.
        let request = if is_session_slot {
            self.build_agent_launch_request(
                session,
                false,
                pty_size,
                AgentLaunchKind::ResumeFallback { status_message },
            )
        } else {
            self.build_tab_launch_request(
                tab_id.to_owned(),
                Some(provider),
                session,
                false,
                pty_size,
                AgentLaunchKind::Tab {
                    is_fresh: false,
                    status_message,
                },
            )
        };
        // 6. Dispatch. The in-flight pre-check above already passed, so an OS
        //    thread-spawn failure is the only `launched:false` left, and it
        //    marks the session Detached for the session-slot tab only: an extra
        //    tab's failure must not tear down live siblings.
        let reaction = match self.apply(Command::DispatchAgentLaunch {
            request: Box::new(request),
        }) {
            Ok(r) => r,
            Err(e) => EventReaction::Status(crate::engine::StatusUpdate::error(format!("{e:#}"))),
        };
        let launched = matches!(
            &reaction,
            EventReaction::DispatchAgentLaunchView(view) if view.launched
        );
        if !launched
            && is_session_slot
            && self.mark_session_status(&session_id, SessionStatus::Detached)
        {
            self.update_pr_sync_sessions();
        }
        ResumeFallbackOutcome::Retried {
            reaction: Box::new(reaction),
        }
    }

    /// Sweep every seeded resume-fallback candidate through both detection
    /// windows and act on each, returning the launch reactions the caller must
    /// apply through its own reaction pipeline. Every surface routes through
    /// here, so continue-then-fresh behavior cannot drift between them.
    ///
    /// Each candidate is decided by the pure `resume_fallback_decision`:
    /// - `RetryExitedMinimal` and `RetryHungTimeout` go to
    ///   `retry_resume_fallback` with the window-appropriate status message,
    ///   collecting a `Retried` reaction. The retry tears down every tab-keyed
    ///   map itself, through `clear_tab_runtime`, so this loop names none.
    /// - `DropNonMinimalExit` drops the candidate so the normal exit-prune path
    ///   detaches it.
    /// - `Wait` leaves it alone.
    ///
    /// Must run before `prune_exited_ptys`: the retry has to pull a
    /// `RetryExited*` candidate's provider out of `providers` before the prune
    /// reaps it and marks the agent Detached.
    pub fn sweep_resume_fallbacks(&mut self, pty_size: (u16, u16)) -> Vec<EventReaction> {
        let mut reactions = Vec::new();
        // Snapshot the candidate ids: `retry_resume_fallback` mutates the map.
        let candidates: Vec<TabId> = self.resume_fallback_candidates.keys().cloned().collect();
        for tab_id in candidates {
            let Some(&started_at) = self.resume_fallback_candidates.get(&tab_id) else {
                continue;
            };
            let Some(session_id) = self.owning_session_for_tab(tab_id.as_str()) else {
                // Stale candidate with no owning session: drop it.
                self.resume_fallback_candidates.remove(&tab_id);
                continue;
            };
            let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
                self.resume_fallback_candidates.remove(&tab_id);
                continue;
            };
            let provider = self.tab_running_provider(&session, &tab_id);
            let Some(client) = self.providers.get(&tab_id) else {
                // A candidate with no live provider is stale (already torn down).
                self.resume_fallback_candidates.remove(&tab_id);
                continue;
            };
            let timeout_ms =
                crate::config::provider_config(&self.config, &provider).resume_wait_timeout_ms;
            let decision = resume_fallback_decision(
                client.is_exited(),
                client.has_minimal_output(RESUME_MINIMAL_OUTPUT_LINES),
                client.has_output(),
                timeout_ms,
                started_at.elapsed(),
            );
            let location = self.session_location_phrase(&session);
            let status_message = match decision {
                ResumeFallbackDecision::RetryExitedMinimal => format!(
                    "No prior session to resume for agent \"{}\". Started a fresh {} session in {}.",
                    session.display_label(),
                    provider.as_str(),
                    location,
                ),
                ResumeFallbackDecision::RetryHungTimeout => format!(
                    "Resume timed out for agent \"{}\" with no visible output. Started a fresh {} session in {}.",
                    session.display_label(),
                    provider.as_str(),
                    location,
                ),
                ResumeFallbackDecision::DropNonMinimalExit => {
                    // A real conversation ended: drop the candidate, let the
                    // exit-prune path detach the agent normally.
                    self.resume_fallback_candidates.remove(&tab_id);
                    continue;
                }
                ResumeFallbackDecision::Wait => continue,
            };
            crate::logger::info(&format!(
                "resume fallback for agent \"{}\": {}",
                session.display_label(),
                match decision {
                    ResumeFallbackDecision::RetryExitedMinimal =>
                        "resume exited without output, retrying fresh",
                    _ => "resume produced no visible output within timeout, retrying fresh",
                }
            ));
            if let ResumeFallbackOutcome::Retried { reaction } =
                self.retry_resume_fallback(tab_id.as_str(), pty_size, status_message)
            {
                // Nothing is cleared by hand here: the retry's teardown goes
                // through `clear_tab_runtime`, and a second list of tab-keyed
                // maps at this site is exactly the drift it exists to prevent.
                reactions.push(*reaction);
            }
        }
        reactions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_session, test_engine};
    use std::time::Instant;

    /// The dormant-tab relaunch request is built in ONE core place
    /// (`dormant_tab_launch_request`) so the fresh-launch wording cannot drift
    /// between the TUI and the web (it had: "Starting a fresh {} session in this
    /// tab." vs "Started a fresh {} tab."). The request carries the message
    /// inside its `AgentLaunchKind::Tab`.
    #[test]
    fn dormant_tab_launch_request_builds_a_fresh_tab_launch_with_one_message() {
        use crate::engine::test_support::sample_tab;
        use crate::worker::AgentLaunchKind;

        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat/x"));
        // An extra, dormant tab (a row but no live provider).
        engine
            .agent_tabs
            .insert(TabId::new("tab-2"), sample_tab("tab-2", "s1", "codex", 1));

        let request = engine
            .dormant_tab_launch_request("tab-2", (24, 80))
            .expect("a dormant extra tab yields a launch request");
        assert_eq!(request.tab_id.as_str(), "tab-2");
        assert_eq!(request.provider.as_str(), "codex");
        // No live sibling of this provider, so it resumes; the sole live/
        // launching tab of its provider is eligible per `tab_resume_decision`.
        match request.kind {
            AgentLaunchKind::Tab {
                is_fresh,
                status_message,
            } => {
                assert!(!is_fresh, "dormant relaunch is never the create-fresh kind");
                // The default test config's codex provider has no resume flag,
                // so it starts fresh: the single-source fresh wording (which had
                // drifted "Starting a fresh {} session in this tab." vs "Started
                // a fresh {} tab." across surfaces).
                assert_eq!(
                    status_message,
                    "Started a fresh codex conversation in this tab."
                );
            }
            other => panic!("expected a Tab launch, got {other:?}"),
        }
        // An empty transcript looks like a failure, so the fresh line is owed.
        assert_eq!(request.status_quiet, crate::statusline::QuietSurfaces::LOUD);
    }

    /// A tab that resumes paints its prior conversation, which is the big and
    /// unmistakable change, so "Resumed the ... conversation" goes unsaid.
    #[test]
    fn a_resuming_dormant_tab_relaunch_is_quiet_on_both_surfaces() {
        use crate::engine::test_support::sample_tab;
        use crate::worker::AgentLaunchKind;

        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat/x");
        session.started_providers = vec!["codex".into()];
        engine.sessions.push(session);
        engine
            .agent_tabs
            .insert(TabId::new("tab-2"), sample_tab("tab-2", "s1", "codex", 1));
        // Make codex resume-capable in this config; the default test provider
        // has no resume flag, so the fresh half of the rule would answer instead.
        engine
            .config
            .providers
            .commands
            .get_mut("codex")
            .expect("a codex provider block")
            .resume_args = Some(vec!["--continue".to_string()]);

        let request = engine
            .dormant_tab_launch_request("tab-2", (24, 80))
            .expect("a dormant extra tab yields a launch request");
        assert!(
            request.resume,
            "no live sibling owns the codex conversation"
        );
        match &request.kind {
            AgentLaunchKind::Tab { status_message, .. } => {
                assert_eq!(
                    status_message,
                    "Resumed the codex conversation in this tab."
                );
            }
            other => panic!("expected a Tab launch, got {other:?}"),
        }
        assert_eq!(request.status_quiet, crate::statusline::QuietSurfaces::BOTH);
    }

    #[test]
    fn dormant_tab_launch_request_is_none_for_an_unknown_tab() {
        let (engine, _tmp) = test_engine();
        assert!(
            engine
                .dormant_tab_launch_request("nope", (24, 80))
                .is_none()
        );
    }

    // The pure resume-fallback DECISION (both detection windows), owned by core
    // so `dux serve` gets the same continue-then-fresh behavior the TUI has.
    use super::{ResumeFallbackDecision as D, resume_fallback_decision};
    use std::time::Duration;

    #[test]
    fn resume_that_exited_with_minimal_output_relaunches_fresh() {
        // Window (a): `--continue` found no prior conversation, printed a short
        // error, and exited. Minimal output after a resume launch => retry fresh.
        assert_eq!(
            resume_fallback_decision(true, true, false, Some(3000), Duration::from_secs(1)),
            D::RetryExitedMinimal,
        );
    }

    #[test]
    fn resume_that_exited_with_real_output_is_dropped_not_retried() {
        // A non-minimal exit means the resume actually ran a conversation that
        // then ended; that is a normal exit, so drop the candidate and let the
        // exit-prune path detach it (never a fresh relaunch).
        assert_eq!(
            resume_fallback_decision(true, false, true, Some(3000), Duration::from_secs(1)),
            D::DropNonMinimalExit,
        );
    }

    #[test]
    fn resume_that_hangs_past_the_timeout_with_no_output_relaunches_fresh() {
        // Window (b): still running, past `resume_wait_timeout_ms`, produced no
        // visible output => treat the resume as hung and retry fresh.
        assert_eq!(
            resume_fallback_decision(false, true, false, Some(2000), Duration::from_millis(2500)),
            D::RetryHungTimeout,
        );
    }

    /// End to end: a resume candidate whose provider exited with minimal output
    /// is retried fresh by the sweep (candidate + provider gone, a launch now in
    /// flight), so `dux serve` (which calls the same sweep) recovers instead of
    /// showing "Agent exited".
    #[test]
    fn sweep_retries_a_minimal_exited_resume_candidate() {
        use crate::pty::PtyClient;
        use std::path::Path;
        use std::time::Instant;

        let (mut engine, tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = tmp.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        // A clean-exiting provider with minimal output (prints one short line).
        let client = PtyClient::spawn(
            "sh",
            &["-c".to_string(), "echo x".to_string()],
            Path::new("."),
            10,
            40,
            100,
        )
        .expect("spawn");
        engine.providers.insert(TabId::new("s1-slot"), client);
        engine
            .resume_fallback_candidates
            .insert(TabId::new("s1-slot"), Instant::now());

        // Wait for the child to exit so the sweep sees `is_exited`.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !engine
            .providers
            .get(TabIdRef::new("s1-slot"))
            .is_some_and(|c| c.is_exited())
        {
            assert!(Instant::now() < deadline, "child never exited");
            std::thread::sleep(Duration::from_millis(20));
        }

        let reactions = engine.sweep_resume_fallbacks((24, 80));
        assert_eq!(reactions.len(), 1, "one retry reaction");
        assert!(
            !engine
                .resume_fallback_candidates
                .contains_key(TabIdRef::new("s1-slot")),
            "the retried candidate is cleared"
        );
        assert!(
            !engine.providers.contains_key(TabIdRef::new("s1-slot")),
            "the retry removed the exited provider"
        );
        assert!(
            engine.is_in_flight(&InFlightKey::AgentLaunch(TabId::new("s1-slot"))),
            "a fresh launch is now in flight"
        );
    }

    /// A resumed provider that exited after printing `rows` lines, in an engine
    /// whose provider command cannot be spawned, so a fresh relaunch fails at
    /// once instead of starting a real CLI on this machine. Returns once the
    /// child has exited, which is the state the maintenance order below expects.
    fn engine_with_an_exited_resume(rows: &[&str]) -> (Engine, tempfile::TempDir) {
        use crate::pty::PtyClient;
        use std::time::Instant;

        let (mut engine, tmp) = test_engine();
        engine.config.providers.commands.insert(
            "claude".to_string(),
            crate::config::ProviderCommandConfig {
                command: "dux-test-no-such-provider-binary".to_string(),
                ..Default::default()
            },
        );
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = tmp.path().to_string_lossy().to_string();
        engine.sessions.push(session);

        let script = format!("printf '{}'; exit 1", rows.join("\\n") + "\\n");
        let client = PtyClient::spawn_with_env(
            "sh",
            &["-c".to_string(), script],
            tmp.path(),
            24,
            80,
            1000,
            &[],
        )
        .expect("spawn sh");
        engine.providers.insert(TabId::new("s1-slot"), client);
        engine.note_resume_launch(&TabId::new("s1-slot"));

        let deadline = Instant::now() + Duration::from_secs(5);
        while !engine
            .providers
            .get(TabIdRef::new("s1-slot"))
            .is_some_and(crate::pty::PtyClient::is_exited)
        {
            assert!(Instant::now() < deadline, "child never exited");
            std::thread::sleep(Duration::from_millis(20));
        }
        (engine, tmp)
    }

    /// The real maintenance order, sweep then prune, for a refusal WORDIER than
    /// the sweep's minimal-output threshold: the sweep leaves it alone and the
    /// prune carries the provider's words out for the warning to quote.
    #[test]
    fn a_wordy_refusal_survives_the_sweep_and_reaches_the_warning() {
        use std::time::Instant;

        let (mut engine, _tmp) = engine_with_an_exited_resume(&[
            "Resuming your conversation.",
            "Looking for a session to continue.",
            "Found session 9f2 for this directory.",
            "That session cannot be continued here.",
            "Your most recent conversation is running in the background.",
            "Use `agents` to attach to it.",
        ]);

        let reactions = engine.sweep_resume_fallbacks((24, 80));
        assert!(
            reactions.is_empty(),
            "a resume that printed real output is dropped by the sweep, never retried"
        );

        let deadline = Instant::now() + Duration::from_secs(3);
        let pruned = loop {
            let pruned = engine.prune_exited_ptys();
            if pruned.iter().any(|p| p.id == "s1-slot") {
                break pruned;
            }
            assert!(Instant::now() < deadline, "the provider was never pruned");
            std::thread::sleep(Duration::from_millis(20));
        };
        let agent = pruned
            .iter()
            .find(|p| p.id == "s1-slot")
            .expect("the slot tab pruned");
        let excerpt = agent
            .refused_resume_excerpt
            .as_ref()
            .expect("a wordy refusal reaches the exit path");
        let warning = crate::tab_verdict::refused_resume_warning(
            &agent.label,
            excerpt,
            "Open the agent to see the full output, or start a fresh session.",
        )
        .expect("there are words to quote");
        assert!(
            warning.contains("could not resume its previous session"),
            "got {warning:?}"
        );
        assert!(
            warning.contains("Use `agents` to attach to it."),
            "the provider's own remedy must survive the whole path, got {warning:?}"
        );
    }

    /// The same order for a SHORT refusal, which is the threshold this pair
    /// exists to document: the sweep reads it as nothing to resume, starts a
    /// fresh session with its own message, and the exit never reaches the prune,
    /// so no refused-resume warning is raised at all.
    #[test]
    fn a_short_refusal_is_taken_as_nothing_to_resume_and_starts_fresh() {
        let (mut engine, _tmp) = engine_with_an_exited_resume(&[
            "Your most recent conversation is running in the background.",
            "Use `agents` to attach to it.",
        ]);

        let reactions = engine.sweep_resume_fallbacks((24, 80));
        assert_eq!(reactions.len(), 1, "the sweep retried the resume fresh");
        assert!(
            !engine.providers.contains_key(TabIdRef::new("s1-slot")),
            "the retry pulled the exited provider before the prune could see it"
        );
        assert!(
            engine
                .prune_exited_ptys()
                .iter()
                .all(|pruned| pruned.id != "s1-slot"),
            "there is nothing left for the prune to report, warning included"
        );

        // And the user is told what happened, in the fallback's own words. The
        // relaunch cannot start (the command does not exist), so the request
        // comes back on the worker channel carrying the message it would have
        // shown.
        let event = engine
            .worker_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the fresh relaunch reports back");
        let kind = match event {
            crate::worker::WorkerEvent::AgentLaunchFailed(data) => data.request.kind,
            crate::worker::WorkerEvent::AgentLaunchReady(data) => data.request.kind,
            _ => panic!("expected a launch outcome from the fresh relaunch"),
        };
        let AgentLaunchKind::ResumeFallback { status_message } = kind else {
            panic!("the relaunch is a resume fallback");
        };
        assert!(
            status_message.starts_with("No prior session to resume for agent"),
            "the fresh-start message is unchanged, got {status_message:?}"
        );
        assert!(
            status_message.contains("Started a fresh claude session in"),
            "and it says what it started and where, got {status_message:?}"
        );
    }

    /// A healthy resume candidate (still running, within its timeout window) is
    /// left alone by the sweep.
    #[test]
    fn sweep_leaves_a_healthy_resume_candidate_alone() {
        use crate::pty::PtyClient;
        use std::path::Path;
        use std::time::Instant;

        let (mut engine, tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = tmp.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        // A long-lived provider (cat blocks on stdin, stays running, no output).
        let client = PtyClient::spawn("cat", &[], Path::new("."), 10, 40, 100).expect("spawn");
        engine.providers.insert(TabId::new("s1-slot"), client);
        engine
            .resume_fallback_candidates
            .insert(TabId::new("s1-slot"), Instant::now());

        let reactions = engine.sweep_resume_fallbacks((24, 80));
        assert!(reactions.is_empty(), "no retry for a healthy resume");
        assert!(
            engine
                .resume_fallback_candidates
                .contains_key(TabIdRef::new("s1-slot")),
            "the candidate is kept for a later tick"
        );
        assert!(
            engine.providers.contains_key(TabIdRef::new("s1-slot")),
            "the provider stays live"
        );
    }

    #[test]
    fn a_healthy_resume_waits() {
        // Still running, within the timeout: leave it alone.
        assert_eq!(
            resume_fallback_decision(false, true, false, Some(3000), Duration::from_millis(500)),
            D::Wait,
        );
        // Still running, past the timeout but it HAS produced output: healthy.
        assert_eq!(
            resume_fallback_decision(false, false, true, Some(2000), Duration::from_secs(5)),
            D::Wait,
        );
        // Still running, no timeout configured: never treated as hung.
        assert_eq!(
            resume_fallback_decision(false, true, false, None, Duration::from_secs(60)),
            D::Wait,
        );
        // Still running, timeout of 0 disables the hung window.
        assert_eq!(
            resume_fallback_decision(false, true, false, Some(0), Duration::from_secs(60)),
            D::Wait,
        );
    }

    #[test]
    fn retry_returns_in_flight_and_touches_nothing_when_launch_pending() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat/x");
        engine.sessions.push(session);
        engine
            .resume_fallback_candidates
            .insert(TabId::new("s1-slot"), Instant::now());
        engine.mark_in_flight(InFlightKey::AgentLaunch(TabId::new("s1-slot")));

        let outcome = engine.retry_resume_fallback("s1-slot", (24, 80), "msg".to_string());

        assert!(matches!(outcome, ResumeFallbackOutcome::InFlight));
        // Protected: candidate still present, in-flight key untouched.
        assert!(
            engine
                .resume_fallback_candidates
                .contains_key(TabIdRef::new("s1-slot"))
        );
        assert!(engine.is_in_flight(&InFlightKey::AgentLaunch(TabId::new("s1-slot"))));
    }

    #[test]
    fn retry_dispatches_and_clears_state_on_happy_path() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat/x");
        engine.sessions.push(session);
        engine
            .resume_fallback_candidates
            .insert(TabId::new("s1-slot"), Instant::now());
        engine.running_provider_pins.insert(
            TabId::new("s1-slot"),
            crate::model::ProviderKind::new("claude"),
        );

        let outcome = engine.retry_resume_fallback("s1-slot", (24, 80), "fresh".to_string());

        assert!(matches!(outcome, ResumeFallbackOutcome::Retried { .. }));
        // Candidate and pin were torn down. The providers check is
        // documentation-only: PtyClient can't be seeded without spawning a
        // real process, so the map is always empty here and this assert can't
        // fail. The load-bearing assertions are the candidate and pin removals.
        assert!(
            !engine
                .resume_fallback_candidates
                .contains_key(TabIdRef::new("s1-slot"))
        );
        assert!(!engine.providers.contains_key(TabIdRef::new("s1-slot")));
        assert!(
            !engine
                .running_provider_pins
                .contains_key(TabIdRef::new("s1-slot"))
        );
        // A launch is now in flight (dispatch marked the key).
        assert!(engine.is_in_flight(&InFlightKey::AgentLaunch(TabId::new("s1-slot"))));
    }

    #[test]
    fn retry_uses_the_pinned_provider_not_the_tabs_own_row_provider() {
        // G-T6: proves the capture-before-remove ordering (G24) is load-bearing.
        // `retry_resume_fallback` captures `tab_running_provider` (which prefers
        // `running_provider_pins`) BEFORE it clears that pin. A retargeted-while-
        // live tab's pin differs from its persisted `agent_tabs` row provider; if
        // the removal ever moved ahead of the capture, the rebuilt launch would
        // silently fall back to the tab's own (stale) row provider instead of the
        // one that was actually running. Configure the pinned provider's command
        // as `cat` (spawns and stays alive) and the tab's own row provider as a
        // nonexistent binary, so a wrong-provider regression is observable as a
        // launch failure instead of `providers` gaining a live entry.
        let (mut engine, tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat/x");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = tmp.path().to_string_lossy().to_string();
        engine.sessions.push(session);
        let tab = crate::model::AgentTab {
            id: "tab-1".to_string(),
            session_id: "s1".to_string(),
            // Deliberately a nonexistent command: falls back to
            // `provider_config`'s "command == provider name" default, which
            // fails to spawn if this is what actually gets used.
            provider: ProviderKind::new("dux-test-nonexistent-provider-zzz"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        // The tab is pinned to "cat" (e.g. from a live run before this exit).
        // That pin must win the capture, not the row's own provider above.
        engine
            .running_provider_pins
            .insert(TabId::new("tab-1"), ProviderKind::new("cat"));
        engine
            .resume_fallback_candidates
            .insert(TabId::new("tab-1"), Instant::now());

        let outcome = engine.retry_resume_fallback("tab-1", (24, 80), "fresh".to_string());
        assert!(matches!(outcome, ResumeFallbackOutcome::Retried { .. }));
        // The pin is torn down as part of the retry regardless of which
        // provider was captured.
        assert!(
            !engine
                .running_provider_pins
                .contains_key(TabIdRef::new("tab-1"))
        );

        // Drain the async launch job's result and feed it back through the
        // engine exactly like the real event loop would.
        let mut saw_ready = false;
        for _ in 0..200 {
            if let Ok(event) = engine.worker_rx.try_recv() {
                match &event {
                    crate::worker::WorkerEvent::AgentLaunchReady(_) => saw_ready = true,
                    crate::worker::WorkerEvent::AgentLaunchFailed(data) => {
                        panic!(
                            "launch must have used the PINNED \"cat\" provider, not the \
                             tab's own row provider; got a launch failure instead: {}",
                            data.message
                        );
                    }
                    _ => {}
                }
                engine.process_worker_event(event);
                if saw_ready {
                    break;
                }
                continue;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(saw_ready, "the launch job never reported ready in time");
        assert!(
            engine.providers.contains_key(TabIdRef::new("tab-1")),
            "a successful relaunch with the pinned provider must populate `providers`"
        );
    }

    #[test]
    fn retry_rebuilds_an_extra_tab_launch_keyed_by_tab_id() {
        // A resumed EXTRA tab must retry fresh under its own tab id, not be
        // silently dropped because its key is not the one in the session's
        // slot.
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat/x");
        engine.sessions.push(session);
        let tab = crate::model::AgentTab {
            id: "tab-1".to_string(),
            session_id: "s1".to_string(),
            provider: crate::model::ProviderKind::new("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        };
        engine.agent_tabs.insert(TabId::new(tab.id.clone()), tab);
        engine
            .resume_fallback_candidates
            .insert(TabId::new("tab-1"), Instant::now());

        let outcome = engine.retry_resume_fallback("tab-1", (24, 80), "fresh".to_string());

        assert!(matches!(outcome, ResumeFallbackOutcome::Retried { .. }));
        // Candidate torn down; the fresh relaunch is in flight under the TAB id.
        assert!(
            !engine
                .resume_fallback_candidates
                .contains_key(TabIdRef::new("tab-1"))
        );
        assert!(engine.is_in_flight(&InFlightKey::AgentLaunch(TabId::new("tab-1"))));
        // No in-flight launch was created under the session id.
        assert!(!engine.is_in_flight(&InFlightKey::AgentLaunch(TabId::new("s1"))));
        // The extra tab's row survives (a fresh relaunch, not a close).
        assert!(engine.agent_tabs.contains_key(TabIdRef::new("tab-1")));
    }

    #[test]
    fn a_failed_fallback_relaunch_leaves_no_tab_keyed_state_behind() {
        // The retry tears the stale attempt down BEFORE it dispatches the fresh
        // one, and the fresh one can fail (here: a provider binary that does not
        // exist). Nothing repopulates a tab-keyed map on that path, so anything
        // the teardown missed stays in memory until some later teardown or a
        // restart. `launched_drop_paste` was exactly that: the process was
        // gone and its drop-paste form was still being published in bootstrap.
        //
        // The teardown therefore goes through `clear_tab_runtime`, the one
        // function that knows the whole tab-keyed map list, so the next map
        // added there does not have to remember this site too.
        //
        // EVERY map that function clears is SEEDED below, and every one is
        // asserted empty afterwards: an assertion on a map never seeded is
        // vacuously true, and a map neither seeded nor asserted is not covered
        // at all.
        //
        // The one entry that CANNOT be seeded is the in-flight `AgentLaunch` key:
        // step 1 of the retry returns `InFlight` and touches nothing when that
        // key is set, so seeding it would test a different path entirely. It is
        // still asserted, where what is being observed is that the FAILED
        // relaunch released its own key.
        let (mut engine, tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat/x");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = tmp.path().to_string_lossy().to_string();
        session.provider = ProviderKind::new("dux-test-nonexistent-provider-zzz");
        engine.sessions.push(session);

        // Everything the launch that just died had left keyed by this tab id.
        engine
            .resume_fallback_candidates
            .insert(TabId::new("s1-slot"), Instant::now());
        engine.providers.insert(
            TabId::new("s1-slot"),
            crate::pty::PtyClient::spawn_with_env("cat", &[], tmp.path(), 24, 80, 1000, &[])
                .expect("spawn the stale attempt's PTY"),
        );
        engine
            .running_provider_pins
            .insert(TabId::new("s1-slot"), ProviderKind::new("codex"));
        engine.launched_drop_paste.insert(
            TabId::new("s1-slot"),
            crate::engine::LaunchedDropPaste {
                provider: "codex".to_string(),
                form: crate::config::WebDragDropPaste::SingleQuoted,
                command_name: "codex".to_string(),
            },
        );
        engine
            .pty_activity
            .insert("s1-slot".to_string(), Instant::now());
        engine
            .pty_input
            .insert("s1-slot".to_string(), Instant::now());
        engine.needs_attention.insert(TabId::new("s1-slot"));
        engine.pty_progress.insert(
            TabId::new("s1-slot"),
            crate::pty::ProgressReport {
                working: true,
                at: Instant::now(),
            },
        );
        engine
            .agent_viewed
            .insert(TabId::new("s1-slot"), Instant::now());

        let outcome = engine.retry_resume_fallback("s1-slot", (24, 80), "fresh".to_string());
        assert!(matches!(outcome, ResumeFallbackOutcome::Retried { .. }));

        // Drive the launch to its FAILURE through the real event path, so this
        // asserts on the state a user is actually left holding.
        let mut saw_failed = false;
        for _ in 0..200 {
            if let Ok(event) = engine.worker_rx.try_recv() {
                if matches!(&event, crate::worker::WorkerEvent::AgentLaunchFailed(_)) {
                    saw_failed = true;
                }
                engine.process_worker_event(event);
                if saw_failed {
                    break;
                }
                continue;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(saw_failed, "the launch job never reported failure in time");

        assert!(
            !engine.providers.contains_key(TabIdRef::new("s1-slot")),
            "the stale attempt's PTY must not outlive the relaunch"
        );
        assert!(
            !engine
                .launched_drop_paste
                .contains_key(TabIdRef::new("s1-slot")),
            "a dead process must not keep publishing its drop-paste profile"
        );
        assert!(
            !engine
                .running_provider_pins
                .contains_key(TabIdRef::new("s1-slot"))
        );
        assert!(
            !engine
                .resume_fallback_candidates
                .contains_key(TabIdRef::new("s1-slot"))
        );
        assert!(
            !engine.pty_activity.contains_key("s1-slot"),
            "stale activity would read as a working tab with no process"
        );
        assert!(
            !engine.pty_input.contains_key("s1-slot"),
            "stale input would read as a typing tab with no process"
        );
        assert!(!engine.needs_attention.contains(TabIdRef::new("s1-slot")));
        assert!(
            !engine.pty_progress.contains_key(TabIdRef::new("s1-slot")),
            "a stale progress override leaves a spinner on a dead tab"
        );
        assert!(!engine.agent_viewed.contains_key(TabIdRef::new("s1-slot")));
        assert!(
            !engine.is_in_flight(&crate::engine::InFlightKey::AgentLaunch(TabId::new(
                "s1-slot"
            ))),
            "the failed relaunch must release its own in-flight key"
        );
    }

    #[test]
    fn retry_returns_not_candidate_when_not_a_candidate() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat/x");
        engine.sessions.push(session);
        // No resume_fallback_candidates entry seeded.

        let outcome = engine.retry_resume_fallback("s1-slot", (24, 80), "msg".to_string());

        assert!(matches!(outcome, ResumeFallbackOutcome::NotCandidate));
        assert!(!engine.is_in_flight(&InFlightKey::AgentLaunch(TabId::new("s1"))));
    }

    #[test]
    fn retry_drops_stale_candidate_when_session_is_gone() {
        let (mut engine, _tmp) = test_engine();
        // Candidate present but no matching session.
        engine
            .resume_fallback_candidates
            .insert(TabId::new("ghost"), Instant::now());

        let outcome = engine.retry_resume_fallback("ghost", (24, 80), "msg".to_string());

        assert!(matches!(outcome, ResumeFallbackOutcome::NotCandidate));
        assert!(
            !engine
                .resume_fallback_candidates
                .contains_key(TabIdRef::new("ghost"))
        );
    }
}
