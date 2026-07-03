//! `Engine::retry_resume_fallback` — the engine-owned resume-fallback retry.
//! One method both TUI retry paths (exit-driven and timeout-driven) call so
//! the provider/candidate/pin removal and the re-dispatch happen atomically
//! inside a single `&mut self` call, closing the window where a session has
//! neither its old nor its new provider.
//!
//! Background and rationale: see
//! `docs/superpowers/specs/2026-05-31-finish-delete-and-resume-fallback-design.md`.

use crate::engine::events::EventReaction;
use crate::engine::{Command, Engine, InFlightKey};
use crate::model::{AgentSession, ProviderKind, SessionStatus};
use crate::worker::{AgentLaunchKind, AgentLaunchRequest};

/// Outcome of an attempted resume-fallback retry. Three states because the
/// caller must react differently to each — collapsing any two corrupts state.
pub enum ResumeFallbackOutcome {
    /// Engine removed the candidate + provider + pin and dispatched a fresh
    /// `resume:false` launch. `reaction` is the `DispatchAgentLaunchView`
    /// follow-up the caller must apply. Treat the session as HANDLED: skip the
    /// normal exit/Detached cleanup AND the post-exit UI/PR follow-ups.
    ///
    /// If the OS thread-spawn itself failed (the only `launched:false` cause
    /// reachable here, since the in-flight pre-check already passed), the
    /// engine has already marked the session `Detached` and `reaction` carries
    /// the spawn-error status. It is still `Retried`.
    ///
    /// `reaction` is boxed because `EventReaction` is large (~272 bytes) and
    /// the other two variants are unit — leaving it unboxed trips clippy's
    /// `large_enum_variant` lint, which is a `-D warnings` CI gate. The
    /// codebase boxes for this same reason in `command.rs`.
    Retried { reaction: Box<EventReaction> },
    /// A launch is already in flight for this session. The engine did NOTHING
    /// (candidate, provider, pin untouched). Treat the session as PROTECTED:
    /// skip the destructive exit cleanup AND the post-exit UI/PR follow-ups,
    /// exactly as if it had been retried. The in-flight launch will resolve.
    InFlight,
    /// The session is no longer an eligible resume candidate. The engine has
    /// removed any stale candidate entry. The caller proceeds with normal
    /// exit handling (fall through to the Detached path).
    NotCandidate,
}

impl Engine {
    /// Build an `AgentLaunchRequest` from engine state. `pty_size` is the only
    /// front-end-sourced input (the TUI's last known PTY size); everything else
    /// (provider config, resolved env, scrollback) comes from engine state.
    /// The TUI's `agent_launch_request` delegates here so there is a single
    /// source of truth for request construction.
    pub fn build_agent_launch_request(
        &self,
        session: AgentSession,
        resume: bool,
        pty_size: (u16, u16),
        kind: AgentLaunchKind,
    ) -> AgentLaunchRequest {
        // Session-slot launch: tab_id == session.id, provider == session.provider.
        let tab_id = session.id.clone();
        self.build_tab_launch_request(tab_id, None, session, resume, pty_size, kind)
    }

    /// Tab-aware launch-request builder. `tab_provider = None` uses the session's
    /// own provider (the session-slot tab, `tab_id == session.id`); `Some(provider)`
    /// launches that provider for an extra tab.
    ///
    /// Resume eligibility is dynamic, not positional: a tab launches with the
    /// provider's `--continue` flag only if it is the sole/first provider coming
    /// up in the shared worktree — i.e. no OTHER tab of this session currently has
    /// a live provider or an in-flight launch. `--continue` is directory-scoped
    /// and always grabs the *most-recent* conversation, so at most one tab may
    /// resume; the first tab into an otherwise-empty worktree resumes, and every
    /// tab launched while another is already live/launching starts fresh.
    pub fn build_tab_launch_request(
        &self,
        tab_id: String,
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
        let env = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .and_then(|project| {
                crate::config::resolve_agent_env(&self.config.env, &project.env).ok()
            })
            .unwrap_or_default();
        AgentLaunchRequest {
            session,
            tab_id,
            provider,
            provider_config,
            env,
            resume,
            pty_size,
            scrollback_lines: self.config.ui.agent_scrollback_lines,
            kind,
        }
    }

