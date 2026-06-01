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
use crate::model::{AgentSession, SessionStatus};
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
        let provider_config = crate::config::provider_config(&self.config, &session.provider);
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
        session_id: &str,
        pty_size: (u16, u16),
        status_message: String,
    ) -> ResumeFallbackOutcome {
        // 1. A launch already in flight: protect the session, touch nothing.
        if self.is_in_flight(&InFlightKey::AgentLaunch(session_id.to_string())) {
            return ResumeFallbackOutcome::InFlight;
        }
        // 2. Not (any longer) a candidate: nothing to retry.
        if !self.resume_fallback_candidates.contains_key(session_id) {
            return ResumeFallbackOutcome::NotCandidate;
        }
        // 3. Session gone: drop the stale candidate, fall through.
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
            self.resume_fallback_candidates.remove(session_id);
            return ResumeFallbackOutcome::NotCandidate;
        };
        // 4. Tear down the stale resume attempt.
        self.resume_fallback_candidates.remove(session_id);
        self.providers.remove(session_id);
        self.running_provider_pins.remove(session_id);
        // 5. Build a fresh, non-resume launch request.
        let request = self.build_agent_launch_request(
            session,
            false,
            pty_size,
            AgentLaunchKind::ResumeFallback { status_message },
        );
        // 6. Dispatch. `launched:false` is reachable only via OS thread-spawn
        //    failure now (the in-flight pre-check above already passed), so on
        //    failure we mark the session Detached and surface the error
        //    reaction.
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
        if !launched {
            self.mark_session_status(session_id, SessionStatus::Detached);
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
        // Candidate, provider, and pin were torn down.
        assert!(!engine.resume_fallback_candidates.contains_key("s1"));
        assert!(!engine.providers.contains_key("s1"));
        assert!(!engine.running_provider_pins.contains_key("s1"));
        // A launch is now in flight (dispatch marked the key).
        assert!(engine.is_in_flight(&InFlightKey::AgentLaunch("s1".to_string())));
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