    /// Attempt a resume-fallback retry for `session_id`. Synchronous: all state
    /// transitions happen inside this one `&mut self` call so no other
    /// `drain_events` tick can observe a half-applied state. See
    /// `ResumeFallbackOutcome` for how the caller must treat each result.
    pub fn retry_resume_fallback(
        &mut self,
        tab_id: &str,
        pty_size: (u16, u16),
        status_message: String,
    ) -> ResumeFallbackOutcome {
        // 1. A launch already in flight for THIS tab: protect it, touch nothing.
        if self.is_in_flight(&InFlightKey::AgentLaunch(tab_id.to_string())) {
            return ResumeFallbackOutcome::InFlight;
        }
        // 2. Not (any longer) a candidate: nothing to retry.
        if !self.resume_fallback_candidates.contains_key(tab_id) {
            return ResumeFallbackOutcome::NotCandidate;
        }
        // 3. Owning session gone: drop the stale candidate, fall through. Resume
        //    candidates are keyed by tab id, so resolve the owning session (the
        //    session-slot tab resolves to itself; an extra tab via its row).
        let Some(session_id) = self.owning_session_for_tab(tab_id) else {
            self.resume_fallback_candidates.remove(tab_id);
            return ResumeFallbackOutcome::NotCandidate;
        };
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
            self.resume_fallback_candidates.remove(tab_id);
            return ResumeFallbackOutcome::NotCandidate;
        };
        // Capture the provider that was resuming (the exited tab's own provider)
        // BEFORE tearing down the pin, so the fresh relaunch reuses it.
        let is_session_slot = tab_id == session.id;
        let provider = self.tab_running_provider(&session, tab_id);
        // 4. Tear down the stale resume attempt (all keyed by tab id).
        self.resume_fallback_candidates.remove(tab_id);
        self.providers.remove(tab_id);
        self.running_provider_pins.remove(tab_id);
        // 5. Build a fresh, non-resume launch request. The session-slot tab goes
        //    through the session-slot path (ResumeFallback view drives its status
        //    line); an extra tab rebuilds its own provider as a Tab launch so the
        //    ready/failed handlers stay tab-scoped and never flip session state.
        let request = if is_session_slot {
            self.build_agent_launch_request(
                session,
                false,
                pty_size,
                AgentLaunchKind::ResumeFallback { status_message },
            )
        } else {
            self.build_tab_launch_request(
                tab_id.to_string(),
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
        // 6. Dispatch. `launched:false` is reachable only via OS thread-spawn
        //    failure now (the in-flight pre-check above already passed), so on
        //    failure we mark the session Detached — but only for the session-slot
        //    tab, since an extra tab's failure must not tear down live siblings.
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
        if !launched && is_session_slot {
            self.mark_session_status(&session_id, SessionStatus::Detached);
        }
        ResumeFallbackOutcome::Retried {
            reaction: Box::new(reaction),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_session, test_engine};
    use std::time::Instant;

    #[test]
    fn retry_returns_in_flight_and_touches_nothing_when_launch_pending() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat/x");
        engine.sessions.push(session);
        engine
            .resume_fallback_candidates
            .insert("s1".to_string(), Instant::now());
        engine.mark_in_flight(InFlightKey::AgentLaunch("s1".to_string()));

        let outcome = engine.retry_resume_fallback("s1", (24, 80), "msg".to_string());

        assert!(matches!(outcome, ResumeFallbackOutcome::InFlight));
        // Protected: candidate still present, in-flight key untouched.
        assert!(engine.resume_fallback_candidates.contains_key("s1"));
        assert!(engine.is_in_flight(&InFlightKey::AgentLaunch("s1".to_string())));
    }

    #[test]
    fn retry_dispatches_and_clears_state_on_happy_path() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat/x");
        engine.sessions.push(session);
        engine
            .resume_fallback_candidates
            .insert("s1".to_string(), Instant::now());
        engine
            .running_provider_pins
            .insert("s1".to_string(), crate::model::ProviderKind::new("claude"));

        let outcome = engine.retry_resume_fallback("s1", (24, 80), "fresh".to_string());

        assert!(matches!(outcome, ResumeFallbackOutcome::Retried { .. }));
        // Candidate and pin were torn down. The providers check is
        // documentation-only: PtyClient can't be seeded without spawning a
        // real process, so the map is always empty here and this assert can't
        // fail — the load-bearing assertions are the candidate and pin removals.
        assert!(!engine.resume_fallback_candidates.contains_key("s1"));
        assert!(!engine.providers.contains_key("s1"));
        assert!(!engine.running_provider_pins.contains_key("s1"));
        // A launch is now in flight (dispatch marked the key).
        assert!(engine.is_in_flight(&InFlightKey::AgentLaunch("s1".to_string())));
    }

    #[test]
    fn retry_rebuilds_an_extra_tab_launch_keyed_by_tab_id() {
        // A resumed EXTRA tab (candidate keyed by tab id, not session id) that
        // exits with no output must retry fresh under its own tab id — not be
        // silently dropped because the key isn't a session id.
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
        engine.agent_tabs.insert(tab.id.clone(), tab);
        engine
            .resume_fallback_candidates
            .insert("tab-1".to_string(), Instant::now());

        let outcome = engine.retry_resume_fallback("tab-1", (24, 80), "fresh".to_string());

        assert!(matches!(outcome, ResumeFallbackOutcome::Retried { .. }));
        // Candidate torn down; the fresh relaunch is in flight under the TAB id.
        assert!(!engine.resume_fallback_candidates.contains_key("tab-1"));
        assert!(engine.is_in_flight(&InFlightKey::AgentLaunch("tab-1".to_string())));
        // No in-flight launch was created under the session id.
        assert!(!engine.is_in_flight(&InFlightKey::AgentLaunch("s1".to_string())));
        // The extra tab's row survives (a fresh relaunch, not a close).
        assert!(engine.agent_tabs.contains_key("tab-1"));
    }

    #[test]
    fn retry_returns_not_candidate_when_not_a_candidate() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat/x");
        engine.sessions.push(session);
        // No resume_fallback_candidates entry seeded.

        let outcome = engine.retry_resume_fallback("s1", (24, 80), "msg".to_string());

        assert!(matches!(outcome, ResumeFallbackOutcome::NotCandidate));
        assert!(!engine.is_in_flight(&InFlightKey::AgentLaunch("s1".to_string())));
    }

    #[test]
    fn retry_drops_stale_candidate_when_session_is_gone() {
        let (mut engine, _tmp) = test_engine();
        // Candidate present but no matching session.
        engine
            .resume_fallback_candidates
            .insert("ghost".to_string(), Instant::now());

        let outcome = engine.retry_resume_fallback("ghost", (24, 80), "msg".to_string());

        assert!(matches!(outcome, ResumeFallbackOutcome::NotCandidate));
        assert!(!engine.resume_fallback_candidates.contains_key("ghost"));
    }
}
